/**
 * Tailwind 类配方 —— 集中一处，供 main.ts 拼装 DOM 时使用。
 *
 * ---------------------------------------------------------------------------
 * 为什么单独一个文件
 * ---------------------------------------------------------------------------
 * Tailwind v4 的文本扫描器会把**注释和代码字符串**里"长得像类名"的 token 当成候选类，
 * 生成一堆没有任何元素引用的 CSS。实测：`@layer utilities` 里 58 个类名有 28 个无引用，
 * 构成 26 条规则、2350 字节，占产物的 **19.9%**。
 *
 * 来源是两类，而且都躲不掉：
 *   1. `style.css` 自己的注释 —— 注释里必须提类名（不提就没法读），一提就被扫描。
 *   2. `main.ts` 的代码字符串 —— `list.style.display = "block"`、`ctxMenu.hidden`
 *      这类写法会命中 `.block` / `.hidden`。
 *
 * 所以解法不是改注释（会反复回归），而是**收紧扫描范围**：见 src/style.css 顶部
 * 的 `source(none)` + `@source` 两条指令。
 *
 * ⚠️ 代价与纪律：新增任何"带 Tailwind 类名的文件"，必须同步加进 src/style.css 的
 *    `@source` 列表，否则那些类不会被生成 —— **静默失效，构建不报错，只是样式没了**。
 *    本文件与 index.html 是当前唯一的两个来源。
 *
 * 命名：常量名描述**角色**（ROW_NAME 是候选名），不是外观。外观改了只动常量值。
 */

/**
 * 候选行本体。外观走原子类；几何（height/margin/border/overflow）与状态
 * （:hover / .active / .active .name / .active .idx）留在 style.css 的 `#results li`
 * —— 那里的 height/margin 直接参与 main.ts 的 rows * ROW_H 算式，写成
 * `h-[calc(var(--row-h)-2px)]` 只会把同一个算式藏进字符串里。
 */
export const ROW = "rounded-md text-fg transition-colors";

/**
 * 选中态钩子。**不是工具类** —— 由 setActive() 用 classList.toggle 切换，
 * render() 用它做 `querySelector("li.active")` 定位当前行。
 * Tailwind 里没有"元素带某 class"这种变体，所以它必须是个真的类名。
 */
export const ROW_ACTIVE = "active";

/**
 * 序号（Alt+数字速启的对应关系）。text-hint 是 11/16 那一档，与 footer 共用；
 * 定宽 12px 在 CSS 里（布局几何）。tabular-nums 让位数变化不撑动后面的图标。
 */
export const ROW_IDX = "idx text-hint text-muted tabular-nums text-center select-none";

/**
 * 名称 + 副标题的纵向容器。
 * `min-w-0` 是下面两个 truncate 生效的前提：弹性项默认 min-width:auto，
 * 不归零就会撑破容器、省略号出不来。
 */
export const ROW_META = "meta flex flex-col min-w-0";

/** 候选名：14/1.25，强调色文字，超长单行截断。 */
export const ROW_NAME = "name text-body text-fg-strong truncate";

/**
 * 副标题（本地化名下面的原生名，见 main.ts 的 subtitle）。
 * 名称与副标题共占一行行高，超长单行截断。
 */
export const ROW_SUB = "sub text-meta text-muted truncate";
