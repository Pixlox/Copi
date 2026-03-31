use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::sync::runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CopiConfig {
    pub general: GeneralConfig,
    pub appearance: AppearanceConfig,
    pub privacy: PrivacyConfig,
    pub sync: SyncConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// Whether LAN sync is enabled
    pub enabled: bool,
    /// This device's display name (auto-detected if not set)
    pub device_name: Option<String>,
    /// Auto-connect to paired devices when they come online
    pub auto_connect: bool,
    /// Sync embeddings along with clips (increases bandwidth but avoids regeneration)
    pub sync_embeddings: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfigPayload {
    pub enabled: bool,
    pub device_name: Option<String>,
    pub auto_connect: bool,
    pub sync_embeddings: bool,
}

impl From<SyncConfig> for SyncConfigPayload {
    fn from(value: SyncConfig) -> Self {
        Self {
            enabled: value.enabled,
            device_name: value.device_name,
            auto_connect: value.auto_connect,
            sync_embeddings: value.sync_embeddings,
        }
    }
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
            sync: SyncConfig::default(),
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

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device_name: None,
            auto_connect: true,
            sync_embeddings: true,
        }
    }
}

fn config_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    if let Ok(dir) = app.path().app_config_dir() {
        return dir.join("config.toml");
    }

    if let Ok(dir) = app.path().app_local_data_dir() {
        return dir.join("config.toml");
    }

    std::env::temp_dir().join("copi").join("config.toml")
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
        if let Some(autolaunch) = app.try_state::<tauri_plugin_autostart::AutoLaunchManager>() {
            if let Ok(enabled) = autolaunch.is_enabled() {
                config.general.launch_at_login = enabled;
            }
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
            if let Some(autolaunch) = app.try_state::<tauri_plugin_autostart::AutoLaunchManager>()
            {
                if config.general.launch_at_login {
                    autolaunch.enable().map_err(|e| e.to_string())?;
                } else {
                    autolaunch.disable().map_err(|e| e.to_string())?;
                }
            }
        }
    }

    save_config(&app, &config)?;
    let _ = app.emit("sync:config-updated", SyncConfigPayload::from(config.sync.clone()));
    crate::sync::apply_config_change(&app, existing.as_ref(), &config);

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
    let db_path = app
        .path()
        .app_data_dir()
        .or_else(|_| app.path().app_local_data_dir())
        .unwrap_or_else(|_| std::env::temp_dir().join("copi"))
        .join("copi.db");

    let mut total_size = 0u64;
    for path in [
        db_path.clone(),
        db_path.with_extension("db-wal"),
        db_path.with_extension("db-shm"),
    ] {
        if let Ok(metadata) = std::fs::metadata(path) {
            total_size = total_size.saturating_add(metadata.len());
        }
    }

    Ok(total_size)
}

#[tauri::command]
pub async fn clear_all_history(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<crate::AppState>();
    let mut conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let sync_version = crate::sync::engine::SyncEngine::next_sync_version(&tx).unwrap_or(0);
    tx.execute(
        "UPDATE clips SET deleted = 1, sync_version = ?1 WHERE deleted = 0",
        rusqlite::params![sync_version],
    )
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

    let state = app.state::<crate::AppState>();
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT sync_id FROM clips WHERE deleted = 1")
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    drop(conn);

    for id in ids {
        runtime::queue_clip_sync_change(&app, id);
    }
    let _ = app.emit("clips-changed", ());
    let _ = app.emit("collections-changed", ());
    Ok(())
}
