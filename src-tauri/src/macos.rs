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

pub(crate) fn get_frontmost_app_name() -> String {
    get_frontmost_app_info()
        .map(|app| app.name)
        .unwrap_or_default()
}

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

#[cfg(not(target_os = "macos"))]
pub(crate) fn get_frontmost_app_info() -> Option<FrontmostApp> {
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
    None
}

fn dirs_cache_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        let dir = std::path::PathBuf::from(home).join("Library/Caches");
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

#[cfg(not(target_os = "macos"))]
pub(crate) fn get_pasteboard_change_count() -> i64 {
    0
}

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
