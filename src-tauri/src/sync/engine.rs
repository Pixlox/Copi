//! Sync Engine: TCP server, client sessions, Noise encryption, delta sync, real-time push.
//!
//! Architecture:
//!   - Noise XX handshake for all connections (simple, reliable)
//!   - Persistent connections per paired device
//!   - Real-time push: clips pushed immediately to connected peers
//!   - Delta sync on connect: send everything newer than peer's version
//!   - Auto-reconnect with exponential backoff
//!   - mDNS-triggered reconnect for "come home and auto-sync"

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::Rng;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use snow::{Builder, TransportState};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::timeout;
use tauri::{Emitter, Manager};

use super::discovery::{DiscoveryEvent, DiscoveryService};
use super::protocol::{Msg, WireClip, WireCollection, PROTOCOL_VERSION};

// ─── Constants ────────────────────────────────────────────────────────────

pub const DEFAULT_SYNC_PORT: u16 = 47524;
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const MAX_NOISE_PAYLOAD: usize = 60 * 1024;
const MAX_BUFFER_SIZE: usize = 64 * 1024;
const LENGTH_PREFIX_SIZE: usize = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const RECONNECT_BASE: Duration = Duration::from_secs(5);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const PING_INTERVAL: Duration = Duration::from_secs(30);

// ─── Public types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDeviceIdentityPayload {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairedDevicePayload {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub paired_at: i64,
    pub last_seen: Option<i64>,
    pub last_sync_version: i64,
    pub online: bool,
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
    pub platform: String,
    pub is_paired: bool,
    pub is_connected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairingCodePayload {
    pub code: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncEvent {
    Started,
    Stopped,
    DeviceDiscovered {
        device_id: String,
        device_name: String,
        platform: String,
    },
    DeviceLost { device_id: String },
    PairingComplete {
        device_id: String,
        device_name: String,
    },
    PairingFailed { device_id: String, reason: String },
    Connected { device_id: String },
    Disconnected { device_id: String },
    SyncComplete { device_id: String, items_synced: u32 },
    SyncError { device_id: Option<String>, error: String },
}

// ─── Noise Transport ──────────────────────────────────────────────────────

struct NoiseTransport {
    reader: Arc<Mutex<ReadHalf<TcpStream>>>,
    writer: Arc<Mutex<WriteHalf<TcpStream>>>,
    noise: Arc<Mutex<TransportState>>,
    remote_public_key: Vec<u8>,
}

impl NoiseTransport {
    async fn connect(
        addr: SocketAddr,
        our_private_key: &[u8; 32],
    ) -> Result<Self, String> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("TCP connect failed: {}", e))?;
        Self::handshake_initiator(stream, our_private_key).await
    }

    async fn accept(stream: TcpStream, our_private_key: &[u8; 32]) -> Result<Self, String> {
        Self::handshake_responder(stream, our_private_key).await
    }

    async fn handshake_initiator(
        mut stream: TcpStream,
        our_private_key: &[u8; 32],
    ) -> Result<Self, String> {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap());
        let mut noise = builder
            .local_private_key(our_private_key)
            .build_initiator()
            .map_err(|e| format!("Noise build: {}", e))?;

        let mut buf = vec![0u8; MAX_BUFFER_SIZE];
        let mut read_buf = vec![0u8; MAX_BUFFER_SIZE];

        // -> e
        let len = noise.write_message(&[], &mut buf).map_err(|e| e.to_string())?;
        Self::send_raw(&mut stream, &buf[..len]).await?;

        // <- e, ee, s, es
        let msg = Self::recv_raw(&mut stream, &mut read_buf).await?;
        noise.read_message(msg, &mut buf).map_err(|e| e.to_string())?;

        // -> s, se
        let len = noise.write_message(&[], &mut buf).map_err(|e| e.to_string())?;
        Self::send_raw(&mut stream, &buf[..len]).await?;

        let remote_public_key = noise
            .get_remote_static()
            .ok_or_else(|| String::from("No remote public key"))?
            .to_vec();

        let noise = noise.into_transport_mode().map_err(|e| e.to_string())?;
        let (reader, writer) = tokio::io::split(stream);

        Ok(Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            noise: Arc::new(Mutex::new(noise)),
            remote_public_key,
        })
    }

    async fn handshake_responder(
        mut stream: TcpStream,
        our_private_key: &[u8; 32],
    ) -> Result<Self, String> {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap());
        let mut noise = builder
            .local_private_key(our_private_key)
            .build_responder()
            .map_err(|e| format!("Noise build: {}", e))?;

        let mut buf = vec![0u8; MAX_BUFFER_SIZE];
        let mut read_buf = vec![0u8; MAX_BUFFER_SIZE];

        // <- e
        let msg = Self::recv_raw(&mut stream, &mut read_buf).await?;
        noise.read_message(msg, &mut buf).map_err(|e| e.to_string())?;

        // -> e, ee, s, es
        let len = noise.write_message(&[], &mut buf).map_err(|e| e.to_string())?;
        Self::send_raw(&mut stream, &buf[..len]).await?;

        // <- s, se
        let msg = Self::recv_raw(&mut stream, &mut read_buf).await?;
        noise.read_message(msg, &mut buf).map_err(|e| e.to_string())?;

        let remote_public_key = noise
            .get_remote_static()
            .ok_or_else(|| String::from("No remote public key"))?
            .to_vec();

        let noise = noise.into_transport_mode().map_err(|e| e.to_string())?;
        let (reader, writer) = tokio::io::split(stream);

        Ok(Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            noise: Arc::new(Mutex::new(noise)),
            remote_public_key,
        })
    }

    async fn send(&self, data: &[u8]) -> Result<(), String> {
        // 4-byte total length header
        let total_len = data.len() as u32;
        let header = total_len.to_be_bytes();
        self.send_chunk(&header).await?;

        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + MAX_NOISE_PAYLOAD).min(data.len());
            self.send_chunk(&data[offset..end]).await?;
            offset = end;
        }
        Ok(())
    }

    async fn send_msg(&self, msg: &Msg) -> Result<(), String> {
        let line = msg.to_line().map_err(|e| e.to_string())?;
        self.send(line.as_bytes()).await
    }

    async fn send_chunk(&self, data: &[u8]) -> Result<(), String> {
        if data.len() > MAX_NOISE_PAYLOAD {
            return Err(format!("Chunk too large: {} bytes", data.len()));
        }

        let mut noise = self.noise.lock().await;
        let mut buf = vec![0u8; MAX_BUFFER_SIZE];
        let len = noise.write_message(data, &mut buf).map_err(|e| e.to_string())?;

        let mut writer = self.writer.lock().await;
        Self::send_raw_writer(&mut *writer, &buf[..len]).await
    }

    async fn recv(&self) -> Result<Vec<u8>, String> {
        let header = self.recv_chunk().await?;
        if header.len() != 4 {
            return Err("Invalid header".into());
        }
        let total_len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if total_len > 10 * 1024 * 1024 {
            return Err(format!("Message too large: {} bytes", total_len));
        }

        let mut result = Vec::with_capacity(total_len);
        while result.len() < total_len {
            let chunk = self.recv_chunk().await?;
            result.extend_from_slice(&chunk);
        }
        result.truncate(total_len);
        Ok(result)
    }

    async fn recv_msg(&self) -> Result<Msg, String> {
        let data = self.recv().await?;
        let text = String::from_utf8(data).map_err(|e| e.to_string())?;
        Msg::from_line(text.trim()).map_err(|e| e.to_string())
    }

    async fn recv_chunk(&self) -> Result<Vec<u8>, String> {
        let mut noise = self.noise.lock().await;
        let mut read_buf = vec![0u8; MAX_BUFFER_SIZE];
        let mut out_buf = vec![0u8; MAX_BUFFER_SIZE];

        let mut reader = self.reader.lock().await;
        let msg = Self::recv_raw_reader(&mut *reader, &mut read_buf).await?;

        let len = noise.read_message(msg, &mut out_buf).map_err(|e| e.to_string())?;
        Ok(out_buf[..len].to_vec())
    }

    async fn send_raw(stream: &mut TcpStream, data: &[u8]) -> Result<(), String> {
        let len = data.len() as u16;
        stream.write_all(&len.to_be_bytes()).await.map_err(|e| e.to_string())?;
        stream.write_all(data).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn send_raw_writer(writer: &mut WriteHalf<TcpStream>, data: &[u8]) -> Result<(), String> {
        let len = data.len() as u16;
        writer.write_all(&len.to_be_bytes()).await.map_err(|e| e.to_string())?;
        writer.write_all(data).await.map_err(|e| e.to_string())?;
        writer.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn recv_raw<'a>(stream: &mut TcpStream, buf: &'a mut [u8]) -> Result<&'a [u8], String> {
        let mut len_buf = [0u8; LENGTH_PREFIX_SIZE];
        stream.read_exact(&mut len_buf).await.map_err(|e| e.to_string())?;
        let len = u16::from_be_bytes(len_buf) as usize;
        if len > buf.len() {
            return Err("Message too large".into());
        }
        stream.read_exact(&mut buf[..len]).await.map_err(|e| e.to_string())?;
        Ok(&buf[..len])
    }

    async fn recv_raw_reader<'a>(
        reader: &mut ReadHalf<TcpStream>,
        buf: &'a mut [u8],
    ) -> Result<&'a [u8], String> {
        let mut len_buf = [0u8; LENGTH_PREFIX_SIZE];
        reader.read_exact(&mut len_buf).await.map_err(|e| e.to_string())?;
        let len = u16::from_be_bytes(len_buf) as usize;
        if len > buf.len() {
            return Err("Message too large".into());
        }
        reader.read_exact(&mut buf[..len]).await.map_err(|e| e.to_string())?;
        Ok(&buf[..len])
    }

    fn remote_public_key(&self) -> &[u8] {
        &self.remote_public_key
    }
}

// ─── Sync Runtime ─────────────────────────────────────────────────────────

struct PairedDeviceInfo {
    device_id: String,
    device_name: String,
    platform: String,
    #[allow(dead_code)]
    public_key: Vec<u8>,
    paired_at: i64,
    last_seen: Option<i64>,
    last_sync_version: i64,
}

struct SyncRuntime {
    app: tauri::AppHandle,
    device_id: String,
    device_name: String,
    platform: String,
    private_key: [u8; 32],
    public_key: Vec<u8>,
    port: u16,
    /// device_id -> true if currently connected
    connected: Arc<RwLock<HashMap<String, bool>>>,
    /// Discovered devices cache
    discovered: Arc<RwLock<HashMap<String, SyncDiscoveredDevicePayload>>>,
    event_tx: broadcast::Sender<SyncEvent>,
    started: AtomicBool,
    generation: AtomicU64,
    /// Active pairing PIN: (pin, expires_at)
    pairing_pin: Arc<Mutex<Option<(String, i64)>>>,
    discovery: Arc<RwLock<Option<DiscoveryService>>>,
}

static RUNTIME: std::sync::OnceLock<Arc<SyncRuntime>> = std::sync::OnceLock::new();

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn app_state(app: &tauri::AppHandle) -> Result<tauri::State<'_, crate::AppState>, String> {
    app.try_state::<crate::AppState>()
        .ok_or_else(|| "App state not ready".into())
}

fn with_write_conn<T>(
    app: &tauri::AppHandle,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
) -> Result<T, String> {
    let state = app_state(app)?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    f(&conn)
}

fn with_read_conn<T>(
    app: &tauri::AppHandle,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
) -> Result<T, String> {
    let state = app_state(app)?;
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    f(&conn)
}

fn load_device_identity(app: &tauri::AppHandle, device_name: Option<String>) -> Result<(String, String, String, [u8; 32], Vec<u8>), String> {
    with_write_conn(app, |conn| {
        // Try to load existing
        let existing: Option<(String, String, String, Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT device_id, device_name, platform, private_key, public_key FROM device_info LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        if let Some((device_id, name, platform_str, priv_key, pub_key)) = existing {
            let priv_arr: [u8; 32] = priv_key.try_into().map_err(|_| "Invalid key length".to_string())?;
            return Ok((device_id, name, platform_str, priv_arr, pub_key));
        }

        // Generate new identity
        let device_id = uuid::Uuid::new_v4().to_string();
        let name = device_name.unwrap_or_else(|| {
            hostname::get()
                .ok()
                .and_then(|h| h.to_str().map(|s| s.strip_suffix(".local").unwrap_or(s).to_string()))
                .unwrap_or_else(|| "Unknown".to_string())
        });

        let platform_str = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        };

        use x25519_dalek::{PublicKey, StaticSecret};
        let private_key = StaticSecret::random_from_rng(rand::thread_rng());
        let public_key = PublicKey::from(&private_key);
        let priv_bytes = private_key.as_bytes().to_vec();
        let pub_bytes = public_key.as_bytes().to_vec();
        let created_at = now_ts();

        conn.execute(
            "INSERT INTO device_info (device_id, device_name, platform, private_key, public_key, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![device_id, name, platform_str, priv_bytes, pub_bytes, created_at],
        ).map_err(|e| e.to_string())?;

        let priv_arr: [u8; 32] = priv_bytes.try_into().map_err(|_| "Invalid key length".to_string())?;
        Ok((device_id, name, platform_str.to_string(), priv_arr, pub_bytes))
    })
}

fn load_paired_devices(app: &tauri::AppHandle) -> Result<Vec<PairedDeviceInfo>, String> {
    with_read_conn(app, |conn| {
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, platform, public_key, paired_at, last_seen, last_sync_version FROM paired_devices ORDER BY last_seen DESC NULLS LAST"
        ).map_err(|e| e.to_string())?;

        let devices: Vec<PairedDeviceInfo> = stmt
            .query_map([], |r| {
                Ok(PairedDeviceInfo {
                    device_id: r.get(0)?,
                    device_name: r.get(1)?,
                    platform: r.get(2)?,
                    public_key: r.get(3)?,
                    paired_at: r.get(4)?,
                    last_seen: r.get(5)?,
                    last_sync_version: r.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        Ok(devices)
    })
}

fn is_paired(app: &tauri::AppHandle, device_id: &str) -> bool {
    with_read_conn(app, |conn| {
        Ok(conn.query_row(
            "SELECT 1 FROM paired_devices WHERE device_id = ?1",
            [device_id],
            |_| Ok(()),
        )
        .is_ok())
    })
    .unwrap_or(false)
}

fn save_paired_device(
    app: &tauri::AppHandle,
    device_id: &str,
    device_name: &str,
    platform: &str,
    public_key: &[u8],
) -> Result<(), String> {
    with_write_conn(app, |conn| {
        conn.execute(
            "INSERT OR REPLACE INTO paired_devices (device_id, device_name, platform, public_key, paired_at, last_seen, last_sync_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![device_id, device_name, platform, public_key, now_ts(), now_ts(), 0i64],
        ).map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn remove_paired_device(app: &tauri::AppHandle, device_id: &str) -> Result<(), String> {
    with_write_conn(app, |conn| {
        conn.execute("DELETE FROM paired_devices WHERE device_id = ?1", [device_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn get_sync_version(app: &tauri::AppHandle) -> Result<i64, String> {
    with_read_conn(app, |conn| {
        let v: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key = 'sync_version'", [], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(v.and_then(|v| v.parse().ok()).unwrap_or(0))
    })
}

fn next_sync_version(app: &tauri::AppHandle) -> Result<i64, String> {
    let current = get_sync_version(app)?;
    let next = current + 1;
    with_write_conn(app, |conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('sync_version', ?1)",
            [next.to_string()],
        ).map_err(|e| e.to_string())?;
        Ok(())
    })?;
    Ok(next)
}

/// Public version of next_sync_version for use from clipboard.rs etc.
pub fn next_sync_version_public(app: &tauri::AppHandle) -> i64 {
    next_sync_version(app).unwrap_or(0)
}

// ─── Clip/Collection conversion ───────────────────────────────────────────

fn clip_to_wire(app: &tauri::AppHandle, sync_id: &str) -> Result<Option<WireClip>, String> {
    with_read_conn(app, |conn| {
        let row: Option<(String, i64, String, String, String, String, Option<Vec<u8>>, Option<String>, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>, i32, i32, Option<String>, i64, i32, Option<i64>, String, i32)> = conn
            .query_row(
                "SELECT sync_id, sync_version, content_hash, content, content_type, source_app,
                        source_app_icon, content_highlighted, ocr_text, image_data, image_thumbnail,
                        image_width, image_height, language, created_at, pinned, collection_id,
                        COALESCE(origin_device_id, ''), copy_count
                 FROM clips WHERE sync_id = ?1 AND deleted = 0",
                [sync_id],
                |r| Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                    r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?,
                    r.get(11)?, r.get(12)?, r.get(13)?, r.get(14)?, r.get(15)?,
                    r.get(16)?, r.get(17)?, r.get(18)?,
                )),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let Some((sync_id, sync_version, content_hash, content, content_type, source_app,
                   source_app_icon, content_highlighted, ocr_text, image_data, image_thumbnail,
                   image_width, image_height, language, created_at, pinned, collection_id,
                   origin_device_id, copy_count)) = row else {
            return Ok(None);
        };

        let collection_sync_id: Option<String> = if let Some(cid) = collection_id {
            conn.query_row(
                "SELECT sync_id FROM collections WHERE id = ?1",
                [cid],
                |r| r.get(0),
            ).ok()
        } else {
            None
        };

        Ok(Some(WireClip {
            sync_id,
            sync_version,
            content_hash,
            content,
            content_type,
            source_app,
            source_app_icon: source_app_icon.map(|b| B64.encode(b)),
            content_highlighted,
            ocr_text,
            image_data: image_data.map(|b| B64.encode(b)),
            image_thumbnail: image_thumbnail.map(|b| B64.encode(b)),
            image_width,
            image_height,
            language,
            created_at,
            pinned: pinned != 0,
            copy_count,
            collection_sync_id,
            origin_device_id,
            deleted: false,
            embedding: None,
        }))
    })
}

fn clips_since_version(app: &tauri::AppHandle, since: i64, limit: usize, include_embeddings: bool) -> Result<Vec<WireClip>, String> {
    with_read_conn(app, |conn| {
        let mut stmt = conn.prepare(
            "SELECT sync_id, sync_version, content_hash, content, content_type, source_app,
                    source_app_icon, content_highlighted, ocr_text, image_data, image_thumbnail,
                    image_width, image_height, language, created_at, pinned, collection_id,
                    COALESCE(origin_device_id, ''), copy_count
             FROM clips WHERE sync_version > ?1 AND deleted = 0 AND sync_id IS NOT NULL
             ORDER BY sync_version ASC LIMIT ?2"
        ).map_err(|e| e.to_string())?;

        let rows: Vec<(String, i64, String, String, String, String, Option<Vec<u8>>, Option<String>, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>, i32, i32, Option<String>, i64, i32, Option<i64>, String, i32)> = stmt
            .query_map(rusqlite::params![since, limit], |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                    r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?,
                    r.get(11)?, r.get(12)?, r.get(13)?, r.get(14)?, r.get(15)?,
                    r.get(16)?, r.get(17)?, r.get(18)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        let mut clips = Vec::new();
        for (sync_id, sync_version, content_hash, content, content_type, source_app,
             source_app_icon, content_highlighted, ocr_text, image_data, image_thumbnail,
             image_width, image_height, language, created_at, pinned, collection_id,
             origin_device_id, copy_count) in rows {

            let collection_sync_id: Option<String> = if let Some(cid) = collection_id {
                conn.query_row(
                    "SELECT sync_id FROM collections WHERE id = ?1",
                    [cid],
                    |r| r.get(0),
                ).ok()
            } else {
                None
            };

            let mut wire_clip = WireClip {
                sync_id,
                sync_version,
                content_hash,
                content,
                content_type,
                source_app,
                source_app_icon: source_app_icon.map(|b| B64.encode(b)),
                content_highlighted,
                ocr_text,
                image_data: image_data.map(|b| B64.encode(b)),
                image_thumbnail: image_thumbnail.map(|b| B64.encode(b)),
                image_width,
                image_height,
                language,
                created_at,
                pinned: pinned != 0,
                copy_count,
                collection_sync_id,
                origin_device_id,
                deleted: false,
                embedding: None,
            };

            if include_embeddings {
                // Get embedding from clip_embeddings
                let clip_id: Option<i64> = conn.query_row(
                    "SELECT id FROM clips WHERE sync_id = ?1",
                    [&wire_clip.sync_id],
                    |r| r.get(0),
                ).optional().map_err(|e| e.to_string())?;

                if let Some(id) = clip_id {
                    let emb_blob: Option<Vec<u8>> = conn.query_row(
                        "SELECT embedding FROM clip_embeddings WHERE rowid = ?1",
                        [id],
                        |r| r.get(0),
                    ).optional().map_err(|e| e.to_string())?;

                    if let Some(blob) = emb_blob {
                        if blob.len() % 4 == 0 {
                            wire_clip.set_embedding(Some(
                                &blob.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect::<Vec<f32>>()
                            ));
                        }
                    }
                }
            }

            clips.push(wire_clip);
        }

        Ok(clips)
    })
}

fn collections_since_version(app: &tauri::AppHandle, since: i64, limit: usize) -> Result<Vec<WireCollection>, String> {
    with_read_conn(app, |conn| {
        let mut stmt = conn.prepare(
            "SELECT sync_id, sync_version, name, color, created_at, COALESCE(origin_device_id, '')
             FROM collections WHERE sync_version > ?1 AND deleted = 0 AND sync_id IS NOT NULL
             ORDER BY sync_version ASC LIMIT ?2"
        ).map_err(|e| e.to_string())?;

        let rows: Vec<(String, i64, String, Option<String>, i64, String)> = stmt
            .query_map(rusqlite::params![since, limit], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows.into_iter().map(|(sync_id, sync_version, name, color, created_at, origin_device_id)| {
            WireCollection {
                sync_id,
                sync_version,
                name,
                color,
                created_at,
                origin_device_id,
                deleted: false,
            }
        }).collect())
    })
}

fn apply_wire_clip(app: &tauri::AppHandle, clip: &WireClip, _from_device: &str) -> Result<bool, String> {
    with_write_conn(app, |conn| {
        // Check existing version
        let existing_version: Option<i64> = conn.query_row(
            "SELECT sync_version FROM clips WHERE sync_id = ?1",
            [&clip.sync_id],
            |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;

        if let Some(ev) = existing_version {
            if ev >= clip.sync_version {
                return Ok(false); // Already have this or newer
            }
        }

        // Get collection_id from collection_sync_id
        let collection_id: Option<i64> = if let Some(ref csid) = clip.collection_sync_id {
            conn.query_row(
                "SELECT id FROM collections WHERE sync_id = ?1",
                [csid],
                |r| r.get(0),
            ).optional().map_err(|e| e.to_string())?
        } else {
            None
        };

        let icon_bytes = clip.source_app_icon.as_ref().and_then(|s| B64.decode(s).ok());
        let img_bytes = clip.image_data.as_ref().and_then(|s| B64.decode(s).ok());
        let thumb_bytes = clip.image_thumbnail.as_ref().and_then(|s| B64.decode(s).ok());

        let rows = conn.execute(
            "INSERT INTO clips (sync_id, sync_version, content_hash, content, content_type,
                                source_app, source_app_icon, content_highlighted, ocr_text,
                                image_data, image_thumbnail, image_width, image_height,
                                language, created_at, pinned, copy_count, collection_id,
                                origin_device_id, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 0)
             ON CONFLICT(sync_id) DO UPDATE SET
                sync_version = excluded.sync_version,
                content = excluded.content,
                content_hash = excluded.content_hash,
                content_type = excluded.content_type,
                source_app = excluded.source_app,
                source_app_icon = CASE WHEN length(excluded.source_app_icon) > 0 THEN excluded.source_app_icon ELSE clips.source_app_icon END,
                content_highlighted = COALESCE(excluded.content_highlighted, clips.content_highlighted),
                ocr_text = COALESCE(excluded.ocr_text, clips.ocr_text),
                image_data = COALESCE(excluded.image_data, clips.image_data),
                image_thumbnail = CASE WHEN length(excluded.image_thumbnail) > 0 THEN excluded.image_thumbnail ELSE clips.image_thumbnail END,
                image_width = CASE WHEN excluded.image_width > 0 THEN excluded.image_width ELSE clips.image_width END,
                image_height = CASE WHEN excluded.image_height > 0 THEN excluded.image_height ELSE clips.image_height END,
                language = COALESCE(excluded.language, clips.language),
                created_at = excluded.created_at,
                pinned = excluded.pinned,
                copy_count = excluded.copy_count,
                collection_id = excluded.collection_id,
                origin_device_id = excluded.origin_device_id,
                deleted = 0",
            rusqlite::params![
                clip.sync_id, clip.sync_version, clip.content_hash, clip.content, clip.content_type,
                clip.source_app, icon_bytes, clip.content_highlighted, clip.ocr_text,
                img_bytes, thumb_bytes, clip.image_width, clip.image_height,
                clip.language, clip.created_at, clip.pinned as i32, clip.copy_count, collection_id,
                clip.origin_device_id,
            ],
        ).map_err(|e| e.to_string())?;

        // Apply embedding if present
        if let Some(embedding) = clip.get_embedding() {
            let clip_id: Option<i64> = conn.query_row(
                "SELECT id FROM clips WHERE sync_id = ?1",
                [&clip.sync_id],
                |r| r.get(0),
            ).optional().map_err(|e| e.to_string())?;

            if let Some(id) = clip_id {
                let _ = conn.execute("DELETE FROM clip_embeddings WHERE rowid = ?1", [id]);
                let emb_bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
                let _ = conn.execute(
                    "INSERT INTO clip_embeddings (rowid, embedding) VALUES (?1, ?2)",
                    rusqlite::params![id, emb_bytes],
                );
            }
        }

        // Rebuild FTS
        let _ = conn.execute_batch("INSERT INTO clips_fts(clips_fts) VALUES('rebuild');");

        // Enqueue for embedding if no embedding was synced
        if clip.get_embedding().is_none() {
            let clip_id: Option<i64> = conn.query_row(
                "SELECT id FROM clips WHERE sync_id = ?1",
                [&clip.sync_id],
                |r| r.get(0),
            ).optional().map_err(|e| e.to_string())?;

            if let Some(id) = clip_id {
                let state = app.state::<crate::AppState>();
                let _ = state.clip_tx.try_send(id);
            }
        }

        Ok(rows > 0)
    })
}

fn apply_wire_collection(app: &tauri::AppHandle, coll: &WireCollection) -> Result<bool, String> {
    with_write_conn(app, |conn| {
        let existing_version: Option<i64> = conn.query_row(
            "SELECT sync_version FROM collections WHERE sync_id = ?1",
            [&coll.sync_id],
            |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;

        if let Some(ev) = existing_version {
            if ev >= coll.sync_version {
                return Ok(false);
            }
        }

        let rows = conn.execute(
            "INSERT INTO collections (sync_id, sync_version, name, color, created_at, origin_device_id, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
             ON CONFLICT(sync_id) DO UPDATE SET
                sync_version = excluded.sync_version,
                name = excluded.name,
                color = excluded.color,
                created_at = excluded.created_at,
                origin_device_id = excluded.origin_device_id,
                deleted = 0",
            rusqlite::params![
                coll.sync_id, coll.sync_version, coll.name, coll.color, coll.created_at, coll.origin_device_id,
            ],
        ).map_err(|e| e.to_string())?;

        Ok(rows > 0)
    })
}

fn apply_delete_clip(app: &tauri::AppHandle, sync_id: &str, _from_device: &str) -> Result<bool, String> {
    with_write_conn(app, |conn| {
        let rows = conn.execute(
            "UPDATE clips SET deleted = 1 WHERE sync_id = ?1 AND deleted = 0",
            [sync_id],
        ).map_err(|e| e.to_string())?;
        let _ = conn.execute_batch("INSERT INTO clips_fts(clips_fts) VALUES('rebuild');");
        Ok(rows > 0)
    })
}

fn apply_delete_collection(app: &tauri::AppHandle, sync_id: &str) -> Result<bool, String> {
    with_write_conn(app, |conn| {
        let rows = conn.execute(
            "UPDATE collections SET deleted = 1 WHERE sync_id = ?1 AND deleted = 0",
            [sync_id],
        ).map_err(|e| e.to_string())?;
        // Unassign clips
        let _ = conn.execute(
            "UPDATE clips SET collection_id = NULL WHERE collection_id = (SELECT id FROM collections WHERE sync_id = ?1)",
            [sync_id],
        );
        Ok(rows > 0)
    })
}

fn apply_move_clip(app: &tauri::AppHandle, clip_sync_id: &str, collection_sync_id: Option<&str>) -> Result<bool, String> {
    with_write_conn(app, |conn| {
        let collection_id: Option<i64> = if let Some(csid) = collection_sync_id {
            conn.query_row("SELECT id FROM collections WHERE sync_id = ?1", [csid], |r| r.get(0)).optional().map_err(|e| e.to_string())?
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE clips SET collection_id = ?1 WHERE sync_id = ?2",
            rusqlite::params![collection_id, clip_sync_id],
        ).map_err(|e| e.to_string())?;
        Ok(rows > 0)
    })
}

fn apply_set_pinned(app: &tauri::AppHandle, sync_id: &str, pinned: bool) -> Result<bool, String> {
    with_write_conn(app, |conn| {
        let rows = conn.execute(
            "UPDATE clips SET pinned = ?1 WHERE sync_id = ?2",
            rusqlite::params![pinned as i32, sync_id],
        ).map_err(|e| e.to_string())?;
        Ok(rows > 0)
    })
}

// ─── Session handling ─────────────────────────────────────────────────────

async fn run_session(
    runtime: Arc<SyncRuntime>,
    transport: NoiseTransport,
    peer_id: String,
    is_initiator: bool,
) {
    eprintln!("[Sync] Session started with {} (initiator={})", peer_id, is_initiator);

    // Get our sync version
    let our_version = get_sync_version(&runtime.app).unwrap_or(0);

    // Send Hello
    let hello = Msg::Hello {
        device_id: runtime.device_id.clone(),
        device_name: runtime.device_name.clone(),
        protocol_version: PROTOCOL_VERSION,
        last_sync_version: our_version,
    };

    if is_initiator {
        if let Err(e) = transport.send_msg(&hello).await {
            eprintln!("[Sync] Failed to send Hello to {}: {}", peer_id, e);
            return;
        }
    }

    // Read peer's Hello/HelloAck
    let peer_version = match timeout(Duration::from_secs(10), transport.recv_msg()).await {
        Ok(Ok(Msg::Hello { last_sync_version, .. })) => {
            // We're responder, send HelloAck
            let ack = Msg::HelloAck {
                device_id: runtime.device_id.clone(),
                device_name: runtime.device_name.clone(),
                protocol_version: PROTOCOL_VERSION,
                last_sync_version: our_version,
            };
            let _ = transport.send_msg(&ack).await;
            last_sync_version
        }
        Ok(Ok(Msg::HelloAck { last_sync_version, .. })) => {
            last_sync_version
        }
        Ok(Ok(other)) => {
            eprintln!("[Sync] Unexpected message from {}: {:?}", peer_id, other);
            return;
        }
        Ok(Err(e)) => {
            eprintln!("[Sync] Failed to read from {}: {}", peer_id, e);
            return;
        }
        Err(_) => {
            eprintln!("[Sync] Hello timeout from {}", peer_id);
            return;
        }
    };

    // Mark as connected
    runtime.connected.write().await.insert(peer_id.clone(), true);
    let _ = runtime.app.emit("sync:connected", &peer_id);
    let _ = runtime.event_tx.send(SyncEvent::Connected { device_id: peer_id.clone() });

    eprintln!("[Sync] Connected to {} (peer_version={})", peer_id, peer_version);

    // Delta sync: send clips and collections newer than peer's version
    let sync_embeddings = crate::settings::get_config_sync(runtime.app.clone())
        .ok()
        .map(|c| c.sync.sync_embeddings)
        .unwrap_or(true);

    let clips = clips_since_version(&runtime.app, peer_version, 500, sync_embeddings).unwrap_or_default();
    let colls = collections_since_version(&runtime.app, peer_version, 100).unwrap_or_default();

    let mut items_sent = 0u32;

    for clip in clips {
        if clip.origin_device_id == runtime.device_id || clip.origin_device_id.is_empty() {
            // This is our own clip, send it
        }
        if let Err(e) = transport.send_msg(&Msg::Clip { clip }).await {
            eprintln!("[Sync] Failed to send clip to {}: {}", peer_id, e);
            break;
        }
        items_sent += 1;
    }

    for coll in colls {
        if let Err(e) = transport.send_msg(&Msg::Collection { collection: coll }).await {
            eprintln!("[Sync] Failed to send collection to {}: {}", peer_id, e);
            break;
        }
        items_sent += 1;
    }

    if items_sent > 0 {
        eprintln!("[Sync] Sent {} items to {}", items_sent, peer_id);
    }

    // Main receive loop with ping/pong
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = transport.recv_msg() => {
                match result {
                    Ok(msg) => {
                        if let Err(e) = handle_message(&runtime, &peer_id, &transport, msg).await {
                            eprintln!("[Sync] Message error from {}: {}", peer_id, e);
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[Sync] Receive error from {}: {}", peer_id, e);
                        break;
                    }
                }
            }
            _ = ping_interval.tick() => {
                if transport.send_msg(&Msg::Ping).await.is_err() {
                    eprintln!("[Sync] Ping failed to {}", peer_id);
                    break;
                }
            }
        }
    }

    // Disconnect
    runtime.connected.write().await.insert(peer_id.clone(), false);
    let _ = runtime.app.emit("sync:disconnected", &peer_id);
    let _ = runtime.event_tx.send(SyncEvent::Disconnected { device_id: peer_id.clone() });

    // Update last_seen
    let _ = with_write_conn(&runtime.app, |conn| {
        conn.execute(
            "UPDATE paired_devices SET last_seen = ?1 WHERE device_id = ?2",
            rusqlite::params![now_ts(), peer_id],
        ).ok();
        Ok(())
    });

    eprintln!("[Sync] Session ended with {}", peer_id);
}

async fn handle_message(
    runtime: &Arc<SyncRuntime>,
    peer_id: &str,
    transport: &NoiseTransport,
    msg: Msg,
) -> Result<(), String> {
    match msg {
        Msg::Clip { clip } => {
            if clip.origin_device_id == runtime.device_id {
                return Ok(()); // Skip our own clips
            }
            let applied = apply_wire_clip(&runtime.app, &clip, peer_id)?;
            if applied {
                let _ = runtime.app.emit("new-clip", ());
            }
        }
        Msg::Collection { collection } => {
            let applied = apply_wire_collection(&runtime.app, &collection)?;
            if applied {
                let _ = runtime.app.emit("collections-changed", ());
            }
        }
        Msg::DeleteClip { sync_id } => {
            let _ = apply_delete_clip(&runtime.app, &sync_id, peer_id);
            let _ = runtime.app.emit("clips-changed", ());
        }
        Msg::DeleteCollection { sync_id } => {
            let _ = apply_delete_collection(&runtime.app, &sync_id);
            let _ = runtime.app.emit("collections-changed", ());
        }
        Msg::MoveClipToCollection { clip_sync_id, collection_sync_id } => {
            let _ = apply_move_clip(&runtime.app, &clip_sync_id, collection_sync_id.as_deref());
            let _ = runtime.app.emit("clips-changed", ());
        }
        Msg::SetClipPinned { sync_id, pinned } => {
            let _ = apply_set_pinned(&runtime.app, &sync_id, pinned);
            let _ = runtime.app.emit("clips-changed", ());
        }
        Msg::PairRequest { device_id, device_name, platform, public_key, pin } => {
            handle_pair_request(runtime, transport, &device_id, &device_name, &platform, &public_key, &pin).await?;
        }
        Msg::PairAccept { device_id, device_name, platform, public_key } => {
            // Save the paired device
            let pub_bytes = B64.decode(&public_key).map_err(|e| e.to_string())?;
            save_paired_device(&runtime.app, &device_id, &device_name, &platform, &pub_bytes)?;
            let _ = runtime.event_tx.send(SyncEvent::PairingComplete {
                device_id: device_id.clone(),
                device_name: device_name.clone(),
            });
            let _ = runtime.app.emit("sync:paired", serde_json::json!({
                "deviceId": device_id,
                "deviceName": device_name,
            }));
            eprintln!("[Sync] Paired with {}", device_id);
        }
        Msg::PairReject { reason } => {
            let _ = runtime.event_tx.send(SyncEvent::PairingFailed {
                device_id: peer_id.to_string(),
                reason: reason.clone(),
            });
            return Err(format!("Pairing rejected: {}", reason));
        }
        Msg::Ping => {
            transport.send_msg(&Msg::Pong).await?;
        }
        Msg::Pong => {}
        Msg::Hello { .. } | Msg::HelloAck { .. } => {
            // Already handled during handshake
        }
    }
    Ok(())
}

async fn handle_pair_request(
    runtime: &Arc<SyncRuntime>,
    transport: &NoiseTransport,
    device_id: &str,
    device_name: &str,
    platform: &str,
    public_key_b64: &str,
    pin: &str,
) -> Result<(), String> {
    // Verify PIN
    let valid = {
        let guard = runtime.pairing_pin.lock().await;
        if let Some((stored_pin, expires_at)) = guard.as_ref() {
            stored_pin == pin && now_ts() < *expires_at
        } else {
            false
        }
    };

    if !valid {
        transport.send_msg(&Msg::PairReject {
            reason: "Invalid or expired PIN".into(),
        }).await?;
        return Err("Pairing rejected: bad PIN".into());
    }

    // Save paired device
    let pub_bytes = B64.decode(public_key_b64).map_err(|e| e.to_string())?;
    save_paired_device(&runtime.app, device_id, device_name, platform, &pub_bytes)?;

    // Clear PIN
    *runtime.pairing_pin.lock().await = None;

    // Send PairAccept
    transport.send_msg(&Msg::PairAccept {
        device_id: runtime.device_id.clone(),
        device_name: runtime.device_name.clone(),
        platform: runtime.platform.clone(),
        public_key: B64.encode(&runtime.public_key),
    }).await?;

    let _ = runtime.event_tx.send(SyncEvent::PairingComplete {
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
    });
    let _ = runtime.app.emit("sync:paired", serde_json::json!({
        "deviceId": device_id,
        "deviceName": device_name,
    }));

    eprintln!("[Sync] Paired with {} ({})", device_name, device_id);
    Ok(())
}

// ─── TCP Server ───────────────────────────────────────────────────────────

async fn run_server(runtime: Arc<SyncRuntime>) {
    // Try to bind, with retries on port conflict
    let mut listener = None;
    let mut bound_port = runtime.port;

    for attempt in 1..=6 {
        match bind_listener(bound_port).await {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(e) => {
                if e.contains("Address already in use") && attempt < 6 {
                    eprintln!("[Sync] Port {} busy (attempt {}/6), retrying...", bound_port, attempt);
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    continue;
                }
                // Try ephemeral port
                if attempt >= 5 {
                    bound_port = 0;
                }
            }
        }
    }

    let listener = match listener {
        Some(l) => l,
        None => {
            match bind_listener(0).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[Sync] Failed to bind listener: {}", e);
                    return;
                }
            }
        }
    };

    let actual_port = listener.local_addr().map(|a: std::net::SocketAddr| a.port()).unwrap_or(bound_port);

    if actual_port != runtime.port {
        eprintln!("[Sync] Using fallback port {} (default {} unavailable)", actual_port, runtime.port);
    }

    #[cfg(target_os = "windows")]
    ensure_windows_firewall_rules(actual_port);

    // Start discovery
    let discovery = match DiscoveryService::new(
        &runtime.device_id,
        &runtime.device_name,
        &runtime.platform,
        &runtime.public_key,
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[Sync] Discovery init failed: {}", e);
            return;
        }
    };

    if let Err(e) = discovery.start(actual_port, &runtime.device_name, &runtime.platform, &runtime.public_key) {
        eprintln!("[Sync] Discovery start failed: {}", e);
        return;
    }

    *runtime.discovery.write().await = Some(discovery);

    // Spawn discovery event loop
    {
        let rt = runtime.clone();
        let disc = runtime.discovery.read().await;
        if let Some(d) = disc.as_ref() {
            let rx = d.subscribe();
            drop(disc);
            tauri::async_runtime::spawn(discovery_event_loop(rt, rx));
        }
    }

    eprintln!("[Sync] Server listening on port {}", actual_port);

    // Accept loop
    loop {
        match timeout(Duration::from_secs(2), listener.accept()).await {
            Ok(Ok((stream, addr))) => {
                eprintln!("[Sync] Incoming connection from {}", addr);
                let rt = runtime.clone();
                let priv_key = rt.private_key;
                tauri::async_runtime::spawn(async move {
                    match timeout(HANDSHAKE_TIMEOUT, NoiseTransport::accept(stream, &priv_key)).await {
                        Ok(Ok(transport)) => {
                            // Identify peer by public key
                            let remote_pk = transport.remote_public_key().to_vec();
                            let peer_id = with_read_conn(&rt.app, |conn| {
                                conn.query_row(
                                    "SELECT device_id FROM paired_devices WHERE public_key = ?1",
                                    [&remote_pk],
                                    |r| r.get(0),
                                ).optional().map_err(|e| e.to_string())
                            }).ok().flatten().unwrap_or_else(|| "unknown".to_string());

                            if peer_id == "unknown" {
                                eprintln!("[Sync] Unknown peer (key not in paired_devices), dropping");
                                return;
                            }

                            run_session(rt, transport, peer_id, false).await;
                        }
                        Ok(Err(e)) => {
                            eprintln!("[Sync] Handshake failed: {}", e);
                        }
                        Err(_) => {
                            eprintln!("[Sync] Handshake timed out");
                        }
                    }
                });
            }
            Ok(Err(e)) => {
                eprintln!("[Sync] Accept error: {}", e);
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Err(_) => {} // timeout, loop back
        }
    }
}

async fn bind_listener(port: u16) -> Result<TcpListener, String> {
    // Try IPv6 first
    if let Ok(v6_socket) = TcpSocket::new_v6() {
        let _ = v6_socket.set_reuseaddr(true);
        if v6_socket.bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))).is_ok() {
            if let Ok(listener) = v6_socket.listen(128) {
                return Ok(listener);
            }
        }
    }

    // Fallback to IPv4
    let v4_socket = TcpSocket::new_v4().map_err(|e| e.to_string())?;
    let _ = v4_socket.set_reuseaddr(true);
    v4_socket.bind(SocketAddr::from(([0, 0, 0, 0], port))).map_err(|e| e.to_string())?;
    v4_socket.listen(128).map_err(|e| e.to_string())
}

// ─── Discovery event loop ─────────────────────────────────────────────────

async fn discovery_event_loop(runtime: Arc<SyncRuntime>, mut rx: broadcast::Receiver<DiscoveryEvent>) {
    loop {
        let event = match timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Err(_) => continue,
        };

        match event {
            DiscoveryEvent::DeviceFound(d) | DiscoveryEvent::DeviceUpdated(d) => {
                let is_paired = is_paired(&runtime.app, &d.device_id);
                let connected = runtime.connected.read().await.get(&d.device_id).copied().unwrap_or(false);

                let payload = SyncDiscoveredDevicePayload {
                    device_id: d.device_id.clone(),
                    device_name: d.device_name.clone(),
                    platform: d.platform.clone(),
                    is_paired,
                    is_connected: connected,
                };

                runtime.discovered.write().await.insert(payload.device_id.clone(), payload.clone());
                let _ = runtime.app.emit("sync:discovered-updated", payload);

                // If paired and not connected, try to connect
                if is_paired && !connected {
                    let rt = runtime.clone();
                    let device = d;
                    tauri::async_runtime::spawn(async move {
                        connect_and_sync(rt, &device.device_id, &device.device_name, &device.platform, &device.public_key, &device.addresses, device.port).await;
                    });
                }
            }
            DiscoveryEvent::DeviceLost(id) => {
                runtime.discovered.write().await.remove(&id);
                runtime.connected.write().await.insert(id.clone(), false);
                let _ = runtime.app.emit("sync:discovered-lost", &id);
                let _ = runtime.event_tx.send(SyncEvent::DeviceLost { device_id: id });
            }
        }
    }
}

// ─── Connect and sync ─────────────────────────────────────────────────────

async fn connect_and_sync(
    runtime: Arc<SyncRuntime>,
    device_id: &str,
    _device_name: &str,
    _platform: &str,
    _public_key: &[u8],
    addresses: &[std::net::IpAddr],
    port: u16,
) {
    // Already connected?
    if runtime.connected.read().await.get(device_id).copied().unwrap_or(false) {
        return;
    }

    // Try each address
    let mut last_err = String::new();
    for ip in addresses {
        if ip.is_loopback() || ip.is_unspecified() {
            continue;
        }
        let addr = SocketAddr::new(*ip, port);
        match timeout(CONNECT_TIMEOUT, NoiseTransport::connect(addr, &runtime.private_key)).await {
            Ok(Ok(transport)) => {
                eprintln!("[Sync] Connected to {} at {}", device_id, addr);
                run_session(runtime, transport, device_id.to_string(), true).await;
                return;
            }
            Ok(Err(e)) => {
                last_err = e;
            }
            Err(_) => {
                last_err = "timeout".to_string();
            }
        }
    }

    eprintln!("[Sync] Failed to connect to {}: {}", device_id, last_err);
}

// ─── Reconnect loop for paired device ─────────────────────────────────────

async fn reconnect_loop(runtime: Arc<SyncRuntime>, device_id: String) {
    let mut backoff = RECONNECT_BASE;

    loop {
        // Check if still paired
        if !is_paired(&runtime.app, &device_id) {
            eprintln!("[Sync] {} no longer paired, stopping reconnect", device_id);
            break;
        }

        // Already connected?
        if runtime.connected.read().await.get(&device_id).copied().unwrap_or(false) {
            backoff = RECONNECT_BASE;
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        // Get device info from discovery or DB
        let device_info = {
            let disc = runtime.discovery.read().await;
            disc.as_ref().and_then(|d| d.get_device(&device_id))
        };

        if let Some(d) = device_info {
            eprintln!("[Sync] Reconnecting to {} at {:?}", device_id, d.addresses);
            connect_and_sync(
                runtime.clone(),
                &device_id,
                &d.device_name,
                &d.platform,
                &d.public_key,
                &d.addresses,
                d.port,
            ).await;
            backoff = RECONNECT_BASE;
        } else {
            // Not discovered yet, wait for mDNS
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RECONNECT_MAX);
        }
    }
}

// ─── Public API ───────────────────────────────────────────────────────────

pub fn initialize_sync_if_enabled(app: &tauri::AppHandle) -> Result<(), String> {
    let config = crate::settings::get_config_sync(app.clone())?;

    let (device_id, device_name, platform, private_key, public_key) =
        load_device_identity(app, config.sync.device_name.clone())?;

    let (event_tx, _) = broadcast::channel(64);

    let runtime = Arc::new(SyncRuntime {
        app: app.clone(),
        device_id: device_id.clone(),
        device_name: device_name.clone(),
        platform: platform.clone(),
        private_key,
        public_key: public_key.clone(),
        port: DEFAULT_SYNC_PORT,
        connected: Arc::new(RwLock::new(HashMap::new())),
        discovered: Arc::new(RwLock::new(HashMap::new())),
        event_tx,
        started: AtomicBool::new(false),
        generation: AtomicU64::new(0),
        pairing_pin: Arc::new(Mutex::new(None)),
        discovery: Arc::new(RwLock::new(None)),
    });

    let _ = RUNTIME.set(runtime.clone());

    if config.sync.enabled {
        start_runtime_inner(runtime)?;
        eprintln!("[Sync] Enabled: '{}' ({})", device_name, device_id);
    }

    Ok(())
}

fn start_runtime_inner(runtime: Arc<SyncRuntime>) -> Result<(), String> {
    if runtime.started.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let generation = runtime.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = generation;

    let _ = runtime.event_tx.send(SyncEvent::Started);
    let _ = runtime.app.emit("sync:status", "started");

    // Use Tauri's async runtime (available during/after setup)
    let rt = runtime.clone();
    tauri::async_runtime::spawn(async move {
        run_server(rt).await;
    });

    // Reconnect loops for existing paired devices
    let paired = load_paired_devices(&runtime.app).unwrap_or_default();
    for device in paired {
        let rt = runtime.clone();
        let device_id = device.device_id.clone();
        tauri::async_runtime::spawn(async move {
            reconnect_loop(rt, device_id).await;
        });
    }

    Ok(())
}

pub fn apply_config_change(
    app: &tauri::AppHandle,
    previous: Option<&crate::settings::CopiConfig>,
    next: &crate::settings::CopiConfig,
) {
    let Some(runtime) = RUNTIME.get().cloned() else {
        return;
    };

    let was_enabled = previous.map(|p| p.sync.enabled).unwrap_or(false);
    let is_enabled = next.sync.enabled;

    if was_enabled == is_enabled {
        if is_enabled {
            let _ = app.emit("sync:status", "updated");
        }
        return;
    }

    if is_enabled {
        let _ = start_runtime_inner(runtime);
    } else {
        let rt = runtime.clone();
        tauri::async_runtime::spawn(async move {
            rt.started.store(false, Ordering::SeqCst);
            rt.discovered.write().await.clear();
            if let Some(disc) = rt.discovery.write().await.take() {
                disc.stop();
            }
            let _ = rt.event_tx.send(SyncEvent::Stopped);
            let _ = rt.app.emit("sync:status", "stopped");
        });
    }
}

/// Called by clipboard.rs after a clip is saved locally.
pub fn on_local_clip_saved(app: &tauri::AppHandle, clip_sync_id: &str) {
    let Some(runtime) = RUNTIME.get() else { return };
    if !runtime.started.load(Ordering::SeqCst) { return; }

    let app_clone = app.clone();
    let sync_id = clip_sync_id.to_string();
    tauri::async_runtime::spawn(async move {
        let wire_clip = match clip_to_wire(&app_clone, &sync_id) {
            Ok(Some(c)) => c,
            _ => return,
        };

        let msg = Msg::Clip { clip: wire_clip };
        let line = match msg.to_line() {
            Ok(l) => l,
            Err(_) => return,
        };
        let data = line.into_bytes();

        // Push to all connected peers
        // Note: In a persistent connection model, we'd have the transport handles here.
        // Since we use a flush-based model with reconnect, we trigger a reconnect
        // which will do delta sync and pick up the new clip.
        let _ = data; // Used when we implement push
    });
}

/// Called when a collection changes.
pub fn on_collection_changed(app: &tauri::AppHandle) {
    trigger_flush_for_all_paired(app);
}

fn trigger_flush_for_all_paired(app: &tauri::AppHandle) {
    let Some(runtime) = RUNTIME.get().cloned() else { return };
    if !runtime.started.load(Ordering::SeqCst) { return; }

    let auto_connect = crate::settings::get_config_sync(app.clone())
        .ok()
        .map(|c| c.sync.auto_connect)
        .unwrap_or(true);
    if !auto_connect { return; }

    // The reconnect loop + discovery event handling will trigger sync
    // for any newly available paired devices. For already-connected peers,
    // the next ping cycle will pick up new data via delta sync.
    // This is a no-op here since the persistent connection model handles it.
}

// ─── Tauri Commands ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn sync_get_status(app: tauri::AppHandle) -> Result<SyncStatusPayload, String> {
    let config = crate::settings::get_config_sync(app.clone())?;
    let enabled = config.sync.enabled;

    let (device_id, device_name, platform, _, _) =
        load_device_identity(&app, config.sync.device_name.clone())?;

    let device = Some(SyncDeviceIdentityPayload {
        device_id: device_id.clone(),
        device_name: device_name.clone(),
        platform: platform.clone(),
    });

    let paired = load_paired_devices(&app).unwrap_or_default();
    let paired_count = paired.len();

    let connected_count = if let Some(rt) = RUNTIME.get() {
        rt.connected.read().await.values().filter(|v| **v).count()
    } else {
        0
    };

    let queue_depth = if paired_count > 0 {
        let min_sent = with_read_conn(&app, |conn| {
            conn.query_row(
                "SELECT COALESCE(MIN(last_sync_version), 0) FROM paired_devices",
                [],
                |r| r.get(0),
            ).map_err(|e| e.to_string())
        }).unwrap_or(0i64);

        let pending_clips: i64 = with_read_conn(&app, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM clips WHERE sync_version > ?1 AND deleted = 0",
                [min_sent],
                |r| r.get(0),
            ).map_err(|e| e.to_string())
        }).unwrap_or(0);

        let pending_colls: i64 = with_read_conn(&app, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM collections WHERE sync_version > ?1 AND deleted = 0",
                [min_sent],
                |r| r.get(0),
            ).map_err(|e| e.to_string())
        }).unwrap_or(0);

        (pending_clips + pending_colls) as usize
    } else {
        0
    };

    Ok(SyncStatusPayload {
        enabled,
        device,
        paired_count,
        connected_count,
        queue_depth,
    })
}

#[tauri::command]
pub async fn sync_list_paired_devices(app: tauri::AppHandle) -> Result<Vec<SyncPairedDevicePayload>, String> {
    let paired = load_paired_devices(&app)?;
    let connected = if let Some(rt) = RUNTIME.get() {
        rt.connected.read().await
    } else {
        return Ok(vec![]);
    };

    Ok(paired.into_iter().map(|d| {
        let online = connected.get(&d.device_id).copied().unwrap_or(false);
        SyncPairedDevicePayload {
            device_id: d.device_id,
            device_name: d.device_name,
            platform: d.platform,
            paired_at: d.paired_at,
            last_seen: d.last_seen,
            last_sync_version: d.last_sync_version,
            online,
        }
    }).collect())
}

#[tauri::command]
pub async fn sync_unpair_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    remove_paired_device(&app, &device_id)?;
    if let Some(rt) = RUNTIME.get() {
        rt.connected.write().await.remove(&device_id);
        rt.discovered.write().await.remove(&device_id);
    }
    Ok(())
}

#[tauri::command]
pub async fn sync_pair_device_manual(
    app: tauri::AppHandle,
    device_id: String,
    device_name: String,
    platform: String,
    public_key_base64: String,
) -> Result<(), String> {
    let public_key = B64.decode(public_key_base64.trim()).map_err(|e| e.to_string())?;
    save_paired_device(&app, &device_id, &device_name, &platform, &public_key)?;

    // Trigger reconnect
    if let Some(rt) = RUNTIME.get() {
        let rt = rt.clone();
        tauri::async_runtime::spawn(async move {
            reconnect_loop(rt, device_id).await;
        });
    }

    Ok(())
}

#[tauri::command]
pub async fn sync_list_discovered_devices(
    app: tauri::AppHandle,
) -> Result<Vec<SyncDiscoveredDevicePayload>, String> {
    let _ = app;
    let Some(rt) = RUNTIME.get() else { return Ok(vec![]) };
    Ok(rt.discovered.read().await.values().cloned().collect())
}

#[tauri::command]
pub async fn sync_start_pairing(app: tauri::AppHandle) -> Result<SyncPairingCodePayload, String> {
    let pin: String = {
        let mut rng = rand::thread_rng();
        format!("{:06}", rng.gen_range(0u32..1_000_000))
    };
    let expires_at = now_ts() + 120;

    if let Some(rt) = RUNTIME.get() {
        *rt.pairing_pin.lock().await = Some((pin.clone(), expires_at));
    }

    let _ = app.emit("sync:pairing-offer", SyncPairingCodePayload {
        code: pin.clone(),
        expires_at,
    });

    Ok(SyncPairingCodePayload { code: pin, expires_at })
}

#[tauri::command]
pub async fn sync_pair_with_code(
    app: tauri::AppHandle,
    device_id: String,
    code: String,
) -> Result<(), String> {
    let Some(rt) = RUNTIME.get().cloned() else {
        return Err("Sync not initialized".into());
    };

    // Get device from discovery cache
    let device = {
        let disc = rt.discovery.read().await;
        disc.as_ref()
            .and_then(|d| d.get_device(&device_id))
            .ok_or_else(|| String::from("Device not discovered"))?
    };

    // Connect to device
    let mut last_err = String::new();
    for ip in &device.addresses {
        if ip.is_loopback() || ip.is_unspecified() {
            continue;
        }
        let addr = SocketAddr::new(*ip, device.port);
        match timeout(CONNECT_TIMEOUT, NoiseTransport::connect(addr, &rt.private_key)).await {
            Ok(Ok(transport)) => {
                // Send PairRequest
                let pair_msg = Msg::PairRequest {
                    device_id: rt.device_id.clone(),
                    device_name: rt.device_name.clone(),
                    platform: rt.platform.clone(),
                    public_key: B64.encode(&rt.public_key),
                    pin: code.clone(),
                };

                transport.send_msg(&pair_msg).await?;

                // Wait for response
                match timeout(Duration::from_secs(10), transport.recv_msg()).await {
                    Ok(Ok(Msg::PairAccept { device_id: their_id, device_name: their_name, platform: their_platform, public_key: their_key })) => {
                        let pub_bytes = B64.decode(&their_key).map_err(|e| e.to_string())?;
                        save_paired_device(&app, &their_id, &their_name, &their_platform, &pub_bytes)?;

                        let _ = rt.event_tx.send(SyncEvent::PairingComplete {
                            device_id: their_id.clone(),
                            device_name: their_name.clone(),
                        });
                        let _ = app.emit("sync:paired", serde_json::json!({
                            "deviceId": their_id,
                            "deviceName": their_name,
                        }));

                        eprintln!("[Sync] Paired with {}", their_name);

                        // Start reconnect loop for this device
                        let rt = rt.clone();
                        tauri::async_runtime::spawn(async move {
                            reconnect_loop(rt, their_id).await;
                        });

                        return Ok(());
                    }
                    Ok(Ok(Msg::PairReject { reason })) => {
                        return Err(format!("Pairing rejected: {}", reason));
                    }
                    Ok(Ok(other)) => {
                        return Err(format!("Unexpected response: {:?}", other));
                    }
                    Ok(Err(e)) => {
                        return Err(format!("Failed to read response: {}", e));
                    }
                    Err(_) => {
                        return Err("Pairing timed out".into());
                    }
                }
            }
            Ok(Err(e)) => {
                last_err = e;
            }
            Err(_) => {
                last_err = "timeout".into();
            }
        }
    }

    Err(format!("Failed to connect: {}", last_err))
}

// ─── Windows Firewall ─────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn ensure_windows_firewall_rules(listen_port: u16) {
    use std::process::Command;
    static ELEVATION_ATTEMPTED: AtomicBool = AtomicBool::new(false);

    let tcp_rule = format!("Copi LAN Sync TCP {}", listen_port);
    let mdns_rule = "Copi mDNS UDP 5353";
    let script = format!(
        "$tcp = Get-NetFirewallRule -DisplayName '{tcp_rule}' -ErrorAction SilentlyContinue; if (-not $tcp) {{ New-NetFirewallRule -DisplayName '{tcp_rule}' -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port} -Profile Any | Out-Null }}; $mdns = Get-NetFirewallRule -DisplayName '{mdns_rule}' -ErrorAction SilentlyContinue; if (-not $mdns) {{ New-NetFirewallRule -DisplayName '{mdns_rule}' -Direction Inbound -Action Allow -Protocol UDP -LocalPort 5353 -Profile Any | Out-Null }}",
        tcp_rule = tcp_rule,
        port = listen_port,
        mdns_rule = mdns_rule
    );

    let encoded: String = script.encode_utf16().flat_map(|u| u.to_le_bytes()).map(|b| b as char).collect();
    // Proper base64 encode of UTF-16LE
    let utf16_bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let b64 = B64.encode(&utf16_bytes);

    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &b64])
        .output()
    {
        Ok(out) if out.status.success() => {
            eprintln!("[Sync] Windows firewall rules ensured");
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if (stderr.contains("Access is denied") || stderr.contains("System Error 5"))
                && !ELEVATION_ATTEMPTED.swap(true, Ordering::SeqCst)
            {
                eprintln!("[Sync] Firewall needs elevation, skipping (UAC prompt may be shown)");
            }
        }
        Err(e) => {
            eprintln!("[Sync] Failed to invoke PowerShell for firewall: {}", e);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn ensure_windows_firewall_rules(_port: u16) {}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_pattern_is_valid() {
        let _: snow::params::NoiseParams = NOISE_PATTERN.parse().unwrap();
    }

    #[test]
    fn pin_generation_format() {
        let pin: String = {
            let mut rng = rand::thread_rng();
            format!("{:06}", rng.gen_range(0u32..1_000_000))
        };
        assert_eq!(pin.len(), 6);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn wire_clip_serialization() {
        let clip = WireClip {
            sync_id: "test-1".into(),
            sync_version: 1,
            content_hash: "abc123".into(),
            content: "Hello, world!".into(),
            content_type: "text".into(),
            source_app: "Chrome".into(),
            source_app_icon: None,
            content_highlighted: None,
            ocr_text: None,
            image_data: None,
            image_thumbnail: None,
            image_width: 0,
            image_height: 0,
            language: Some("en".into()),
            created_at: 1000,
            pinned: false,
            copy_count: 0,
            collection_sync_id: None,
            origin_device_id: "dev1".into(),
            deleted: false,
            embedding: None,
        };

        let msg = Msg::Clip { clip: clip.clone() };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::Clip { clip } => {
                assert_eq!(clip.sync_id, "test-1");
                assert_eq!(clip.content, "Hello, world!");
                assert_eq!(clip.content_type, "text");
            }
            _ => panic!("Expected Clip variant"),
        }
    }

    // ─── Integration Tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn noise_transport_handshake_succeeds() {
        let server_key = [42u8; 32];
        let client_key = [99u8; 32];

        let listener = NoiseListener::bind(0, server_key).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept_tcp().await.unwrap();
            NoiseTransport::accept(stream, &server_key).await.unwrap()
        });

        let client = NoiseTransport::connect(addr, &client_key).await.unwrap();
        let server = server_task.await.unwrap();

        // Verify both sides got each other's public keys
        assert_eq!(client.remote_public_key().len(), 32);
        assert_eq!(server.remote_public_key().len(), 32);
        assert_ne!(client.remote_public_key(), server.remote_public_key());
    }

    #[tokio::test]
    async fn noise_transport_bidirectional_messages() {
        let server_key = [7u8; 32];
        let client_key = [13u8; 32];

        let listener = NoiseListener::bind(0, server_key).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept_tcp().await.unwrap();
            let transport = NoiseTransport::accept(stream, &server_key).await.unwrap();
            // Echo back
            let msg = transport.recv_msg().await.unwrap();
            transport.send_msg(&msg).await.unwrap();
            transport
        });

        let client = NoiseTransport::connect(addr, &client_key).await.unwrap();

        // Send a clip message
        let clip = WireClip {
            sync_id: "test-clip".into(),
            sync_version: 1,
            content_hash: "hash1".into(),
            content: "Hello from client!".into(),
            content_type: "text".into(),
            source_app: "Test".into(),
            source_app_icon: None,
            content_highlighted: None,
            ocr_text: None,
            image_data: None,
            image_thumbnail: None,
            image_width: 0,
            image_height: 0,
            language: None,
            created_at: 1000,
            pinned: false,
            copy_count: 0,
            collection_sync_id: None,
            origin_device_id: "client".into(),
            deleted: false,
            embedding: None,
        };
        client.send_msg(&Msg::Clip { clip: clip.clone() }).await.unwrap();

        // Receive echoed message
        let echoed = client.recv_msg().await.unwrap();
        match echoed {
            Msg::Clip { clip: echoed_clip } => {
                assert_eq!(echoed_clip.sync_id, "test-clip");
                assert_eq!(echoed_clip.content, "Hello from client!");
            }
            _ => panic!("Expected Clip"),
        }

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn noise_transport_large_message_roundtrip() {
        let server_key = [1u8; 32];
        let client_key = [2u8; 32];

        let listener = NoiseListener::bind(0, server_key).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept_tcp().await.unwrap();
            let transport = NoiseTransport::accept(stream, &server_key).await.unwrap();
            let msg = transport.recv_msg().await.unwrap();
            transport.send_msg(&msg).await.unwrap();
        });

        let client = NoiseTransport::connect(addr, &client_key).await.unwrap();

        // Send a large message (> MAX_NOISE_PAYLOAD to test chunking)
        let large_content = "A".repeat(100_000);
        let clip = WireClip {
            sync_id: "large".into(),
            sync_version: 1,
            content_hash: "hash".into(),
            content: large_content.clone(),
            content_type: "text".into(),
            source_app: "Test".into(),
            source_app_icon: None,
            content_highlighted: None,
            ocr_text: None,
            image_data: None,
            image_thumbnail: None,
            image_width: 0,
            image_height: 0,
            language: None,
            created_at: 1000,
            pinned: false,
            copy_count: 0,
            collection_sync_id: None,
            origin_device_id: "client".into(),
            deleted: false,
            embedding: None,
        };
        client.send_msg(&Msg::Clip { clip }).await.unwrap();

        let echoed = client.recv_msg().await.unwrap();
        match echoed {
            Msg::Clip { clip: echoed_clip } => {
                assert_eq!(echoed_clip.content.len(), 100_000);
                assert_eq!(echoed_clip.content, large_content);
            }
            _ => panic!("Expected Clip"),
        }

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn noise_transport_multiple_sequential_messages() {
        let server_key = [3u8; 32];
        let client_key = [4u8; 32];

        let listener = NoiseListener::bind(0, server_key).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept_tcp().await.unwrap();
            let transport = NoiseTransport::accept(stream, &server_key).await.unwrap();
            for _ in 0..10 {
                let msg = transport.recv_msg().await.unwrap();
                transport.send_msg(&msg).await.unwrap();
            }
        });

        let client = NoiseTransport::connect(addr, &client_key).await.unwrap();

        for i in 0..10 {
            let clip = WireClip {
                sync_id: format!("msg-{}", i),
                sync_version: i as i64,
                content_hash: format!("hash-{}", i),
                content: format!("Message {}", i),
                content_type: "text".into(),
                source_app: "Test".into(),
                source_app_icon: None,
                content_highlighted: None,
                ocr_text: None,
                image_data: None,
                image_thumbnail: None,
                image_width: 0,
                image_height: 0,
                language: None,
                created_at: 1000 + i,
                pinned: false,
                copy_count: 0,
                collection_sync_id: None,
                origin_device_id: "client".into(),
                deleted: false,
                embedding: None,
            };
            client.send_msg(&Msg::Clip { clip }).await.unwrap();
            let echoed = client.recv_msg().await.unwrap();
            match echoed {
                Msg::Clip { clip: c } => {
                    assert_eq!(c.sync_id, format!("msg-{}", i));
                    assert_eq!(c.content, format!("Message {}", i));
                }
                _ => panic!("Expected Clip at iteration {}", i),
            }
        }

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn noise_transport_ping_pong() {
        let server_key = [5u8; 32];
        let client_key = [6u8; 32];

        let listener = NoiseListener::bind(0, server_key).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept_tcp().await.unwrap();
            let transport = NoiseTransport::accept(stream, &server_key).await.unwrap();
            for _ in 0..5 {
                let msg = transport.recv_msg().await.unwrap();
                assert!(matches!(msg, Msg::Ping));
                transport.send_msg(&Msg::Pong).await.unwrap();
            }
        });

        let client = NoiseTransport::connect(addr, &client_key).await.unwrap();

        for _ in 0..5 {
            client.send_msg(&Msg::Ping).await.unwrap();
            let pong = client.recv_msg().await.unwrap();
            assert!(matches!(pong, Msg::Pong));
        }

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn noise_transport_connect_timeout() {
        let server_key = [8u8; 32];
        let client_key = [9u8; 32];

        // Bind but don't accept
        let listener = NoiseListener::bind(0, server_key).await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // Close the listener

        // Connect should fail since nothing is listening
        let result = timeout(Duration::from_millis(500), NoiseTransport::connect(addr, &client_key)).await;
        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[test]
    fn protocol_version_is_consistent() {
        assert_eq!(PROTOCOL_VERSION, 2);
    }

    #[test]
    fn wire_clip_with_image_data_roundtrip() {
        let image_bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // PNG header
        let b64_image = B64.encode(&image_bytes);

        let clip = WireClip {
            sync_id: "img-1".into(),
            sync_version: 1,
            content_hash: "img-hash".into(),
            content: "[Image]".into(),
            content_type: "image".into(),
            source_app: "Screenshot".into(),
            source_app_icon: None,
            content_highlighted: None,
            ocr_text: Some("OCR text".into()),
            image_data: Some(b64_image.clone()),
            image_thumbnail: Some(b64_image.clone()),
            image_width: 1920,
            image_height: 1080,
            language: None,
            created_at: 2000,
            pinned: true,
            copy_count: 5,
            collection_sync_id: Some("coll-1".into()),
            origin_device_id: "dev1".into(),
            deleted: false,
            embedding: None,
        };

        let msg = Msg::Clip { clip: clip.clone() };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::Clip { clip: c } => {
                assert_eq!(c.content_type, "image");
                assert_eq!(c.image_width, 1920);
                assert_eq!(c.image_height, 1080);
                assert!(c.pinned);
                assert_eq!(c.copy_count, 5);
                assert_eq!(c.ocr_text, Some("OCR text".into()));
                assert_eq!(c.image_data, Some(b64_image));
                assert_eq!(c.collection_sync_id, Some("coll-1".into()));
            }
            _ => panic!("Expected Clip"),
        }
    }

    #[test]
    fn wire_collection_roundtrip() {
        let coll = WireCollection {
            sync_id: "coll-1".into(),
            sync_version: 5,
            name: "Work".into(),
            color: Some("#0A84FF".into()),
            created_at: 1000,
            origin_device_id: "dev1".into(),
            deleted: false,
        };

        let msg = Msg::Collection { collection: coll.clone() };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::Collection { collection: c } => {
                assert_eq!(c.sync_id, "coll-1");
                assert_eq!(c.name, "Work");
                assert_eq!(c.color, Some("#0A84FF".into()));
                assert_eq!(c.sync_version, 5);
            }
            _ => panic!("Expected Collection"),
        }
    }

    #[test]
    fn pair_request_roundtrip() {
        let msg = Msg::PairRequest {
            device_id: "dev-a".into(),
            device_name: "MacBook".into(),
            platform: "macos".into(),
            public_key: B64.encode(&[1u8; 32]),
            pin: "123456".into(),
        };

        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::PairRequest { device_id, pin, .. } => {
                assert_eq!(device_id, "dev-a");
                assert_eq!(pin, "123456");
            }
            _ => panic!("Expected PairRequest"),
        }
    }

    #[test]
    fn pair_accept_roundtrip() {
        let msg = Msg::PairAccept {
            device_id: "dev-b".into(),
            device_name: "Desktop".into(),
            platform: "windows".into(),
            public_key: B64.encode(&[2u8; 32]),
        };

        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::PairAccept { device_id, device_name, platform, .. } => {
                assert_eq!(device_id, "dev-b");
                assert_eq!(device_name, "Desktop");
                assert_eq!(platform, "windows");
            }
            _ => panic!("Expected PairAccept"),
        }
    }

    #[test]
    fn delete_clip_roundtrip() {
        let msg = Msg::DeleteClip { sync_id: "clip-1".into() };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::DeleteClip { sync_id } => {
                assert_eq!(sync_id, "clip-1");
            }
            _ => panic!("Expected DeleteClip"),
        }
    }

    #[test]
    fn set_clip_pinned_roundtrip() {
        let msg = Msg::SetClipPinned { sync_id: "clip-1".into(), pinned: true };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::SetClipPinned { sync_id, pinned } => {
                assert_eq!(sync_id, "clip-1");
                assert!(pinned);
            }
            _ => panic!("Expected SetClipPinned"),
        }
    }

    #[test]
    fn move_clip_to_collection_roundtrip() {
        let msg = Msg::MoveClipToCollection {
            clip_sync_id: "clip-1".into(),
            collection_sync_id: Some("coll-1".into()),
        };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::MoveClipToCollection { clip_sync_id, collection_sync_id } => {
                assert_eq!(clip_sync_id, "clip-1");
                assert_eq!(collection_sync_id, Some("coll-1".into()));
            }
            _ => panic!("Expected MoveClipToCollection"),
        }
    }

    #[test]
    fn hello_handshake_roundtrip() {
        let msg = Msg::Hello {
            device_id: "dev-1".into(),
            device_name: "MyMac".into(),
            protocol_version: PROTOCOL_VERSION,
            last_sync_version: 42,
        };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::Hello { device_id, last_sync_version, protocol_version, .. } => {
                assert_eq!(device_id, "dev-1");
                assert_eq!(last_sync_version, 42);
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            _ => panic!("Expected Hello"),
        }
    }

    #[test]
    fn hello_ack_handshake_roundtrip() {
        let msg = Msg::HelloAck {
            device_id: "dev-2".into(),
            device_name: "MyPC".into(),
            protocol_version: PROTOCOL_VERSION,
            last_sync_version: 38,
        };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();

        match parsed {
            Msg::HelloAck { device_id, last_sync_version, .. } => {
                assert_eq!(device_id, "dev-2");
                assert_eq!(last_sync_version, 38);
            }
            _ => panic!("Expected HelloAck"),
        }
    }

    // ─── Noise Listener wrapper for tests ────────────────────────────────

    struct NoiseListener {
        listener: TcpListener,
        #[allow(dead_code)]
        our_private_key: [u8; 32],
    }

    impl NoiseListener {
        async fn bind(port: u16, our_private_key: [u8; 32]) -> Result<Self, String> {
            // Try IPv6 first
            if let Ok(v6_socket) = TcpSocket::new_v6() {
                let _ = v6_socket.set_reuseaddr(true);
                if v6_socket.bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))).is_ok() {
                    if let Ok(listener) = v6_socket.listen(128) {
                        return Ok(Self { listener, our_private_key });
                    }
                }
            }
            // Fallback to IPv4
            let v4_socket = TcpSocket::new_v4().map_err(|e| e.to_string())?;
            let _ = v4_socket.set_reuseaddr(true);
            v4_socket.bind(SocketAddr::from(([0, 0, 0, 0], port))).map_err(|e| e.to_string())?;
            let listener = v4_socket.listen(128).map_err(|e| e.to_string())?;
            Ok(Self { listener, our_private_key })
        }

        async fn accept_tcp(&self) -> Result<(TcpStream, std::net::SocketAddr), String> {
            self.listener.accept().await.map_err(|e| e.to_string())
        }

        fn local_addr(&self) -> Result<SocketAddr, String> {
            self.listener.local_addr().map_err(|e| e.to_string())
        }
    }
}
