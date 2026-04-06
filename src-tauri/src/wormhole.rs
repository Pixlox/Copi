//! Copi Wormhole - Large file transfer between synced devices
//!
//! Provides manual file transfer for any file size, with AirDrop-style
//! progress feedback including speed and ETA estimation.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, SeekFrom};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

use crate::AppState;

// ─── Constants ────────────────────────────────────────────────────

/// Default expiration time in hours
pub const DEFAULT_EXPIRATION_HOURS: u32 = 24;

/// Chunk sizes based on file size for optimal throughput
const CHUNK_SIZE_SMALL: usize = 32 * 1024; // <10MB: 32KB
const CHUNK_SIZE_MEDIUM: usize = 64 * 1024; // 10-100MB: 64KB
const CHUNK_SIZE_LARGE: usize = 256 * 1024; // 100MB-1GB: 256KB
const CHUNK_SIZE_XLARGE: usize = 512 * 1024; // >1GB: 512KB

/// Progress event throttling
const PROGRESS_EMIT_INTERVAL_MS: u64 = 100;
const PROGRESS_EMIT_MIN_BYTES: u64 = 1_000_000; // 1MB

// ─── Wire Protocol Types ──────────────────────────────────────────

/// Wormhole offer - sent when a file is made available
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeOffer {
    pub id: String,
    pub file_name: String,
    pub file_size: u64,
    pub mime_type: Option<String>,
    pub checksum: String,
    pub expires_at: String,
}

/// Wormhole request - receiver requests file download
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeRequest {
    pub file_id: String,
    pub resume_from: Option<u64>,
}

/// Wormhole chunk - streamed file data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeChunk {
    pub file_id: String,
    pub offset: u64,
    pub data: Vec<u8>,
    pub is_final: bool,
}

/// Wormhole rejection/error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeReject {
    pub file_id: String,
    pub reason: String,
}

// ─── Database/State Types ─────────────────────────────────────────

/// File status in the wormhole system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WormholeStatus {
    Pending,
    Uploading,
    Available,
    Downloading,
    Completed,
    Expired,
    Cancelled,
    Failed,
}

impl WormholeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uploading => "uploading",
            Self::Available => "available",
            Self::Downloading => "downloading",
            Self::Completed => "completed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "uploading" => Self::Uploading,
            "available" => Self::Available,
            "downloading" => Self::Downloading,
            "completed" => Self::Completed,
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// Wormhole file record (mirrors database schema)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeFile {
    pub id: String,
    pub file_name: String,
    pub file_size: u64,
    pub mime_type: Option<String>,
    pub checksum: String,
    pub origin_device_id: String,
    pub origin_device_name: Option<String>,
    pub is_local: bool,
    pub status: WormholeStatus,
    pub bytes_transferred: u64,
    pub transfer_started_at: Option<String>,
    pub transfer_completed_at: Option<String>,
    pub local_path: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

/// Transfer progress for UI updates (AirDrop-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferProgress {
    pub file_id: String,
    pub file_name: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub percent_complete: u8,
    pub speed_bytes_per_sec: u64,
    pub estimated_seconds_remaining: u64,
    pub is_upload: bool,
}

// ─── Runtime State ────────────────────────────────────────────────

/// Tracks active transfer state for progress calculation
struct ActiveTransfer {
    file_id: String,
    file_name: String,
    total_bytes: u64,
    bytes_transferred: AtomicU64,
    start_time: Instant,
    last_emit_time: AsyncMutex<Instant>,
    last_emit_bytes: AtomicU64,
    recent_speeds: AsyncMutex<VecDeque<f64>>,
    is_upload: bool,
    cancelled: AtomicU64, // 0 = not cancelled, 1 = cancelled
}

impl ActiveTransfer {
    fn new(file_id: String, file_name: String, total_bytes: u64, is_upload: bool) -> Self {
        Self {
            file_id,
            file_name,
            total_bytes,
            bytes_transferred: AtomicU64::new(0),
            start_time: Instant::now(),
            last_emit_time: AsyncMutex::new(Instant::now()),
            last_emit_bytes: AtomicU64::new(0),
            recent_speeds: AsyncMutex::new(VecDeque::with_capacity(5)),
            is_upload,
            cancelled: AtomicU64::new(0),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst) != 0
    }

    fn cancel(&self) {
        self.cancelled.store(1, Ordering::SeqCst);
    }

    async fn update_progress(&self, new_bytes: u64) -> Option<TransferProgress> {
        let current = self
            .bytes_transferred
            .fetch_add(new_bytes, Ordering::SeqCst)
            + new_bytes;

        let mut last_emit = self.last_emit_time.lock().await;
        let last_bytes = self.last_emit_bytes.load(Ordering::SeqCst);
        let elapsed_ms = last_emit.elapsed().as_millis() as u64;
        let bytes_since_emit = current.saturating_sub(last_bytes);

        // Throttle: emit at most every 100ms OR every 1MB
        if elapsed_ms < PROGRESS_EMIT_INTERVAL_MS && bytes_since_emit < PROGRESS_EMIT_MIN_BYTES {
            return None;
        }

        // Calculate speed (rolling average of last 5 measurements)
        let elapsed_secs = elapsed_ms as f64 / 1000.0;
        let instant_speed = if elapsed_secs > 0.0 {
            bytes_since_emit as f64 / elapsed_secs
        } else {
            0.0
        };

        let mut speeds = self.recent_speeds.lock().await;
        speeds.push_back(instant_speed);
        if speeds.len() > 5 {
            speeds.pop_front();
        }
        let avg_speed = if speeds.is_empty() {
            0.0
        } else {
            speeds.iter().sum::<f64>() / speeds.len() as f64
        };

        // Calculate ETA
        let remaining_bytes = self.total_bytes.saturating_sub(current);
        let eta_seconds = if avg_speed > 0.0 {
            (remaining_bytes as f64 / avg_speed) as u64
        } else {
            0
        };

        // Update emit tracking
        *last_emit = Instant::now();
        self.last_emit_bytes.store(current, Ordering::SeqCst);

        let percent = if self.total_bytes > 0 {
            ((current as f64 / self.total_bytes as f64) * 100.0) as u8
        } else {
            0
        };

        Some(TransferProgress {
            file_id: self.file_id.clone(),
            file_name: self.file_name.clone(),
            bytes_transferred: current,
            total_bytes: self.total_bytes,
            percent_complete: percent.min(100),
            speed_bytes_per_sec: avg_speed as u64,
            estimated_seconds_remaining: eta_seconds,
            is_upload: self.is_upload,
        })
    }
}

/// Global wormhole state
pub struct WormholeState {
    /// Active transfers (file_id -> transfer state)
    active_transfers: RwLock<HashMap<String, Arc<ActiveTransfer>>>,
    /// Pending download writers (file_id -> temp file path)
    pending_downloads: RwLock<HashMap<String, PathBuf>>,
}

impl WormholeState {
    pub fn new() -> Self {
        Self {
            active_transfers: RwLock::new(HashMap::new()),
            pending_downloads: RwLock::new(HashMap::new()),
        }
    }

    pub async fn start_transfer(
        &self,
        file_id: String,
        file_name: String,
        total_bytes: u64,
        is_upload: bool,
    ) -> Arc<ActiveTransfer> {
        let transfer = Arc::new(ActiveTransfer::new(file_id.clone(), file_name, total_bytes, is_upload));
        self.active_transfers
            .write()
            .await
            .insert(file_id, transfer.clone());
        transfer
    }

    pub async fn get_transfer(&self, file_id: &str) -> Option<Arc<ActiveTransfer>> {
        self.active_transfers.read().await.get(file_id).cloned()
    }

    pub async fn remove_transfer(&self, file_id: &str) {
        self.active_transfers.write().await.remove(file_id);
    }

    pub async fn cancel_transfer(&self, file_id: &str) -> bool {
        if let Some(transfer) = self.active_transfers.read().await.get(file_id) {
            transfer.cancel();
            true
        } else {
            false
        }
    }
}

impl Default for WormholeState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Utility Functions ────────────────────────────────────────────

/// Get optimal chunk size based on file size
pub fn get_optimal_chunk_size(file_size: u64) -> usize {
    match file_size {
        0..=10_000_000 => CHUNK_SIZE_SMALL,
        10_000_001..=100_000_000 => CHUNK_SIZE_MEDIUM,
        100_000_001..=1_000_000_000 => CHUNK_SIZE_LARGE,
        _ => CHUNK_SIZE_XLARGE,
    }
}

/// Compute SHA-256 checksum of a file (streaming, memory efficient)
pub async fn compute_file_checksum(path: &std::path::Path) -> Result<String> {
    let file = File::open(path).await.context("open file for checksum")?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let n = reader.read(&mut buffer).await.context("read file")?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Verify checksum of downloaded file
pub async fn verify_file_checksum(path: &std::path::Path, expected: &str) -> Result<bool> {
    let actual = compute_file_checksum(path).await?;
    Ok(actual == expected)
}

/// Get platform-specific Downloads folder
pub fn get_downloads_folder() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| {
        #[cfg(target_os = "windows")]
        {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                return PathBuf::from(profile).join("Downloads");
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join("Downloads");
            }
        }
        std::env::temp_dir()
    })
}

/// Detect MIME type from file extension
pub fn detect_mime_type(file_name: &str) -> Option<String> {
    let ext = std::path::Path::new(file_name)
        .extension()?
        .to_str()?
        .to_lowercase();

    let mime = match ext.as_str() {
        // Documents
        "pdf" => "application/pdf",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        "ppt" | "pptx" => "application/vnd.ms-powerpoint",
        "txt" => "text/plain",
        "rtf" => "application/rtf",
        "csv" => "text/csv",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        // Video
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        // Archives
        "zip" => "application/zip",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        // Code
        "js" => "text/javascript",
        "ts" => "text/typescript",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "rb" => "text/x-ruby",
        "go" => "text/x-go",
        "java" => "text/x-java",
        "c" | "h" => "text/x-c",
        "cpp" | "hpp" => "text/x-c++",
        // Executables
        "exe" => "application/x-msdownload",
        "dmg" => "application/x-apple-diskimage",
        "app" => "application/x-apple-app",
        "deb" => "application/x-deb",
        "rpm" => "application/x-rpm",
        _ => return None,
    };

    Some(mime.to_string())
}

/// Format bytes for human display
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes < KB {
        format!("{} B", bytes)
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    }
}

// ─── Database Operations ──────────────────────────────────────────

/// Initialize wormhole database table
pub fn init_wormhole_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS wormhole_files (
            id TEXT PRIMARY KEY,
            file_name TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            mime_type TEXT,
            checksum TEXT NOT NULL,
            origin_device_id TEXT NOT NULL,
            origin_device_name TEXT,
            is_local INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'pending',
            bytes_transferred INTEGER DEFAULT 0,
            transfer_started_at TEXT,
            transfer_completed_at TEXT,
            local_path TEXT,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_wormhole_status ON wormhole_files(status);
        CREATE INDEX IF NOT EXISTS idx_wormhole_expires ON wormhole_files(expires_at);
        CREATE INDEX IF NOT EXISTS idx_wormhole_origin ON wormhole_files(origin_device_id);
        ",
    )
}

/// Insert a new wormhole file record
pub fn insert_wormhole_file(conn: &rusqlite::Connection, file: &WormholeFile) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO wormhole_files (
            id, file_name, file_size, mime_type, checksum,
            origin_device_id, origin_device_name, is_local, status,
            bytes_transferred, transfer_started_at, transfer_completed_at,
            local_path, created_at, expires_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        rusqlite::params![
            file.id,
            file.file_name,
            file.file_size as i64,
            file.mime_type,
            file.checksum,
            file.origin_device_id,
            file.origin_device_name,
            file.is_local as i32,
            file.status.as_str(),
            file.bytes_transferred as i64,
            file.transfer_started_at,
            file.transfer_completed_at,
            file.local_path,
            file.created_at,
            file.expires_at,
        ],
    )?;
    Ok(())
}

/// Update wormhole file status
pub fn update_wormhole_status(
    conn: &rusqlite::Connection,
    file_id: &str,
    status: WormholeStatus,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE wormhole_files SET status = ?1 WHERE id = ?2",
        rusqlite::params![status.as_str(), file_id],
    )?;
    Ok(())
}

/// Update wormhole transfer progress
pub fn update_wormhole_progress(
    conn: &rusqlite::Connection,
    file_id: &str,
    bytes_transferred: u64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE wormhole_files SET bytes_transferred = ?1 WHERE id = ?2",
        rusqlite::params![bytes_transferred as i64, file_id],
    )?;
    Ok(())
}

/// Mark transfer as started
pub fn mark_transfer_started(conn: &rusqlite::Connection, file_id: &str) -> rusqlite::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE wormhole_files SET transfer_started_at = ?1, status = ?2 WHERE id = ?3",
        rusqlite::params![now, WormholeStatus::Uploading.as_str(), file_id],
    )?;
    Ok(())
}

/// Mark transfer as completed
pub fn mark_transfer_completed(
    conn: &rusqlite::Connection,
    file_id: &str,
    local_path: Option<&str>,
) -> rusqlite::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE wormhole_files SET transfer_completed_at = ?1, status = ?2, local_path = COALESCE(?3, local_path) WHERE id = ?4",
        rusqlite::params![now, WormholeStatus::Completed.as_str(), local_path, file_id],
    )?;
    Ok(())
}

/// Get wormhole file by ID
pub fn get_wormhole_file(conn: &rusqlite::Connection, file_id: &str) -> rusqlite::Result<Option<WormholeFile>> {
    conn.query_row(
        "SELECT id, file_name, file_size, mime_type, checksum,
                origin_device_id, origin_device_name, is_local, status,
                bytes_transferred, transfer_started_at, transfer_completed_at,
                local_path, created_at, expires_at
         FROM wormhole_files WHERE id = ?1",
        [file_id],
        |row| {
            Ok(WormholeFile {
                id: row.get(0)?,
                file_name: row.get(1)?,
                file_size: row.get::<_, i64>(2)? as u64,
                mime_type: row.get(3)?,
                checksum: row.get(4)?,
                origin_device_id: row.get(5)?,
                origin_device_name: row.get(6)?,
                is_local: row.get::<_, i32>(7)? != 0,
                status: WormholeStatus::from_str(&row.get::<_, String>(8)?),
                bytes_transferred: row.get::<_, i64>(9)? as u64,
                transfer_started_at: row.get(10)?,
                transfer_completed_at: row.get(11)?,
                local_path: row.get(12)?,
                created_at: row.get(13)?,
                expires_at: row.get(14)?,
            })
        },
    )
    .optional()
}

/// List all wormhole files (active, not expired)
pub fn list_wormhole_files(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<WormholeFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_name, file_size, mime_type, checksum,
                origin_device_id, origin_device_name, is_local, status,
                bytes_transferred, transfer_started_at, transfer_completed_at,
                local_path, created_at, expires_at
         FROM wormhole_files
         WHERE status NOT IN ('expired', 'cancelled')
         ORDER BY created_at DESC",
    )?;

    let files = stmt
        .query_map([], |row| {
            Ok(WormholeFile {
                id: row.get(0)?,
                file_name: row.get(1)?,
                file_size: row.get::<_, i64>(2)? as u64,
                mime_type: row.get(3)?,
                checksum: row.get(4)?,
                origin_device_id: row.get(5)?,
                origin_device_name: row.get(6)?,
                is_local: row.get::<_, i32>(7)? != 0,
                status: WormholeStatus::from_str(&row.get::<_, String>(8)?),
                bytes_transferred: row.get::<_, i64>(9)? as u64,
                transfer_started_at: row.get(10)?,
                transfer_completed_at: row.get(11)?,
                local_path: row.get(12)?,
                created_at: row.get(13)?,
                expires_at: row.get(14)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(files)
}

/// Delete wormhole file record
pub fn delete_wormhole_file(conn: &rusqlite::Connection, file_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM wormhole_files WHERE id = ?1", [file_id])?;
    Ok(())
}

/// Get expired files
pub fn get_expired_files(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<WormholeFile>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, file_name, file_size, mime_type, checksum,
                origin_device_id, origin_device_name, is_local, status,
                bytes_transferred, transfer_started_at, transfer_completed_at,
                local_path, created_at, expires_at
         FROM wormhole_files
         WHERE expires_at < ?1 AND status NOT IN ('expired', 'cancelled', 'completed')",
    )?;

    let files = stmt
        .query_map([now], |row| {
            Ok(WormholeFile {
                id: row.get(0)?,
                file_name: row.get(1)?,
                file_size: row.get::<_, i64>(2)? as u64,
                mime_type: row.get(3)?,
                checksum: row.get(4)?,
                origin_device_id: row.get(5)?,
                origin_device_name: row.get(6)?,
                is_local: row.get::<_, i32>(7)? != 0,
                status: WormholeStatus::from_str(&row.get::<_, String>(8)?),
                bytes_transferred: row.get::<_, i64>(9)? as u64,
                transfer_started_at: row.get(10)?,
                transfer_completed_at: row.get(11)?,
                local_path: row.get(12)?,
                created_at: row.get(13)?,
                expires_at: row.get(14)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(files)
}

/// Mark expired files
pub fn mark_expired_files(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE wormhole_files SET status = 'expired' WHERE expires_at < ?1 AND status NOT IN ('expired', 'cancelled', 'completed')",
        [now],
    )?;
    Ok(count)
}

/// Clear completed/expired files from database
pub fn clear_completed_files(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    let count = conn.execute(
        "DELETE FROM wormhole_files WHERE status IN ('completed', 'expired', 'cancelled', 'failed')",
        [],
    )?;
    Ok(count)
}

// ─── Tauri Commands ───────────────────────────────────────────────

/// Offer a file for transfer via Wormhole
#[tauri::command]
pub async fn wormhole_offer_file(
    path: String,
    app: AppHandle,
) -> Result<WormholeFile, String> {
    let path = PathBuf::from(&path);

    // Validate file exists
    if !path.exists() {
        return Err(format!("File not found: {}", path.display()));
    }

    // Reject directories
    if path.is_dir() {
        return Err("Folders are not supported. Please select individual files.".to_string());
    }

    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("Cannot read file: {}", e))?;

    let file_size = metadata.len();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    eprintln!(
        "[Wormhole] Offering file: {} ({} bytes)",
        file_name, file_size
    );

    // Compute checksum
    let checksum = compute_file_checksum(&path)
        .await
        .map_err(|e| format!("Failed to compute checksum: {}", e))?;

    // Get device info
    let state = app.state::<AppState>();
    let sync = state
        .sync
        .get()
        .ok_or("Sync not initialized")?;

    let device_id = sync.device_id.clone();
    let device_name = sync.device_name.clone();

    // Get expiration time from config (default 24h)
    let expiration_hours = crate::settings::get_config_sync(app.clone())
        .map(|c| c.sync.wormhole_expiration_hours.unwrap_or(DEFAULT_EXPIRATION_HOURS))
        .unwrap_or(DEFAULT_EXPIRATION_HOURS);

    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::hours(expiration_hours as i64);

    let file_id = uuid::Uuid::new_v4().to_string();
    let mime_type = detect_mime_type(&file_name);

    let wormhole_file = WormholeFile {
        id: file_id.clone(),
        file_name: file_name.clone(),
        file_size,
        mime_type: mime_type.clone(),
        checksum: checksum.clone(),
        origin_device_id: device_id.clone(),
        origin_device_name: Some(device_name.clone()),
        is_local: true,
        status: WormholeStatus::Available,
        bytes_transferred: 0,
        transfer_started_at: None,
        transfer_completed_at: None,
        local_path: Some(path.to_string_lossy().to_string()),
        created_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };

    // Store in database
    {
        let conn = state.db_write.lock().map_err(|e| e.to_string())?;
        insert_wormhole_file(&conn, &wormhole_file).map_err(|e| e.to_string())?;
    }

    // Broadcast offer to all connected peers
    let offer = WormholeOffer {
        id: file_id.clone(),
        file_name,
        file_size,
        mime_type,
        checksum,
        expires_at: expires_at.to_rfc3339(),
    };

    // Broadcast via sync (we'll add this to sync.rs)
    if let Err(e) = sync.broadcast_wormhole_offer(offer).await {
        eprintln!("[Wormhole] Failed to broadcast offer: {}", e);
    }

    // Emit event to local UI
    let _ = app.emit("wormhole://file-offered", &wormhole_file);

    eprintln!("[Wormhole] File offered: {}", file_id);
    Ok(wormhole_file)
}

/// Retract a file offer
#[tauri::command]
pub async fn wormhole_retract(file_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();

    // Update database
    {
        let conn = state.db_write.lock().map_err(|e| e.to_string())?;
        update_wormhole_status(&conn, &file_id, WormholeStatus::Cancelled)
            .map_err(|e| e.to_string())?;
    }

    // Cancel any active transfer
    if let Some(wormhole_state) = app.try_state::<WormholeState>() {
        wormhole_state.cancel_transfer(&file_id).await;
    }

    // Broadcast retraction to peers
    if let Some(sync) = state.sync.get() {
        if let Err(e) = sync.broadcast_wormhole_retract(&file_id).await {
            eprintln!("[Wormhole] Failed to broadcast retraction: {}", e);
        }
    }

    // Emit event to local UI
    let _ = app.emit("wormhole://file-retracted", &file_id);

    eprintln!("[Wormhole] File retracted: {}", file_id);
    Ok(())
}

/// Request download of a file
#[tauri::command]
pub async fn wormhole_request_download(
    file_id: String,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();

    // Get file info
    let file = {
        let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
        get_wormhole_file(&conn, &file_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "File not found".to_string())?
    };

    if file.is_local {
        return Err("Cannot download your own file".to_string());
    }

    // Update status to downloading
    {
        let conn = state.db_write.lock().map_err(|e| e.to_string())?;
        update_wormhole_status(&conn, &file_id, WormholeStatus::Downloading)
            .map_err(|e| e.to_string())?;
    }

    // Send request to origin device
    if let Some(sync) = state.sync.get() {
        let request = WormholeRequest {
            file_id: file_id.clone(),
            resume_from: None, // TODO: support resume
        };
        if let Err(e) = sync
            .send_wormhole_request(&file.origin_device_id, request)
            .await
        {
            // Revert status on failure
            let conn = state.db_write.lock().map_err(|e| e.to_string())?;
            let _ = update_wormhole_status(&conn, &file_id, WormholeStatus::Available);
            return Err(format!("Failed to request download: {}", e));
        }
    }

    eprintln!("[Wormhole] Download requested: {}", file_id);
    Ok(())
}

/// Cancel an in-progress download
#[tauri::command]
pub async fn wormhole_cancel_download(file_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();

    // Cancel active transfer
    if let Some(wormhole_state) = app.try_state::<WormholeState>() {
        wormhole_state.cancel_transfer(&file_id).await;
    }

    // Update database
    {
        let conn = state.db_write.lock().map_err(|e| e.to_string())?;
        update_wormhole_status(&conn, &file_id, WormholeStatus::Cancelled)
            .map_err(|e| e.to_string())?;
    }

    // Emit event
    let _ = app.emit("wormhole://download-cancelled", &file_id);

    eprintln!("[Wormhole] Download cancelled: {}", file_id);
    Ok(())
}

/// List all wormhole files
#[tauri::command]
pub async fn wormhole_list_files(app: AppHandle) -> Result<Vec<WormholeFile>, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    list_wormhole_files(&conn).map_err(|e| e.to_string())
}

/// Get a single wormhole file by ID
#[tauri::command]
pub async fn wormhole_get_file(file_id: String, app: AppHandle) -> Result<Option<WormholeFile>, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    get_wormhole_file(&conn, &file_id).map_err(|e| e.to_string())
}

/// Clear completed/expired files
#[tauri::command]
pub async fn wormhole_clear_completed(app: AppHandle) -> Result<usize, String> {
    let state = app.state::<AppState>();
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    clear_completed_files(&conn).map_err(|e| e.to_string())
}

/// Get count of pending incoming files (for badge)
#[tauri::command]
pub async fn wormhole_get_pending_count(app: AppHandle) -> Result<usize, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wormhole_files WHERE is_local = 0 AND status IN ('pending', 'available')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    Ok(count as usize)
}

/// Open wormhole window
#[tauri::command]
pub async fn open_wormhole_window(app: AppHandle) -> Result<(), String> {
    use tauri::WebviewWindowBuilder;
    use tauri::WebviewUrl;

    if let Some(window) = app.get_webview_window("wormhole") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        // Create window if it doesn't exist
        let window = WebviewWindowBuilder::new(&app, "wormhole", WebviewUrl::App("/wormhole.html".into()))
            .title("Wormhole")
            .inner_size(420.0, 560.0)
            .min_inner_size(380.0, 480.0)
            .max_inner_size(500.0, 700.0)
            .center()
            .resizable(true)
            .build()
            .map_err(|e| e.to_string())?;

        // Apply vibrancy on macOS
        #[cfg(target_os = "macos")]
        {
            use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
            let _ = apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, Some(12.0));
        }
    }

    // Update dock visibility on macOS
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }

    Ok(())
}

// ─── Background Tasks ─────────────────────────────────────────────

/// Start the wormhole expiry cleanup task
pub fn start_expiry_task(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;

            if let Some(state) = app.try_state::<AppState>() {
                // Get expired files and broadcast retractions
                let expired_files = {
                    let Ok(conn) = state.db_read_pool.get() else {
                        continue;
                    };
                    get_expired_files(&conn).unwrap_or_default()
                };

                for file in &expired_files {
                    if file.is_local {
                        // Broadcast retraction for our expired files
                        if let Some(sync) = state.sync.get() {
                            let _ = sync.broadcast_wormhole_retract(&file.id).await;
                        }
                    }
                    // Emit expiry event
                    let _ = app.emit("wormhole://file-expired", &file.id);
                }

                // Mark expired in database
                if !expired_files.is_empty() {
                    if let Ok(conn) = state.db_write.lock() {
                        let count = mark_expired_files(&conn).unwrap_or(0);
                        if count > 0 {
                            eprintln!("[Wormhole] Marked {} files as expired", count);
                        }
                    }
                }
            }
        }
    });
}

// ─── File Streaming ───────────────────────────────────────────────

/// Stream file chunks to a peer (called when we receive a WormholeRequest)
pub async fn stream_file_to_peer(
    app: &AppHandle,
    file_id: &str,
    peer_device_id: &str,
    resume_from: u64,
) -> Result<()> {
    let state = app.state::<AppState>();

    // Get file info
    let file = {
        let conn = state.db_read_pool.get().context("get db connection")?;
        get_wormhole_file(&conn, file_id)
            .context("query wormhole file")?
            .ok_or_else(|| anyhow!("File not found: {}", file_id))?
    };

    let local_path = file
        .local_path
        .as_ref()
        .ok_or_else(|| anyhow!("No local path for file"))?;

    let path = PathBuf::from(local_path);
    if !path.exists() {
        return Err(anyhow!("Source file no longer exists: {}", local_path));
    }

    let sync = state
        .sync
        .get()
        .ok_or_else(|| anyhow!("Sync not initialized"))?;

    // Start tracking transfer
    let wormhole_state = app
        .try_state::<WormholeState>()
        .ok_or_else(|| anyhow!("Wormhole state not initialized"))?;

    let transfer = wormhole_state
        .start_transfer(file_id.to_string(), file.file_name.clone(), file.file_size, true)
        .await;

    // Mark transfer started
    {
        let conn = state.db_write.lock().map_err(|_| anyhow!("db lock"))?;
        mark_transfer_started(&conn, file_id)?;
    }

    let chunk_size = get_optimal_chunk_size(file.file_size);
    let mut source = File::open(&path).await.context("open source file")?;

    if resume_from > 0 {
        source
            .seek(SeekFrom::Start(resume_from))
            .await
            .context("seek to resume position")?;
        transfer
            .bytes_transferred
            .store(resume_from, Ordering::SeqCst);
    }

    let mut reader = BufReader::with_capacity(chunk_size * 2, source);
    let mut buffer = vec![0u8; chunk_size];
    let mut offset = resume_from;

    loop {
        if transfer.is_cancelled() {
            eprintln!("[Wormhole] Transfer cancelled: {}", file_id);
            return Err(anyhow!("Transfer cancelled"));
        }

        let bytes_read = reader.read(&mut buffer).await.context("read chunk")?;
        if bytes_read == 0 {
            break;
        }

        let is_final = offset + bytes_read as u64 >= file.file_size;

        let chunk = WormholeChunk {
            file_id: file_id.to_string(),
            offset,
            data: buffer[..bytes_read].to_vec(),
            is_final,
        };

        // Send chunk to peer
        sync.send_wormhole_chunk(peer_device_id, chunk)
            .await
            .context("send chunk")?;

        offset += bytes_read as u64;

        // Update progress and emit event
        if let Some(progress) = transfer.update_progress(bytes_read as u64).await {
            let _ = app.emit("wormhole://transfer-progress", &progress);
        }
    }

    // Mark complete
    {
        let conn = state.db_write.lock().map_err(|_| anyhow!("db lock"))?;
        mark_transfer_completed(&conn, file_id, None)?;
    }

    wormhole_state.remove_transfer(file_id).await;

    // Send completion message
    sync.send_wormhole_complete(peer_device_id, file_id)
        .await
        .context("send complete")?;

    let _ = app.emit("wormhole://transfer-complete", file_id);

    eprintln!(
        "[Wormhole] File transfer complete: {} ({} bytes)",
        file_id, file.file_size
    );

    Ok(())
}

/// Handle incoming file chunk (called when we receive WormholeChunk)
pub async fn handle_incoming_chunk(
    app: &AppHandle,
    chunk: WormholeChunk,
) -> Result<()> {
    let state = app.state::<AppState>();
    let wormhole_state = app
        .try_state::<WormholeState>()
        .ok_or_else(|| anyhow!("Wormhole state not initialized"))?;

    // Get or create transfer state
    let transfer = if let Some(t) = wormhole_state.get_transfer(&chunk.file_id).await {
        t
    } else {
        // First chunk - get file info and set up download
        let file = {
            let conn = state.db_read_pool.get().context("get db connection")?;
            get_wormhole_file(&conn, &chunk.file_id)
                .context("query file")?
                .ok_or_else(|| anyhow!("Unknown file: {}", chunk.file_id))?
        };

        wormhole_state
            .start_transfer(
                chunk.file_id.clone(),
                file.file_name.clone(),
                file.file_size,
                false,
            )
            .await
    };

    if transfer.is_cancelled() {
        return Err(anyhow!("Download cancelled"));
    }

    // Determine download path
    let download_path = {
        let file = {
            let conn = state.db_read_pool.get().context("get db connection")?;
            get_wormhole_file(&conn, &chunk.file_id)
                .context("query file")?
                .ok_or_else(|| anyhow!("Unknown file: {}", chunk.file_id))?
        };

        let downloads_dir = get_downloads_folder();
        let mut path = downloads_dir.join(&file.file_name);

        // Handle duplicate filenames
        if path.exists() && chunk.offset == 0 {
            // Extract stem and ext as owned Strings before the loop to avoid borrow issues
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            for i in 1..100 {
                let new_name = if ext.is_empty() {
                    format!("{} ({})", stem, i)
                } else {
                    format!("{} ({}).{}", stem, i, ext)
                };
                path = downloads_dir.join(new_name);
                if !path.exists() {
                    break;
                }
            }
        }

        path
    };

    // Open file for writing
    let mut file = if chunk.offset == 0 {
        File::create(&download_path)
            .await
            .context("create download file")?
    } else {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&download_path)
            .await
            .context("open download file")?;
        f.seek(SeekFrom::Start(chunk.offset))
            .await
            .context("seek to offset")?;
        f
    };

    // Write chunk
    file.write_all(&chunk.data)
        .await
        .context("write chunk data")?;

    // Update progress
    if let Some(progress) = transfer.update_progress(chunk.data.len() as u64).await {
        let _ = app.emit("wormhole://transfer-progress", &progress);
    }

    // Handle completion
    if chunk.is_final {
        file.flush().await.context("flush file")?;

        // Verify checksum
        let file_info = {
            let conn = state.db_read_pool.get().context("get db connection")?;
            get_wormhole_file(&conn, &chunk.file_id)
                .context("query file")?
                .ok_or_else(|| anyhow!("Unknown file: {}", chunk.file_id))?
        };

        let checksum_valid = verify_file_checksum(&download_path, &file_info.checksum)
            .await
            .unwrap_or(false);

        if !checksum_valid {
            eprintln!(
                "[Wormhole] Checksum mismatch for {}, deleting partial download",
                chunk.file_id
            );
            let _ = tokio::fs::remove_file(&download_path).await;

            // Update status to failed
            if let Ok(conn) = state.db_write.lock() {
                let _ = update_wormhole_status(&conn, &chunk.file_id, WormholeStatus::Failed);
            }

            wormhole_state.remove_transfer(&chunk.file_id).await;

            let _ = app.emit("wormhole://transfer-failed", serde_json::json!({
                "file_id": chunk.file_id,
                "reason": "Checksum verification failed"
            }));

            return Err(anyhow!("Checksum verification failed"));
        }

        // Mark complete in database
        {
            let conn = state.db_write.lock().map_err(|_| anyhow!("db lock"))?;
            mark_transfer_completed(
                &conn,
                &chunk.file_id,
                Some(&download_path.to_string_lossy()),
            )?;
        }

        wormhole_state.remove_transfer(&chunk.file_id).await;

        let _ = app.emit("wormhole://transfer-complete", serde_json::json!({
            "file_id": chunk.file_id,
            "path": download_path.to_string_lossy()
        }));

        eprintln!(
            "[Wormhole] Download complete: {} -> {}",
            chunk.file_id,
            download_path.display()
        );
    }

    Ok(())
}

use rusqlite::OptionalExtension;

// ─── Test Commands (for debugging) ────────────────────────────────

/// Debug command: Test wormhole by creating a test file and offering it
#[tauri::command]
pub async fn wormhole_debug_test(app: AppHandle) -> Result<String, String> {
    use std::io::Write;
    
    eprintln!("[Wormhole Debug] Starting debug test...");
    
    // Create a test file in temp directory
    let temp_dir = std::env::temp_dir();
    let test_file_path = temp_dir.join("wormhole_test_file.txt");
    
    // Write some test content
    let test_content = format!(
        "Wormhole Test File\nCreated at: {}\nThis is a test file for debugging the Copi Wormhole feature.\nRandom data: {}",
        chrono::Utc::now().to_rfc3339(),
        uuid::Uuid::new_v4()
    );
    
    {
        let mut file = std::fs::File::create(&test_file_path).map_err(|e| e.to_string())?;
        file.write_all(test_content.as_bytes()).map_err(|e| e.to_string())?;
    }
    
    eprintln!("[Wormhole Debug] Created test file at: {}", test_file_path.display());
    
    // Now offer the file
    let result = wormhole_offer_file(test_file_path.to_string_lossy().to_string(), app.clone()).await?;
    
    eprintln!("[Wormhole Debug] File offered with ID: {:?}", result);
    
    // List all files
    let files = wormhole_list_files(app.clone()).await?;
    eprintln!("[Wormhole Debug] Current wormhole files: {}", files.len());
    for file in &files {
        eprintln!("  - {} ({}, {} bytes, status: {:?}, local: {})", 
            file.file_name, 
            file.id, 
            file.file_size,
            file.status,
            file.is_local
        );
    }
    
    // Check sync state
    let state = app.state::<AppState>();
    let sync_connected = if let Some(sync) = state.sync.get() {
        sync.connected_peers().await.len()
    } else {
        0
    };
    
    eprintln!("[Wormhole Debug] Connected sync peers: {}", sync_connected);
    
    Ok(format!(
        "Test complete!\nFile offered: {:?}\nFile path: {}\nConnected peers: {}\nTotal wormhole files: {}",
        result,
        test_file_path.display(),
        sync_connected,
        files.len()
    ))
}

/// Debug command: List wormhole files with detailed info (printed to console)
#[tauri::command]
pub async fn wormhole_debug_list(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    
    let files = list_wormhole_files(&conn).map_err(|e| e.to_string())?;
    
    eprintln!("[Wormhole Debug] === File List ({} files) ===", files.len());
    
    let mut output = format!("Total files: {}\n\n", files.len());
    
    for file in &files {
        let file_info = format!(
            "ID: {}\n  Name: {}\n  Size: {} bytes\n  Status: {:?}\n  Local: {}\n  Origin: {} ({})\n  Created: {}\n  Expires: {}\n\n",
            file.id,
            file.file_name,
            file.file_size,
            file.status,
            file.is_local,
            file.origin_device_name.as_deref().unwrap_or("Unknown"),
            file.origin_device_id,
            file.created_at,
            file.expires_at
        );
        eprintln!("{}", file_info);
        output.push_str(&file_info);
    }
    
    Ok(output)
}

/// Debug command: Check sync connection status
#[tauri::command]
pub async fn wormhole_debug_sync_status(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    
    let Some(sync) = state.sync.get() else {
        return Ok("Sync not initialized".to_string());
    };
    
    let connected_peers = sync.connected_peers().await;
    let peer_count = connected_peers.len();
    
    let mut output = format!("Connected peers: {}\n\n", peer_count);
    
    for device_id in &connected_peers {
        output.push_str(&format!("  - {}\n", device_id));
    }
    
    eprintln!("[Wormhole Debug] Sync status:\n{}", output);
    
    Ok(output)
}
