use super::device::{DeviceIdentity, PairedDevice, Platform};
use super::discovery::{DiscoveryEvent, DiscoveryService, DEFAULT_SYNC_PORT};
use super::engine::SyncEngine;
use super::pairing::PairingCode;
use super::protocol::SyncOperation;
use super::{
    SyncDeviceIdentityPayload, SyncDiscoveredDevicePayload, SyncEvent, SyncPairedDevicePayload,
    SyncPairingCodePayload, SyncService, SyncStatusPayload,
};
use base64::Engine as _;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tokio::sync::broadcast;
use tokio::sync::RwLock;

struct SyncRuntime {
    service: Arc<SyncService>,
    app: tauri::AppHandle,
    discovered: Arc<RwLock<HashMap<String, SyncDiscoveredDevicePayload>>>,
    started: AtomicBool,
    generation: AtomicU64,
}

static RUNTIME: OnceLock<Arc<SyncRuntime>> = OnceLock::new();

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Select the best IP address from a list of addresses.
/// Prefers routable IPv4 addresses, then IPv6 global addresses, then any available.
fn select_best_address(addresses: &[IpAddr]) -> Option<IpAddr> {
    // Priority 1: Routable IPv4 (not loopback, not link-local)
    if let Some(ip) = addresses.iter().find(|ip| {
        if let IpAddr::V4(v4) = ip {
            !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified()
        } else {
            false
        }
    }) {
        return Some(*ip);
    }

    // Priority 2: Global IPv6 (not link-local, not loopback)
    if let Some(ip) = addresses.iter().find(|ip| {
        if let IpAddr::V6(v6) = ip {
            !v6.is_loopback() && !v6.is_unspecified() 
                // Check it's not link-local (fe80::/10)
                && (v6.segments()[0] & 0xffc0) != 0xfe80
        } else {
            false
        }
    }) {
        return Some(*ip);
    }

    // Fallback: any address
    addresses.first().copied()
}

fn app_state(app: &tauri::AppHandle) -> Result<tauri::State<'_, crate::AppState>, String> {
    app.try_state::<crate::AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())
}

fn with_write_conn<T>(
    app: &tauri::AppHandle,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
) -> Result<T, String> {
    let state = app_state(app)?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    f(&conn)
}

fn load_or_generate_identity(
    app: &tauri::AppHandle,
    device_name: Option<String>,
) -> Result<DeviceIdentity, String> {
    with_write_conn(app, |conn| {
        DeviceIdentity::load_or_generate(conn, device_name).map_err(|e| e.to_string())
    })
}

fn map_paired(d: PairedDevice) -> SyncPairedDevicePayload {
    SyncPairedDevicePayload {
        device_id: d.device_id,
        device_name: d.device_name,
        platform: d.platform,
        paired_at: d.paired_at,
        last_seen: d.last_seen,
        last_sync_version: d.last_sync_version,
    }
}

// Local operations are sent from DB deltas; no local op builders needed here.

fn parse_platform(value: &str) -> Platform {
    match value.to_ascii_lowercase().as_str() {
        "macos" | "mac" => Platform::MacOS,
        "windows" | "win" => Platform::Windows,
        "linux" => Platform::Linux,
        _ => Platform::Unknown,
    }
}

async fn flush_pending_for_device(runtime: Arc<SyncRuntime>, device_id: String) {
    let (ops, target_version, peer_public_key) = match with_write_conn(&runtime.app, |conn| {
        let peer: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT public_key, last_sent_version FROM paired_devices WHERE device_id = ?1",
                [device_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((peer_public_key, last_sent_version)) = peer else {
            return Ok((Vec::<SyncOperation>::new(), 0_i64, Vec::<u8>::new()));
        };

        let local_id: Option<String> = conn
            .query_row("SELECT device_id FROM device_info LIMIT 1", [], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let local_id = local_id.unwrap_or_default();

        let sync_embeddings = crate::settings::get_config_sync(runtime.app.clone())
            .ok()
            .map(|c| c.sync.sync_embeddings)
            .unwrap_or(true);

        let mut ops = SyncEngine::get_operations_since(conn, last_sent_version, &local_id, 500, sync_embeddings)
            .map_err(|e| e.to_string())?;
        if ops.is_empty() {
            return Ok((Vec::<SyncOperation>::new(), 0_i64, peer_public_key));
        }
        ops.sort_by_key(|o| o.version());
        let target_version = ops.last().map(|o| o.version()).unwrap_or(last_sent_version);
        Ok((ops, target_version, peer_public_key))
    }) {
        Ok(tuple) => tuple,
        Err(e) => {
            eprintln!("[Sync] Flush prep failed for {}: {}", device_id, e);
            return;
        }
    };

    if ops.is_empty() {
        return;
    }

    let local_identity = match load_or_generate_identity(&runtime.app, None) {
        Ok(i) => i,
        Err(_) => return,
    };
    let mut local_priv = [0_u8; 32];
    local_priv.copy_from_slice(local_identity.private_key.as_bytes());

    let resolved_addr = {
        let discovery_guard = runtime.service.discovery.read().await;
        let Some(ds) = discovery_guard.as_ref() else {
            return;
        };
        let Some(dev) = ds.get_device(&device_id).await else {
            return;
        };
        let Some(ip) = select_best_address(&dev.addresses) else {
            return;
        };
        SocketAddr::new(ip, dev.port)
    };

    let transport = match super::transport::SecureTransport::connect(
        resolved_addr,
        &local_priv,
        Some(&peer_public_key),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[Sync] Connect failed {}: {}", device_id, e);
            return;
        }
    };

    let push = super::protocol::SyncMessage::PushOperations(super::protocol::PushOperations {
        operations: ops.clone(),
        target_version,
    });
    let bytes = match push.to_bytes() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[Sync] Serialize failed: {}", e);
            return;
        }
    };

    if transport.send(&bytes).await.is_err() {
        return;
    }
    let resp = match transport.recv().await {
        Ok(b) => b,
        Err(_) => return,
    };

    let ack = match super::protocol::SyncMessage::from_bytes(&resp) {
        Ok(super::protocol::SyncMessage::Ack(ack)) => ack,
        _ => return,
    };

    if !ack.success {
        return;
    }

    let new_version = ack.new_version.unwrap_or(target_version);
    let _ = with_write_conn(&runtime.app, |conn| {
        conn.execute(
            "UPDATE paired_devices SET last_sent_version = MAX(COALESCE(last_sent_version, 0), ?1), last_seen = ?2 WHERE device_id = ?3",
            params![new_version, now_ts(), device_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    });

    let _ = runtime
        .service
        .event_tx
        .send(SyncEvent::SyncComplete { device_id, items_synced: ops.len() as u32 });
}

async fn flush_loop(runtime: Arc<SyncRuntime>, generation: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    loop {
        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            break;
        }

        interval.tick().await;

        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            break;
        }

        let enabled = *runtime.service.enabled.read().await;
        if !enabled {
            continue;
        }

        let auto_connect = crate::settings::get_config_sync(runtime.app.clone())
            .ok()
            .map(|c| c.sync.auto_connect)
            .unwrap_or(true);
        if !auto_connect {
            continue;
        }

        let devices = list_paired_devices(runtime.app.clone()).await.unwrap_or_default();
        for d in devices {
            flush_pending_for_device(runtime.clone(), d.device_id).await;
        }
    }
}

async fn handle_incoming_connection(runtime: Arc<SyncRuntime>, transport: super::transport::SecureTransport) {
    if !*runtime.service.enabled.read().await {
        return;
    }

    let remote_pk = transport.remote_public_key().to_vec();
    let remote_device = with_write_conn(&runtime.app, |conn| {
        let pair: Option<String> = conn
            .query_row(
                "SELECT device_id FROM paired_devices WHERE public_key = ?1",
                [remote_pk],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(pair)
    })
    .ok()
    .flatten();

    let payload = match transport.recv().await {
        Ok(v) => v,
        Err(_) => return,
    };

    // Pairing lane: plain JSON message over already encrypted channel
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
        if value.get("kind").and_then(|v| v.as_str()) == Some("pair_with_code") {
            let code = value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let from_device_id = value
                .get("fromDeviceId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let from_device_name = value
                .get("fromDeviceName")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();
            let from_platform = value
                .get("fromPlatform")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let (stored_code, expires_at, local_identity) = match with_write_conn(&runtime.app, |conn| {
                let stored_code: Option<String> = conn
                    .query_row("SELECT value FROM settings WHERE key = 'sync_pair_offer_code'", [], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?;
                let expires_at: i64 = conn
                    .query_row(
                        "SELECT COALESCE(value, '0') FROM settings WHERE key = 'sync_pair_offer_expires'",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let identity = DeviceIdentity::load_or_generate(conn, None).map_err(|e| e.to_string())?;
                Ok((stored_code.unwrap_or_default(), expires_at, identity))
            }) {
                Ok(v) => v,
                Err(_) => return,
            };

            if stored_code != code || now_ts() > expires_at {
                let response = serde_json::json!({ "ok": false, "error": "Invalid or expired pairing code" });
                let _ = transport
                    .send(serde_json::to_vec(&response).unwrap_or_default().as_slice())
                    .await;
                return;
            }

            let peer_platform = match from_platform.as_str() {
                "macos" | "mac" => Platform::MacOS,
                "windows" | "win" => Platform::Windows,
                "linux" => Platform::Linux,
                _ => Platform::Unknown,
            };

            let _ = with_write_conn(&runtime.app, |conn| {
                PairedDevice {
                    device_id: from_device_id.clone(),
                    device_name: from_device_name.clone(),
                    platform: peer_platform,
                    public_key: transport.remote_public_key().to_vec(),
                    paired_at: now_ts(),
                    last_seen: Some(now_ts()),
                    last_sync_version: 0,
                }
                .save(conn)
                .map_err(|e| e.to_string())?;

                conn.execute("DELETE FROM settings WHERE key IN ('sync_pair_offer_code', 'sync_pair_offer_expires', 'sync_pair_offer_device')", [])
                    .map_err(|e| e.to_string())?;
                Ok(())
            });

            let response = serde_json::json!({
                "ok": true,
                "deviceName": local_identity.device_name,
                "platform": format!("{}", local_identity.platform).to_lowercase()
            });
            let _ = transport
                .send(serde_json::to_vec(&response).unwrap_or_default().as_slice())
                .await;

            let _ = runtime.service.event_tx.send(SyncEvent::PairingComplete {
                device_id: from_device_id.clone(),
                device_name: from_device_name,
            });

            flush_pending_for_device(runtime, from_device_id).await;
            return;
        }
    }

    let Some(remote_device_id) = remote_device else {
        return;
    };

    let msg = match super::protocol::SyncMessage::from_bytes(&payload) {
        Ok(m) => m,
        Err(_) => return,
    };

    let mut applied = 0usize;
    let mut highest_version = 0_i64;

    if let super::protocol::SyncMessage::PushOperations(batch) = msg {
        let _ = with_write_conn(&runtime.app, |conn| {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            for op in &batch.operations {
                if SyncEngine::apply_operation(&tx, op, super::protocol::ConflictStrategy::LastWriteWins)
                    .map_err(|e| e.to_string())?
                {
                    applied += 1;
                }
                highest_version = highest_version.max(op.version());
            }
            tx.execute_batch("INSERT INTO clips_fts(clips_fts) VALUES('rebuild');")
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;

            conn.execute(
                "UPDATE paired_devices SET last_sync_version = MAX(COALESCE(last_sync_version, 0), ?1), last_seen = ?2 WHERE device_id = ?3",
                params![highest_version, now_ts(), remote_device_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        });

        let ack = super::protocol::SyncMessage::Ack(super::protocol::AckMessage {
            success: true,
            new_version: Some(highest_version),
            error: None,
            conflicts: Vec::new(),
        });
        if let Ok(bytes) = ack.to_bytes() {
            let _ = transport.send(&bytes).await;
        }
    }

    if applied > 0 {
        let _ = runtime.app.emit("new-clip", ());
        let _ = runtime
            .service
            .event_tx
            .send(SyncEvent::SyncComplete {
                device_id: remote_device_id,
                items_synced: applied as u32,
            });
    }
}

async fn listener_loop(runtime: Arc<SyncRuntime>, private_key: [u8; 32], generation: u64) {
    let listener = match super::transport::SecureListener::bind(DEFAULT_SYNC_PORT, private_key).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[Sync] Listener bind failed: {}", e);
            return;
        }
    };

    loop {
        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            break;
        }

        match tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
            Ok(Ok(transport)) => {
                let rt = runtime.clone();
                tauri::async_runtime::spawn(async move {
                    handle_incoming_connection(rt, transport).await;
                });
            }
            Ok(Err(e)) => {
                eprintln!("[Sync] Accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Err(_) => {}
        }
    }
}

async fn discovery_event_loop(
    runtime: Arc<SyncRuntime>,
    generation: u64,
    mut rx: broadcast::Receiver<DiscoveryEvent>,
) {
    loop {
        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            break;
        }

        let event = match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Err(_) => continue,
        };

        match event {
            DiscoveryEvent::DeviceFound(d) | DiscoveryEvent::DeviceUpdated(d) => {
                let is_paired = with_write_conn(&runtime.app, |conn| {
                    PairedDevice::is_paired(conn, &d.device_id).map_err(|e| e.to_string())
                })
                .unwrap_or(false);

                let connected = with_write_conn(&runtime.app, |conn| {
                    let now = now_ts();
                    let count: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM paired_devices WHERE device_id = ?1 AND last_seen IS NOT NULL AND last_seen >= ?2",
                            params![d.device_id, now - 90],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    Ok(count > 0)
                })
                .unwrap_or(false);

                let payload = SyncDiscoveredDevicePayload {
                    device_id: d.device_id.clone(),
                    device_name: d.device_name,
                    platform: d.platform,
                    is_paired,
                    is_connected: connected,
                };

                runtime
                    .discovered
                    .write()
                    .await
                    .insert(payload.device_id.clone(), payload.clone());

                let _ = runtime.app.emit("sync:discovered-updated", payload);

                if is_paired {
                    let rt = runtime.clone();
                    let device_id = d.device_id.clone();
                    tauri::async_runtime::spawn(async move {
                        flush_pending_for_device(rt, device_id).await;
                    });
                }
            }
            DiscoveryEvent::DeviceLost(id) => {
                runtime.discovered.write().await.remove(&id);
                let _ = runtime.app.emit("sync:discovered-lost", id.clone());
                let _ = runtime.service.event_tx.send(SyncEvent::Disconnected { device_id: id });
            }
        }
    }
}

fn publish_pairing_code(app: &tauri::AppHandle, code: String, expires_at: i64) {
    let _ = app.emit("sync:pairing-offer", SyncPairingCodePayload { code, expires_at });
}

pub fn initialize_sync_if_enabled(app: &tauri::AppHandle) -> Result<(), String> {
    let config = crate::settings::get_config_sync(app.clone())?;

    let service = Arc::new(SyncService::new());
    let identity = load_or_generate_identity(app, config.sync.device_name.clone())?;

    let runtime = Arc::new(SyncRuntime {
        service: service.clone(),
        app: app.clone(),
        discovered: Arc::new(RwLock::new(HashMap::new())),
        started: AtomicBool::new(false),
        generation: AtomicU64::new(0),
    });

    let _ = RUNTIME.set(runtime.clone());

    let service_for_identity = service.clone();
    let identity_for_task = identity.clone();
    tauri::async_runtime::spawn(async move {
        *service_for_identity.identity.write().await = Some(identity_for_task);
    });

    if config.sync.enabled {
        start_runtime_inner(runtime.clone(), config.sync.enabled)?;
        eprintln!(
            "[Sync] Enabled for device '{}' ({})",
            identity.device_name, identity.device_id
        );
    }

    Ok(())
}

fn start_runtime_inner(runtime: Arc<SyncRuntime>, enabled: bool) -> Result<(), String> {
    let app = runtime.app.clone();
    let cfg = crate::settings::get_config_sync(app.clone())?;
    if !enabled || !cfg.sync.enabled {
        return Ok(());
    }

    if runtime.started.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let generation = runtime.generation.fetch_add(1, Ordering::SeqCst) + 1;

    tauri::async_runtime::spawn({
        let runtime = runtime.clone();
        async move {
            *runtime.service.enabled.write().await = true;
            let _ = runtime.service.event_tx.send(SyncEvent::Started);
        }
    });

    let identity = load_or_generate_identity(&app, cfg.sync.device_name).ok();
    if let Some(identity) = identity {
        let info = identity.to_info();
        if let Ok(discovery) = DiscoveryService::new(&info) {
            if discovery.start(&info, DEFAULT_SYNC_PORT).is_ok() {
                // Subscribe BEFORE storing to avoid race condition
                let event_rx = discovery.subscribe();
                
                let rt = runtime.clone();
                tauri::async_runtime::spawn(async move {
                    *rt.service.discovery.write().await = Some(discovery);
                });

                let rt = runtime.clone();
                tauri::async_runtime::spawn(async move {
                    discovery_event_loop(rt, generation, event_rx).await;
                });
            }
        }

        let mut private_key = [0u8; 32];
        private_key.copy_from_slice(identity.private_key.as_bytes());
        let rt = runtime.clone();
        tauri::async_runtime::spawn(async move {
            listener_loop(rt, private_key, generation).await;
        });
    }

    let rt = runtime.clone();
    tauri::async_runtime::spawn(async move {
        flush_loop(rt, generation).await;
    });

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
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = app.emit("sync:status", "updated");
            });
        }
        return;
    }

    if is_enabled {
        let _ = start_runtime_inner(runtime, true);
    } else {
        tauri::async_runtime::spawn(async move {
            runtime.generation.fetch_add(1, Ordering::SeqCst);
            runtime.started.store(false, Ordering::SeqCst);
            runtime.discovered.write().await.clear();
            *runtime.service.enabled.write().await = false;
            if let Some(discovery) = runtime.service.discovery.write().await.take() {
                let _ = discovery.stop();
            }
            let _ = runtime.service.event_tx.send(SyncEvent::Stopped);
        });
    }
}

pub async fn get_status(app: tauri::AppHandle) -> Result<SyncStatusPayload, String> {
    let config = crate::settings::get_config_sync(app.clone())?;
    let enabled = config.sync.enabled;
    let device = load_or_generate_identity(&app, config.sync.device_name.clone())
        .ok()
        .map(|identity| SyncDeviceIdentityPayload {
            device_id: identity.device_id,
            device_name: identity.device_name,
            platform: identity.platform,
        });

    let (paired_count, queue_depth, connected_count) = with_write_conn(&app, |conn| {
        let paired_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM paired_devices", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;

        let queue_depth = if paired_count > 0 {
            let min_sent: i64 = conn
                .query_row(
                    "SELECT COALESCE(MIN(last_sent_version), 0) FROM paired_devices",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let pending_clips: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM clips WHERE sync_id IS NOT NULL AND sync_version > ?1",
                    params![min_sent],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let pending_collections: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM collections WHERE sync_id IS NOT NULL AND sync_version > ?1",
                    params![min_sent],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            (pending_clips + pending_collections) as usize
        } else {
            0
        };

        let connected_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paired_devices WHERE last_seen IS NOT NULL AND last_seen >= ?1",
                params![now_ts() - 90],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok((paired_count as usize, queue_depth, connected_count as usize))
    })?;

    Ok(SyncStatusPayload {
        enabled,
        device,
        paired_count,
        connected_count,
        queue_depth,
    })
}

pub async fn list_paired_devices(app: tauri::AppHandle) -> Result<Vec<SyncPairedDevicePayload>, String> {
    with_write_conn(&app, |conn| {
        PairedDevice::load_all(conn)
            .map(|v| v.into_iter().map(map_paired).collect())
            .map_err(|e| e.to_string())
    })
}

pub async fn unpair_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    with_write_conn(&app, |conn| {
        PairedDevice::remove(conn, &device_id).map_err(|e| e.to_string())
    })?;
    if let Some(runtime) = RUNTIME.get() {
        runtime.discovered.write().await.remove(&device_id);
    }
    Ok(())
}

pub async fn pair_device_manual(
    app: tauri::AppHandle,
    device_id: String,
    device_name: String,
    platform: String,
    public_key_base64: String,
) -> Result<(), String> {
    let platform = parse_platform(&platform);
    let public_key = base64::engine::general_purpose::STANDARD
        .decode(public_key_base64.trim())
        .map_err(|e| e.to_string())?;

    with_write_conn(&app, |conn| {
        PairedDevice {
            device_id,
            device_name,
            platform,
            public_key,
            paired_at: now_ts(),
            last_seen: Some(now_ts()),
            last_sync_version: 0,
        }
        .save(conn)
        .map_err(|e| e.to_string())
    })
}

pub async fn list_discovered_devices(
    _app: tauri::AppHandle,
) -> Result<Vec<SyncDiscoveredDevicePayload>, String> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(Vec::new());
    };
    Ok(runtime
        .discovered
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>())
}

pub async fn start_pairing(app: tauri::AppHandle) -> Result<SyncPairingCodePayload, String> {
    let code = PairingCode::generate().to_string();
    let expires_at = now_ts() + 120;

    let local_id = with_write_conn(&app, |conn| {
        conn.query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())
    })?
    .unwrap_or_default();

    with_write_conn(&app, |conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES(?1, ?2)",
            ["sync_pair_offer_code", code.as_str()],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES(?1, ?2)",
            ["sync_pair_offer_expires", expires_at.to_string().as_str()],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES(?1, ?2)",
            ["sync_pair_offer_device", local_id.as_str()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })?;

    publish_pairing_code(&app, code.clone(), expires_at);

    Ok(SyncPairingCodePayload { code, expires_at })
}

pub async fn pair_with_code(
    app: tauri::AppHandle,
    device_id: String,
    code: String,
) -> Result<(), String> {
    let Some(runtime) = RUNTIME.get().cloned() else {
        return Err("Sync runtime not initialized".to_string());
    };

    let discovered = {
        let discovery = runtime.service.discovery.read().await;
        let Some(discovery) = discovery.as_ref() else {
            return Err("Discovery not available".to_string());
        };
        discovery
            .get_device(&device_id)
            .await
            .ok_or_else(|| "Device not discovered".to_string())?
    };
    let ip = select_best_address(&discovered.addresses)
        .ok_or_else(|| "No address for selected device".to_string())?;

    let (local_identity, local_id) = {
        let identity = load_or_generate_identity(&app, None)?;
        let id = identity.device_id.clone();
        (identity, id)
    };

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(local_identity.private_key.as_bytes());
    let transport = super::transport::SecureTransport::connect(
        SocketAddr::new(ip, discovered.port),
        &private_key,
        None,
    )
    .await
    .map_err(|e| e.to_string())?;

    let local_pub = base64::engine::general_purpose::STANDARD.encode(local_identity.public_key.as_bytes());
    let payload = serde_json::json!({
        "kind": "pair_with_code",
        "code": code,
        "fromDeviceId": local_id,
        "fromDeviceName": local_identity.device_name,
        "fromPlatform": format!("{}", local_identity.platform).to_lowercase(),
        "fromPublicKey": local_pub,
    });
    transport
        .send(serde_json::to_vec(&payload).map_err(|e| e.to_string())?.as_slice())
        .await
        .map_err(|e| e.to_string())?;

    let resp = transport.recv().await.map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_slice(&resp).map_err(|e| e.to_string())?;
    if value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let device_name = value
            .get("deviceName")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let platform = value
            .get("platform")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let remote_pk = transport.remote_public_key().to_vec();
        pair_device_manual(
            app.clone(),
            device_id.clone(),
            device_name,
            platform,
            base64::engine::general_purpose::STANDARD.encode(remote_pk),
        )
        .await?;

        let _ = runtime
            .service
            .event_tx
            .send(SyncEvent::PairingComplete {
                device_id: device_id.clone(),
                device_name: discovered.device_name.clone(),
            });

        flush_pending_for_device(runtime, device_id).await;
        return Ok(());
    }

    Err(value
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("Pairing rejected")
        .to_string())
}

fn trigger_flush_for_all_paired(app: tauri::AppHandle) {
    let Some(runtime) = RUNTIME.get().cloned() else {
        return;
    };

    tauri::async_runtime::spawn(async move {
        if !*runtime.service.enabled.read().await {
            return;
        }

        let auto_connect = crate::settings::get_config_sync(app.clone())
            .ok()
            .map(|c| c.sync.auto_connect)
            .unwrap_or(true);
        if !auto_connect {
            return;
        }

        let devices = list_paired_devices(app).await.unwrap_or_default();
        for d in devices {
            flush_pending_for_device(runtime.clone(), d.device_id).await;
        }
    });
}

pub fn queue_clip_sync_change(app: &tauri::AppHandle, _sync_id: String) {
    trigger_flush_for_all_paired(app.clone());
}

pub fn queue_collection_sync_change(app: &tauri::AppHandle, _sync_id: String) {
    trigger_flush_for_all_paired(app.clone());
}

pub fn queue_embedding_sync_change(
    app: &tauri::AppHandle,
    _clip_sync_id: String,
    _version: i64,
) -> Result<(), String> {
    trigger_flush_for_all_paired(app.clone());
    Ok(())
}
