//! Microsoft Store (packaged / UWP) app support — the index's fourth data source.
//!
//! Why it exists: packaged apps installed from the Microsoft Store do **not** drop a
//! classic `.lnk` into the Start-Menu folders this app crawls — the Shell renders
//! their Start-Menu entry straight from the Appx registration. A `.lnk`-only index
//! therefore misses every pure Store app (计算器、照片、Store 版微信/QQ…); only the
//! "Store edition" of traditional desktop apps, which still ship a `.lnk`, show up.
//!
//! What we do instead: enumerate the Shell's **AppsFolder** (`shell:AppsFolder` —
//! the folder behind the Start menu's app list) via the modern `IShellItem` +
//! `BHID_EnumItems` route. **Important design detail:** AppsFolder lists *classic
//! desktop apps as well*, and `PKEY_AppUserModel_ID` is non-empty for those too —
//! its value is then either the *file path* or whatever AUMID the desktop app put on
//! its shortcut (Chrome → `Chrome`, Office → `Microsoft.Office.EXCEL.EXE.15`,
//! Postman → `com.squirrel.Postman.Postman`). Indexing those would duplicate the
//! Start Menu, so three filters keep this source clean:
//!
//! 1. path-like AUMIDs (`C:\...`, `*.exe`, `*.lnk`) are dropped — file entries the
//!    `.lnk` scan already covers;
//! 2. `System.AppUserModel.PackageFamilyName` must be non-empty. Probed empirically:
//!    only Appx/MSIX registrations carry it, so it — not the AUMID's *shape* — is
//!    what actually separates a Store/UWP app from a desktop one that merely
//!    registered an AUMID;
//! 3. the leftover overlap (a packaged app that *also* ships a Start-Menu `.lnk`,
//!    e.g. Python / Outlook / Microsoft Store) is dropped by name in `build_index`,
//!    which keeps the `.lnk` row and skips the duplicate store row.
//!
//! Serialization: a store app's `launch_path` is `store:<AUMID>`. Everything
//! downstream keys off that prefix: `launch_app` activates by AUMID, icon extraction
//! routes to a Shell-namespace lookup, and the frontend uses it to suppress the
//! file-oriented context menu. Store apps also cannot run elevated — the "run as
//! admin" request is silently ignored at the `launch_app` call site.
//!
//! This module talks COM + the Shell namespace, so everything is best-effort: any
//! failure yields an empty list or an `Err`, never a panic.

use std::ptr;

/// Serialized launch-path prefix for store apps.
pub const LAUNCH_PREFIX: &str = "store:";

/// A single indexed store app. `name` is the localized name the Shell reports;
/// `aumid` is the AppUserModelID used to launch it (`store:<aumid>`).
pub struct StoreApp {
    pub name: String,
    pub aumid: String,
}

pub fn is_store_launch_path(path: &str) -> bool {
    path.starts_with(LAUNCH_PREFIX)
}

pub fn strip_launch_prefix(path: &str) -> &str {
    path.strip_prefix(LAUNCH_PREFIX).unwrap_or(path)
}

/// AppsFolder shows legacy Start-Menu `.lnk` items whose AppUserModelID property
/// holds the *file path* of the target. Those are already indexed by the `.lnk`
/// scan, so we drop them here before any name-based dedupe has to work.
fn aumid_looks_like_path(aumid: &str) -> bool {
    let lower = aumid.to_ascii_lowercase();
    lower.contains('\\') || lower.contains('/') || lower.ends_with(".exe") || lower.ends_with(".lnk")
}

#[cfg(windows)]
pub fn enumerate_store_apps() -> Vec<StoreApp> {
    use std::collections::BTreeSet;

    use windows::core::{BSTR, PCWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED, IBindCtx,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY};
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, BHID_PropertyStore, IEnumShellItems, IShellItem,
        SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY,
    };

    // Hand-built PKEYs so we don't need the `Win32_Storage_EnhancedStorage` feature
    // their constants live under in the windows crate. Both belong to the
    // AppUserModel property set (fmtid 9F4C2855-…).
    const APP_USER_MODEL_FMTID: windows::core::GUID =
        windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: APP_USER_MODEL_FMTID,
        pid: 5,
    };
    // `System.AppUserModel.PackageFamilyName` (e.g. `Microsoft.WindowsCalculator_8wekyb3d8bbwe`).
    // Probed empirically: it is non-empty **only** for Appx/MSIX registrations, while
    // classic desktop apps in AppsFolder only expose pids 5/9/14/18/25/35. That makes
    // it the reliable "is a real packaged (Store/UWP) app" test.
    const PKEY_APP_USER_MODEL_PACKAGE_FAMILY_NAME: PROPERTYKEY = PROPERTYKEY {
        fmtid: APP_USER_MODEL_FMTID,
        pid: 17,
    };

    // COM apartment: prefer the caller's existing apartment; only start (and later
    // tear down) our own when none existed yet.
    let (co_owned, co_ready) = unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // RPC_E_CHANGED_MODE (0x80010106): the thread already has an apartment in a
        // different model — that apartment still serves COM, so just go with it.
        // HRESULT.0 is i32, so the hex constant needs the wrap-through-u32 cast.
        let ready = hr.is_ok() || hr.0 == 0x8001_0106u32 as i32;
        let owned = hr == windows::core::HRESULT(0);
        (owned, ready)
    };
    if !co_ready {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    unsafe {
        let root_wide: Vec<u16> = "shell:AppsFolder"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let folder: IShellItem = match SHCreateItemFromParsingName::<_, _, IShellItem>(
            PCWSTR(root_wide.as_ptr()),
            Option::<&IBindCtx>::None,
        ) {
            Ok(folder) => folder,
            Err(_) => {
                if co_owned {
                    CoUninitialize();
                }
                return out;
            }
        };

        let enumerator: IEnumShellItems = match folder.BindToHandler(
            Option::<&IBindCtx>::None,
            &BHID_EnumItems,
        ) {
            Ok(enumerator) => enumerator,
            Err(_) => {
                if co_owned {
                    CoUninitialize();
                }
                return out;
            }
        };

        // Both AppUserModel properties come off a single property store: the AUMID we
        // launch by, and the package family name that tells packaged apps apart from
        // classic desktop ones.
        let props = |item: &IShellItem| -> (Option<String>, Option<String>) {
            let Ok(ps): Result<IPropertyStore, _> =
                item.BindToHandler(Option::<&IBindCtx>::None, &BHID_PropertyStore)
            else {
                return (None, None);
            };
            let get = |key: &PROPERTYKEY| -> Option<String> {
                let Ok(pv) = ps.GetValue(key) else {
                    return None;
                };
                let Ok(bstr) = BSTR::try_from(&pv) else {
                    return None;
                };
                if bstr.is_empty() {
                    return None;
                }
                let s = String::from_utf16_lossy(bstr.as_wide());
                if s.is_empty() { None } else { Some(s) }
            };
            (
                get(&PKEY_APP_USER_MODEL_ID),
                get(&PKEY_APP_USER_MODEL_PACKAGE_FAMILY_NAME),
            )
        };

        loop {
            let mut one: Option<IShellItem> = None;
            let mut fetched: u32 = 0;
            if enumerator
                .Next(std::slice::from_mut(&mut one), Some(&mut fetched))
                .is_err()
            {
                break; // S_FALSE → enumeration over
            }
            if fetched == 0 {
                break;
            }
            let Some(item) = one else {
                continue;
            };

            // Localized display name (what the user sees in the Start menu).
            let Some(name) = (|| {
                let Ok(pwstr) = item.GetDisplayName(SIGDN_NORMALDISPLAY) else {
                    return None;
                };
                let name = pwstr.to_string().ok();
                CoTaskMemFree(Some(pwstr.0.cast()));
                name
            })() else {
                continue;
            };

            let (aumid, package_family) = props(&item);
            // AppUserModelID — filled for every AppsFolder item. Path-like values are
            // the legacy `.lnk` entries the Start-Menu scan already covers.
            let Some(aumid) = aumid else { continue };
            if aumid_looks_like_path(&aumid) {
                continue;
            }
            // Packaged-only filter. AppsFolder lists classic desktop apps too, and
            // their AUMID is just whatever the shortcut registered (`Chrome`,
            // `Microsoft.Office.EXCEL.EXE.15`, `com.ccswitch.desktop`, …) — indexing
            // those here is exactly what duplicated Start-Menu entries.
            // `PackageFamilyName` only exists on Appx/MSIX registrations, so it is the
            // reliable "this really is a Store/UWP app" test (unlike AUMID *format*,
            // which some desktop apps mimic).
            if package_family.is_none() {
                continue;
            }
            let name = name.trim();
            if name.is_empty() || !seen.insert(aumid.clone()) {
                continue;
            }
            out.push(StoreApp {
                name: name.to_string(),
                aumid,
            });
        }

        if co_owned {
            CoUninitialize();
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(not(windows))]
pub fn enumerate_store_apps() -> Vec<StoreApp> {
    Vec::new()
}

#[cfg(windows)]
/// Launch a store app by its AppUserModelID. UWP apps cannot run elevated, so this
/// ignores admin and just activates normally (the call site never asks for it).
pub fn launch(aumid: &str) -> Result<(), String> {
    use windows::core::{IUnknown, PCWSTR};
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, CoCreateInstance, CoInitializeEx, CoUninitialize,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::Shell::{
        AO_NONE, ApplicationActivationManager, IApplicationActivationManager,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // COM guard: CoCreateInstance of the out-of-proc activation manager needs a COM
    // apartment on this thread; the IPC caller doesn't guarantee one. Same pattern as
    // `enumerate_store_apps` / `icon_data_uri` — reuse an existing apartment, own only
    // what we started.
    let (co_owned, co_ready) = unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ready = hr.is_ok() || hr.0 == 0x8001_0106u32 as i32;
        (hr == windows::core::HRESULT(0), ready)
    };
    if !co_ready {
        return Err("COM unavailable on this thread".to_string());
    }

    let aumid_wide: Vec<u16> = aumid.encode_utf16().chain(std::iter::once(0)).collect();

    let result = (|| {
        // Canonical route: IApplicationActivationManager activates a registered packaged
        // app by AUMID (this is the same mechanism the Start menu uses).
        let activated = unsafe {
            CoCreateInstance::<_, IApplicationActivationManager>(
                &ApplicationActivationManager,
                Option::<&IUnknown>::None,
                CLSCTX_LOCAL_SERVER,
            )
            .and_then(|mgr| {
                mgr.ActivateApplication(PCWSTR(aumid_wide.as_ptr()), PCWSTR::null(), AO_NONE)
            })
            .map(|_pid| ())
        };
        if activated.is_ok() {
            return Ok(());
        }

        // Fallback: let the Shell open the AppsFolder item directly, mirroring a Start
        // menu click. Worth trying — activation can fail for apps registered in unusual
        // deployment scopes.
        let shell_path = format!("shell:AppsFolder\\{aumid}");
        let path_wide: Vec<u16> = shell_path.encode_utf16().chain(std::iter::once(0)).collect();
        let ret = unsafe {
            ShellExecuteW(
                None,
                None,
                PCWSTR(path_wide.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if (ret.0 as isize) > 32 {
            Ok(())
        } else {
            Err(format!(
                "failed to launch store app {aumid} (code {})",
                ret.0 as isize
            ))
        }
    })();

    if co_owned {
        unsafe {
            CoUninitialize();
        }
    }
    result
}

#[cfg(not(windows))]
pub fn launch(_aumid: &str) -> Result<(), String> {
    Err("store apps are unsupported on this platform".to_string())
}

#[cfg(windows)]
/// Extract a store app's icon. There is no file to hand `SHGetFileInfoW`, so we
/// resolve `shell:AppsFolder\<AUMID>` into a PIDL and let `SHGetFileInfoW` walk the
/// Shell namespace with `SHGFI_PIDL`; the shared `hicon → PNG` pipeline does the
/// rest. Best-effort — `None` sends the frontend back to a letter avatar.
pub fn icon_data_uri(aumid: &str) -> Result<Option<String>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, IBindCtx,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{ILFree, SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW};

    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // RPC_E_CHANGED_MODE: reuse the caller's existing apartment when present.
        let ready = hr.is_ok() || hr.0 == 0x8001_0106u32 as i32;
        if !ready {
            return Ok(None);
        }
        let must_uninit = hr == windows::core::HRESULT(0);

        let shell_path = format!("shell:AppsFolder\\{aumid}");
        let wide: Vec<u16> = shell_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut pidl: *mut ITEMIDLIST = ptr::null_mut();
        let resolved = SHParseDisplayName(
            PCWSTR(wide.as_ptr()),
            Option::<&IBindCtx>::None,
            &mut pidl,
            0,
            None,
        );

        if resolved.is_err() || pidl.is_null() {
            if must_uninit {
                CoUninitialize();
            }
            return Ok(None);
        }

        let mut info: SHFILEINFOW = std::mem::zeroed();
        let ok = SHGetFileInfoW(
            PCWSTR(pidl as *const u16),
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info as *mut SHFILEINFOW),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            windows::Win32::UI::Shell::SHGFI_FLAGS(0x108), // SHGFI_ICON | SHGFI_PIDL
        );
        ILFree(Some(pidl));
        if must_uninit {
            CoUninitialize();
        }

        if ok == 0 || info.hIcon.0.is_null() {
            return Ok(None);
        }
        crate::icons::icon_data_uri_from_hicon(info.hIcon)
    }
}

#[cfg(not(windows))]
pub fn icon_data_uri(_aumid: &str) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_apps_roundtrip() {
        // Real-shell smoke test on Windows: enumeration must not panic, must return
        // only true (non-path) AppUserModelIDs, and the helpers must tolerate them.
        // No store apps on a machine just means the Vec is empty — a valid outcome.
        let apps = enumerate_store_apps();
        eprintln!("xon store scan found {} app(s)", apps.len());
        for app in apps.iter().take(12) {
            eprintln!("  {}  <-  {}", app.name, app.aumid);
        }
        assert!(
            apps.iter().all(|a| !aumid_looks_like_path(&a.aumid)),
            "file-path AppUserModelIDs must have been filtered"
        );
        // A packaged app's AUMID is always "<PackageFamilyName>!<AppId>" (and it is the
        // PackageFamilyName property that survived the filter). A desktop entry that
        // leaked back in would show up as a violation of this.
        assert!(
            apps.iter().all(|a| a.aumid.contains('!')),
            "packaged apps must expose a '<family>!<appid>' AUMID"
        );

        if let Some(first) = apps.first() {
            let launch = format!("{LAUNCH_PREFIX}{}", first.aumid);
            assert!(is_store_launch_path(&launch));
            assert_eq!(strip_launch_prefix(&launch), first.aumid);
            let _ = icon_data_uri(&first.aumid);
        }
    }

    #[test]
    fn store_rows_shadowed_by_start_menu() {
        // Diagnostics, not a policy assertion: some packaged apps *also* ship a
        // Start-Menu `.lnk` (Python, Outlook, Microsoft Store…). `build_index` drops
        // those store rows by name; printing the overlap keeps that from being silent
        // when the Start Menu or the filter changes.
        let mut menu_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        for root in crate::start_menu_roots() {
            if !root.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
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
                let info = crate::parse_lnk(entry.path());
                if crate::drop_shortcut(&info) {
                    continue;
                }
                menu_keys.insert(crate::index_name_key(&info.name));
            }
        }

        let shadowed: Vec<String> = enumerate_store_apps()
            .into_iter()
            .filter(|a| menu_keys.contains(&crate::index_name_key(&a.name)))
            .map(|a| a.name)
            .collect();
        eprintln!("store rows shadowed by a start-menu .lnk: {shadowed:?}");
    }
}
