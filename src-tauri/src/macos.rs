#[derive(Clone, Debug, Default)]
pub(crate) struct FrontmostApp {
    pub(crate) name: String,
    pub(crate) bundle_id: String,
    pub(crate) path: String,
}

impl FrontmostApp {
    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_empty() && self.bundle_id.is_empty() && self.path.is_empty()
    }

    pub(crate) fn is_copi(&self) -> bool {
        self.name.eq_ignore_ascii_case("copi") || self.bundle_id == "com.copi.app"
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn get_frontmost_app_bundle_id() -> Option<String> {
    get_frontmost_app_info()
        .map(|app| app.bundle_id)
        .filter(|bundle_id| !bundle_id.is_empty())
}

#[cfg(target_os = "macos")]
pub(crate) fn get_frontmost_app_info() -> Option<FrontmostApp> {
    use objc2_app_kit::NSWorkspace;

    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;

    let path = app
        .bundleURL()
        .and_then(|url| url.path())
        .map(|path| path.to_string())
        .unwrap_or_default();

    let bundle_id = app
        .bundleIdentifier()
        .map(|bundle_id| bundle_id.to_string())
        .unwrap_or_default();

    let name = app
        .localizedName()
        .map(|name| name.to_string())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            std::path::Path::new(&path)
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_default();

    let info = FrontmostApp {
        name: name.trim().to_string(),
        bundle_id: bundle_id.trim().to_string(),
        path: path.trim().to_string(),
    };

    (!info.is_empty()).then_some(info)
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub(crate) fn get_frontmost_app_info() -> Option<FrontmostApp> {
    None
}

#[cfg(target_os = "windows")]
pub(crate) fn get_frontmost_app_info() -> Option<FrontmostApp> {
    use std::path::Path;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }

    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    if pid == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }

    let mut buffer = vec![0u16; 1024];
    let mut len = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len) };
    unsafe {
        CloseHandle(process);
    }
    if ok == 0 || len == 0 {
        return None;
    }

    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    let stem = Path::new(&path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let name = friendly_windows_app_name(stem);

    let info = FrontmostApp {
        name: name.trim().to_string(),
        bundle_id: stem.trim().to_ascii_lowercase(),
        path: path.trim().to_string(),
    };

    (!info.is_empty()).then_some(info)
}

#[cfg(target_os = "windows")]
fn friendly_windows_app_name(stem: &str) -> String {
    let trimmed = stem.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mapped = match trimmed.to_ascii_lowercase().as_str() {
        "windowsterminal" => Some("Windows Terminal"),
        "msedge" => Some("Microsoft Edge"),
        "code" => Some("VS Code"),
        "pwsh" => Some("PowerShell"),
        "cmd" => Some("Command Prompt"),
        "devenv" => Some("Visual Studio"),
        "explorer" => Some("File Explorer"),
        "applicationframehost" => Some("Windows App"),
        _ => None,
    };
    if let Some(name) = mapped {
        return name.to_string();
    }

    let mut split = String::with_capacity(trimmed.len() + 4);
    let mut prev_is_alnum_lower = false;
    for ch in trimmed.chars() {
        let ch = if ch == '_' || ch == '-' { ' ' } else { ch };
        if ch.is_ascii_uppercase() && prev_is_alnum_lower {
            split.push(' ');
        }
        split.push(ch);
        prev_is_alnum_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }

    split
        .split_whitespace()
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            if matches!(lower.as_str(), "id" | "ui" | "api" | "cpu" | "gpu" | "sql") {
                return lower.to_ascii_uppercase();
            }
            let mut chars = lower.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = String::with_capacity(lower.len());
                    out.push(first.to_ascii_uppercase());
                    out.push_str(chars.as_str());
                    out
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "windows")]
pub(crate) fn get_foreground_window_handle() -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        None
    } else {
        Some(hwnd as isize)
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn get_foreground_window_handle() -> Option<isize> {
    None
}

#[cfg(target_os = "macos")]
pub(crate) fn get_app_icon_png(app_info: &FrontmostApp) -> Option<Vec<u8>> {
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSSize, NSString};

    let app_path = app_info.path.trim();
    if app_path.is_empty() || app_info.is_copi() {
        return None;
    }

    let cache_key = if !app_info.bundle_id.is_empty() {
        app_info.bundle_id.as_str()
    } else if !app_info.path.is_empty() {
        app_info.path.as_str()
    } else {
        app_info.name.as_str()
    };

    let icon_cache = dirs_cache_dir()?.join("copi").join("icons");
    let _ = std::fs::create_dir_all(&icon_cache);
    let cached_path = icon_cache.join(format!("v2_{}.png", sanitize_filename(cache_key)));

    if let Ok(bytes) = std::fs::read(&cached_path) {
        if !bytes.is_empty() {
            return Some(bytes);
        }
    }

    let workspace = NSWorkspace::sharedWorkspace();
    let full_path = NSString::from_str(app_path);
    let icon = workspace.iconForFile(&full_path);
    icon.setSize(NSSize::new(32.0, 32.0));

    let tiff = icon.TIFFRepresentation()?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }?;

    let png_bytes = png.to_vec();
    if png_bytes.is_empty() {
        return None;
    }

    let _ = std::fs::write(&cached_path, &png_bytes);
    Some(png_bytes)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn get_app_icon_png(_app_info: &FrontmostApp) -> Option<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStrExt;
        use std::path::Path;
        use std::{ffi::OsStr, mem::size_of, ptr::null_mut};
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        };
        use windows_sys::Win32::UI::Shell::{
            SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL};

        fn to_wide(path: &str) -> Vec<u16> {
            OsStr::new(path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        }

        fn write_png_rgba(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
            let mut out = Vec::new();
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(bytes).ok()?;
            drop(writer);
            Some(out)
        }

        fn extract_icon_png(path: &str) -> Option<Vec<u8>> {
            let mut file_info: SHFILEINFOW = unsafe { std::mem::zeroed() };
            let wide_path = to_wide(path);
            let ok = unsafe {
                SHGetFileInfoW(
                    wide_path.as_ptr(),
                    0,
                    &mut file_info,
                    size_of::<SHFILEINFOW>() as u32,
                    SHGFI_ICON | SHGFI_LARGEICON,
                )
            };
            if ok == 0 || file_info.hIcon.is_null() {
                return None;
            }

            let icon = file_info.hIcon;
            let width = 32i32;
            let height = 32i32;

            let dc = unsafe { CreateCompatibleDC(null_mut()) };
            if dc.is_null() {
                unsafe {
                    let _ = DestroyIcon(icon);
                }
                return None;
            }

            let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };

            let mut bits_ptr: *mut c_void = null_mut();
            let dib =
                unsafe { CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, null_mut(), 0) };
            if dib.is_null() || bits_ptr.is_null() {
                unsafe {
                    let _ = DeleteDC(dc);
                    let _ = DestroyIcon(icon);
                }
                return None;
            }

            let prev = unsafe { SelectObject(dc, dib as _) };

            // Initialize bitmap to fully transparent before drawing
            let pixel_count = (width * height) as usize;
            unsafe {
                let pixels = std::slice::from_raw_parts_mut(bits_ptr as *mut u32, pixel_count);
                pixels.fill(0);
            }

            let drawn =
                unsafe { DrawIconEx(dc, 0, 0, icon, width, height, 0, null_mut(), DI_NORMAL) };

            let png = if drawn != 0 {
                let pixel_len = (width * height * 4) as usize;
                let bgra = unsafe { std::slice::from_raw_parts(bits_ptr as *const u8, pixel_len) };
                let mut rgba = Vec::with_capacity(pixel_len);
                for pixel in bgra.chunks_exact(4) {
                    rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
                write_png_rgba(&rgba, width as u32, height as u32)
            } else {
                None
            };

            unsafe {
                let _ = SelectObject(dc, prev);
                let _ = DeleteObject(dib as _);
                let _ = DeleteDC(dc);
                let _ = DestroyIcon(icon);
            }

            png
        }

        let app_path = _app_info.path.trim();
        if app_path.is_empty() || _app_info.is_copi() {
            return None;
        }

        let cache_dir = dirs_cache_dir()?.join("copi").join("icons");
        let _ = std::fs::create_dir_all(&cache_dir);
        let cache_key = Path::new(app_path)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(app_path);
        let cache_file = cache_dir.join(format!("win_{}.png", sanitize_filename(cache_key)));
        if let Ok(bytes) = std::fs::read(&cache_file) {
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }

        let png = extract_icon_png(app_path)?;
        let _ = std::fs::write(&cache_file, &png);
        return Some(png);
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dirs_cache_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        let dir = std::path::PathBuf::from(home).join("Library/Caches");
        if dir.exists() {
            return Some(dir);
        }
    }

    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA").ok()?;
        let dir = std::path::PathBuf::from(local);
        if dir.exists() {
            return Some(dir);
        }
    }

    None
}

#[cfg(target_os = "macos")]
pub(crate) fn get_pasteboard_change_count() -> i64 {
    use objc2_app_kit::NSPasteboard;
    let pb = NSPasteboard::generalPasteboard();
    pb.changeCount() as i64
}

/// Get the app that owns the current clipboard content via NSPasteboard.
/// This is more accurate than frontmost app detection since it reads
/// the actual owner of the pasteboard content.
#[cfg(target_os = "macos")]
pub(crate) fn get_clipboard_source_app() -> Option<FrontmostApp> {
    use objc2_app_kit::{NSPasteboard, NSWorkspace};
    use objc2_foundation::NSString;

    let pb = NSPasteboard::generalPasteboard();

    // Different apps/OS versions expose different metadata keys.
    const SOURCE_KEYS: [&str; 4] = [
        "com.apple.pasteboard.source",
        "com.apple.pasteboard.source-bundle",
        "org.nspasteboard.source",
        "org.nspasteboard.source-bundle",
    ];

    let items = pb.pasteboardItems()?;
    if items.is_empty() {
        return None;
    }

    let item = items.objectAtIndex(0);

    for key in SOURCE_KEYS {
        let key_type = NSString::from_str(key);
        let bundle_id = item
            .stringForType(&key_type)
            .or_else(|| pb.stringForType(&key_type))
            .map(|v| v.to_string())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && v != "com.copi.app");

        if let Some(bundle_id) = bundle_id {
            return app_info_from_bundle_id(&NSWorkspace::sharedWorkspace(), &bundle_id);
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn app_info_from_bundle_id(
    workspace: &objc2_app_kit::NSWorkspace,
    bundle_id: &str,
) -> Option<FrontmostApp> {
    use objc2_foundation::NSString;

    let bundle_ns = NSString::from_str(bundle_id);
    let path = workspace
        .URLForApplicationWithBundleIdentifier(&bundle_ns)
        .and_then(|url| url.path())
        .map(|path| path.to_string())
        .unwrap_or_default();

    let name = std::path::Path::new(&path)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| bundle_id_to_app_name(bundle_id));

    let info = FrontmostApp {
        name: name.trim().to_string(),
        bundle_id: bundle_id.trim().to_string(),
        path: path.trim().to_string(),
    };

    (!info.is_empty()).then_some(info)
}

#[cfg(target_os = "macos")]
fn bundle_id_to_app_name(bundle_id: &str) -> String {
    // Extract readable name from bundle ID
    // e.g., "com.apple.mail" -> "Mail", "com.discord" -> "Discord"
    bundle_id
        .rsplit('.')
        .next()
        .map(|s| {
            // Capitalize first letter
            let mut chars = s.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn get_clipboard_source_app() -> Option<FrontmostApp> {
    None
}

#[cfg(target_os = "windows")]
pub(crate) fn get_pasteboard_change_count() -> i64 {
    use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;

    let seq = unsafe { GetClipboardSequenceNumber() };
    if seq == 0 {
        -1
    } else {
        seq as i64
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub(crate) fn get_pasteboard_change_count() -> i64 {
    -1
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
