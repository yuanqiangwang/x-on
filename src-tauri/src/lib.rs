mod icons;
mod portable;
mod system;

use portable::{file_stem, launch_portable, load_manifest, parse_portable, save_manifest, PortableEntry};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::menu::{IsMenuItem, Menu, MenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use walkdir::WalkDir;

const DEFAULT_ACCELERATOR: &str = "Alt+Space";

/// Marks a shortcut as scan noise — uninstallers, help/readme links, updaters.
/// Matched case-insensitively against the shortcut's visible name (`file_stem`)
/// and its resolved target's *file name* only (so a legit app under a folder
/// literally named "update" still survives). Keep this list lean: over-broad
/// entries could drop real apps.
const JUNK_KEYWORDS: &[&str] = &[
    // uninstallers
    "uninstall",
    "uninstaller",
    "unins",
    "remove",
    // help / readme / updater links
    "help",
    "readme",
    "update",
    "updater",
    "website",
    // documentation — English standalone doc links (safe: no real product is
    // named "…documentation"/"…manual"/"…guide")
    "documentation",
    "documents",
    "manual",
    "guide",
    // Chinese doc-noise as *phrases*, deliberately NOT the bare 文档 — that word
    // is part of real apps (腾讯文档 / 金山文档 / 石墨文档) and would drop them.
    "帮助文档",
    "使用文档",
    "用户手册",
    "使用手册",
    "使用说明",
    // Chinese uninstall / help
    "卸载",
    "卸载程序",
    "卸载工具",
    "帮助",
];

/// The mutable, in-memory app index. Scanned once at startup and cached here.
#[derive(Default)]
struct Index {
    apps: Mutex<Vec<AppInfo>>,
}

impl Index {
    /// Replace the whole index atomically. The index is always a complete
    /// snapshot — never a partial one — so every rebuild funnels through here.
    fn reload(&self, apps: Vec<AppInfo>) {
        *self.apps.lock().unwrap() = apps;
    }
}

/// The tray icon, held so we can swap its menu (the portable submenu changes) via
/// `set_menu` instead of building a second icon.
#[derive(Default)]
struct TrayHandle(Mutex<Option<TrayIcon<tauri::Wry>>>);

/// Cache of extracted app icons, keyed by launch path. Extraction is expensive
/// (shell + GDI + PNG + base64), so we keep results — including misses — in
/// memory to avoid re-extracting on every keystroke.
#[derive(Clone, Default)]
struct IconCache {
    map: Arc<Mutex<HashMap<String, Option<String>>>>,
}

/// Tracks the last settings.json text this process wrote itself, so the file
/// watcher can skip the notification caused by our own write (avoiding a
/// write→read→write loop when we persist config on behalf of a tray action).
#[derive(Default)]
struct ConfigWatcher {
    own_write: Mutex<Option<String>>,
}

/// Last autostart value we pushed to the OS, so the file watcher only re-applies
/// when the config actually changed (avoiding redundant enable/disable per save).
#[derive(Default)]
struct AppliedAutostart(Mutex<Option<bool>>);

/// The index filesystem watcher. Held in state (not a bare local) so it lives for
/// the process lifetime, and so the watcher callback can tell whether a change is
/// relevant without re-reading the manifest. `dirs` is the set of folders watched.
#[derive(Default)]
struct IndexWatcher {
    watcher: Mutex<Option<Box<dyn notify::Watcher + Send>>>,
    dirs: Mutex<HashSet<PathBuf>>,
}

/// An entry in the launcher index — a single launchable app.
///
/// `launch_path` is the `.lnk` itself (what we hand to the OS to launch).
/// `target_path` is the shortcut's resolved target, used to deduplicate apps
/// that share a program. `icon` is a base64 PNG data URI, filled lazily by the
/// frontend via the `get_app_icons` command.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub launch_path: String,
    pub target_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// 额外的检索名（如系统工具的英文原生名 "Control Panel"），只参与匹配、不计入显示。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// Launcher settings, read from the app config dir on startup.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default = "Config::default_accelerator")]
    accelerator: String,
    /// Whether to register the app to launch at login. Off by default; the user
    /// opts in from the tray menu so the app never silently self-starts.
    #[serde(default)]
    autostart: bool,
    /// UI font family, mirroring VS Code's editor.fontFamily: a system font name
    /// the user types into settings.json. Empty means the system default. Only
    /// `font` is hot-applied when the file is watched.
    #[serde(default)]
    font: String,
    /// Max results shown in the launcher list; the window height follows it.
    /// Hot-applied when the file is watched. Default 6.
    #[serde(default = "Config::default_result_rows")]
    result_rows: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            accelerator: DEFAULT_ACCELERATOR.to_string(),
            autostart: false,
            font: String::new(),
            result_rows: Config::default_result_rows(),
        }
    }
}

impl Config {
    fn default_accelerator() -> String {
        DEFAULT_ACCELERATOR.to_string()
    }

    fn default_result_rows() -> usize {
        6
    }
}

fn load_config(app: &AppHandle) -> Config {
    let mut cfg = Config::default();
    let Some(dir) = app.path().app_config_dir().ok() else {
        return cfg;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return cfg;
    }

    // Preferred: the current settings.json.
    let settings = dir.join("settings.json");
    if let Ok(text) = std::fs::read_to_string(&settings) {
        // JSONC: allow // comments so the user can annotate fields for themselves.
        if let Ok(parsed) = json5::from_str::<Config>(&text) {
            cfg = parsed;
        }
        return cfg;
    }

    // One-time migration from the legacy config.json: read it, write it under the
    // new name, then drop the old file so we never re-read it.
    let legacy = dir.join("config.json");
    if let Ok(text) = std::fs::read_to_string(&legacy) {
        if let Ok(parsed) = json5::from_str::<Config>(&text) {
            cfg = parsed;
        }
        let _ = save_config(app, &cfg);
        let _ = std::fs::remove_file(&legacy);
    }

    cfg
}

/// Persist the launcher config back to `settings.json` in the app config dir.
/// Records the written text so the notifications it triggers on itself are
/// recognized as own-writes and dropped (see `apply_settings`).
/// Write `text` to settings.json and record it as our own write, so the watchdog
/// skips the notification it causes. Every writer funnels through here so the
/// "did we cause this change" bookkeeping lives in one place.
fn persist_settings(app: &AppHandle, text: &str) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("settings.json");
    if let Some(state) = app.try_state::<ConfigWatcher>() {
        *state.own_write.lock().unwrap() = Some(text.to_string());
    }
    std::fs::write(&file, text).map_err(|e| e.to_string())
}

fn save_config(app: &AppHandle, cfg: &Config) -> Result<(), String> {
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    persist_settings(app, &text)
}

/// Write the initial settings.json with explanatory comments (VS Code-style JSONC)
/// so a first-time user sees what each field means and which needs a restart.
fn write_default_settings_template(app: &AppHandle) -> Result<(), String> {
    let template = r#"{
  // 全局唤醒快捷键，改后需重启生效（只在启动时注册）
  "accelerator": "Alt+Space",
  // 开机自启：改后立即生效（写入/删除系统自启项）
  "autostart": false,
  // 界面字体：系统已安装字体名，改后立即生效；留空 = 系统默认字体
  "font": "",
  // 列表最大候选行数：改后立即生效（窗口高度随之自适应）
  "resultRows": 6
}"#;
    persist_settings(app, template)
}

/// The two start-menu roots we crawl: per-user and shared.
fn start_menu_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Some(p) = std::env::var_os("PROGRAMDATA") {
        roots.push(PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    roots
}

/// Parse a `.lnk` into an `AppInfo`. Broken shortcuts resolve to no target —
/// we still index them so the app is launchable from its `.lnk`.
fn parse_lnk(path: &Path) -> AppInfo {
    // 显示名用 shell 本地化名（"控制面板"），原生英文文件名（"Control Panel"）作别名，
    // 这样中文「控制面板」和英文「control panel」都能检索命中。
    let display = shell_display_name(path).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = if display.is_empty() { stem.clone() } else { display.clone() };
    let aliases: Vec<String> = if !display.is_empty() && stem != display {
        vec![stem]
    } else {
        vec![]
    };

    let target_path = lnk::ShellLink::open(path, lnk::encoding::WINDOWS_1252)
        .ok()
        .and_then(|s| s.link_target())
        .unwrap_or_default();

    AppInfo {
        name,
        comment: None,
        launch_path: path.to_string_lossy().to_string(),
        target_path,
        icon: None,
        aliases,
    }
}

/// Windows 上取一个 shell 项的本地化显示名（`SHGFI_DISPLAYNAME`）。对 .lnk 会解析其
/// 指向目标的本地化名——这正是开始菜单扫描要用它而非 file_stem 的原因（系统工具的
/// 英文文件名 vs 中文显示名）。失败/空时返回 `None`，调用方回退 `file_stem`。
#[cfg(windows)]
fn shell_display_name(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_FLAGS};

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info as *mut SHFILEINFOW),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_FLAGS(0x200), // SHGFI_DISPLAYNAME：本地化显示名，不产生图标
        )
    };
    if ok == 0 {
        return None;
    }
    let end = info
        .szDisplayName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDisplayName.len());
    let s = String::from_utf16_lossy(&info.szDisplayName[..end])
        .trim()
        .to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(not(windows))]
fn shell_display_name(_path: &Path) -> Option<String> {
    None
}

/// True when a shortcut looks like noise (uninstaller / help / updater). Checks
/// the shortcut name and the target's file name so we never match on a folder in
/// the path (e.g. "C:\...\update\app.exe" stays).
fn is_junk(name: &str, target_path: &str) -> bool {
    let target_file = std::path::Path::new(target_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let hay = format!("{} {}", name, target_file).to_lowercase();
    JUNK_KEYWORDS.iter().any(|k| hay.contains(k))
}

/// True when a shortcut's target still looks like a live local resource.
/// "Invalid" means the target parses to a *local absolute path* that no longer
/// exists on disk. Deliberately NOT dropped (return true):
///   - empty target (link_target() failed) — the .lnk may still launch;
///   - UNC / network-share paths (\\server\share, //server/share) — a local
///     existence check would be slow and flaky (offline == falsely missing);
///   - relative / no-drive paths (incl. %ENV%) — Shell expands these;
///   - shell-namespace items (::{CLSID}) — exist only in the Shell namespace.
fn link_is_valid(target: &str) -> bool {
    if target.is_empty() {
        return true;
    }
    // UNC / network, or the extended-length prefix \\?\ (local but no drive):
    // treat as unresolvable-here → keep. (starts_with("\\\\") covers "\\?\C:\")
    if target.starts_with("\\\\") || target.starts_with("//") {
        return true;
    }
    let p = Path::new(target);
    // No drive/root (relative, or %ENV%\…) → Shell resolves it → keep.
    if p.is_relative() {
        return true;
    }
    // 本地绝对路径：仅当纯 ASCII 且确认不存在时才判为失效。含非 ASCII（真实中文路径，
    // 或 lnk crate 用 WINDOWS-1252 解码 UTF-8 产生的乱码 ÎÐÅ…）时无法可靠判断存在性，
    // 宽容保留——避免误杀安装在中文路径下的应用（如 微信开发者工具）。
    if !target.is_ascii() {
        return true;
    }
    p.exists()
}

/// True when a Start-Menu shortcut should not be indexed: install/uninstall noise,
/// or its local target no longer exists (宽松：网络 / 解析失败 / 相对路径保留，见
/// link_is_valid). One named place for the drop policy of a start-menu entry.
fn drop_shortcut(a: &AppInfo) -> bool {
    is_junk(&a.name, &a.target_path) || !link_is_valid(&a.target_path)
}

fn build_index(app: &AppHandle) -> Vec<AppInfo> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut apps = Vec::new();

    for root in start_menu_roots() {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let is_lnk = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
            if !is_lnk {
                continue;
            }
            let app = parse_lnk(entry.path());
            // Drop shortcuts that are install/uninstall noise, or whose local
            // target no longer exists (see drop_shortcut).
            if drop_shortcut(&app) {
                continue;
            }
            // Dedupe by resolved target: keep the first shortcut for a program.
            if !app.target_path.is_empty() && !seen.insert(app.target_path.clone()) {
                continue;
            }
            apps.push(app);
        }
    }

    // Portable apps (user-specified exes): independent entries, NOT deduped against
    // Start-Menu apps. Only deduped among themselves by path.
    let mut seen_portable: HashSet<String> = HashSet::new();
    for e in &load_manifest(app).apps {
        if !seen_portable.insert(e.path.clone()) {
            continue;
        }
        // 便携 .exe 已被删除 → 不呈现。只拦索引、不改 manifest，用户仍能
        // 在托盘「移除便携应用」里看到并主动摘除。
        if !link_is_valid(&e.path) {
            continue;
        }
        apps.push(parse_portable(e));
    }

    // 系统功能：内置的「系统设置」URI 入口，作为索引的第三类固定数据源。不建子系统，
    // 只 append；启动走现有 open_path（ShellExecuteW），图标走现有首字母兜底，去重用
    // uri 自身（`ms-settings:` / `::{CLSID}` 与文件路径天然不同，不会撞开始菜单去重）。
    for f in crate::system::builtin_system_features() {
        let uri = f.uri.to_string();
        apps.push(AppInfo {
            name: f.name.to_string(),
            comment: Some("系统".to_string()),
            launch_path: uri.clone(),
            target_path: uri,
            icon: None,
            aliases: Vec::new(),
        });
    }

    apps
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
        // Best-effort focus: `set_focus` is often enough with `alwaysOnTop`.
        let _ = window.set_focus();
        // Tell the frontend to clear + refocus the input.
        let _ = window.emit("wake", ());
    }
}

fn register_shortcut(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = load_config(app);
    let toggle = cfg
        .accelerator
        .parse::<Shortcut>()
        .unwrap_or_else(|_| DEFAULT_ACCELERATOR.parse().expect("valid default"));

    // The handler needs its own copy since `register` consumes `toggle`.
    let handler_key = toggle.clone();
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, shortcut, event| {
                if shortcut == &handler_key && event.state() == ShortcutState::Pressed {
                    toggle_window(app);
                }
            })
            .build(),
    )?;

    app.global_shortcut().register(toggle)?;
    Ok(())
}

fn build_tray(app: &tauri::App) -> tauri::Result<TrayIcon<tauri::Wry>> {
    let menu = build_tray_menu(app.handle())?;

    let mut tray = TrayIconBuilder::new();
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    let built = tray
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            match id.as_str() {
                "add-portable" => add_portable(app),
                "open-config" => open_config(app),
                "rescan" => {
                    let handle = app.clone();
                    std::thread::spawn(move || rebuild_index(&handle));
                }
                "quit" => app.exit(0),
                _ => {
                    if let Some(path) = id.strip_prefix("remove-portable:") {
                        remove_portable(app, path);
                    }
                }
            }
        })
        .build(app)?;

    Ok(built)
}

/// Regenerate the tray menu, with the current portable list as a remove submenu.
fn build_tray_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let add = MenuItem::with_id(app, "add-portable", "添加便携应用…", true, None::<&str>)?;
    let open_config_item =
        MenuItem::with_id(app, "open-config", "打开配置文件…", true, None::<&str>)?;
    let rescan = MenuItem::with_id(app, "rescan", "重扫索引", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::new(app)?;
    menu.append_items(&[&add])?;

    let m = load_manifest(app);
    if !m.apps.is_empty() {
        let mut remove: Vec<MenuItem<tauri::Wry>> = Vec::new();
        for e in &m.apps {
            let id = format!("remove-portable:{}", e.path);
            let label = e.name.clone().unwrap_or_else(|| file_stem(&e.path));
            remove.push(MenuItem::with_id(app, id, label, true, None::<&str>)?);
        }
        let refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
            remove.iter().map(|x| &*x as &dyn IsMenuItem<tauri::Wry>).collect();
        let sub = Submenu::with_items(app, "移除便携应用", true, &refs)?;
        menu.append_items(&[&sub])?;
    }

    menu.append_items(&[&open_config_item, &rescan, &quit])?;
    Ok(menu)
}

/// Swap the current tray menu for a freshly-built one, without creating a second
/// tray icon. No-op if the tray isn't stored (e.g. before setup finished).
fn refresh_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;
    if let Some(state) = app.try_state::<TrayHandle>() {
        if let Some(tray) = state.0.lock().unwrap().as_ref() {
            tray.set_menu(Some(menu))?;
        }
    }
    Ok(())
}

/// Rebuild the in-memory index from disk, then tell the frontend to reload.
/// The single owner of "rebuild + notify": the tray rescan, the Start-Menu /
/// portable watcher and portable add/remove all funnel through here.
fn rebuild_index(app: &AppHandle) {
    let apps = build_index(app);
    if let Some(state) = app.try_state::<Index>() {
        state.reload(apps);
    }
    let _ = app.emit("index-updated", ());
}

/// Rebuild the index + tray submenu, then tell the frontend to reload. Called
/// after any portable add/remove (the manifest already hit disk).
fn refresh_after_change(app: &AppHandle) {
    rebuild_index(app);
    let _ = refresh_tray(app);
}

/// Tray action: pick a `.exe`, append it to the manifest, and refresh.
fn add_portable(app: &AppHandle) {
    let handle = app.clone();
    app.dialog()
        .file()
        .add_filter("executable", &["exe"])
        .pick_file(move |file| {
            let Some(file) = file else { return; };
            let Ok(path) = file.into_path() else { return; };
            let path_str = path.to_string_lossy().to_string();
            let mut m = load_manifest(&handle);
            if m.apps.iter().any(|e| e.path == path_str) {
                return;
            }
            m.apps.push(PortableEntry { path: path_str.clone(), name: None });
            if save_manifest(&handle, &m).is_ok() {
                refresh_after_change(&handle);
            }
        });
}

/// Tray action: drop a portable by path (from the remove submenu id) and refresh.
fn remove_portable(app: &AppHandle, path: &str) {
    let mut m = load_manifest(app);
    m.apps.retain(|e| e.path != path);
    if save_manifest(app, &m).is_ok() {
        refresh_after_change(app);
    }
}

/// Tray action: open settings.json in the default editor. Ensures the file exists
/// (writing the current config) so the user always sees something to edit.
fn open_config(app: &AppHandle) {
    let Some(dir) = app.path().app_config_dir().ok() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("settings.json");
    if !file.exists() {
        let _ = write_default_settings_template(app);
    }
    if let Err(e) = app.opener().open_path(file.to_string_lossy().as_ref(), None::<&str>) {
        eprintln!("open config failed: {e}");
    }
}

/// Read settings.json and forward the fresh config to the frontend — unless it is
/// one of our own writes, which the watcher then drops to avoid a self-trigger
/// loop (a tray action persists config, which rewrites the watched file).
fn apply_settings(app: &AppHandle) {
    let Some(dir) = app.path().app_config_dir().ok() else {
        return;
    };
    let file = dir.join("settings.json");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };

    if let Some(state) = app.try_state::<ConfigWatcher>() {
        let own = state.own_write.lock().unwrap();
        if own.as_deref() == Some(text.as_str()) {
            return;
        }
    }

    let Ok(cfg) = json5::from_str::<Config>(&text) else {
        return;
    };

    apply_autostart(app, cfg.autostart);
    let _ = app.emit("settings-changed", cfg);
}

/// Bring the OS autostart registration in line with the config value, only when it
/// actually changed. Backed by an `AppliedAutostart` state set at startup, so the
/// file watcher doesn't re-apply the same value on every save.
fn apply_autostart(app: &AppHandle, want: bool) {
    use tauri_plugin_autostart::ManagerExt;

    let Some(state) = app.try_state::<AppliedAutostart>() else {
        return;
    };
    let applied = *state.0.lock().unwrap();
    if applied == Some(want) {
        return;
    }

    let result = if want {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    if let Err(e) = result {
        eprintln!("autostart apply failed: {e}");
        return;
    }
    *state.0.lock().unwrap() = Some(want);
}

/// Watch settings.json so `font` edits apply live. Runs on a dedicated thread;
/// never returns. The watcher is kept alive by the parking loop below.
fn watch_config(app: &AppHandle) -> notify::Result<()> {
    use notify::{RecursiveMode, Watcher};

    let Some(dir) = app.path().app_config_dir().ok() else {
        return Ok(());
    };
    let _ = std::fs::create_dir_all(&dir);
    let settings = dir.join("settings.json");
    if !settings.exists() {
        // Seed a commented settings.json so the user has a file to edit and the
        // watch target exists. (Own write; the watcher drops it.)
        let _ = write_default_settings_template(app);
    }

    let handle = app.clone();
    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return; };
            let targets_settings = event
                .paths
                .iter()
                .any(|p| p.to_string_lossy().ends_with("settings.json"));
            if targets_settings {
                apply_settings(&handle);
            }
        })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    // Park forever: dropping the watcher would stop notifications. `park` blocks
    // this thread without spinning; the OS reaps it on process exit.
    std::thread::park();
    Ok(())
}

/// Watch the Start-Menu roots so shortcuts that installers/uninstallers add or
/// remove refresh the index automatically — no manual rescan or restart. Runs
/// forever on its own thread; kept alive by the parking loop below.
/// True when a change to `paths` should rebuild the index: a `.lnk` (start-menu
/// install/uninstall) or anything inside a portable exe's own folder. A portable
/// parent is watched non-recursively, precisely so we catch the exe being deleted
/// or renamed.
fn index_change_relevant(app: &AppHandle, paths: &[PathBuf]) -> bool {
    if paths.iter().any(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
    }) {
        return true;
    }
    let Some(state) = app.try_state::<IndexWatcher>() else {
        return false;
    };
    let dirs = state.dirs.lock().unwrap();
    paths
        .iter()
        .any(|p| p.parent().is_some_and(|parent| dirs.contains(parent)))
}

/// Collapse a stream of notifications into a single `fire` callback after a quiet
/// window — installers write a batch of `.lnk` files in a burst, so we wait for the
/// stream to go quiet before rescanning once per install, not per file. Feed events
/// through the returned sender; `run` on its own thread.
struct Debouncer<F: Fn()> {
    rx: std::sync::mpsc::Receiver<()>,
    quiet: std::time::Duration,
    fire: F,
}

impl<F: Fn()> Debouncer<F> {
    fn run(self) {
        while self.rx.recv().is_ok() {
            while self.rx.recv_timeout(self.quiet).is_ok() {}
            (self.fire)();
        }
    }
}

/// Watch the Start-Menu roots (recursive) plus every registered portable exe's
/// folder (non-recursive), so installs/uninstalls and portable exe-deletions all
/// refresh the index live. Portable folders are watched from startup (the manifest
/// below), not extended while running — that would pin a directory the user has
/// just added and may want to delete.
fn watch_index_changes(app: &AppHandle) -> notify::Result<()> {
    use notify::RecursiveMode;

    let menu_roots: HashSet<PathBuf> =
        start_menu_roots().into_iter().filter(|r| r.exists()).collect();
    let portable_dirs: HashSet<PathBuf> = load_manifest(app)
        .apps
        .iter()
        .filter_map(|e| Path::new(&e.path).parent().map(|p| p.to_path_buf()))
        .filter(|p| p.exists())
        .collect();

    // The watcher reports into a channel; a debounce thread drains it and
    // rebuilds once per burst. Each consumer gets its own AppHandle clone.
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let watcher_handle = app.clone();
    let mut watcher: Box<dyn notify::Watcher + Send> = Box::new(
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return; };
            if index_change_relevant(&watcher_handle, &event.paths) {
                let _ = tx.send(());
            }
        })?,
    );

    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for root in &menu_roots {
        watcher.watch(root, RecursiveMode::Recursive)?;
        dirs.insert(root.clone());
    }
    for d in &portable_dirs {
        if menu_roots.contains(d) {
            continue; // already watched recursively
        }
        watcher.watch(d, RecursiveMode::NonRecursive)?;
        dirs.insert(d.clone());
    }

    // Keep the watcher (and its dir set) for the process lifetime; the callback
    // reads `dirs` to decide whether a change is relevant.
    if let Some(state) = app.try_state::<IndexWatcher>() {
        *state.watcher.lock().unwrap() = Some(watcher);
        *state.dirs.lock().unwrap() = dirs;
    }

    // Debounce: an installer writes a batch of .lnk files in a burst. The
    // Debouncer collapses them into one rebuild after a quiet window.
    let debouncer = Debouncer {
        rx,
        quiet: std::time::Duration::from_millis(400),
        fire: {
            let handle = app.clone();
            move || rebuild_index(&handle)
        },
    };
    std::thread::spawn(move || debouncer.run());

    // Watcher lives in state (app-lifetime), so this thread may return.
    Ok(())
}

/// Extend the index watcher into a portable exe's folder so removing the exe hides
/// it. Idempotent: no-op when that folder is already watched (including when it IS
/// a start-menu root, which is already recursive).
/// Current launcher config; the frontend reads `font` (and peers) at load and
/// again on every settings-changed event.
#[tauri::command]
fn get_config(app: AppHandle) -> Config {
    load_config(&app)
}

#[tauri::command]
fn scan_apps(state: State<Index>) -> Vec<AppInfo> {
    state.apps.lock().map(|g| g.clone()).unwrap_or_default()
}

#[tauri::command]
fn rescan(app: AppHandle, state: State<Index>) -> Vec<AppInfo> {
    let apps = build_index(&app);
    state.reload(apps.clone());
    apps
}

/// True for the built-in system entries (`ms-settings:` URIs and shell CLSIDs).
/// They aren't files on disk, so they can't be elevated — `as_admin` is ignored
/// for them rather than surfacing a UAC prompt that can never work.
fn is_system_uri(path: &str) -> bool {
    path.starts_with("ms-settings:") || path.starts_with("::{")
}

/// Launch an index entry, optionally elevated ("以管理员身份运行").
///
/// Elevation goes through `ShellExecuteW`'s `runas` verb — the `opener` plugin
/// only does a plain `open`, and whether a UAC prompt appears is the OS's call.
/// A `.lnk` keeps its own args/working dir (we pass no `lpDirectory`); a portable
/// exe keeps its cwd rule either way (see `portable::launch_portable`).
#[tauri::command]
fn launch_app(app: AppHandle, app_path: String, as_admin: bool) -> Result<(), String> {
    let elevate = as_admin && !is_system_uri(&app_path);

    // Which kind of entry this is is decided by the portable manifest, not by the
    // launch path's suffix, so a `.lnk` pointing at an exe still launches as a
    // shortcut.
    if portable::is_known_portable(&app, &app_path) {
        launch_portable(&app_path, elevate)?;
    } else if elevate {
        run_elevated(&app_path)?;
    } else if let Err(e) = app.opener().open_path(app_path.as_str(), None::<&str>) {
        return Err(e.to_string());
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    Ok(())
}

/// Launch any shell item (a `.lnk`, an `.exe`, …) elevated, via `ShellExecuteW`
/// with the `runas` verb. `lpDirectory` is left null so the shortcut's own working
/// directory wins; portables are handled separately because they need their own.
#[cfg(windows)]
fn run_elevated(path: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, HWND};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWDEFAULT;

    let file: Vec<u16> = std::path::Path::new(path)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();

    // ShellExecute returns an HINSTANCE; > 32 means success, <= 32 is an error
    // code (e.g. 1223 when the user dismisses the UAC prompt).
    let ret: HINSTANCE = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(std::ptr::null()),
            PCWSTR(std::ptr::null()),
            SW_SHOWDEFAULT,
        )
    };
    if (ret.0 as isize) <= 32 {
        return Err(format!("ShellExecute runas failed (code {})", ret.0 as isize));
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_elevated(_path: &str) -> Result<(), String> {
    Ok(())
}

/// Reveal a file in Explorer with the item selected — the shell's own
/// "打开文件所在的位置".
#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        // `explorer /select,<path>` must arrive as ONE argument: Explorer reads the
        // rest of that token as the path, so splitting flag and path into two args
        // would silently open "This PC" instead of the folder.
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

/// Batch-fetch app icons for a set of launch paths, as base64 PNG data URIs.
///
/// One IPC round-trip per render instead of one per row, with a shared in-memory
/// cache so repeated keystrokes don't re-extract icons. Extraction is done on a
/// blocking thread so the UI thread stays responsive.
#[tauri::command]
async fn get_app_icons(
    state: State<'_, IconCache>,
    paths: Vec<String>,
) -> Result<HashMap<String, Option<String>>, String> {
    let cache = state.map.clone();

    // Split into cache hits (fast, sync) and misses (extract off-thread).
    let mut out: HashMap<String, Option<String>> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let guard = cache.lock().unwrap();
        for p in &paths {
            if let Some(v) = guard.get(p) {
                out.insert(p.clone(), v.clone());
            } else {
                missing.push(p.clone());
            }
        }
    }

    if !missing.is_empty() {
        let fresh = tauri::async_runtime::spawn_blocking(move || {
            let mut got: HashMap<String, Option<String>> = HashMap::new();
            for p in &missing {
                // Only successes get cached. A miss stays uncached so the frontend's
                // retry budget can re-extract once the shell icon cache has warmed —
                // caching a permanent None here would defeat that retry.
                if let Ok(v) = icons::icon_data_uri(p) {
                    got.insert(p.clone(), v);
                }
            }
            if let Ok(mut guard) = cache.lock() {
                for (k, v) in &got {
                    guard.insert(k.clone(), v.clone());
                }
            }
            got
        })
        .await
        .map_err(|e| e.to_string())?;
        out.extend(fresh);
    }

    Ok(out)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Second launch focuses the existing window instead of double-registering
        // the global shortcut.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(Index::default())
        .manage(IconCache::default())
        .manage(ConfigWatcher::default())
        .manage(AppliedAutostart::default())
        .manage(IndexWatcher::default())
        .invoke_handler(tauri::generate_handler![
            scan_apps,
            rescan,
            launch_app,
            reveal_in_explorer,
            get_app_icons,
            get_config
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            register_shortcut(&handle)?;

            let tray = build_tray(app)?;
            app.manage(TrayHandle(Mutex::new(Some(tray))));

            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                let cfg = load_config(&handle);
                if cfg.autostart {
                    let _ = handle.autolaunch().enable();
                } else {
                    // 清理此前无条件 enable 可能留下的注册表项，保证默认关真正生效。
                    if handle.autolaunch().is_enabled().unwrap_or(false) {
                        let _ = handle.autolaunch().disable();
                    }
                }
                if let Some(state) = handle.try_state::<AppliedAutostart>() {
                    *state.0.lock().unwrap() = Some(cfg.autostart);
                }
            }

            // Watch settings.json so a `font` edit applies live, without restart.
            std::thread::spawn({
                let handle = handle.clone();
                move || {
                    let _ = watch_config(&handle);
                }
            });

            // Watch Start-Menu + portable folders so installs/removes refresh
            // the index live.
            std::thread::spawn({
                let handle = handle.clone();
                move || {
                    let _ = watch_index_changes(&handle);
                }
            });

            // Async initial scan so first activation is instant.
            std::thread::spawn(move || {
                let apps = build_index(&handle);
                if let Some(state) = handle.try_state::<Index>() {
                    state.reload(apps);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running x-on");
}
