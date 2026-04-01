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
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tokio::sync::broadcast;
use tokio::sync::RwLock;

#[cfg(target_os = "windows")]
fn encode_powershell_script(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(target_os = "windows")]
fn run_powershell_script(script: &str) -> std::io::Result<std::process::Output> {
    use std::process::Command;

    let encoded = encode_powershell_script(script);
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .output()
}

#[cfg(target_os = "windows")]
fn ensure_windows_firewall_rule_for_copi() {
    use std::process::Command;

    static ELEVATION_ATTEMPTED: AtomicBool = AtomicBool::new(false);

    let Ok(exe_path) = std::env::current_exe() else {
        eprintln!("[Sync] Could not determine executable path for firewall setup");
        return;
    };
    let exe_path = exe_path.to_string_lossy().replace('\'', "''");

    let rule_name = "Copi";
    let create_script = format!(
        "$rule = Get-NetFirewallRule -DisplayName '{rule}' -ErrorAction SilentlyContinue; if (-not $rule) {{ New-NetFirewallRule -DisplayName '{rule}' -Direction Inbound -Action Allow -Protocol TCP -Program '{program}' -Profile Private,Public | Out-Null }}",
        rule = rule_name,
        program = exe_path
    );

    match run_powershell_script(&create_script) {
        Ok(out) if out.status.success() => {
            eprintln!("[Sync] Windows firewall rule ensured ({})", rule_name);
            return;
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let denied = stderr.contains("Access is denied")
                || stderr.contains("System Error 5")
                || stderr.contains("PermissionDenied");

            if denied {
                if ELEVATION_ATTEMPTED.swap(true, Ordering::SeqCst) {
                    eprintln!(
                        "[Sync] Firewall rule missing and elevation already attempted this run"
                    );
                    return;
                }

                let elevate_script = format!(
                    "$rule = Get-NetFirewallRule -DisplayName '{rule}' -ErrorAction SilentlyContinue; if (-not $rule) {{ New-NetFirewallRule -DisplayName '{rule}' -Direction Inbound -Action Allow -Protocol TCP -Program '{program}' -Profile Private,Public | Out-Null }}",
                    rule = rule_name,
                    program = exe_path
                );
                let elevate_encoded = encode_powershell_script(&elevate_script);
                let elevate_cmd = format!(
                    "Start-Process powershell -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}'",
                    elevate_encoded
                );

                match Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        &elevate_cmd,
                    ])
                    .status()
                {
                    Ok(_) => {
                        eprintln!(
                            "[Sync] Requested elevated Windows firewall setup (UAC prompt may be shown)"
                        );
                    }
                    Err(e) => {
                        eprintln!("[Sync] Failed to request elevated firewall setup: {}", e);
                    }
                }
            } else {
                eprintln!(
                    "[Sync] Failed to ensure Windows firewall rule ({}): {}",
                    rule_name,
                    stderr.trim()
                );
            }
        }
        Err(e) => {
            eprintln!(
                "[Sync] Failed to invoke PowerShell for firewall setup: {}",
                e
            );
        }
    }
}

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

fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

fn is_address_usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && !is_ipv6_link_local(v6),
    }
}

/// Sort addresses by connection preference: routable/private IPv4 first, then
/// non-link-local IPv6. Link-local addresses are excluded because the discovery
/// payload only carries `IpAddr` (no interface scope), and macOS requires a
/// scope ID to connect to `fe80::/10` peers.
fn sort_addresses_by_preference(addresses: &[IpAddr]) -> Vec<IpAddr> {
    let mut seen = HashSet::new();
    let mut sorted: Vec<IpAddr> = addresses
        .iter()
        .copied()
        .filter(|ip| is_address_usable(ip))
        .filter(|ip| seen.insert(*ip))
        .collect();

    sorted.sort_by_key(|ip| match ip {
        IpAddr::V4(_) => 0,
        IpAddr::V6(_) => 1,
    });

    sorted
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
    eprintln!(
        "[Sync] flush_pending_for_device: starting for {}",
        device_id
    );

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
            eprintln!(
                "[Sync] flush_pending_for_device: {} NOT in paired_devices table",
                device_id
            );
            return Ok((Vec::<SyncOperation>::new(), 0_i64, Vec::<u8>::new()));
        };
        eprintln!(
            "[Sync] flush_pending_for_device: {} last_sent_version={}",
            device_id, last_sent_version
        );

        let local_id: Option<String> = conn
            .query_row("SELECT device_id FROM device_info LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let local_id = local_id.unwrap_or_default();

        let sync_embeddings = crate::settings::get_config_sync(runtime.app.clone())
            .ok()
            .map(|c| c.sync.sync_embeddings)
            .unwrap_or(true);

        let mut ops = SyncEngine::get_operations_since(
            conn,
            last_sent_version,
            &local_id,
            50,
            sync_embeddings,
        )
        .map_err(|e| e.to_string())?;
        eprintln!(
            "[Sync] flush_pending_for_device: {} found {} ops since version {}",
            device_id,
            ops.len(),
            last_sent_version
        );
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
        eprintln!(
            "[Sync] flush_pending_for_device: {} no pending ops, done",
            device_id
        );
        return;
    }

    eprintln!(
        "[Sync] flush_pending_for_device: {} sending {} ops, target_version={}",
        device_id,
        ops.len(),
        target_version
    );

    let local_identity = match load_or_generate_identity(&runtime.app, None) {
        Ok(i) => i,
        Err(e) => {
            eprintln!(
                "[Sync] flush_pending_for_device: {} failed to load identity: {}",
                device_id, e
            );
            return;
        }
    };
    let mut local_priv = [0_u8; 32];
    local_priv.copy_from_slice(local_identity.private_key.as_bytes());

    let candidate_addrs = {
        let discovery_guard = runtime.service.discovery.read().await;
        let Some(ds) = discovery_guard.as_ref() else {
            eprintln!(
                "[Sync] flush_pending_for_device: {} discovery service not available",
                device_id
            );
            return;
        };
        let Some(dev) = ds.get_device(&device_id) else {
            eprintln!(
                "[Sync] flush_pending_for_device: {} NOT found in mDNS discovery cache",
                device_id
            );
            return;
        };
        let original_count = dev.addresses.len();
        let addrs = sort_addresses_by_preference(&dev.addresses);
        let dropped_count = original_count.saturating_sub(addrs.len());
        if dropped_count > 0 {
            eprintln!(
                "[Sync] flush_pending_for_device: {} filtered out {} unusable address(es)",
                device_id, dropped_count
            );
        }
        if addrs.is_empty() {
            eprintln!(
                "[Sync] flush_pending_for_device: {} has no usable addresses",
                device_id
            );
            return;
        }
        eprintln!(
            "[Sync] flush_pending_for_device: {} trying {} addresses",
            device_id,
            addrs.len()
        );
        addrs
            .into_iter()
            .map(|ip| SocketAddr::new(ip, dev.port))
            .collect::<Vec<_>>()
    };

    let mut transport = None;
    for addr in &candidate_addrs {
        match super::transport::SecureTransport::connect(*addr, &local_priv, Some(&peer_public_key))
            .await
        {
            Ok(t) => {
                transport = Some(t);
                break;
            }
            Err(e) => {
                eprintln!("[Sync] Connect to {} failed for {}: {}", addr, device_id, e);
            }
        }
    }
    let Some(transport) = transport else {
        eprintln!("[Sync] All addresses exhausted for {}", device_id);
        return;
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

    eprintln!(
        "[Sync] flush_pending_for_device: {} about to send {} bytes",
        device_id,
        bytes.len()
    );
    if let Err(e) = transport.send(&bytes).await {
        eprintln!(
            "[Sync] flush_pending_for_device: {} send failed: {} (payload={} bytes)",
            device_id,
            e,
            bytes.len()
        );
        return;
    }
    eprintln!(
        "[Sync] flush_pending_for_device: {} sent {} bytes, awaiting ACK",
        device_id,
        bytes.len()
    );

    let resp = match transport.recv().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "[Sync] flush_pending_for_device: {} recv failed: {}",
                device_id, e
            );
            return;
        }
    };

    let ack = match super::protocol::SyncMessage::from_bytes(&resp) {
        Ok(super::protocol::SyncMessage::Ack(ack)) => ack,
        Ok(other) => {
            eprintln!(
                "[Sync] flush_pending_for_device: {} unexpected response type: {:?}",
                device_id,
                std::mem::discriminant(&other)
            );
            return;
        }
        Err(e) => {
            eprintln!(
                "[Sync] flush_pending_for_device: {} failed to parse response: {}",
                device_id, e
            );
            return;
        }
    };

    if !ack.success {
        eprintln!(
            "[Sync] flush_pending_for_device: {} ACK failed: {:?}",
            device_id, ack.error
        );
        return;
    }

    eprintln!(
        "[Sync] flush_pending_for_device: {} Phase 1 SUCCESS, new_version={:?}",
        device_id, ack.new_version
    );

    let new_version = ack.new_version.unwrap_or(target_version);
    let _ = with_write_conn(&runtime.app, |conn| {
        conn.execute(
            "UPDATE paired_devices SET last_sent_version = MAX(COALESCE(last_sent_version, 0), ?1), last_seen = ?2 WHERE device_id = ?3",
            params![new_version, now_ts(), device_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    });

    // Phase 2: Send images if receiver requested any
    if !ack.needs_images.is_empty() {
        eprintln!(
            "[Sync] flush_pending_for_device: {} Phase 2 - sending {} images",
            device_id,
            ack.needs_images.len()
        );

        // Get image data for requested clips
        let images = match with_write_conn(&runtime.app, |conn| {
            SyncEngine::get_image_data_for_clips(conn, &ack.needs_images).map_err(|e| e.to_string())
        }) {
            Ok(imgs) => imgs,
            Err(e) => {
                eprintln!(
                    "[Sync] flush_pending_for_device: {} failed to get images: {}",
                    device_id, e
                );
                vec![]
            }
        };

        // Send images one at a time to avoid large payloads
        for img in images {
            let img_size = img.image_data.len();
            eprintln!(
                "[Sync] flush_pending_for_device: {} sending image for clip {} ({} bytes)",
                device_id, img.sync_id, img_size
            );

            let msg = super::protocol::SyncMessage::PushImageData(img);
            let bytes = match msg.to_bytes() {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "[Sync] flush_pending_for_device: {} failed to serialize image: {}",
                        device_id, e
                    );
                    continue;
                }
            };

            if let Err(e) = transport.send(&bytes).await {
                eprintln!(
                    "[Sync] flush_pending_for_device: {} image send failed: {}",
                    device_id, e
                );
                break; // Stop sending more images on connection error
            }

            // Wait for ACK
            match transport.recv().await {
                Ok(resp) => match super::protocol::SyncMessage::from_bytes(&resp) {
                    Ok(super::protocol::SyncMessage::Ack(img_ack)) => {
                        if img_ack.success {
                            eprintln!(
                                "[Sync] flush_pending_for_device: {} image ACK received",
                                device_id
                            );
                        } else {
                            eprintln!(
                                "[Sync] flush_pending_for_device: {} image ACK failed: {:?}",
                                device_id, img_ack.error
                            );
                        }
                    }
                    Ok(_) => {
                        eprintln!(
                            "[Sync] flush_pending_for_device: {} unexpected response to image",
                            device_id
                        );
                    }
                    Err(e) => {
                        eprintln!("[Sync] flush_pending_for_device: {} failed to parse image response: {}", device_id, e);
                    }
                },
                Err(e) => {
                    eprintln!(
                        "[Sync] flush_pending_for_device: {} image recv failed: {}",
                        device_id, e
                    );
                    break;
                }
            }
        }

        eprintln!(
            "[Sync] flush_pending_for_device: {} Phase 2 complete",
            device_id
        );
    }

    let _ = runtime.service.event_tx.send(SyncEvent::SyncComplete {
        device_id,
        items_synced: ops.len() as u32,
    });
}

async fn flush_loop(runtime: Arc<SyncRuntime>, generation: u64) {
    eprintln!("[Sync] flush_loop: started (generation={})", generation);
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    let mut tick_count = 0_u64;
    loop {
        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            eprintln!("[Sync] flush_loop: stopping (generation changed or stopped)");
            break;
        }

        interval.tick().await;
        tick_count += 1;

        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            eprintln!("[Sync] flush_loop: stopping (generation changed or stopped)");
            break;
        }

        let enabled = *runtime.service.enabled.read().await;
        if !enabled {
            if tick_count % 10 == 1 {
                eprintln!("[Sync] flush_loop: sync disabled, skipping tick");
            }
            continue;
        }

        let auto_connect = crate::settings::get_config_sync(runtime.app.clone())
            .ok()
            .map(|c| c.sync.auto_connect)
            .unwrap_or(true);
        if !auto_connect {
            if tick_count % 10 == 1 {
                eprintln!("[Sync] flush_loop: auto_connect disabled, skipping tick");
            }
            continue;
        }

        let devices = list_paired_devices(runtime.app.clone())
            .await
            .unwrap_or_default();
        if !devices.is_empty() {
            eprintln!(
                "[Sync] flush_loop: tick #{}, checking {} paired device(s)",
                tick_count,
                devices.len()
            );
        }
        for d in devices {
            flush_pending_for_device(runtime.clone(), d.device_id).await;
        }
    }
}

async fn handle_incoming_connection(
    runtime: Arc<SyncRuntime>,
    transport: super::transport::SecureTransport,
) {
    let remote_pk_hex = hex::encode(transport.remote_public_key());
    eprintln!(
        "[Sync] handle_incoming_connection: remote_pk={}",
        &remote_pk_hex[..16]
    );

    if !*runtime.service.enabled.read().await {
        eprintln!("[Sync] handle_incoming_connection: sync disabled, dropping connection");
        return;
    }

    let remote_pk = transport.remote_public_key().to_vec();
    let remote_device = with_write_conn(&runtime.app, |conn| {
        let pair: Option<String> = conn
            .query_row(
                "SELECT device_id FROM paired_devices WHERE public_key = ?1",
                [&remote_pk],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(pair)
    })
    .ok()
    .flatten();

    eprintln!(
        "[Sync] handle_incoming_connection: remote_device={:?}",
        remote_device
    );

    let payload = match transport.recv().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[Sync] handle_incoming_connection: recv failed: {}", e);
            return;
        }
    };
    eprintln!(
        "[Sync] handle_incoming_connection: received {} bytes",
        payload.len()
    );

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

            let (stored_code, expires_at, local_identity) = match with_write_conn(
                &runtime.app,
                |conn| {
                    let stored_code: Option<String> = conn
                        .query_row(
                            "SELECT value FROM settings WHERE key = 'sync_pair_offer_code'",
                            [],
                            |r| r.get(0),
                        )
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
                    let identity =
                        DeviceIdentity::load_or_generate(conn, None).map_err(|e| e.to_string())?;
                    Ok((stored_code.unwrap_or_default(), expires_at, identity))
                },
            ) {
                Ok(v) => v,
                Err(_) => return,
            };

            if stored_code != code || now_ts() > expires_at {
                let response =
                    serde_json::json!({ "ok": false, "error": "Invalid or expired pairing code" });
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
        eprintln!("[Sync] handle_incoming_connection: unknown device (public key not in paired_devices), dropping");
        return;
    };

    let msg = match super::protocol::SyncMessage::from_bytes(&payload) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "[Sync] handle_incoming_connection: failed to parse SyncMessage: {}",
                e
            );
            return;
        }
    };

    let mut applied = 0usize;
    let mut highest_version = 0_i64;
    let mut needs_images: Vec<String> = Vec::new();

    if let super::protocol::SyncMessage::PushOperations(batch) = msg {
        eprintln!("[Sync] handle_incoming_connection: {} sent PushOperations with {} ops, target_version={}", 
                  remote_device_id, batch.operations.len(), batch.target_version);

        match with_write_conn(&runtime.app, |conn| {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            for op in &batch.operations {
                // Track clips that are images but didn't include image_data (need Phase 2)
                if let super::protocol::SyncOperation::UpsertClip(clip_data) = op {
                    if clip_data.content_type == "image" && clip_data.image_data.is_none() {
                        needs_images.push(clip_data.sync_id.clone());
                    }
                }

                match SyncEngine::apply_operation(
                    &tx,
                    op,
                    super::protocol::ConflictStrategy::LastWriteWins,
                ) {
                    Ok(true) => {
                        applied += 1;
                    }
                    Ok(false) => {
                        eprintln!(
                            "[Sync] apply_operation returned false for op version={}, type={:?}",
                            op.version(),
                            op.operation_type_name()
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "[Sync] apply_operation ERROR for op version={}: {}",
                            op.version(),
                            e
                        );
                        return Err(e.to_string());
                    }
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
        }) {
            Ok(_) => {
                eprintln!(
                    "[Sync] handle_incoming_connection: {} transaction committed successfully",
                    remote_device_id
                );
            }
            Err(e) => {
                eprintln!(
                    "[Sync] handle_incoming_connection: {} transaction FAILED: {}",
                    remote_device_id, e
                );
            }
        }

        eprintln!(
            "[Sync] handle_incoming_connection: {} applied {} ops, {} need images",
            remote_device_id,
            applied,
            needs_images.len()
        );

        // Send ACK with list of clips that need image data
        let ack = super::protocol::SyncMessage::Ack(super::protocol::AckMessage {
            success: true,
            new_version: Some(highest_version),
            error: None,
            conflicts: Vec::new(),
            needs_images: needs_images.clone(),
        });
        if let Ok(bytes) = ack.to_bytes() {
            let _ = transport.send(&bytes).await;
        }
        eprintln!(
            "[Sync] handle_incoming_connection: {} sent ACK, awaiting Phase 2 images",
            remote_device_id
        );

        // Phase 2: Receive image data if any were requested
        if !needs_images.is_empty() {
            let mut images_received = 0;
            loop {
                // Try to receive image data with timeout
                match tokio::time::timeout(Duration::from_secs(30), transport.recv()).await {
                    Ok(Ok(img_payload)) => {
                        match super::protocol::SyncMessage::from_bytes(&img_payload) {
                            Ok(super::protocol::SyncMessage::PushImageData(img_data)) => {
                                eprintln!("[Sync] handle_incoming_connection: {} received image for clip {} ({} bytes)", 
                                         remote_device_id, img_data.sync_id, img_data.image_data.len());

                                // Apply image data
                                let _ = with_write_conn(&runtime.app, |conn| {
                                    SyncEngine::apply_image_data(conn, &img_data)
                                        .map_err(|e| e.to_string())
                                });

                                images_received += 1;

                                // Send ACK for image
                                let img_ack = super::protocol::SyncMessage::Ack(
                                    super::protocol::AckMessage {
                                        success: true,
                                        new_version: None,
                                        error: None,
                                        conflicts: Vec::new(),
                                        needs_images: Vec::new(),
                                    },
                                );
                                if let Ok(bytes) = img_ack.to_bytes() {
                                    let _ = transport.send(&bytes).await;
                                }

                                // Check if we've received all expected images
                                if images_received >= needs_images.len() {
                                    eprintln!("[Sync] handle_incoming_connection: {} received all {} images", 
                                             remote_device_id, images_received);
                                    break;
                                }
                            }
                            Ok(super::protocol::SyncMessage::Disconnect) => {
                                eprintln!("[Sync] handle_incoming_connection: {} sent Disconnect, ending Phase 2", remote_device_id);
                                break;
                            }
                            Ok(other) => {
                                eprintln!("[Sync] handle_incoming_connection: {} unexpected message type during Phase 2: {:?}", 
                                         remote_device_id, std::mem::discriminant(&other));
                                break;
                            }
                            Err(e) => {
                                eprintln!("[Sync] handle_incoming_connection: {} failed to parse Phase 2 message: {}", remote_device_id, e);
                                break;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        eprintln!(
                            "[Sync] handle_incoming_connection: {} Phase 2 recv error: {}",
                            remote_device_id, e
                        );
                        break;
                    }
                    Err(_) => {
                        eprintln!("[Sync] handle_incoming_connection: {} Phase 2 timeout waiting for images", remote_device_id);
                        break;
                    }
                }
            }
            eprintln!(
                "[Sync] handle_incoming_connection: {} Phase 2 complete, received {}/{} images",
                remote_device_id,
                images_received,
                needs_images.len()
            );
        }
    }

    if applied > 0 {
        let _ = runtime.app.emit("new-clip", ());
        let _ = runtime.service.event_tx.send(SyncEvent::SyncComplete {
            device_id: remote_device_id,
            items_synced: applied as u32,
        });
    }
}

async fn listener_loop(
    runtime: Arc<SyncRuntime>,
    listener: super::transport::SecureListener,
    generation: u64,
) {
    loop {
        if !runtime.started.load(Ordering::SeqCst)
            || runtime.generation.load(Ordering::SeqCst) != generation
        {
            break;
        }

        // Timeout only covers the TCP accept (waiting for new connections).
        // The Noise handshake runs in a spawned task with its own timeout.
        match tokio::time::timeout(Duration::from_secs(2), listener.accept_tcp()).await {
            Ok(Ok((stream, addr))) => {
                eprintln!("[Sync] Incoming TCP connection from {}", addr);
                let rt = runtime.clone();
                let key = listener.private_key();
                tauri::async_runtime::spawn(async move {
                    // Give the Noise handshake a generous timeout (separate from accept polling)
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        super::transport::SecureTransport::accept(stream, &key),
                    )
                    .await
                    {
                        Ok(Ok(transport)) => {
                            handle_incoming_connection(rt, transport).await;
                        }
                        Ok(Err(e)) => {
                            eprintln!("[Sync] Handshake with {} failed: {}", addr, e);
                        }
                        Err(_) => {
                            eprintln!("[Sync] Handshake with {} timed out", addr);
                        }
                    }
                });
            }
            Ok(Err(e)) => {
                eprintln!("[Sync] TCP accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Err(_) => {} // timeout — loop back to check shutdown flag
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
                let _ = runtime
                    .service
                    .event_tx
                    .send(SyncEvent::Disconnected { device_id: id });
            }
        }
    }
}

fn publish_pairing_code(app: &tauri::AppHandle, code: String, expires_at: i64) {
    let _ = app.emit(
        "sync:pairing-offer",
        SyncPairingCodePayload { code, expires_at },
    );
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
        let mut private_key = [0u8; 32];
        private_key.copy_from_slice(identity.private_key.as_bytes());
        let rt = runtime.clone();
        let info_for_bootstrap = info.clone();
        tauri::async_runtime::spawn(async move {
            let mut listener = None;
            let mut primary_err = String::new();

            for attempt in 1..=6 {
                match super::transport::SecureListener::bind(DEFAULT_SYNC_PORT, private_key).await {
                    Ok(l) => {
                        listener = Some(l);
                        break;
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        primary_err = msg.clone();

                        if msg.contains("Address already in use") && attempt < 6 {
                            eprintln!(
                                "[Sync] Listener bind on default port {} busy (attempt {}/6), retrying...",
                                DEFAULT_SYNC_PORT, attempt
                            );
                            tokio::time::sleep(Duration::from_millis(350)).await;
                            continue;
                        }
                        break;
                    }
                }
            }

            let listener = match listener {
                Some(l) => l,
                None => {
                    eprintln!(
                        "[Sync] Listener bind on default port {} failed: {}. Retrying on ephemeral port.",
                        DEFAULT_SYNC_PORT, primary_err
                    );

                    match super::transport::SecureListener::bind(0, private_key).await {
                        Ok(l) => l,
                        Err(fallback_err) => {
                            eprintln!(
                                "[Sync] Listener bind failed on both default and ephemeral ports: {}; {}",
                                primary_err, fallback_err
                            );
                            let _ = rt.service.event_tx.send(SyncEvent::SyncError {
                                device_id: None,
                                error: format!(
                                    "Failed to bind sync listener on port {} (and fallback): {}",
                                    DEFAULT_SYNC_PORT, fallback_err
                                ),
                            });
                            return;
                        }
                    }
                }
            };

            let listen_port = match listener.local_addr() {
                Ok(addr) => addr.port(),
                Err(e) => {
                    eprintln!("[Sync] Failed to resolve listener local_addr: {}", e);
                    return;
                }
            };

            if listen_port != DEFAULT_SYNC_PORT {
                eprintln!(
                    "[Sync] Using fallback sync port {} (default {} unavailable)",
                    listen_port, DEFAULT_SYNC_PORT
                );
            }

            #[cfg(target_os = "windows")]
            ensure_windows_firewall_rule_for_copi();

            match DiscoveryService::new(&info_for_bootstrap) {
                Ok(discovery) => {
                    if let Err(e) = discovery.start(&info_for_bootstrap, listen_port) {
                        eprintln!("[Sync] Discovery start failed: {}", e);
                    } else {
                        let event_rx = discovery.subscribe();
                        *rt.service.discovery.write().await = Some(discovery);

                        let rt_events = rt.clone();
                        tauri::async_runtime::spawn(async move {
                            discovery_event_loop(rt_events, generation, event_rx).await;
                        });
                    }
                }
                Err(e) => {
                    eprintln!("[Sync] Discovery init failed: {}", e);
                }
            }

            listener_loop(rt, listener, generation).await;
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

pub async fn list_paired_devices(
    app: tauri::AppHandle,
) -> Result<Vec<SyncPairedDevicePayload>, String> {
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
        conn.query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| {
            row.get::<_, String>(0)
        })
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
            .ok_or_else(|| "Device not discovered".to_string())?
    };
    let candidate_addrs = sort_addresses_by_preference(&discovered.addresses);
    if candidate_addrs.is_empty() {
        return Err("No usable address for selected device".to_string());
    }

    let (local_identity, local_id) = {
        let identity = load_or_generate_identity(&app, None)?;
        let id = identity.device_id.clone();
        (identity, id)
    };

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(local_identity.private_key.as_bytes());

    let mut transport = None;
    let mut last_err = String::new();
    for addr in &candidate_addrs {
        match super::transport::SecureTransport::connect(
            SocketAddr::new(*addr, discovered.port),
            &private_key,
            None,
        )
        .await
        {
            Ok(t) => {
                transport = Some(t);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                eprintln!("[Sync] Pairing connect to {} failed: {}", addr, last_err);
            }
        }
    }
    let transport = transport.ok_or_else(|| format!("All addresses failed: {}", last_err))?;

    let local_pub =
        base64::engine::general_purpose::STANDARD.encode(local_identity.public_key.as_bytes());
    let payload = serde_json::json!({
        "kind": "pair_with_code",
        "code": code,
        "fromDeviceId": local_id,
        "fromDeviceName": local_identity.device_name,
        "fromPlatform": format!("{}", local_identity.platform).to_lowercase(),
        "fromPublicKey": local_pub,
    });
    transport
        .send(
            serde_json::to_vec(&payload)
                .map_err(|e| e.to_string())?
                .as_slice(),
        )
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

        let _ = runtime.service.event_tx.send(SyncEvent::PairingComplete {
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

#[cfg(test)]
mod tests {
    use super::sort_addresses_by_preference;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn sort_addresses_prefers_ipv4_and_filters_unusable() {
        let addrs = vec![
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fe80::1".parse().unwrap()),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
        ];

        let sorted = sort_addresses_by_preference(&addrs);

        assert_eq!(
            sorted,
            vec![
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                IpAddr::V6("2001:db8::1".parse().unwrap()),
            ]
        );
    }
}
