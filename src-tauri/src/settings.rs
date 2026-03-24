use serde::{Deserialize, Serialize};
use tauri::Manager;

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
    pub privacy_rules: Vec<String>,
}

impl Default for CopiConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                hotkey: "alt+space".to_string(),
                launch_at_login: false,
                default_paste_behaviour: "copy".to_string(),
                history_retention_days: 90,
                auto_check_updates: true,
            },
            appearance: AppearanceConfig {
                theme: "dark".to_string(),
                compact_mode: false,
                show_app_icons: true,
            },
            privacy: PrivacyConfig {
                excluded_apps: vec![
                    "1Password".to_string(),
                    "com.agilebits.onepassword".to_string(),
                    "Keychain Access".to_string(),
                    "com.apple.keychainaccess".to_string(),
                ],
                privacy_rules: vec![r"^sk-[a-zA-Z0-9]{48}$".to_string(), r"^\d{16}$".to_string()],
            },
        }
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
    if !path.exists() {
        let config = CopiConfig::default();
        save_config(&app, &config)?;
        return Ok(config);
    }
    let content = std::fs::read_to_string(&path).map_err(|e: std::io::Error| e.to_string())?;
    toml::from_str(&content).map_err(|e| e.to_string())
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
                let _ = autolaunch.enable();
            } else {
                let _ = autolaunch.disable();
            }
        }
    }

    save_config(&app, &config)
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
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_db_size(app: tauri::AppHandle) -> Result<u64, String> {
    let db_path = app.path().app_data_dir().unwrap().join("copi.db");
    std::fs::metadata(&db_path).map(|m| m.len()).or(Ok(0))
}

#[tauri::command]
pub async fn clear_all_history(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<crate::AppState>();
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    conn.execute_batch("DELETE FROM clip_embeddings; DELETE FROM clips_fts; DELETE FROM clips;")
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn export_history_json(app: tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<crate::AppState>();
    let conn = state.db_read.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, content, content_type, source_app, created_at, pinned FROM clips ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let clips: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "content": row.get::<_, String>(1)?,
                "content_type": row.get::<_, String>(2)?,
                "source_app": row.get::<_, String>(3)?,
                "created_at": row.get::<_, i64>(4)?,
                "pinned": row.get::<_, i64>(5)? != 0,
            }))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&clips).map_err(|e| e.to_string())
}
