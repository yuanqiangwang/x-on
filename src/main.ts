import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { AppMatcher, type AppInfo } from "./matcher";

// 检索匹配深模块：index 重建拼音索引、search 排序返回。分层/同音抑制全藏其内部。
const matcher = new AppMatcher();

// 系统功能图标：ms-settings / shell 命名空间在壳那里被当成"文件"，若走 SHGetFileInfoW
// 提取会得到无辨识度的通用文档图标（还白费一次 IPC）。系统项辨识靠名字，这里给每项配
// 一个贴切的彩色 emoji，忽略那个白文档图标。没配到的兜底统一齿轮 ⚙️。
const SYSTEM_EMOJI: Record<string, string> = {
  "ms-settings:personalization": "🎨",
  "ms-settings:display": "🖥️",
  "ms-settings:sound": "🔊",
  "ms-settings:network": "🌐",
  "ms-settings:bluetooth": "📡",
  "ms-settings:storage": "💾",
  "ms-settings:about": "ℹ️",
  "ms-settings:defaultapps": "🧩",
  "ms-settings:powersleep": "🔋",
  "ms-settings:notifications": "🔔",
  "ms-settings:lockscreen": "🔒",
  "ms-settings:colors": "🌈",
  "ms-settings:taskbar": "📊",
  "ms-settings:themes": "🖼️",
  "ms-settings:dateandtime": "🕒",
  "ms-settings:printers": "🖨️",
  "ms-settings:windowsupdate": "🔄",
  "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}": "💻", // 此电脑
  "::{645FF040-5081-101B-9F08-00AA002F954E}": "🗑️", // 回收站
};
const isSystemUri = (path: string) => path.startsWith("ms-settings:") || path.startsWith("::{");
// Store（UWP）条目的 launchPath 是 `store:<AUMID>`，不是磁盘文件：启动走 AUMID 激活、
// 图标走 shell PIDL，右键的「提权 / 打开所在位置」对它都不成立（后端提权也会静默忽略）。
const isStorePath = (path: string) => path.startsWith("store:");
function systemEmoji(path: string): string | null {
  if (SYSTEM_EMOJI[path]) return SYSTEM_EMOJI[path];
  return isSystemUri(path) ? "⚙️" : null;
}

type Settings = {
  accelerator?: string;
  autostart?: boolean;
  font?: string | null;
  resultRows?: number;
};

// UI font chain: the user-supplied font name from settings.json is prepended
// (VS Code editor.fontFamily style); empty falls back to the system-font default
// in index.html's :root --font. Kept in sync with that value.
const FALLBACK_FONT_CHAIN =
  '"Microsoft YaHei", "Segoe UI", system-ui, sans-serif';

function applyFont(font?: string | null) {
  const docStyle = document.documentElement.style;
  const name = font?.trim();
  if (name) {
    docStyle.setProperty("--font", `"${name}", ${FALLBACK_FONT_CHAIN}`);
  } else {
    // Empty => let the :root default (system font) apply.
    docStyle.removeProperty("--font");
  }
}

const win = getCurrentWebviewWindow();
const input = document.getElementById("search") as HTMLInputElement;
// 失焦即隐藏：窗口一失去焦点（点到别处）就收起，下次 Alt+Space 再唤起。
// 收起后立刻把界面复位到空态：下次 show() 的第一帧就应该是空输入 + 空态高度，
// 否则用户会先看到上一次的搜索结果、再看到窗口"塌"回去（Raycast 为此专门等
// WebView 画完才显示窗口，是同一类闪烁）。此刻窗口已隐藏，重排看不见。
win.onFocusChanged(({ payload: focused }) => {
  if (!focused) {
    win.hide();
    resetForNextWake();
  }
});
const list = document.getElementById("results") as HTMLUListElement;
const ctxMenu = document.getElementById("ctxmenu") as HTMLDivElement;

let apps: AppInfo[] = [];
let filtered: AppInfo[] = [];
let selected = 0;

// 窗口自适配常量：单条候选行高（与 index.html 的 --row-h 同步）、空查询窗口高、
// 结果列表上边距。CHROME/GAP 为估算，实际高度按 dev 视口校准。
// ⚠️ CHROME 承载了 index.html 里全部竖向间距（app padding、搜索行 padding-bottom、
// footer 的 margin/padding）：改那些值必须同步改这里。
const ROW_H = 38;
const CHROME = 91;
const GAP = 8;
let maxRows = 6; // 候选最大行数，来自 settings.json 的 resultRows，热更新

// Cached icon data URI per launch path, or `null` once we've given up on a row.
// `iconTries` caps the retry budget so a transient extraction failure (e.g. the
// shell icon cache warming up on first render) isn't cached as a permanent miss.
// Loaded in batched IPC calls per render (not one invoke per row); `MAX_ICONS_PER_BATCH`
// is a chunk size, so the whole set is covered rather than just the first 50.
const iconCache = new Map<string, string | null>();
const iconFetching = new Set<string>();
const iconTries = new Map<string, number>();
const MAX_ICONS_PER_BATCH = 50;
const MAX_ICON_TRIES = 3;

async function loadApps(force = false) {
  const cmd = force ? "rescan" : "scan_apps";
  const data = await invoke<AppInfo[]>(cmd);
  // Backend rows always come back with `icon` unset, so re-apply any icon or
  // permanent-miss we already cached for this launch path. Otherwise a rescan
  // (tray "rescan", index-updated, Start-Menu watcher) swaps in fresh objects
  // whose `icon` is undefined and every avatar collapses to a letter until some
  // *miss* happens to retrigger extraction. Cached hits return instantly, so the
  // icons only flicker if they were never successfully extracted.
  apps = (data ?? []).map((a) => {
    const cached = iconCache.get(a.launchPath);
    return cached !== undefined ? { ...a, icon: cached } : a;
  });
  matcher.index(apps);
  render();
}

/** 只切换选中态高亮，不整表重绘——鼠标划过时逐行 render 会抖，且白跑一轮图标加载。 */
function setActive(i: number) {
  if (i === selected) return;
  selected = i;
  Array.from(list.children).forEach((el, idx) => {
    el.classList.toggle("active", idx === i);
  });
}

// ---------------------------------------------------------------------------
// 副标题：本地化显示名下面补一行「原生名」。
//
// 开始菜单里大量系统工具的 `.lnk` 文件名是英文（`control panel.lnk`），而 shell 给的
// 显示名是本地化后的中文（控制面板）—— 后端把 stem 存进 `aliases` 让英文/拼音也能命中，
// 这里把它显示出来，用户才知道这条为什么被匹配上、以及它确实就是那个程序。
//
// 原生名优先于 comment（"系统"/"便携应用"这类来源标签信息量更低），两者都有时同行拼
// 放：行高固定 38px，摆不下第二行副标题。
// ---------------------------------------------------------------------------
const fold = (s: string) => s.replace(/\s+/g, "").toLowerCase();

function subtitle(a: AppInfo): string | null {
  const name = fold(a.name);
  const native = (a.aliases ?? [])
    .map((s) => s.trim())
    .find((s) => s && fold(s) !== name);
  const comment = a.comment?.trim() ?? "";
  if (native && comment) return `${native} · ${comment}`;
  return native || comment || null;
}

// ---------------------------------------------------------------------------
// 右键菜单：只有两项 —— 「以管理员身份运行」「打开文件所在的位置」。
// 系统功能项（ms-settings: / ::{CLSID}）和商店应用（store:<AUMID>）都不是磁盘上的
// 文件：提权会被后端忽略，位置也无从打开，所以这类行不弹菜单，而不是弹一个点了没
// 反应的菜单。
// ---------------------------------------------------------------------------

/**
 * "打开所在的位置"的目标：优先快捷方式解析出的真实目标（想看的是程序装在哪，而不是
 * 快捷方式放在哪），不可信时回退到条目自身（`.lnk` / 便携 exe）。
 *
 * 不可信 = 解析出的目标含非 ASCII：`lnk` crate 按 WINDOWS-1252 解码目标，中文安装路径
 * 会解成乱码（后端 `link_is_valid` 有同样说明），拿它去 Explorer 定位必然落空；而
 * `.lnk` 自身路径来自文件系统、便携路径来自 manifest，都是真 UTF-8。
 */
function revealPath(a: AppInfo): string | null {
  const target = (a.targetPath || "").trim();
  const trustworthy =
    /^[\x20-\x7e]*$/.test(target) && !isSystemUri(target) && !isStorePath(target);
  const p = (trustworthy && target ? target : a.launchPath).trim();
  return p && !isSystemUri(p) && !isStorePath(p) ? p : null;
}

/// 复位到空态：清空输入、回到第一条、按空态重算窗口高度。
/// 隐藏时和 wake 时各调一次，保证"显示出来的第一帧"永远是干净的。
function resetForNextWake() {
  input.value = "";
  selected = 0;
  render();
}

function openContextMenu(a: AppInfo, x: number, y: number) {
  const items: { label: string; run: () => void }[] = [];
  if (!isSystemUri(a.launchPath) && !isStorePath(a.launchPath)) {
    items.push({ label: "以管理员身份运行", run: () => launch(a, true) });
    const target = revealPath(a);
    if (target) {
      items.push({
        label: "打开文件所在的位置",
        run: () =>
          invoke("reveal_in_explorer", { path: target }).catch((e) => console.error(e)),
      });
    }
  }
  if (items.length === 0) return;

  ctxMenu.textContent = "";
  for (const it of items) {
    const el = document.createElement("div");
    el.className = "item";
    el.textContent = it.label;
    el.addEventListener("click", () => {
      closeContextMenu();
      it.run();
    });
    ctxMenu.appendChild(el);
  }

  ctxMenu.hidden = false;
  // 贴着光标，但不越出窗口（窗口 640 宽、高度按结果自适应）。
  const box = ctxMenu.getBoundingClientRect();
  ctxMenu.style.left = `${Math.max(6, Math.min(x, window.innerWidth - box.width - 6))}px`;
  ctxMenu.style.top = `${Math.max(6, Math.min(y, window.innerHeight - box.height - 6))}px`;
}

function closeContextMenu() {
  if (ctxMenu.hidden) return;
  ctxMenu.hidden = true;
  ctxMenu.textContent = "";
}

function render() {
  filtered = matcher.search(input.value);
  if (selected >= filtered.length) selected = Math.max(0, filtered.length - 1);

  list.textContent = "";
  filtered.forEach((a, i) => {
    const li = document.createElement("li");
    li.className = i === selected ? "active" : "";

    // 序号：与 Alt+数字 直接对应（1..9，0 = 第 10 项）。超出十项仍按真实序号显示，
    // 只是没有对应的单键可敲。
    const idx = document.createElement("span");
    idx.className = "idx";
    idx.textContent = String(i + 1);
    li.appendChild(idx);

    li.appendChild(avatarEl(a));

    const meta = document.createElement("div");
    meta.className = "meta";
    const nameEl = document.createElement("span");
    nameEl.className = "name";
    nameEl.textContent = a.name;
    meta.appendChild(nameEl);
    const sub = subtitle(a);
    if (sub) {
      const c = document.createElement("span");
      c.className = "sub";
      c.textContent = sub;
      meta.appendChild(c);
    }
    li.appendChild(meta);

    // 点击即启动（此前只选中）。悬停同步键盘选中态，"指到某项再回车"才不跳回第一项。
    li.addEventListener("click", () => launch(a));
    li.addEventListener("mouseenter", () => setActive(i));
    // 右键：屏蔽 webview 自带菜单，换成只含两项的自绘菜单。
    li.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      setActive(i);
      openContextMenu(a, e.clientX, e.clientY);
    });

    list.appendChild(li);
  });

  const active = list.querySelector("li.active") as HTMLElement | null;
  active?.scrollIntoView({ block: "nearest" });

  loadMissingIcons(filtered);

  // 窗口高度自适应：空查询 = 只显示输入行；有结果 = 输入行 + 可见候选行数。
  const hasQuery = input.value.trim().length > 0;
  list.style.display = hasQuery ? "block" : "none";
  const rows = hasQuery ? Math.min(filtered.length, maxRows) : 0;
  // 每次都 setSize（不缓存）：show 会按 config 尺寸重置窗口，每次 render 强制贴回内容高度。
  const height = CHROME + (hasQuery ? GAP + rows * ROW_H : 0);
  win.setSize(new LogicalSize(640, height)).catch(() => {});
}

function avatarEl(a: AppInfo): HTMLElement {
  const emoji = systemEmoji(a.launchPath);
  if (emoji) {
    const av = document.createElement("div");
    av.className = "avatar emoji";
    av.textContent = emoji;
    return av;
  }
  if (a.icon) {
    const img = document.createElement("img");
    img.src = a.icon;
    return img;
  }
  const av = document.createElement("div");
  av.className = "avatar";
  av.textContent = (a.name.trim()[0] ?? "?").toUpperCase();
  return av;
}

// Batch-load icons for rows that don't have a cached result yet. `MAX_ICONS_PER_BATCH`
// is now a chunk size — we cover the whole set in batches rather than only the first
// 50, so a long list's tail (visible after scrolling) still gets icons. A failed
// extraction is counted against a retry budget instead of being cached as a permanent
// miss, so a one-off failure (e.g. the shell icon cache warming up) recovers.
async function loadMissingIcons(apps: AppInfo[]) {
  const pending = apps.filter(
    (a) =>
      !isSystemUri(a.launchPath) && // 系统项用 emoji，不为它们提取图标（省一次 IPC）
      !iconCache.has(a.launchPath) &&
      !iconFetching.has(a.launchPath) &&
      (iconTries.get(a.launchPath) ?? 0) < MAX_ICON_TRIES,
  );
  if (pending.length === 0) return;

  for (let i = 0; i < pending.length; i += MAX_ICONS_PER_BATCH) {
    const chunk = pending.slice(i, i + MAX_ICONS_PER_BATCH);
    const paths = chunk.map((a) => a.launchPath);
    paths.forEach((p) => iconFetching.add(p));
    try {
      const map = await invoke<Record<string, string | null>>("get_app_icons", { paths });
      for (const p of paths) {
        const uri = map[p] ?? null;
        if (uri) {
          iconCache.set(p, uri);
        } else {
          const t = (iconTries.get(p) ?? 0) + 1;
          iconTries.set(p, t);
          if (t >= MAX_ICON_TRIES) iconCache.set(p, null);
        }
      }
    } catch {
      // Whole-batch failure: don't blame the icons, just spend one retry each.
      paths.forEach((p) => iconTries.set(p, (iconTries.get(p) ?? 0) + 1));
    } finally {
      paths.forEach((p) => iconFetching.delete(p));
    }
  }

  // Reflect any newly-cached icons onto the in-memory apps for the render pass.
  for (const a of apps) {
    const uri = iconCache.get(a.launchPath);
    if (uri) a.icon = uri;
  }
  render();
}

function launch(app: AppInfo, asAdmin = false) {
  invoke("launch_app", { appPath: app.launchPath, asAdmin }).catch((e) => console.error(e));
}

// 屏蔽 webview 自带菜单（重新加载 / 检查…）。搜索框是唯一例外：那里原生的粘贴、全选
// 仍然有用。候选行有自己的 contextmenu 处理，这里只负责把其它区域的原生菜单按掉。
document.addEventListener("contextmenu", (e) => {
  if (e.target !== input) e.preventDefault();
});

// 点到菜单外就收起。菜单内不收：mousedown 时把节点删掉，后续 click 就落空了。
document.addEventListener("mousedown", (e) => {
  if (!ctxMenu.hidden && !ctxMenu.contains(e.target as Node)) closeContextMenu();
});

let debounceTimer: number | undefined;
input.addEventListener("input", () => {
  closeContextMenu();
  selected = 0;
  // Debounce so fast typing triggers a single filter/render + icon batch, not one
  // per keystroke — that's what made "to" feel laggy while icons loaded.
  if (debounceTimer !== undefined) clearTimeout(debounceTimer);
  debounceTimer = window.setTimeout(() => render(), 40);
});

// Direction keys + Enter are selection/navigation, so they should mean the same
// whether focus is on the search box, a clicked result row, or the body — the
// list's native scrolling would otherwise hijack them once focus leaves the
// input. Listen on the document in the capture phase so this runs before any
// widget handler. Keys part of an in-progress IME candidate session (isComposing)
// are passed through so arrow keys keep picking pinyin candidates and Esc keeps
// cancelling them, instead of navigating the list.
document.addEventListener(
  "keydown",
  (e) => {
    if (e.isComposing) return;

    // Alt + 序号 直接启动（0 = 第 10 项）。裸数字键必须留给搜索框 —— "7-Zip"、
    // "360" 这类名字本身就带数字，所以用 Alt 修饰而不是直接吃数字键。
    if (e.altKey && !e.ctrlKey && !e.metaKey && /^[0-9]$/.test(e.key)) {
      e.preventDefault();
      const pick = filtered[e.key === "0" ? 9 : Number(e.key) - 1];
      if (pick) launch(pick);
      return;
    }

    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (filtered.length) selected = (selected + 1) % filtered.length;
      render();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (filtered.length) selected = (selected - 1 + filtered.length) % filtered.length;
      render();
    } else if (e.key === "Enter") {
      const pick = filtered[selected];
      // Ctrl(+Shift)+回车 = 以管理员身份运行（Windows 惯例是 Ctrl+Shift+Enter）。
      if (pick) launch(pick, e.ctrlKey || e.metaKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      // 菜单开着时 Esc 只关菜单，不连带把窗口收掉。
      if (!ctxMenu.hidden) {
        closeContextMenu();
        return;
      }
      win.hide();
      resetForNextWake();
    }
  },
  true,
);

// Whenever the backend wakes us (global shortcut / single-instance), clear and refocus.
// 正常情况下隐藏时已经复位过了，这里再补一次，覆盖"窗口还可见就被再次唤起"的情形。
win.listen("wake", () => {
  closeContextMenu();
  resetForNextWake();
  input.focus();
});

// Reload the index when it changes underneath us (tray portable add/remove, rescan).
// The index was already rebuilt in the backend; just re-read the fresh snapshot.
win.listen("index-updated", () => loadApps(false));

// Hot-apply a font change the user made in settings.json (backend watches it).
win.listen<Settings>("settings-changed", (e) => {
  applyFont(e.payload.font);
  if (typeof e.payload.resultRows === "number" && e.payload.resultRows > 0) {
    maxRows = Math.floor(e.payload.resultRows);
    render(); // 行数变化，重新 fit 窗口
  }
});

input.addEventListener("focus", render);

// 首次渲染 emoji 时 WebView 要沿字体链逐个 fallback，第一次出现会慢一拍
// （Raycast 踩过同一个坑，解法也是预热）。启动时把所有会用到的 emoji 一次性
// 渲染掉，"Segoe UI Emoji" 就常驻了，之后系统项头像不会再有首次卡顿。
function prewarmEmojiFont() {
  const probe = document.createElement("div");
  probe.setAttribute("aria-hidden", "true");
  probe.style.cssText =
    "position:fixed;left:-9999px;top:0;white-space:nowrap;" +
    `font-family:"Segoe UI Emoji",${FALLBACK_FONT_CHAIN};font-size:20px;line-height:1;`;
  probe.textContent = [...new Set([...Object.values(SYSTEM_EMOJI), "⚙️"])].join("");
  document.body.appendChild(probe);
  void probe.offsetHeight; // 强制一次布局，字体才会真正加载
  probe.remove();
}

(async () => {
  prewarmEmojiFont();
  try {
    const settings = await invoke<Settings>("get_config");
    applyFont(settings.font);
    if (typeof settings.resultRows === "number" && settings.resultRows > 0) {
      maxRows = Math.floor(settings.resultRows);
    }
  } catch (e) {
    console.error(e);
  }
  await loadApps(true);
  input.focus();
})();
