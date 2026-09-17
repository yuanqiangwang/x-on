/**
 * Tailwind 类配方 —— 集中一处，供 main.ts 拼装 DOM 时使用。
 *
 * ---------------------------------------------------------------------------
 * 为什么单独一个文件
 * ---------------------------------------------------------------------------
 * Tailwind v4 的文本扫描器会把**注释和代码字符串**里"长得像类名"的 token 当成候选类，
 * 生成一堆没有任何元素引用的 CSS。当初自动扫描整个项目时实测：`@layer utilities` 里
 * 58 个类名有 28 个无引用，构成 26 条规则、2350 字节，占产物的 **19.9%**。
 *
 * 两个来源，都躲不掉：
 *   1. CSS 文件的注释 —— 注释里必须提类名（不提就没法读），一提就被扫描。
 *   2. main.ts 的代码字符串 —— 给 DOM 属性赋的字面量会被当成同名工具类的候选。
 *
 * 所以解法不是改注释（会反复回归），而是**收紧扫描范围**：见 src/style.css 顶部的
 * `source(none)` + 两条 `@source`。本文件与 index.html 是当前唯一的两个来源。
 *
 * ⚠️ 代价与纪律（两条，都是静默失败 —— 构建不报错，只是结果不对）：
 *   1. 新增任何"带 Tailwind 类名的文件"，必须同步加进 style.css 的 `@source` 列表，
 *      否则那些类不会被生成，样式直接没了。
 *   2. **本文件自身就在扫描范围内**，所以这里的注释只允许出现本文件真正用到的类名。
 *      想举例一个"故意不用"的类名（反例），写进 style.css —— 它不在 @source 里，
 *      注释提什么都没人扫（已实测：它注释里那几个任意值类名一个都没进产物）。
 *
 *   第 2 条是踩过坑才写下的：本文件曾用反例解释"为什么不用某个字重类"，结果那个类
 *   被真的生成了，还顺带拖出一条 @property 注册 —— 注释把想否定的东西实现了。
 *   本文件比 style.css 的注释更危险，正因为它在扫描范围内。
 *
 * 命名：常量名描述**角色**（ROW_NAME 是候选名），不是外观。外观改了只动常量值。
 */

/**
 * 候选行本体。外观走原子类；几何（height/margin/border/overflow）与状态
 * （:hover / .active / .active .name / .active .idx）留在 style.css 的 `#results li`
 * —— 那里的 height/margin 直接参与 main.ts 的 rows * ROW_H 算式，写成"h- 加一个
 * calc 任意值"只会把同一个算式藏进 class 字符串里，读的人反而看不出它和 ROW_H 的关系。
 */
export const ROW = "rounded-md text-fg transition-colors";

/**
 * 选中态钩子。**不是工具类** —— 由 setActive() 用 classList.toggle 切换，
 * render() 用它做 `querySelector("li.active")` 定位当前行。
 * Tailwind 里没有"元素带某 class"这种变体，所以它必须是个真的类名。
 */
export const ROW_ACTIVE = "active";

/**
 * 序号（Alt+数字速启的对应关系）。text-meta 是 12/16 那一档，与副标题、footer 共用。
 * 定宽在 CSS 里（布局几何，15px —— 要装得下两位数，理由见 style.css）。
 * tabular-nums 在这张字上是空操作（YaHei 没有 tnum 特性），但数字本来就等宽，留着无害。
 */
export const ROW_IDX = "idx text-meta text-muted tabular-nums text-center select-none";

/**
 * 名称 + 副标题的纵向容器。
 * `min-w-0` 是下面两个 truncate 生效的前提：弹性项默认 min-width:auto，
 * 不归零就会撑破容器、省略号出不来。
 */
export const ROW_META = "meta flex flex-col min-w-0";

/**
 * 候选名：14/1.25，全 UI 最高对比度（15:1），超长单行截断。
 *
 * ⚠️ 刻意不加 500 字重（Tailwind 那个字重工具类）：本机 YaHei 只发行 400 / 700 / 290
 *    三张字重，**没有 500**，那个类会静默退回 400 —— 不报错，写了等于没写。
 *    700 技术上存在，但候选名已经靠对比度拿到最高优先级，再叠字重是双重强调；
 *    且 14px 粗体汉字在深底上有膨胀感、会显糊，8 行密排时反而削弱"选中行"这个唯一焦点。
 *    结论：层级只用字号 + 颜色两轴，不用字重。实测数据与那个类名见 style.css 的排版刻度。
 */
export const ROW_NAME = "name text-body text-fg-strong truncate";

/**
 * 副标题（本地化名下面的原生名，见 main.ts 的 subtitle）。
 *
 * text-meta 的 16px 行高不是随手选的：它与 .name 的 17.5px 相加必须 ≤ 34px 的
 * 内容盒（17.5 + 16 = 33.5 ✓），而 16px 恰好又是 footer 参与 CHROME 算式的那个值 ——
 * 两处需求撞在同一个数上，所以小字只留一个令牌。改它要同步改 main.ts 的 CHROME。
 * 超长单行截断。
 */
export const ROW_SUB = "sub text-meta text-muted truncate";

/**
 * 无命中提示（Step 3d）：查询一条都没匹配上时，在列表里占一行候选的位置。
 * 复用小字档并压到次级色 —— 它是一条状态说明，不是候选。
 *
 * `empty-note` 不是工具类，是 main.ts 与 style.css 之间的钩子；style.css 那条规则
 * 只管 pointer-events: none（挡掉悬停高亮与 mousemove 的选中同步），排版与颜色在这里。
 */
export const NOTE = "empty-note text-meta text-muted";

/**
 * 右键菜单项（Step 3c）。外观走原子类；项内的 padding 与 line-height（定死 26px 行高，
 * 贴 Windows 菜单的手感）、`white-space: nowrap` 和悬停态留在 style.css ——
 * 行高不跟 `text-meta` 走（那是 16px，会得到 24px 的项高），它是这个组件的几何。
 *
 * `item` 不是工具类，是 style.css 里悬停态规则的钩子（与 ROW_ACTIVE 同理）。
 */
export const CTX_ITEM = "item rounded-md text-fg transition-colors";
