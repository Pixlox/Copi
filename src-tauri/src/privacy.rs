pub fn should_capture(_content: &str, app: &tauri::AppHandle) -> bool {
    let config = match crate::settings::get_config_sync(app.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[Privacy] Config load failed, defaulting to no-capture: {}",
                e
            );
            return false;
        }
    };

    let source = crate::macos::get_frontmost_app_info().unwrap_or_default();
    let source_name = source.name.to_lowercase();
    let source_bundle = source.bundle_id.to_lowercase();
    let source_path = source.path.to_lowercase();
    for excluded in &config.privacy.excluded_apps {
        let token = excluded.to_lowercase();
        if (!source_name.is_empty() && source_name.contains(&token))
            || (!source_bundle.is_empty() && source_bundle.contains(&token))
            || (!source_path.is_empty() && source_path.contains(&token))
        {
            return false;
        }
    }

    true
}
