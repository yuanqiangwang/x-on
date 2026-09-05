# x-on — 极速启动器


**x-on 一个极简Windows 启动器**

[![Tauri](https://img.shields.io/badge/Tauri%20v2-285780?style=flat-square&logo=tauri&logoColor=white)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![TypeScript](https://img.shields.io/badge/TypeScript-3178c6?style=flat-square&logo=typescript&logoColor=white)](https://www.typescriptlang.org)
![Windows](https://img.shields.io/badge/Windows-0078d6?style=flat-square&logo=windows&logoColor=white)

---

## 拼音级模糊

你不需要精确记住名字，只要记得形状。

```
› wx          →   微信
› 微          →   微信  微信开发者工具
› gj          →   谷歌 Chrome
› blbl        →   哔哩哔哩
```

x-on 按 `名称精确 > 前缀 > 全拼 > 首字母 > 子串 > 子序列` 分级匹配（CJK 感知，整段喂给 pinyin-pro），并权衡命中强度排序——短线命中最优先。

## 特性

| 项 | 说明 |
|---|---|
| ⚡ 亚秒唤起 | `Alt+Space` 全局快捷键，单实例，窗口常驻 |
| 🧠 拼音模糊 | 全拼 / 首字母 / 子序列，CJK 感知 |
| 🖼 原生图标 | `SHGetFileInfoW` + GDI 即时提取，非截图、非缓存假象 |
| 🎨 neon-geek | 终端绿 + 近黑，内置霞鹜文楷屏显版 |
| ⚙️ 配置即一切 | 单一 `settings.json`（JSONC，可写注释），font / autostart 热更新 |
| 🧩 便携应用 | 把任意 `.exe` 塞进索引（托盘右键即可） |
| 🔍 自动索引 | 扫描开始菜单 `.lnk` + 便携清单，开机后台重建 |
| 🔒 开机自启 | 配置一行开关，写 / 删系统自启项 |

## 快速开始

```bash
npm install
npm run tauri dev      # 开发（vite + 热重载）
npm run tauri build    # 构建 release（产物在 src-tauri/target/release/）
```

启动后按 `Alt+Space`，输入，回车。感觉它。

## 配置

只有一个文件：`%APPDATA%\com.xon.launcher\settings.json`（托盘右键「打开配置文件…」直达）。

```jsonc
{
  // 全局唤醒快捷键，改后需重启生效（仅在启动时注册）
  "accelerator": "Alt+Space",
  // 开机自启：改后立即生效（写入/删除系统自启项）
  "autostart": false,
  // 界面字体：系统已安装字体名，改后立即生效；留空 = 内置霞鹜文楷
  "font": ""
}
```

- **font**：填你系统里任何字体名，如 `"font": "Cascadia Mono"`；留空用内置霞鹜文楷。改配置**即时生效**（`notify` 监听 + 热更新）。
- **accelerator**：改了要重启。
- 初始文件自带中文注释说明，像 VS Code 的 `settings.json`。

## 技术栈

**Tauri v2 / Rust** · TypeScript · Vite · pinyin-pro · walkdir · lnk · notify

- 索引：Windows 开始菜单 `.lnk` 遍历（`walkdir` + `lnk`）+ 便携 manifest
- 图标：`SHGetFileInfoW` + GDI → BGRA → PNG base64（原生提取）
- 热配置：`notify` 监听 `settings.json` → `emit settings-changed` → 前端换 CSS 变量

## 许可

- 内置字体 **霞鹜文楷**（屏显版 GB）：[SIL Open Font License 1.1](public/fonts/OFL.txt)

> 启动器是用来消失的。最成功的启动器，你永远感觉不到它。
