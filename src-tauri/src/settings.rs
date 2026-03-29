use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CopiConfig {
    pub general: GeneralConfig,
    pub appearance: AppearanceConfig,
    pub privacy: PrivacyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub hotkey: String,
    pub launch_at_login: bool,
    pub default_paste_behaviour: String,
    pub history_retention_days: i64,
    pub auto_check_updates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: String,
    pub compact_mode: bool,
    pub show_app_icons: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub excluded_apps: Vec<String>,
}

impl Default for CopiConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                hotkey: "alt+space".to_string(),
                launch_at_login: false,
                default_paste_behaviour: "paste".to_string(),
                history_retention_days: 90,
                auto_check_updates: true,
            },
            appearance: AppearanceConfig {
                theme: "dark".to_string(),
                compact_mode: false,
                show_app_icons: true,
            },
            privacy: PrivacyConfig {
                excluded_apps: default_excluded_apps(),
            },
        }
    }
}

#[cfg(target_os = "windows")]
fn default_excluded_apps() -> Vec<String> {
    vec![
        "1Password".to_string(),
        "Bitwarden".to_string(),
        "KeePass".to_string(),
        "LastPass".to_string(),
        "Windows Security".to_string(),
        "Credential Manager".to_string(),
    ]
}

#[cfg(not(target_os = "windows"))]
fn default_excluded_apps() -> Vec<String> {
    vec![
        "1Password".to_string(),
        "com.agilebits.onepassword".to_string(),
        "Keychain Access".to_string(),
        "com.apple.keychainaccess".to_string(),
    ]
}

#[cfg(target_os = "windows")]
fn normalize_windows_excluded_apps(apps: &mut Vec<String>) {
    let mac_only = [
        "keychain access",
        "com.apple.keychainaccess",
        "com.agilebits.onepassword",
    ];
    apps.retain(|app| {
        let token = app.trim().to_ascii_lowercase();
        !token.is_empty() && !mac_only.contains(&token.as_str())
    });

    if apps.is_empty() {
        *apps = default_excluded_apps();
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        CopiConfig::default().general
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        CopiConfig::default().appearance
    }
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        CopiConfig::default().privacy
    }
}

fn config_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path()
        .app_config_dir()
        .expect("Failed to get config dir")
        .join("config.toml")
}

#[tauri::command]
pub async fn get_config(app: tauri::AppHandle) -> Result<CopiConfig, String> {
    get_config_sync(app)
}

// Sync version for use from non-async contexts (cleanup task, setup)
pub fn get_config_sync(app: tauri::AppHandle) -> Result<CopiConfig, String> {
    let path = config_path(&app);
    let mut config = if !path.exists() {
        let config = CopiConfig::default();
        save_config(&app, &config)?;
        config
    } else {
        let content = std::fs::read_to_string(&path).map_err(|e: std::io::Error| e.to_string())?;
        toml::from_str(&content).map_err(|e| e.to_string())?
    };

    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        if let Ok(enabled) = app.autolaunch().is_enabled() {
            config.general.launch_at_login = enabled;
        }
    }

    #[cfg(target_os = "windows")]
    normalize_windows_excluded_apps(&mut config.privacy.excluded_apps);

    Ok(config)
}

#[tauri::command]
pub async fn set_config(app: tauri::AppHandle, config: CopiConfig) -> Result<(), String> {
    let existing = get_config_sync(app.clone()).ok();

    if existing
        .as_ref()
        .map(|current| current.general.hotkey != config.general.hotkey)
        .unwrap_or(true)
    {
        crate::hotkey::register_hotkey(&app, &config.general.hotkey)?;
    }

    // Handle autostart toggle
    let login_changed = existing
        .as_ref()
        .map(|current| current.general.launch_at_login != config.general.launch_at_login)
        .unwrap_or(true);

    if login_changed {
        #[cfg(desktop)]
        {
            use tauri_plugin_autostart::ManagerExt;
            let autolaunch = app.autolaunch();
            if config.general.launch_at_login {
                autolaunch.enable().map_err(|e| e.to_string())?;
            } else {
                autolaunch.disable().map_err(|e| e.to_string())?;
            }
        }
    }

    save_config(&app, &config)?;

    if existing
        .as_ref()
        .map(|current| {
            current.general.history_retention_days != config.general.history_retention_days
        })
        .unwrap_or(true)
    {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ =
                tokio::task::spawn_blocking(move || crate::cleanup_old_clips(&app_handle)).await;
        });
    }

    Ok(())
}

fn save_config(app: &tauri::AppHandle, config: &CopiConfig) -> Result<(), String> {
    let path = config_path(app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        toml::to_string_pretty(config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let _ = app.emit("config-changed", config.clone());
    Ok(())
}

#[tauri::command]
pub async fn get_db_size(app: tauri::AppHandle) -> Result<u64, String> {
    let db_path = app.path().app_data_dir().unwrap().join("copi.db");
    std::fs::metadata(&db_path).map(|m| m.len()).or(Ok(0))
}

#[tauri::command]
pub async fn clear_all_history(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<crate::AppState>();
    let mut conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM clips", [])
        .map_err(|e| e.to_string())?;
    tx.execute("DROP TABLE IF EXISTS clip_embeddings", [])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "CREATE VIRTUAL TABLE clip_embeddings USING vec0(embedding float[384])",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.execute_batch("INSERT INTO clips_fts(clips_fts) VALUES('rebuild');")
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);
    let _ = app.emit("clips-changed", ());
    let _ = app.emit("collections-changed", ());
    Ok(())
}
