//! Native icon extraction (Windows).
//!
//! Uses `SHGetFileInfoW` + GDI to pull a shortcut's icon as a 32bpp bitmap, then
//! encodes it to PNG and returns a base64 data URI. Best-effort: on any failure we
//! return `None` so the frontend drops back to a letter avatar — extraction is a
//! nice-to-have, never a hard dependency.
//!
//! ⚠️ `windows`-crate call signatures (bitflag/struct types above ~0.58) are the most
//! version-sensitive spot in this project. If this module fails to compile on your
//! pinned `windows` version, swap `icon_data_uri` for
//! [`file_icon_provider::get_file_icon`] on an older-than-here, or return `None`.

use std::io::Cursor;

use base64::Engine as _;

pub fn icon_data_uri(path: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let Some(png) = png_for(std::path::Path::new(path))? else {
            return Ok(None);
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        return Ok(Some(format!("data:image/png;base64,{b64}")));
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(None)
    }
}

#[cfg(windows)]
fn png_for(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    let Some((rgba, w, h)) = extract_rgba(path)? else {
        return Ok(None);
    };
    encode_png(&rgba, w as u32, h as u32).map(Some)
}

#[cfg(windows)]
/// Pull an app's icon as top-down RGBA pixels. Uses `GetDIBits` so it works for
/// both DIB sections and the device-dependent bitmaps that most shell icons are —
/// the previous `bmBits != null` special case silently dropped the latter.
fn extract_rgba(path: &std::path::Path) -> Result<Option<(Vec<u8>, usize, usize)>, String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
    };
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_FLAGS};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    // Any GDI object we create below must be released on every exit from here.
    unsafe fn release_all(hbm: HGDIOBJ, mask: HBITMAP, hicon: HICON) {
        let _ = DeleteObject(hbm);
        if !mask.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(mask.0));
        }
        let _ = DestroyIcon(hicon);
    }

    // Wide, NUL-terminated path.
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let ok = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info as *mut SHFILEINFOW),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_FLAGS(0x110), // SHGFI_ICON | SHGFI_LARGEICON (32px)
        );
        if ok == 0 {
            return Ok(None);
        }
        let hicon = info.hIcon;
        if hicon.0.is_null() {
            return Ok(None);
        }

        let mut iconinfo: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut iconinfo).is_err() {
            return Ok(None);
        }
        let hbm_color = iconinfo.hbmColor; // HBITMAP — GetDIBits wants this type
        let hbm = HGDIOBJ(hbm_color.0); // HGDIOBJ — GetObjectW / DeleteObject want this
        let hbm_mask = iconinfo.hbmMask;

        // Source format, so we know whether the icon carries an alpha channel.
        let mut bm: BITMAP = std::mem::zeroed();
        let got = GetObjectW(
            hbm,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        );
        if got == 0 {
            release_all(hbm, hbm_mask, hicon);
            return Ok(None);
        }
        let w = bm.bmWidth as usize;
        let h = bm.bmHeight as usize;
        if w == 0 || h == 0 || bm.bmBitsPixel == 0 {
            release_all(hbm, hbm_mask, hicon);
            return Ok(None);
        }
        let opaque = bm.bmBitsPixel < 32;

        let screen = HWND(std::ptr::null_mut());
        let hdc: HDC = GetDC(screen);
        if hdc.0.is_null() {
            release_all(hbm, hbm_mask, hicon);
            return Ok(None);
        }

        // Ask GDI to read the bitmap back as a 32bpp BGRA DIB (bottom-up). This
        // works for DDBs and DIB sections alike, and normalizes any depth.
        let mut hdr: BITMAPINFOHEADER = std::mem::zeroed();
        hdr.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        hdr.biWidth = w as i32;
        hdr.biHeight = h as i32; // positive => bottom-up
        hdr.biPlanes = 1;
        hdr.biBitCount = 32;
        hdr.biCompression = BI_RGB.0;
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = hdr;

        let mut buf = vec![0u8; w * h * 4];
        let copied = GetDIBits(
            hdc,
            hbm_color,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = ReleaseDC(screen, hdc);
        if copied == 0 {
            release_all(hbm, hbm_mask, hicon);
            return Ok(None);
        }

        // Bottom-up BGRA -> top-down RGBA; force opaque where no alpha channel.
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            let src_y = h - 1 - y;
            for x in 0..w {
                let i = (src_y * w + x) * 4;
                let o = (y * w + x) * 4;
                rgba[o] = buf[i + 2]; // R
                rgba[o + 1] = buf[i + 1]; // G
                rgba[o + 2] = buf[i]; // B
                rgba[o + 3] = if opaque { 255 } else { buf[i + 3] }; // A
            }
        }

        release_all(hbm, hbm_mask, hicon);
        Ok(Some((rgba, w, h)))
    }
}

#[cfg(windows)]
fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    let mut buf = Cursor::new(Vec::new());
    PngEncoder::new(&mut buf)
        .write_image(rgba, w, h, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}
