//! Portable-app support: a user-specified `.exe` that isn't in the Start Menu.
//!
//! Persisted as a separate manifest (`apps.json`) — deliberately NOT inside
//! `config.json`, which holds behaviour settings. Each entry is `{ path, name? }`;
//! the display name prefers the user alias, then the exe's version-resource
//! description, then the file stem. Launching sets the working directory to the
//! exe's own folder (portables commonly read config/plugins relative to it), which
//! is why we go through `ShellExecuteW(lpDirectory)` instead of `opener`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// One manually-added portable app.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PortableEntry {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The whole manifest file. Kept as a struct (not a bare array) so the schema can
/// grow without a migration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PortableManifest {
    pub apps: Vec<PortableEntry>,
}

pub fn manifest_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("apps.json"))
}

pub fn load_manifest(app: &AppHandle) -> PortableManifest {
    let Some(path) = manifest_path(app) else {
        return PortableManifest::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_manifest(app: &AppHandle, m: &PortableManifest) -> Result<(), String> {
    let path = manifest_path(app).ok_or("no app config dir")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// True when `path` is a registered portable exe (present in the manifest). The
/// type-level way to ask "is this portable" — not a heuristic on the launch path's
/// suffix, so a `.lnk` whose target happens to be an `.exe` still launches as a
/// shortcut.
pub fn is_known_portable(app: &AppHandle, path: &str) -> bool {
    load_manifest(app).apps.iter().any(|e| e.path == path)
}

/// The exe's file stem, minus `.exe` — the last-resort display name.
pub fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Build an `AppInfo` for a portable entry: alias -> version description -> stem.
pub fn parse_portable(e: &PortableEntry) -> crate::AppInfo {
    let name = e
        .name
        .clone()
        .or_else(|| read_exe_description(Path::new(&e.path)))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| file_stem(&e.path));

    crate::AppInfo {
        name,
        comment: Some("便携应用".to_string()),
        launch_path: e.path.clone(),
        target_path: e.path.clone(),
        icon: None,
    }
}

/// Launch a portable exe with its working directory set to the exe's own folder.
/// Uses `ShellExecuteW(lpDirectory = folder)` so it keeps shell semantics (verb,
/// elevation) the same way the `.lnk` path does, but with the right cwd.
#[cfg(windows)]
pub fn launch_portable(path: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, HINSTANCE};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWDEFAULT;

    let exe: Vec<u16> = Path::new(path)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // lpDirectory = the exe's folder; portables resolve config/plugins relative to it.
    let dir: Vec<u16> = Path::new(path)
        .parent()
        .map(|p| wide_str(&p.to_string_lossy()))
        .unwrap_or_default();

    let op = wide_str("open");
    let ret: HINSTANCE = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR(op.as_ptr()),
            PCWSTR(exe.as_ptr()),
            PCWSTR(std::ptr::null()),
            PCWSTR(dir.as_ptr()),
            SW_SHOWDEFAULT,
        )
    };

    // ShellExecute returns an HINSTANCE; > 32 means success, <= 32 is an error code.
    if (ret.0 as isize) <= 32 {
        return Err(format!("ShellExecute failed (code {})", ret.0 as isize));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn launch_portable(_path: &str) -> Result<(), String> {
    Ok(())
}

/// Read the exe's `FileDescription` (fallback `ProductName`) from its version
/// resource, preferring the first translation block. Best-effort: `None` on any
/// failure or when the description is empty, so the caller falls back.
#[cfg(windows)]
fn read_exe_description(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut handle: u32 = 0;
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), Some(&mut handle)) };
    if size == 0 {
        return None;
    }

    let mut buf = vec![0u8; size as usize];
    let ok = unsafe {
        GetFileVersionInfoW(
            PCWSTR(wide.as_ptr()),
            0,
            size,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
        )
    };
    if ok.is_err() {
        return None;
    }

    unsafe {
        // Resolve the first (lang, charset) translation so we can address the right
        // StringFileInfo sub-block.
        let mut tr_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut tr_len: u32 = 0;
        let tr_sub = wide_str("\\VarFileInfo\\Translation");
        if VerQueryValueW(
            buf.as_ptr() as *const std::ffi::c_void,
            PCWSTR(tr_sub.as_ptr()),
            &mut tr_ptr,
            &mut tr_len,
        )
        .0
            != 0
            && tr_len >= 4
        {
            let first = *(tr_ptr as *const u32);
            let lang = first & 0xFFFF;
            let code = (first >> 16) & 0xFFFF;

            for key in ["FileDescription", "ProductName"] {
                let sub = wide_str(&format!(
                    "\\StringFileInfo\\{:04x}{:04x}\\{}",
                    lang, code, key
                ));
                let mut v_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                let mut v_len: u32 = 0;
                if VerQueryValueW(
                    buf.as_ptr() as *const std::ffi::c_void,
                    PCWSTR(sub.as_ptr()),
                    &mut v_ptr,
                    &mut v_len,
                )
                .0
                    != 0
                    && !v_ptr.is_null()
                {
                    if let Some(s) = read_wide(v_ptr as *const u16) {
                        return Some(s);
                    }
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn read_exe_description(_path: &Path) -> Option<String> {
    None
}

/// A NUL-terminated UTF-16 buffer -> trimmed Rust string, `None` if empty/null.
unsafe fn read_wide(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let len = (0usize..).take_while(|&i| *ptr.add(i) != 0).count();
    if len == 0 {
        return None;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let s = String::from_utf16_lossy(slice).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// UTF-8 -> NUL-terminated UTF-16, for the wide Win32 calls.
fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
