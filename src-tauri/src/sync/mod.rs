//! LAN Sync Module for Copi
//!
//! Provides bidirectional clipboard sync between devices on the same local network.
//!
//! Architecture:
//! - `device`: Device identity management (keypair, device info)
//! - `discovery`: mDNS service discovery for finding peers
//! - `transport`: Encrypted TCP transport using Noise protocol
//! - `pairing`: 4-digit code pairing protocol
//! - `protocol`: Sync message types and serialization
//! - `engine`: Sync orchestration and conflict resolution

pub mod device;
pub mod discovery;
pub mod engine;
pub mod pairing;
pub mod protocol;
pub mod runtime;
pub mod transport;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use self::device::{DeviceIdentity, Platform};
use self::discovery::DiscoveryService;

/// Sync service state shared across the application
pub struct SyncService {
    /// This device's identity
    pub identity: Arc<RwLock<Option<DeviceIdentity>>>,
    /// Discovery service for finding peers
    pub discovery: Arc<RwLock<Option<DiscoveryService>>>,
    /// Channel for sync events (for UI updates)
    pub event_tx: broadcast::Sender<SyncEvent>,
    /// Whether sync is enabled
    pub enabled: Arc<RwLock<bool>>,
}

impl SyncService {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            identity: Arc::new(RwLock::new(None)),
            discovery: Arc::new(RwLock::new(None)),
            event_tx,
            enabled: Arc::new(RwLock::new(false)),
        }
    }
}

impl Default for SyncService {
    fn default() -> Self {
        Self::new()
    }
}

/// Events emitted by the sync service for UI updates
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncEvent {
    /// Sync service started
    Started,
    /// Sync service stopped
    Stopped,
    /// Device discovered on network
    DeviceDiscovered {
        device_id: String,
        device_name: String,
        platform: Platform,
    },
    /// Device went offline
    DeviceLost {
        device_id: String,
    },
    /// Pairing request received
    PairingRequest {
        device_id: String,
        device_name: String,
        platform: Platform,
    },
    /// Pairing completed successfully
    PairingComplete {
        device_id: String,
        device_name: String,
    },
    /// Pairing failed
    PairingFailed {
        device_id: String,
        reason: String,
    },
    /// Connected to a paired device
    Connected {
        device_id: String,
    },
    /// Disconnected from a device
    Disconnected {
        device_id: String,
    },
    /// Sync operation completed
    SyncComplete {
        device_id: String,
        items_synced: u32,
    },
    /// Sync error occurred
    SyncError {
        device_id: Option<String>,
        error: String,
    },
    /// Clip received from another device
    ClipReceived {
        sync_id: String,
        from_device: String,
    },
    /// Clip deleted on another device
    ClipDeleted {
        sync_id: String,
        from_device: String,
    },
}

/// Error types for sync operations
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Transport error: {0}")]
    TransportError(String),

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

impl From<rusqlite::Error> for SyncError {
    fn from(e: rusqlite::Error) -> Self {
        SyncError::DatabaseError(e.to_string())
    }
}

impl From<serde_json::Error> for SyncError {
    fn from(e: serde_json::Error) -> Self {
        SyncError::SerializationError(e.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDeviceIdentityPayload {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairedDevicePayload {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub paired_at: i64,
    pub last_seen: Option<i64>,
    pub last_sync_version: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusPayload {
    pub enabled: bool,
    pub device: Option<SyncDeviceIdentityPayload>,
    pub paired_count: usize,
    pub connected_count: usize,
    pub queue_depth: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDiscoveredDevicePayload {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub is_paired: bool,
    pub is_connected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairingCodePayload {
    pub code: String,
    pub expires_at: i64,
}

#[tauri::command]
pub async fn sync_get_status(app: tauri::AppHandle) -> Result<SyncStatusPayload, String> {
    runtime::get_status(app).await
}

#[tauri::command]
pub async fn sync_list_paired_devices(
    app: tauri::AppHandle,
) -> Result<Vec<SyncPairedDevicePayload>, String> {
    runtime::list_paired_devices(app).await
}

#[tauri::command]
pub async fn sync_unpair_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    runtime::unpair_device(app, device_id).await
}

#[tauri::command]
pub async fn sync_pair_device_manual(
    app: tauri::AppHandle,
    device_id: String,
    device_name: String,
    platform: String,
    public_key_base64: String,
) -> Result<(), String> {
    runtime::pair_device_manual(app, device_id, device_name, platform, public_key_base64).await
}

#[tauri::command]
pub async fn sync_list_discovered_devices(
    app: tauri::AppHandle,
) -> Result<Vec<SyncDiscoveredDevicePayload>, String> {
    runtime::list_discovered_devices(app).await
}

#[tauri::command]
pub async fn sync_start_pairing(
    app: tauri::AppHandle,
) -> Result<SyncPairingCodePayload, String> {
    runtime::start_pairing(app).await
}

#[tauri::command]
pub async fn sync_pair_with_code(
    app: tauri::AppHandle,
    device_id: String,
    code: String,
) -> Result<(), String> {
    runtime::pair_with_code(app, device_id, code).await
}

pub fn initialize_sync_if_enabled(app: &tauri::AppHandle) -> Result<(), String> {
    runtime::initialize_sync_if_enabled(app)
}

pub fn apply_config_change(
    app: &tauri::AppHandle,
    previous: Option<&crate::settings::CopiConfig>,
    next: &crate::settings::CopiConfig,
) {
    runtime::apply_config_change(app, previous, next);
}

/// Result type for sync operations
pub type SyncResult<T> = Result<T, SyncError>;
