<div align="center">

  <!-- Logo -->
  <a href="https://github.com/yuanqiangwang/x-on">
    <img src="https://raw.githubusercontent.com/yuanqiangwang/x-on/main/docs/assets/logo.png" alt="xon Logo" width="100" height="100" onError="this.style.display='none'">
  </a>

  <h1 align="center">xon</h1>

  <p align="center">
    <strong>面向中文环境的 Windows 应用启动器 —— 专注拼音搜索，按键即达。</strong>
    <br />
    <em>基于 Tauri v2 与原生 TypeScript 构建：轻量、零延迟自适应窗口、深度中文拼音匹配。</em>
  </p>

  <!-- Badges -->
  <p align="center">
    <a href="https://github.com/yuanqiangwang/x-on/releases"><img src="https://img.shields.io/github/v/release/yuanqiangwang/x-on?style=flat-square&color=3dff9e" alt="Latest Release"></a>
    <a href="https://github.com/yuanqiangwang/x-on/blob/main/LICENSE"><img src="https://img.shields.io/github/license/yuanqiangwang/x-on?style=flat-square&color=green" alt="License"></a>
    <a href="https://github.com/yuanqiangwang/x-on/stargazers"><img src="https://img.shields.io/github/stars/yuanqiangwang/x-on?style=flat-square&logo=github" alt="Stars"></a>
  </p>

  <p align="center">
    <a href="#核心特性">核心特性</a> •
    <a href="#中文优先">中文优先</a> •
    <a href="#性能">性能</a> •
    <a href="#匹配引擎">匹配引擎</a> •
    <a href="#快速开始">快速开始</a> •
    <a href="#快捷键">快捷键</a>
  </p>

  ---
</div>

## xon 是什么

xon 是一款为中文 Windows 环境设计的极速应用启动器。

按下 `Alt + Space` 唤出面板，输入几个字母或拼音，回车即可启动 —— 开始菜单程序、便携应用、系统设置、命令行工具，都在同一个入口里。

它的目标只有一个：**把「打开一个程序」这件事做到最快**。为此它没有扩展系统、没有账号、没有联网，也不依赖任何前端框架。

> *输入 `jsq` 就能打开「计算器」，输入 `wx` 就能打开「微信」。*

## 中文优先

这是 xon 最根本的设计前提。

以英文环境为第一目标的启动器，对中文应用的匹配基本停留在**字面子串**层面，中文用户的真实输入习惯被系统性地忽略了。xon 针对中文环境重新设计了整套匹配逻辑：

- **拼音输入直达应用。** 「微信」可用 `wx`、`weixin` 命中；「钉钉」可用 `dd`、`dingding` 命中。搜索框里不需要先想清楚应用的中文全名。
- **全拼、首字母、词首字母三路并行。** 分别覆盖 `googlechrome`、`gc`、`g-chrome` 这类不同习惯的输入。
- **同音词抑制。** 输入具体汉字时（如「钉钉」），同名拼音的无关候选（如「丁丁」）会被抑制，不再污染结果列表。
- **本地化名称补英文别名。** Windows 的中文条目只有中文显示名（如「控制面板」），xon 会自动补上英文别名，`control panel`、`calc`、`devmgmt.msc` 均可用英文命中。
- **系统功能与 PATH 命令一并纳入。** 「此电脑」「回收站」等 shell 项，以及 37 条系统命令（设备管理器、磁盘管理、远程桌面等）都在索引内，多数并非开始菜单直接提供。

一句话：**面向中文环境、并以此为前提做设计**，而不是给英文交互加一层中文翻译。

## 核心特性

- **中文拼音搜索** —— 自定义多级匹配，支持汉字、全拼、首字母、词首字母与别名。
- **同音抑制** —— 输入确切汉字时优先字面命中，剔除无关拼音候选。
- **零抖动自适应窗口** —— 按结果行数逐像素调整高度，无边框、无重排闪烁。
- **批量图标提取与降级** —— 通过 IPC 批量并行提取应用图标并自动重试；系统项降级为主题化 emoji，避免单调的白色文档图标。
- **原生 Windows 集成** —— 右键「以管理员身份运行」「打开文件所在的位置」，失焦自动隐藏，单实例唤出。
- **主题与配色可配** —— 默认跟随 Windows 的浅色/深色实时切换（终端绿 / 纸白两套）；可固定一端，也可覆盖 6 个基础色自定义整套配色，派生色自动跟随。
- **精简技术栈** —— Tauri v2 后端 + 手写 DOM/TypeScript 前端，无框架、无 UI 库、无 CSS 框架。

## 性能

以下数据由 `pnpm bench` 生成。该脚本完全从外部驱动 xon —— 注入合成按键并轮询 Win32 窗口，应用内部无任何埋点，因此结果可复现、不依赖应用自身的度量。各指标的边界见 [`benchmarks/README.md`](benchmarks/README.md)。

_测量于 2026-09-10，Windows 11 家庭版（Intel Core Ultra 7 255H，32 GB），release 构建 `0.6.0`，在空闲机器上经由其注册的全局热键唤起。黑盒测量：合成按键 → Win32 窗口轮询，p50/p95 分别取 25 与 15 次样本。完整环境与原始样本见 `benchmarks/results/latest.md`。_

<!-- benchmarks:start -->

| 指标 | xon 0.6.0 |
| --- | --- |
| 热键 → 窗口可见（p50） | 8.9 ms |
| 热键 → 窗口可见（p95） | 14.6 ms |
| 按键 → 结果呈现（p50） | 63.3 ms |
| 按键 → 结果呈现（p95） | 72.5 ms |
| 冷启动 → 可用 | n/a |
| 内存，闲置（私有） | 276.7 MB |
| 内存，闲置（工作集） | 523.6 MB |
| 闲置 CPU 占用 | 0.10 % |
| 安装包体积 | 1.58 MB |

<!-- benchmarks:end -->

两项预算分开计量，因为它们是两个不同的问题：**热键 → 可见**（面板出现的速度）与**按键 → 结果**（输入时用户实际感受到的延迟，包含输入防抖、列表重建与图标回传，通常慢数倍）。

在约 66 ms 的按键延迟中，约 40 ms 来自 `src/main.ts` 中有意设置的输入防抖；真正的过滤 + 渲染 + 自适应调整约为 27 ms。该防抖是可调参数而非下限 —— 它的作用是让快速连续输入只触发一次渲染，而不是每次按键都渲染。


## 匹配引擎

xon 采用定制的 7 级评分算法，对显示名与系统别名在一次遍历中同时评分：

```
第 1 级：名称精确匹配          （如 "cmd" -> "cmd"）
第 2 级：名称前缀匹配          （如 "post" -> "Postman"）
第 3 级：全拼前缀匹配          （如 "dingding" -> "钉钉"）
第 4 级：词首字母匹配          （如 "gc" -> "Google Chrome"）
第 5 级：拼音首字母匹配        （如 "dd" -> "钉钉"）
第 6 级：子串 / 全名包含       （如 "panel" -> "控制面板"）
第 7 级：模糊子序列匹配        （如 "vsc" -> "Visual Studio Code"）
```

匹配策略全部集中在 `src/matcher.ts` 这一「深模块」中：对外仅暴露 `index()` 与 `search()`，评分逻辑可独立、确定性地进行单元测试。

## 快捷键

| 快捷键 | 动作 | 说明 |
| --- | --- | --- |
| **`Alt + Space`** | **唤出启动器** | 将 xon 带到前台（默认全局快捷键，可配置） |
| **`↑` / `↓`** | **切换结果** | 在候选中上下移动 |
| **`Enter`** | **启动选中项** | 执行目标应用或系统 URI |
| **`Ctrl + Enter`** | **以管理员身份运行** | 提权启动选中的应用 |
| **`Alt + 1..9, 0`** | **快速启动** | 直接启动第 1 至第 10 项结果 |
| **`Esc`** | **关闭 / 隐藏** | 关闭右键菜单或隐藏启动器窗口 |

## 快速开始

### 环境要求

* [Node.js](https://nodejs.org/)（v18+）
* [Rust](https://www.rust-lang.org/)（Tauri v2 所需）
* WebView2 运行时（Windows 10/11 已预装）

### 安装与开发

```bash
# 1. 克隆仓库
git clone https://github.com/yuanqiangwang/x-on.git
cd x-on

# 2. 安装前端依赖
pnpm install

# 3. 以开发模式运行
pnpm tauri dev
```

### 构建发布版

```bash
pnpm tauri build
```

构建产物位于 `src-tauri/target/release/`，同时生成 NSIS 安装包。

## 配置

配置文件位于 `%APPDATA%\com.xon.launcher\settings.json`，支持随时修改 `font`、`resultRows`、`theme` 与 `colors` 而无需重启：

```jsonc
{
  "accelerator": "Alt+Space",  // 全局快捷键，仅启动时读取，修改后需重启
  "autostart": true,           // 开机自启
  "font": "JetBrains Mono NF", // 界面字体
  "resultRows": 6,             // 最大结果行数
  "theme": "auto",             // 主题：auto = 跟随 Windows 浅色/深色，或固定 light / dark
  "colors": {                  // 自定义配色：只填想改的基础色
    "accent": "#ff7a45"        // 选中行底、描边、渐隐线等派生色会自动跟着算
  }
}
```

`colors` 可覆盖 **6 个基础色**：`bg`、`bg-raised`、`text`、`text-head`、`muted`、`accent`。其余颜色全部由它们派生 —— 所以换强调色只需要填一项，不会出现"强调色换了、派生色还是绿的"这种半截配色。取值是任意 CSS 颜色（`#hex`、`rgb()`、命名色都可以），自定义项对浅色/深色两套主题同时生效。

> 配色请自行保证对比度：输入行与选中行是仅有的两处满强度强调色，用浅色强调色配浅色底会让它们直接读不出来。

便携应用清单独立存放于同目录下的 `apps.json`，可通过托盘菜单「添加便携应用」维护。

## 许可

本项目基于 [MIT License](LICENSE) 开源。

## 相关文档

- [`CONTEXT.md`](CONTEXT.md) —— 领域词汇表（扫描根、便携应用、AppInfo、去重、唤出等）。
- [`docs/adr/0001-opaque-window-with-dom-glass.md`](docs/adr/0001-opaque-window-with-dom-glass.md) —— 窗口为何不透明，以及 Windows 毛玻璃的取舍。
- [`docs/roadmap.md`](docs/roadmap.md) —— 路线图与明确的「不做」清单。
- [`benchmarks/README.md`](benchmarks/README.md) —— 性能测量协议与指标解读。
