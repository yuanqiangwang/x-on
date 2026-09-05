mod icons;
mod portable;

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
}

/// Launcher settings, read from the app config dir on startup.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct Config {
    #[serde(default = "Config::default_accelerator")]
    accelerator: String,
    /// Whether to register the app to launch at login. Off by default; the user
    /// opts in from the tray menu so the app never silently self-starts.
    #[serde(default)]
    autostart: bool,
    /// UI font family, mirroring VS Code's editor.fontFamily: a system font name
    /// the user types into settings.json. Empty means the bundled LXGW WenKai
    /// default. Only `font` is hot-applied when the file is watched.
    #[serde(default)]
    font: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            accelerator: DEFAULT_ACCELERATOR.to_string(),
            autostart: false,
            font: String::new(),
        }
    }
}

impl Config {
    fn default_accelerator() -> String {
        DEFAULT_ACCELERATOR.to_string()
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
fn save_config(app: &AppHandle, cfg: &Config) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("settings.json");
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    if let Some(state) = app.try_state::<ConfigWatcher>() {
        *state.own_write.lock().unwrap() = Some(text.clone());
    }
    std::fs::write(&file, text).map_err(|e| e.to_string())
}

/// Write the initial settings.json with explanatory comments (VS Code-style JSONC)
/// so a first-time user sees what each field means and which needs a restart.
fn write_default_settings_template(app: &AppHandle) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("settings.json");
    let template = r#"{
  // 全局唤醒快捷键，改后需重启生效（只在启动时注册）
  "accelerator": "Alt+Space",
  // 开机自启：改后立即生效（写入/删除系统自启项）
  "autostart": false,
  // 界面字体：系统已安装字体名，改后立即生效；留空 = 内置霞鹜文楷
  "font": ""
}"#;
    if let Some(state) = app.try_state::<ConfigWatcher>() {
        *state.own_write.lock().unwrap() = Some(template.to_string());
    }
    std::fs::write(&file, template).map_err(|e| e.to_string())
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
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

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
    }
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
            // Drop uninstallers / help & updater noise at the source.
            if is_junk(&app.name, &app.target_path) {
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
        apps.push(parse_portable(e));
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
                    std::thread::spawn(move || {
                        let apps = build_index(&handle);
                        if let Some(state) = handle.try_state::<Index>() {
                            if let Ok(mut guard) = state.apps.lock() {
                                *guard = apps;
                            }
                        }
                        let _ = handle.emit("index-updated", ());
                    });
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

/// Rebuild the index + tray submenu, then tell the frontend to reload. Called after
/// any portable add/remove (the manifest already hit disk).
fn refresh_after_change(app: &AppHandle) {
    let apps = build_index(app);
    if let Some(state) = app.try_state::<Index>() {
        if let Ok(mut guard) = state.apps.lock() {
            *guard = apps;
        }
    }
    let _ = refresh_tray(app);
    let _ = app.emit("index-updated", ());
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
            m.apps.push(PortableEntry { path: path_str, name: None });
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
    if let Ok(mut guard) = state.apps.lock() {
        *guard = apps.clone();
    }
    apps
}

/// Launch a `.lnk` through the OS (ShellExecute behind the scenes — preserves the
/// shortcut's own arguments and working directory, and handles CJK paths safely),
/// then hide the launcher.
#[tauri::command]
fn launch_app(app: AppHandle, app_path: String) -> Result<(), String> {
    // A portable app is launched as its raw `.exe` with the working directory set
    // to the exe's folder; a Start-Menu entry is a `.lnk` launched through the
    // opener so it keeps the shortcut's own args/working dir. The extension tells
    // them apart (a Start-Menu entry's launch_path is always the `.lnk`).
    let is_exe = Path::new(&app_path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));

    if is_exe {
        launch_portable(&app_path)?;
    } else if let Err(e) = app.opener().open_path(app_path.as_str(), None::<&str>) {
        return Err(e.to_string());
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    Ok(())
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
                let val = match icons::icon_data_uri(p) {
                    Ok(v) => v,
                    Err(_) => None,
                };
                got.insert(p.clone(), val);
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
        .invoke_handler(tauri::generate_handler![
            scan_apps,
            rescan,
            launch_app,
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

            // Async initial scan so first activation is instant.
            std::thread::spawn(move || {
                let apps = build_index(&handle);
                if let Some(state) = handle.try_state::<Index>() {
                    if let Ok(mut guard) = state.apps.lock() {
                        *guard = apps;
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running x-on");
}
