/**
 * `/` 指令模式的数据表。
 *
 * ---------------------------------------------------------------------------
 * 依赖方向：能力注入，不反向 import
 * ---------------------------------------------------------------------------
 * 指令要干的事（写配置、应用主题、打开设置文件）都住在 main.ts。为了不让
 * main.ts ←→ commands.ts 互相 import，这里不 import 任何东西：能力由
 * `buildCommands(host)` 注入。
 *
 * 好处是这张表能单独读、单独扩 —— 加一条指令不必碰 main.ts 的渲染逻辑。
 * 唯一的约定是 `CommandHost` 这个窄接口。
 *
 * ---------------------------------------------------------------------------
 * 两种形态
 * ---------------------------------------------------------------------------
 *   · 有 `options` → 回车进入二级选项列表，选中即生效（`apply`），列表里标出当前值
 *   · 无 `options` → 回车直接执行（`run`）
 *
 * 首批只有主题一族（2026-09-27 定稿），但形态是照"以后要扩"留的：
 * 列表与启动（`/rows` `/autostart`）、索引与数据（`/rescan`）、文件（`/config`）
 * 都是同一种结构，加进来只是往下面那个数组里补项。
 *
 * 刻意**不做**破坏性指令（`/usage-clear` `/quit`）：启动器是"随手回车"的场景，
 * docs/roadmap.md 第 124–135 行对"命令直通"记过同一条担忧。等有了确认态再说。
 *
 * ---------------------------------------------------------------------------
 * 为什么不用 matcher.ts
 * ---------------------------------------------------------------------------
 * 指令名是纯 ASCII slug（用户拍板），没有拼音需求，也不该进 frecency 权重 ——
 * 那套 7 层打分是给应用名准备的。指令总共十来条，前缀优先 + 包含兜底就够了，
 * 见下面的 `filterCommands`。
 */

/** 二级选项。`value` 既是显示名，也是要写进配置的值。 */
export type CommandOption = {
  value: string;
  /** 一句话说明，占候选行的副标题位。 */
  hint: string;
};

export type Command = {
  /** 指令名，不含前导 `/`。纯英文 slug。 */
  name: string;
  /** 一句话说明，占候选行的副标题位。 */
  hint: string;
  /** 行首图标（emoji）。指令没有 launchPath，不参与图标提取，与系统项同手法。 */
  emoji: string;
  /** 二级选项。有 = 回车进入选择；无 = 回车直接执行。 */
  options?: CommandOption[];
  /** 仅 options 型：当前生效值，用于在列表里标「当前」。 */
  current?: () => string;
  /** 仅 options 型：选中某项后的动作。 */
  apply?: (value: string) => void;
  /** 仅直接执行型：动作。 */
  run?: () => void;
};

/** 指令需要的外部能力，由 main.ts 注入（避免双向 import）。 */
export type CommandHost = {
  /** 当前主题偏好。取值与 settings.json 的 `theme` 一致。 */
  currentTheme: () => string;
  /** 写盘 + 立即生效。 */
  setTheme: (theme: string) => void;
  /** 打开 settings.json（系统默认程序）。 */
  openSettings: () => void;
};

/**
 * 主题三档。
 * ⚠️ 这三个值在四处必须一致：这里、main.ts 的 `ThemePref`、后端 `set_theme` 的白名单、
 * settings.json 的模板注释。加档要四处一起改。
 */
const THEME_OPTIONS: CommandOption[] = [
  { value: "auto", hint: "跟随 Windows" },
  { value: "light", hint: "固定浅色" },
  { value: "dark", hint: "固定深色" },
];

/**
 * 构建指令表。
 *
 * `/theme` 与 `/light` `/dark` `/auto` 是刻意并存的两条路径：前者用来"看一眼现在是什么
 * 再决定"，后者用来"我已经想好了，一步到位"。少任何一条都会让某一类场景多一步。
 */
export function buildCommands(host: CommandHost): Command[] {
  return [
    {
      name: "theme",
      hint: "切换主题",
      emoji: "🌗",
      options: THEME_OPTIONS,
      current: host.currentTheme,
      apply: host.setTheme,
    },
    { name: "light", hint: "切到浅色", emoji: "☀️", run: () => host.setTheme("light") },
    { name: "dark", hint: "切到深色", emoji: "🌙", run: () => host.setTheme("dark") },
    { name: "auto", hint: "跟随 Windows", emoji: "🔄", run: () => host.setTheme("auto") },
    { name: "config", hint: "打开 settings.json", emoji: "📄", run: host.openSettings },
  ];
}

/**
 * 指令名检索：前缀命中排在包含命中之前，组内保持定义序。
 *
 * 空查询返回全部 —— 输入一个 `/` 就是"把指令列出来"，这就是指令的发现路径，
 * 不需要另做 `/help`。
 */
export function filterCommands(commands: Command[], query: string): Command[] {
  const q = query.trim().toLowerCase();
  if (!q) return commands;
  const prefix: Command[] = [];
  const contains: Command[] = [];
  for (const c of commands) {
    const name = c.name.toLowerCase();
    if (name.startsWith(q)) prefix.push(c);
    else if (name.includes(q)) contains.push(c);
  }
  return [...prefix, ...contains];
}

/** 二级选项的过滤。选项值都很短，前缀匹配足够，不做包含匹配（否则 `/theme a` 会把
 *  `dark` 也捞出来 —— 那是噪音，不是帮助）。 */
export function filterOptions(options: CommandOption[], query: string): CommandOption[] {
  const q = query.trim().toLowerCase();
  if (!q) return options;
  return options.filter((o) => o.value.toLowerCase().startsWith(q));
}
