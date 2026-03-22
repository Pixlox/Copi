use regex::Regex;

pub fn should_capture(content: &str, app: &tauri::AppHandle) -> bool {
    let config = match crate::settings::get_config(app.clone()) {
        Ok(c) => c,
        Err(_) => return true,
    };

    // Check excluded apps
    #[cfg(target_os = "macos")]
    {
        let source_app = get_frontmost_app_name();
        for excluded in &config.privacy.excluded_apps {
            if source_app.contains(excluded) {
                return false;
            }
        }
    }

    // Check privacy regex rules
    for pattern in &config.privacy.privacy_rules {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(content) {
                return false;
            }
        }
    }

    true
}

#[cfg(target_os = "macos")]
fn get_frontmost_app_name() -> String {
    use std::process::Command;
    if let Ok(output) = Command::new("osascript")
        .arg("-e")
        .arg("name of application (path to frontmost application as text)")
        .output()
    {
        if let Ok(name) = String::from_utf8(output.stdout) {
            return name.trim().to_string();
        }
    }
    String::new()
}
