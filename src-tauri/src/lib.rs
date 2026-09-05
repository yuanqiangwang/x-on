mod icons;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::MacosLauncher;
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

/// Cache of extracted app icons, keyed by launch path. Extraction is expensive
/// (shell + GDI + PNG + base64), so we keep results — including misses — in
/// memory to avoid re-extracting on every keystroke.
#[derive(Clone, Default)]
struct IconCache {
    map: Arc<Mutex<HashMap<String, Option<String>>>>,
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
}

/// Launcher settings, read from the app config dir on startup.
#[derive(serde::Deserialize)]
struct Config {
    #[serde(default = "Config::default_accelerator")]
    accelerator: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            accelerator: DEFAULT_ACCELERATOR.to_string(),
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
    if let Some(dir) = app.path().app_config_dir().ok() {
        if std::fs::create_dir_all(&dir).is_ok() {
            let file = dir.join("config.json");
            if let Ok(text) = std::fs::read_to_string(&file) {
                if let Ok(parsed) = serde_json::from_str::<Config>(&text) {
                    cfg = parsed;
                }
            }
        }
    }
    cfg
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

fn build_index() -> Vec<AppInfo> {
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

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let rescan = MenuItem::with_id(app, "rescan", "重扫索引", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&rescan, &quit])?;

    let mut tray = TrayIconBuilder::new();
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "rescan" => {
                let handle = app.clone();
                std::thread::spawn(move || {
                    let apps = build_index();
                    if let Some(state) = handle.try_state::<Index>() {
                        if let Ok(mut guard) = state.apps.lock() {
                            *guard = apps;
                        }
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}

#[tauri::command]
fn scan_apps(state: State<Index>) -> Vec<AppInfo> {
    state.apps.lock().map(|g| g.clone()).unwrap_or_default()
}

#[tauri::command]
fn rescan(state: State<Index>) -> Vec<AppInfo> {
    let apps = build_index();
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
    if let Err(e) = app.opener().open_path(app_path.as_str(), None::<&str>) {
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
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(Index::default())
        .manage(IconCache::default())
        .invoke_handler(tauri::generate_handler![
            scan_apps,
            rescan,
            launch_app,
            get_app_icons
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            register_shortcut(&handle)?;
            build_tray(app)?;

            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                let _ = app.autolaunch().enable();
            }

            // Async initial scan so first activation is instant.
            std::thread::spawn(move || {
                let apps = build_index();
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
