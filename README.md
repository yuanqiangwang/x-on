# xon — 一个纯粹的 Windows 启动器

按 `Alt+Space` 唤起，输入名字，回车启动。

## 功能

- **拼音级检索**：`wx` → 微信，`blbl` → 哔哩哔哩，`gc` → 谷歌 Chrome。按 `精确 > 前缀 > 全拼 > 首字母 > 子串 > 子序列` 分级匹配，CJK 感知，支持同音。
- **开始菜单 + 系统功能**：索引开始菜单 `.lnk`，取本地化显示名、英文原生名作别名，中英双语可检索；内置系统设置入口（`ms-settings:`）与「此电脑 / 回收站」。
- **便携应用**：托盘「添加便携应用…」加入任意 `.exe`，随索引实时呈现。
- **热配置**：`settings.json` 改字体、候选行数**即时生效**；开机自启即时生效，`accelerator` 需重启。
- **自动刷新索引**：监视开始菜单与便携目录，装/卸应用或增删 `.exe` 自动重扫，无需手动。
- **图标原生提取**：`SHGetFileInfoW` 取系统图标渲染进列表，不借用第三方。

## 快速开始

```bash
npm install
npm run tauri dev      # 开发（vite + 热重载）
npm run tauri build    # 构建 release
```

构建产物在 `src-tauri/target/release/xon.exe`。

## 配置

配置文件 `%APPDATA%\com.xon.launcher\settings.json`（托盘右键「打开配置文件…」直达），JSONC 格式，初始自带中文注释：

```jsonc
{
  // 全局唤醒快捷键，改后需重启生效（仅启动时注册）
  "accelerator": "Alt+Space",
  // 开机自启：改后立即生效
  "autostart": false,
  // 界面字体：系统已安装字体名，改后立即生效；留空 = 系统默认
  "font": "",
  // 列表最大候选行数：改后立即生效（窗口高度自适应）
  "resultRows": 6
}
```

## 技术栈

Tauri v2 / Rust · TypeScript · Vite · pinyin-pro · walkdir · lnk · notify

- 索引：开始菜单 `.lnk` 遍历（`walkdir` + `lnk`）+ 便携 manifest + 系统功能清单
- 图标：`SHGetFileInfoW` + GDI → BGRA → PNG base64
- 热配置：`notify` 监听 `settings.json` → `emit settings-changed` → 前端换 CSS 变量
