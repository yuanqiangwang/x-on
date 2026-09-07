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
win.onFocusChanged(({ payload: focused }) => {
  if (!focused) win.hide();
});
const list = document.getElementById("results") as HTMLUListElement;

let apps: AppInfo[] = [];
let filtered: AppInfo[] = [];
let selected = 0;

// 窗口自适配常量：单条候选行高（与 index.html 的 --row-h 同步）、空查询窗口高、
// 结果列表上边距。CHROME/GAP 为估算，实际高度按 dev 视口校准。
const ROW_H = 38;
const CHROME = 90;
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

function render() {
  filtered = matcher.search(input.value);
  if (selected >= filtered.length) selected = Math.max(0, filtered.length - 1);

  list.textContent = "";
  filtered.forEach((a, i) => {
    const li = document.createElement("li");
    li.className = i === selected ? "active" : "";

    li.appendChild(avatarEl(a));

    const meta = document.createElement("div");
    meta.className = "meta";
    const nameEl = document.createElement("span");
    nameEl.className = "name";
    nameEl.textContent = a.name;
    meta.appendChild(nameEl);
    if (a.comment) {
      const c = document.createElement("span");
      c.className = "comment";
      c.textContent = a.comment;
      meta.appendChild(c);
    }
    li.appendChild(meta);

    li.addEventListener("click", () => {
      selected = i;
      render();
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

function launch(app: AppInfo) {
  invoke("launch_app", { appPath: app.launchPath }).catch((e) => console.error(e));
}

let debounceTimer: number | undefined;
input.addEventListener("input", () => {
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
      if (pick) launch(pick);
    } else if (e.key === "Escape") {
      e.preventDefault();
      win.hide();
    }
  },
  true,
);

// Whenever the backend wakes us (global shortcut / single-instance), clear and refocus.
win.listen("wake", () => {
  input.value = "";
  selected = 0;
  render();
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

(async () => {
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
