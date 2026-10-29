import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { AppMatcher, type AppInfo, type UsageMap } from "./matcher";
// 候选行的 Tailwind 类配方。单独成文件是为了让 Tailwind 的扫描范围能收紧到
// 「只有本文件 + index.html」—— 否则 style.css 的注释和本文件的代码字符串
// 会被当成候选类，凭空生成约 20% 的无用 CSS。理由与纪律见 ui-classes.ts 头部。
import {
  CTX_ITEM,
  NOTE,
  ROW,
  ROW_ACTIVE,
  ROW_CURRENT,
  ROW_IDX,
  ROW_META,
  ROW_NAME,
  ROW_SUB,
} from "./ui-classes";
// `/` 指令表。它不反向 import 本文件（能力靠 buildCommands 注入），所以这里只拿
// 类型与工厂函数 —— 依赖方向是单向的。
import {
  buildCommands,
  filterCommands,
  filterOptions,
  type Command,
  type CommandOption,
} from "./commands";

// 检索匹配深模块：index 重建拼音索引、search 排序返回。分层/同音抑制全藏其内部。
const matcher = new AppMatcher();

// 系统功能图标：ms-settings / shell 命名空间在壳那里被当成"文件"，若走 SHGetFileInfoW
// 提取会得到无辨识度的通用文档图标（还白费一次 IPC）。系统项辨识靠名字，这里给每项配
// 一个贴切的彩色 emoji，忽略那个白文档图标。没配到的兜底统一齿轮 ⚙️。
// ⚠️ key 必须与 src-tauri/src/system.rs 的 `uri` 逐字一致（包括 network-status /
// storagesense 这类页后缀）：这里是精确匹配，对不上不会报错，只会静默降级成 ⚙️。
const SYSTEM_EMOJI: Record<string, string> = {
  "ms-settings:personalization": "🎨",
  "ms-settings:display": "🖥️",
  "ms-settings:sound": "🔊",
  "ms-settings:network-status": "🌐",
  "ms-settings:bluetooth": "📡",
  "ms-settings:storagesense": "💾",
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
  theme?: ThemePref;
  /// 自定义配色：基础色名 → 任意 CSS 颜色。见下方 THEME_COLOR_KEYS。
  colors?: Record<string, string>;
};

type Theme = "light" | "dark";
/// "auto" = 跟随 Windows 的应用模式（浅色/深色）；另两个值把主题钉死。
type ThemePref = "auto" | Theme;

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

// ---------------------------------------------------------------------------
// 主题：settings.json 的 `theme` = auto（默认，跟随 Windows）/ light / dark。
//
// 系统深浅色以 Tauri 的信号为准（Windows 上 tao 监听 AppsUseLightTheme 注册表，
// 切换时推 tauri://theme-changed），而不是 CSS 的 prefers-color-scheme —— WebView2
// 的颜色方案一旦被显式锁过，prefers-color-scheme 就不再随系统更新，那条通路会
// 静默失效。matchMedia 只在拿不到 Tauri 主题时兜底取初值。
//
// CSS 侧只看 <html data-theme>，不用媒体查询：媒体查询与显式覆盖是两套优先级，
// 混用会出现"用户选了浅色、系统是深色，两边打架"。单一属性、单一来源。
// ---------------------------------------------------------------------------
const prefersLight = window.matchMedia("(prefers-color-scheme: light)");
let systemTheme: Theme = prefersLight.matches ? "light" : "dark";
let themePref: ThemePref = "auto";
/// 用户自定义配色（settings.json 的 colors）。空对象 = 全用主题默认值。
let themeConfig: Record<string, string> = {};
/// 上一次生效的 colors 序列化结果。初值刻意不等于任何序列化结果，
/// 保证 boot 时第一次调用 applyThemeInputs 一定会真的应用一次。
let themeKey = "";

/// 容错：拼错或大小写不符（"Light" / "system" / 空）一律回落到 auto，
/// 而不是把窗口留在一个既不是浅色也不是深色的中间态。
function parseThemePref(value?: string | null): ThemePref {
  const v = value?.trim().toLowerCase();
  return v === "light" || v === "dark" ? v : "auto";
}

// ---------------------------------------------------------------------------
// 自定义配色：colors 只能覆盖这几个"基础色"。选中行底、描边、悬停底、渐隐线这些
// 派生色由 CSS 的相对颜色语法从基础色算出来（见 style.css 的 :root）—— 所以用户改
// 一个 accent 就整族跟着走，不需要也不应该让他逐个填 rgba 变体。
// 未知的键只警告、不生效：静默忽略的话，用户拼错一次会以为"配置根本没起作用"。
// ---------------------------------------------------------------------------
const THEME_COLOR_KEYS = [
  "bg",
  "bg-raised",
  "text",
  "text-head",
  "muted",
  "accent",
] as const;

function applyColors(colors: Record<string, string>) {
  const docStyle = document.documentElement.style;
  // 先清空：settings 里删掉的项必须真的回到主题默认值。
  for (const key of THEME_COLOR_KEYS) docStyle.removeProperty(`--${key}`);
  const unknown: string[] = [];
  for (const [key, value] of Object.entries(colors)) {
    if (!(THEME_COLOR_KEYS as readonly string[]).includes(key)) {
      unknown.push(key);
      continue;
    }
    const v = (value ?? "").trim();
    if (v) docStyle.setProperty(`--${key}`, v);
  }
  if (unknown.length) {
    console.warn(
      `[xon] settings.json 的 colors 里有未知项：${unknown.join(", ")}` +
        `（可用：${THEME_COLOR_KEYS.join(" / ")}）`,
    );
  }
}

/// 把当前生效的 --bg 交给原生窗口背景刷子。
/// 用探针元素让浏览器把任意 CSS 颜色写法（#hex / rgb() / 命名色）解析成 `rgb(r, g, b)`，
/// 再转成 Rust 侧 `Color::from_str` 认的 #rrggbb —— 自定义配色就不必限制写法。
function resolveBgColor(): string {
  const probe = document.createElement("div");
  probe.style.cssText = "position:fixed;visibility:hidden;color:var(--bg)";
  document.body.appendChild(probe);
  const computed = getComputedStyle(probe).color;
  probe.remove();
  const m = computed.match(/rgba?\((\d+)[,\s]+(\d+)[,\s]+(\d+)/);
  if (!m) return "";
  const hex = [m[1], m[2], m[3]]
    .map((n) => Number(n).toString(16).padStart(2, "0"))
    .join("");
  return `#${hex}`;
}

function applyTheme() {
  const theme: Theme = themePref === "auto" ? systemTheme : themePref;
  document.documentElement.dataset.theme = theme;
  // 顺序要紧：先让主题决定基础值，再用用户行内覆盖压上去，最后才读 --bg。
  applyColors(themeConfig);
  // 窗口不透明（ADR 0001）：resize 时新露出的区域由窗口背景刷子先画一帧，不同步的话
  // 每次高度自适应都会闪一条底色不对的边。--bg 可能被用户改过，所以传算完的颜色本身。
  const bg = resolveBgColor();
  if (bg) invoke("set_window_background", { color: bg }).catch(() => {});
}

/// 配置 → 主题的唯一入口。只有 theme / colors 真的变了才重算：set_window_background
/// 会让 tao 重绘整个窗口，改字体、改行数时不该白跑一趟。
function applyThemeInputs(pref: ThemePref, colors?: Record<string, string>) {
  const key = JSON.stringify(colors ?? {});
  if (pref === themePref && key === themeKey) return;
  themePref = pref;
  themeConfig = colors ?? {};
  themeKey = key;
  applyTheme();
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
const hintRecent = document.getElementById("hint-recent") as HTMLElement;
const hintCmd = document.getElementById("hint-cmd") as HTMLElement;
const hintAdmin = document.getElementById("hint-admin") as HTMLElement;
const enterLabel = document.getElementById("enter-label") as HTMLElement;
const footer = document.getElementById("footer") as HTMLElement;
const escLabel = document.getElementById("esc-label") as HTMLElement;

let apps: AppInfo[] = [];
// filtered 恒等于"当前可见的列表"：键盘操作（回车 / Alt+数字 / 方向键）都基于它。
let filtered: AppInfo[] = [];
let selected = 0;
// 空态是否展开了「常用」列表（仅 ↓ 可展开）。空态默认只是一个输入框 —— 唤起零成本、
// 不拉图标；常用列表只在用户主动按 ↓ 时才出现。任何输入 / 收起 / 唤起都会复位。
let browsing = false;

// ---------------------------------------------------------------------------
// `/` 指令模式（2026-09-27）
//
// 输入以 `/` 开头就不再是搜应用，而是搜指令。与"空态常用列表"是并列的另一种视图，
// 但两者共用同一个输入框与同一套键盘导航 —— 见下面的 rowCount / activate。
// 视图共三种：应用候选 / 指令列表 / 某条指令的二级选项。
// ---------------------------------------------------------------------------
/** 指令表。能力在这里注入（commands.ts 不反向 import 本文件）。闭包是延迟求值，
 *  所以引用下面才声明的 themePref 没有初始化顺序问题。 */
const commands: Command[] = buildCommands({
  currentTheme: () => themePref,
  setTheme: (theme) => void applyThemeChange(theme),
  openSettings: () => invoke("open_settings").catch((e) => console.error(e)),
});
/** 已进入二级的指令；null = 停在指令列表（或根本不在指令模式）。 */
let activeCmd: Command | null = null;
/** 当前可见的指令行 / 选项行。与 `filtered` 同级，各管一种视图。 */
let cmdRows: Command[] = [];
let optRows: CommandOption[] = [];
/**
 * 导航层：当前视图的可见行数，以及"第 i 行被选中时该做什么"。
 *
 * 三种视图各自在 render 里装配这两个东西，键盘处理只认它们 —— 于是 ↑↓ / 回车 /
 * Alt+序号 不必为指令模式写第二套分支。指令与选项也有序号，Alt+2 跳过一条指令
 * 和跳过一条候选是同一个手势。
 */
let rowCount = 0;
let activate: (index: number, admin: boolean) => void = () => {};

// 窗口自适配常量：单条候选行高（与 index.html 的 --row-h 同步）、空查询窗口高、
// 结果列表上边距。
// ⚠️ CHROME 承载了 index.html 里全部竖向间距（app padding、搜索行 padding-bottom、
// footer 的 margin/padding）：改那些值必须同步改这里。
//
// CHROME = 94 而不是早期的 91。91 是拿"默认字体"凑出来的，而搜索行与提示条的高度
// 实际由字体的 line-height: normal 决定 —— font 是用户可配的，换个行高高的中文字体
// （霞鹜文楷的 normal 行高比雅黑高一截）内容就会比窗口高出好几像素，最后一行卡片的
// 高亮底边被裁掉。现在 CSS 里把 #search / #footer 的 line-height 钉成了像素值，
// 这个常量才真正与字体无关：
//   12 + 32(搜索行) + 8(搜索行下内边距) + 1(其下边框)
//      + 8 + 38(行) + 8(提示条上外边距) + 8(其上内边距) + 1(其上边框) + 16(提示条行高)
//      + 8                                                = 140
//   140 − GAP(8) − ROW_H(38)                              = 94
const ROW_H = 38;
const CHROME = 94;
const GAP = 8;
let maxRows = 6; // 候选最大行数，来自 settings.json 的 resultRows，热更新

/// 把一份后端配置应用到界面。两条路径共用：`settings-changed` 事件，以及指令写回
/// （后者拿不到事件 —— 见 applyThemeChange）。
function applyConfig(cfg: Settings) {
  applyFont(cfg.font);
  applyThemeInputs(parseThemePref(cfg.theme), cfg.colors);
  if (typeof cfg.resultRows === "number" && cfg.resultRows > 0) {
    const next = Math.floor(cfg.resultRows);
    if (next !== maxRows) {
      maxRows = next;
      render(); // 行数变化，重新 fit 窗口
    }
  }
}

/**
 * 指令（`/light` `/dark` `/auto`，以及 `/theme` 的二级选项）唯一的主题出口：
 * 写盘 + 立即生效。
 *
 * ⚠️ 必须用 command 的返回值自己 apply，**不能**等 `settings-changed`：后端
 * `patch_settings` 会把这次写入记成 own_write，watcher 因此跳过通知 —— 那是防
 * "写→读→写"回环的设计，代价就是"自己改的东西自己负责应用"。等事件等于界面不动。
 *
 * 失败只记日志：指令是"随手试一下"的场景，弹错误反而打断。
 */
async function applyThemeChange(theme: string) {
  try {
    applyConfig(await invoke<Settings>("set_theme", { theme }));
    // 二级列表要重绘，才能把「当前」标记挪到新值上（applyConfig 只在行数变化时自己
    // render）。直接执行型（`/light`）不在此列：那时 activeCmd 为空，列表该留在原地。
    if (activeCmd) render();
  } catch (e) {
    console.error(e);
  }
}

// Cached icon data URI per launch path, or `null` once we've given up on a row.
// `iconTries` caps the retry budget so a transient extraction failure (e.g. the
// shell icon cache warming up on first render) isn't cached as a permanent miss.
// Loaded in batched IPC calls per render (not one invoke per row); `MAX_ICONS_PER_BATCH`
// is a chunk size, so the whole set is covered rather than just the first 50.
const iconCache = new Map<string, string | null>();
const iconFetching = new Set<string>();
const iconTries = new Map<string, number>();
// 已经淡入过的 launchPath（Step 3d）。render() 是整表重建，<img> 每次都会被新建 ——
// 不给淡入加门控的话，每次敲键都会让所有可见图标重播一遍渐显（40ms 去抖挡不住这个）。
const iconRevealed = new Set<string>();
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
  // 后端随索引下发的图标（磁盘缓存命中，见 src-tauri/src/icon_cache.rs）在首帧
  // 就已经在位上，没有"字母头像 → 图标"的硬切换需要遮 —— 直接登记成已呈现，
  // 否则每次冷启动都会白播一次淡入，反而像慢了半拍。
  for (const a of apps) if (a.icon) iconRevealed.add(a.launchPath);
  matcher.index(apps);
  // 权重先就位再渲染：否则首屏会用"无权重"的顺序画一次、再跳成带权重的，闪一下。
  await refreshUsage();
  render();
}

/**
 * 拉取使用记录并注入 matcher（matcher 不自己读存储，见 ADR 0002 第 5 条）。
 * 「记一次」发生在后端 launch_app 成功之后的后台线程里，所以新记录要到下一次唤起
 * 才会体现在排序上 —— 那时权重才真正有用，不值得为它碰启动路径。
 */
async function refreshUsage() {
  try {
    const usage = await invoke<UsageMap>("get_usage");
    matcher.setUsage(usage ?? {});
  } catch (e) {
    console.error(e);
  }
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
 * "打开文件所在的位置"——定位到条目**自身**：开始菜单 `.lnk`、便携 exe、PATH 命令
 * 解析出的完整路径，都是磁盘上的真实文件（与 Windows 开始菜单右键的语义一致）。
 *
 * 有意不用 `targetPath`（快捷方式解析出的目标）：那是"程序装在哪"，而用户要的是
 * "快捷方式 / 条目在哪"。
 *
 * 系统设置 URI（`ms-settings:` / `::{CLSID}`）与商店应用（`store:<AUMID>`）只是激活
 * 标识，磁盘上没有文件、无位置可开，返回 null —— 调用方据此不显示该菜单项。
 */
function revealPath(a: AppInfo): string | null {
  const p = a.launchPath.trim();
  return p && !isSystemUri(p) && !isStorePath(p) ? p : null;
}

/// 复位到空态：清空输入、回到第一条、按空态重算窗口高度。
/// 隐藏时和 wake 时各调一次，保证"显示出来的第一帧"永远是干净的。
function resetForNextWake() {
  input.value = "";
  selected = 0;
  browsing = false;
  // 指令模式也要复位：否则下次唤起会直接落在上次那条指令的二级选项里。
  activeCmd = null;
  render();
}

function openContextMenu(a: AppInfo, x: number, y: number) {
  const items: { label: string; run: () => void }[] = [];
  // 没有磁盘位置（系统设置 URI / 商店应用）→ 两项都无意义：不能提权、也无处可开。
  const location = revealPath(a);
  if (location) {
    items.push({ label: "以管理员身份运行", run: () => launch(a, true) });
    items.push({
      label: "打开文件所在的位置",
      run: () =>
        invoke("reveal_in_explorer", { path: location }).catch((e) => console.error(e)),
    });
  }
  if (items.length === 0) return;

  ctxMenu.textContent = "";
  for (const it of items) {
    const el = document.createElement("div");
    el.className = CTX_ITEM;
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

/**
 * 重建整个列表 —— 三种视图共用这一个入口：
 *   应用候选（有查询 / 空态常用）· 指令列表（输入 `/`）· 二级选项（`/theme` 之后）
 *
 * 分派只做一次判断（输入是不是以 `/` 开头），之后各自装配 rowCount / activate；
 * 收尾部分（滚动、窗口高度、提示栏）完全共用。
 */
function render() {
  const raw = input.value;
  // 输入以 `/` 开头 = 指令模式。`/` 之后的内容既筛指令名，也筛二级选项。
  const slash = raw.startsWith("/") ? raw.slice(1) : null;

  // 二级视图只在"输入仍以该指令名为前缀"时成立。用户把 `/theme` 改成 `/light` 的
  // 过程中会自动退回一级 —— 否则会停在一个永远筛不出东西的选项列表里。
  // （`/theme` → `/themel` 仍算前缀命中，所以"继续打字筛选项"不会被误判成退出。）
  if (activeCmd && (slash === null || !slash.startsWith(activeCmd.name))) activeCmd = null;

  list.textContent = "";

  let showList: boolean;
  let rows: number;
  let noHits = false;

  if (slash !== null) {
    filtered = []; // 指令视图里没有应用候选，别让上一轮的结果留在导航层
    showList = true;
    rows = renderSlashView(slash);
    noHits = rowCount === 0;
  } else {
    // filtered 必须等于"当前可见的列表"，这样空态未展开时不会盲启动一条看不见的候选。
    // 三态：有查询 -> 搜索结果；空态展开 -> 常用列表（↓ 触发）；空态收起 -> 空。
    const query = raw.trim();
    // 截断到 resultRows：窗口只装得下这么多行，navigation 就只该在这些行里走。
    // 排序的 tier 保证"打够了它就在前面"（精确/前缀命中置顶），所以截断藏掉的只是模糊
    // 匹配的尾巴——那正是该继续打字、而不是往下滚的场景。顺带让 Alt+序号、图标加载
    // （loadMissingIcons 只吃可见行）和可见范围三者的边界完全一致。
    filtered = query
      ? matcher.search(raw).slice(0, maxRows)
      : browsing
        ? matcher.frequent(maxRows)
        : [];
    // selected 可能是 -1（刚从"指令一览"态退出来）—— 一并归位到有效范围。
    if (selected < 0 || selected >= filtered.length) {
      selected = Math.max(0, filtered.length - 1);
    }
    filtered.forEach((a, i) => list.appendChild(appRow(a, i)));

    // showList 与 filtered 同源：能看见的才可导航，二者必须一致。
    showList = query.length > 0 || browsing;
    // 列表展开了却一条候选都没有（查询无命中）。此时窗口会留一条空白，而 footer 那几条
    // 键位提示全是失效的 —— 放一行提示占住位置，并让 footer 收起那组提示。提示行的高度
    // 就是一条候选，所以下面的 rows 取 max(…, 1) 之后，CHROME 算式一个字都不用动。
    noHits = showList && filtered.length === 0;
    // browsing 时走到这里只可能是索引为空（frequent 是 ranked 切片，有条目就不会空）。
    if (noHits) list.appendChild(noteRow(query ? "无匹配" : "暂无条目"));
    rows = showList ? Math.max(filtered.length, 1) : 0;

    rowCount = filtered.length;
    activate = (i, admin) => {
      const a = filtered[i];
      if (a) launch(a, admin);
    };

    loadMissingIcons(filtered);
  }

  list.style.display = showList ? "block" : "none";
  (list.querySelector("li.active") as HTMLElement | null)?.scrollIntoView({ block: "nearest" });

  // 窗口高度自适应：列表可见 = 输入行 + 可见行数；否则只显示输入行（空态）。
  // 三种视图共用这一条算式 —— 指令行与选项行的行高就是候选行的行高（都走 #results li
  // 的 height/margin），所以 CHROME 一个字都不用改。
  // 每次都 setSize（不缓存）：show 会按 config 尺寸重置窗口，每次 render 强制贴回内容高度。
  const height = CHROME + (showList ? GAP + rows * ROW_H : 0);
  win.setSize(new LogicalSize(640, height)).catch(() => {});

  // 提示栏随状态切换：空态 = 「↓ 常用 / / 指令 / Esc 隐藏」；列表可见 = 「↑↓ 选择 …」；
  // 无命中 = 只剩 Esc（其余键都不成立）。空态下那些键都不生效，显示出来只会误导。
  // 指令视图另有一套文案：那里 Ctrl+↵ 与 Alt+序号 不成立（指令没有"以管理员身份运行"
  // 这回事），Esc 的含义也从"收起"变成"返回"。
  const inSlash = slash !== null;
  hintRecent.hidden = showList;
  hintCmd.hidden = showList;
  hintAdmin.hidden = inSlash;
  enterLabel.textContent = inSlash ? (activeCmd ? "应用" : "确认") : "启动";
  footer.classList.toggle("empty", !showList);
  footer.classList.toggle("no-hits", noHits);
  // Esc 分三级：二级视图先返回指令列表，指令列表 / 常用列表再收起，其余直接隐藏窗口。
  escLabel.textContent = activeCmd ? "返回" : browsing || inSlash ? "收起" : "隐藏";
}

/**
 * 渲染 `/` 视图，返回"窗口要装的行数"。
 *
 * 一级（指令列表）与二级（某条指令的选项）在这里分派 —— 它们共用同一个输入框，
 * 差别只是拿什么当数据源。无命中时也占一行（提示行），与搜索视图同手法，
 * 所以高度算式不必为它开特例。
 */
function renderSlashView(query: string): number {
  const cmd = activeCmd;
  if (cmd) {
    // 输入框此刻是 `/theme …`：**减掉指令名**，剩下的才是选项查询。
    // （`/theme` → 空 → 列全部；`/themel` → "l" → 只剩 light。忘了减就会拿 "theme"
    // 去匹配 auto/light/dark，结果永远是"无匹配"。）
    optRows = filterOptions(cmd.options ?? [], query.slice(cmd.name.length));
    cmdRows = [];
    optRows.forEach((o, i) => list.appendChild(optionRow(cmd, o, i)));
    rowCount = optRows.length;
    activate = (i) => {
      const o = optRows[i];
      if (o) cmd.apply?.(o.value);
    };
    if (rowCount === 0) list.appendChild(noteRow("无匹配"));
    return Math.max(rowCount, 1);
  }

  cmdRows = filterCommands(commands, query).slice(0, maxRows);
  optRows = [];
  // 输入恰好是一个 `/` 时，列表只是"指令一览"：**不预选中任何行**，回车因此不会误触发
  // 第一条（多半是 /theme，白切一次主题）。按 ↑↓ 或悬停即正常选中 —— 选中态由用户建立。
  // 打了字（`/t`）说明意图明确，回到常规的"默认选第一条"。
  // ⚠️ 防误触靠"不预选中"，不能靠"拦回车"：拦了会把"↑↓ 选中后回车"一起挡掉。
  selected = query === "" ? -1 : Math.max(0, selected);
  cmdRows.forEach((c, i) => list.appendChild(commandRow(c, i)));
  rowCount = cmdRows.length;
  activate = (i) => {
    const c = cmdRows[i];
    if (!c) return;
    if (c.options?.length) {
      // 进二级：把输入框规范化成 `/name` —— "继续打字"就是在筛选项，render 顶部的退出
      // 判断也依赖这个前缀（用户当初可能只打了 `/t` 就回车）。
      activeCmd = c;
      input.value = `/${c.name}`;
      selected = 0;
      render();
    } else {
      c.run?.();
    }
  };
  if (rowCount === 0) list.appendChild(noteRow("无此指令"));
  return Math.max(rowCount, 1);
}

/**
 * 构造一行可见项的外壳：序号 + 图标位 + 「名称 / 副标题」两行。
 *
 * 应用候选、指令、二级选项三种行共用它 —— 它们必须同高，因为行高直接参与
 * `rows * ROW_H` 算式（这正是指令列表不必改 CHROME 的原因）。序号位与 Alt+数字的
 * 对应关系对三种行都成立。
 *
 * 行的类配方在 ui-classes.ts；几何与状态留在 style.css 的 `#results li` —— 那里的
 * height/margin 参与 ROW_H 算式，而 :hover / .active 带 #results ID 作用域，特异性
 * (1,1,1) 压得住 (0,1,0) 的原子类，所以"选中态压得住悬停态"不依赖 Tailwind 的输出
 * 排序。`subClass` 只有"当前生效"的选项行需要换（换成强调色那一档）。
 */
function rowShell(
  i: number,
  avatar: HTMLElement,
  name: string,
  sub: string | null,
  subClass: string = ROW_SUB,
): HTMLLIElement {
  const li = document.createElement("li");
  li.className = i === selected ? `${ROW} ${ROW_ACTIVE}` : ROW;

  // 序号：与 Alt+数字 直接对应（1..9，0 = 第 10 项）。超出十项仍按真实序号显示，
  // 只是没有对应的单键可敲。
  const idx = document.createElement("span");
  idx.className = ROW_IDX;
  idx.textContent = String(i + 1);
  li.appendChild(idx);

  li.appendChild(avatar);

  const meta = document.createElement("div");
  meta.className = ROW_META;
  const nameEl = document.createElement("span");
  nameEl.className = ROW_NAME;
  nameEl.textContent = name;
  meta.appendChild(nameEl);
  if (sub) {
    const c = document.createElement("span");
    c.className = subClass;
    c.textContent = sub;
    meta.appendChild(c);
  }
  li.appendChild(meta);
  return li;
}

/** 一条应用候选行：点击启动，右键弹自绘菜单（只有应用有"位置"和"提权"可言）。 */
function appRow(a: AppInfo, i: number): HTMLLIElement {
  const li = rowShell(i, avatarEl(a), a.name, subtitle(a));
  // 点击即启动（此前只选中）。悬停同步选中态见 list 上的 mousemove。
  li.addEventListener("click", () => launch(a));
  // 右键：屏蔽 webview 自带菜单，换成只含两项的自绘菜单。
  li.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    setActive(i);
    openContextMenu(a, e.clientX, e.clientY);
  });
  return li;
}

/**
 * 一条指令行。刻意不挂右键菜单 —— 菜单那两项（提权运行 / 打开所在位置）对指令都不
 * 成立，弹一个点了没反应的菜单不如不弹（与系统项、商店应用同一判断）。
 */
function commandRow(c: Command, i: number): HTMLLIElement {
  const li = rowShell(i, emojiAvatar(c.emoji), `/${c.name}`, c.hint);
  li.addEventListener("click", () => activate(i, false));
  return li;
}

/**
 * 一条二级选项行。当前生效的那项换成强调色副标题，并把「当前」写在最前面 ——
 * "选完生效"必须有这个反馈，否则用户不知道现在生效的是哪一项。
 */
function optionRow(cmd: Command, o: CommandOption, i: number): HTMLLIElement {
  const current = cmd.current?.() === o.value;
  const li = rowShell(
    i,
    emojiAvatar(cmd.emoji),
    o.value,
    current ? `当前 · ${o.hint}` : o.hint,
    current ? ROW_CURRENT : ROW_SUB,
  );
  li.addEventListener("click", () => activate(i, false));
  return li;
}

/** 无命中 / 无此指令时占一行位置的说明。排版与颜色见 ui-classes.ts 的 NOTE。 */
function noteRow(text: string): HTMLLIElement {
  const li = document.createElement("li");
  li.className = NOTE;
  li.textContent = text;
  return li;
}

/** 彩色 emoji 头像（系统项与指令行共用）：不描边、也不参与图标提取。 */
function emojiAvatar(emoji: string): HTMLElement {
  const av = document.createElement("div");
  av.className = "avatar emoji";
  av.textContent = emoji;
  return av;
}

function avatarEl(a: AppInfo): HTMLElement {
  const emoji = systemEmoji(a.launchPath);
  if (emoji) return emojiAvatar(emoji);
  if (a.icon) {
    const img = document.createElement("img");
    img.src = a.icon;
    // 只在某条路径**第一次**拿到图标时淡入。冷启动时这一格此前显示的是字母兜底，
    // 淡入把"字母忽然换成图标"的硬切换成一次渐显（时长/曲线走 --dur / --ease）。
    // 之后的重建（敲键、rescan、index-updated）都直接出图，不再重播。
    if (!iconRevealed.has(a.launchPath)) {
      iconRevealed.add(a.launchPath);
      img.className = "icon-in";
    }
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
      // 后端已经随索引给了图标（磁盘缓存命中）→ 不必再问一次。
      !a.icon &&
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

// 悬停同步选中态（"指到某项再回车"才不跳回第一项）。用 list 级的 mousemove，而不是给每行
// 挂 mouseenter：列表重建后光标下新插入的行会被浏览器补发 mouseenter（指针其实没动），
// 把刚被键盘选中的行弹回去。mousemove 只在真实移动时触发，天然免疫重建。
list.addEventListener("mousemove", (e) => {
  const li = (e.target as Element | null)?.closest("li");
  if (!li) return;
  const i = Array.from(list.children).indexOf(li);
  if (i >= 0) setActive(i);
});

let debounceTimer: number | undefined;
input.addEventListener("input", () => {
  closeContextMenu();
  selected = 0;
  browsing = false; // 一开始打字就退出常用浏览态，回到搜索结果
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

    // Alt + 序号：直接执行第 N 行（0 = 第 10 项）。三种视图通用 —— 指令与选项也有序号，
    // "Alt+2 跳过前一条"是同一个手势。裸数字键必须留给搜索框（"7-Zip"、"360" 这类名字
    // 本身就带数字），所以用 Alt 修饰而不是直接吃数字键。
    if (e.altKey && !e.ctrlKey && !e.metaKey && /^[0-9]$/.test(e.key)) {
      e.preventDefault();
      const i = e.key === "0" ? 9 : Number(e.key) - 1;
      if (i < rowCount) activate(i, false);
      return;
    }

    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (!rowCount && !input.value.trim()) {
        // 空态未展开时，↓ 的第一次按下 = 展开「常用」并选中第 1 项。这就是"向下进入
        // 列表"这一个手势的自然延伸，不引入新键位（Tab 会让入口键与导航键分家）。
        browsing = true;
        selected = 0;
        render(); // 列表从无到有、窗口还要长高 —— 这一步必须整表重建
      } else if (rowCount) {
        // 仅在已有列表内移动高亮时走 setActive，不要 render：整表重建会把光标下的
        // 那一行销毁再插入，指针未动却让浏览器补发 mouseenter → setActive 把选中态
        // 弹回原处（表现为"鼠标悬停在列表上时方向键失灵"）。顺带省一轮图标加载。
        setActive((selected + 1) % rowCount);
      }
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (rowCount) setActive((selected - 1 + rowCount) % rowCount);
    } else if (e.key === "Enter") {
      // 没有选中行时（指令一览态 selected = -1、或列表为空）activate 自己会忽略 ——
      // 防误触交给"不预选中"，这里不再拦回车（拦了会把"↑↓ 选中后回车"一起挡掉）。
      // Ctrl(+Shift)+回车 = 以管理员身份运行（Windows 惯例是 Ctrl+Shift+Enter）。
      // 对指令与选项无意义，activate 的实现忽略这个参数。
      activate(selected, e.ctrlKey || e.metaKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      // 分级退出：关右键菜单 → 二级返回指令列表 → 收起列表（常用 / 指令）→ 隐藏窗口。
      if (!ctxMenu.hidden) {
        closeContextMenu();
        return;
      }
      if (activeCmd) {
        activeCmd = null;
        selected = 0;
        render();
        return;
      }
      if (browsing) {
        browsing = false;
        selected = 0;
        render();
        return;
      }
      if (input.value.startsWith("/")) {
        // 指令模式：先清掉 `/` 回到空态（与"收起常用"同级），再按一次才隐藏窗口。
        input.value = "";
        selected = 0;
        render();
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
  // 上一次启动记下的次数在这里生效（写盘在后端后台线程，见 ADR 0002）。
  // 此刻列表是空态，不需要重绘 —— 下次 render 自然会用上新权重。
  void refreshUsage();
});

// Reload the index when it changes underneath us (tray portable add/remove, rescan).
// The index was already rebuilt in the backend; just re-read the fresh snapshot.
win.listen("index-updated", () => loadApps(false));

// 系统深浅色变化（Tauri 负责推；Windows 上源自 AppsUseLightTheme 注册表监听）。
// 只有 auto 模式需要跟：显式选了 light/dark 时系统怎么变都与本窗口无关。
void win.onThemeChanged(({ payload }) => {
  systemTheme = payload;
  if (themePref === "auto") applyTheme();
});

// Hot-apply a settings.json change the user made by hand (backend watches it).
// 指令写回走的是另一条路（applyThemeChange），拿不到这个事件 —— 理由见那里的注释。
win.listen<Settings>("settings-changed", (e) => applyConfig(e.payload));

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
  // 系统主题先就位：Tauri 的初值比 matchMedia 可靠（理由见 applyTheme 上方）。
  systemTheme = (await win.theme().catch(() => null)) ?? systemTheme;
  let settings: Settings | null = null;
  try {
    settings = await invoke<Settings>("get_config");
    applyFont(settings.font);
    if (typeof settings.resultRows === "number" && settings.resultRows > 0) {
      maxRows = Math.floor(settings.resultRows);
    }
  } catch (e) {
    console.error(e);
  }
  // 首次应用等配置读完（或读取失败）之后：否则会先按默认深色画一帧、再跳成浅色。
  applyThemeInputs(parseThemePref(settings?.theme), settings?.colors);
  await loadApps(true);
  input.focus();
})();
