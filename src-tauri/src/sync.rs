use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

use crate::AppState;

pub const SYNC_PORT: u16 = 51827;
const SERVICE_TYPE: &str = "_copi._tcp.local.";
const PROTOCOL_VERSION: u8 = 1;
const PIN_TTL: Duration = Duration::from_secs(120);
const RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Msg {
    Hello {
        device_id: String,
        device_name: String,
        protocol_version: u8,
        cursors: HashMap<String, i64>,
    },
    HelloAck {
        device_id: String,
        device_name: String,
        protocol_version: u8,
        cursors: HashMap<String, i64>,
    },
    PairRequest {
        device_id: String,
        device_name: String,
        pin: String,
    },
    PairAccept {
        device_id: String,
        device_name: String,
    },
    PairReject {
        reason: String,
    },
    ClipBatch {
        clips: Vec<WireClip>,
    },
    ClipPush {
        clip: WireClip,
    },
    BlobRequest {
        hash: String,
    },
    BlobData {
        hash: String,
        data: String,
    },
    Ping,
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireClip {
    hash: String,
    created_at: i64,
    source_device: String,
    kind: String,
    content: String,
    source_app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_app_icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ocr_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_highlighted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_hash: Option<String>,
}

#[derive(Clone)]
pub struct PeerWriter(Arc<AsyncMutex<tokio::net::tcp::OwnedWriteHalf>>);

impl PeerWriter {
    async fn send(&self, msg: &Msg) -> Result<()> {
        let mut payload = serde_json::to_vec(msg).context("serialize msg")?;
        payload.push(b'\n');
        let mut writer = self.0.lock().await;
        writer.write_all(&payload).await.context("write msg")?;
        writer.flush().await.context("flush msg")?;
        Ok(())
    }
}

pub struct SyncState {
    pub device_id: String,
    pub device_name: String,
    enabled: AtomicBool,
    generation: AtomicU64,
    live: RwLock<HashMap<String, PeerWriter>>,
    known_addrs: RwLock<HashMap<String, SocketAddr>>,
    discovered: RwLock<HashMap<String, DiscoveredPeer>>,
    pairing_pin: Mutex<Option<(String, Instant)>>,
    _mdns: Option<ServiceDaemon>,
}

impl SyncState {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub async fn push_clip(&self, clip: WireClip) -> Result<()> {
        if !self.is_enabled() {
            eprintln!("[Sync] push_clip: sync disabled");
            return Ok(());
        }
        let peer_count = self.live.read().await.len();
        eprintln!("[Sync] push_clip: broadcasting to {} peers, hash={}", peer_count, clip.hash);
        self.broadcast(Msg::ClipPush { clip }).await
    }

    pub async fn push_blob(&self, hash: String, data: String) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        self.broadcast(Msg::BlobData { hash, data }).await
    }

    async fn broadcast(&self, msg: Msg) -> Result<()> {
        let peers: Vec<(String, PeerWriter)> = {
            let guard = self.live.read().await;
            guard
                .iter()
                .map(|(id, writer)| (id.clone(), writer.clone()))
                .collect()
        };

        let mut failed = Vec::new();
        for (id, writer) in peers {
            if writer.send(&msg).await.is_err() {
                failed.push(id);
            }
        }

        if !failed.is_empty() {
            let mut guard = self.live.write().await;
            for id in failed {
                guard.remove(&id);
            }
        }

        Ok(())
    }

    pub async fn register_peer(&self, device_id: String, writer: PeerWriter) {
        self.live.write().await.insert(device_id, writer);
    }

    pub async fn unregister_peer(&self, device_id: &str) {
        self.live.write().await.remove(device_id);
    }

    pub async fn connected_peers(&self) -> Vec<String> {
        self.live.read().await.keys().cloned().collect()
    }

    pub fn generate_pin(&self) -> String {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let pin = format!("{:06}", (seed % 900_000) + 100_000);
        if let Ok(mut guard) = self.pairing_pin.lock() {
            *guard = Some((pin.clone(), Instant::now() + PIN_TTL));
        }
        pin
    }

    pub fn verify_pin(&self, pin: &str) -> bool {
        if let Ok(mut guard) = self.pairing_pin.lock() {
            if let Some((stored, expires_at)) = guard.as_ref() {
                if Instant::now() > *expires_at {
                    *guard = None;
                    return false;
                }
                return stored == pin;
            }
        }
        false
    }

    pub fn clear_pin(&self) {
        if let Ok(mut guard) = self.pairing_pin.lock() {
            *guard = None;
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrustedPeer {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Clone, Serialize)]
pub struct DiscoveredPeer {
    pub device_id: String,
    pub display_name: String,
    pub addr: String,
}

#[derive(Clone, Serialize)]
pub struct PairedEvent {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Clone, Serialize)]
pub struct SyncIdentityPayload {
    pub device_id: String,
    pub device_name: String,
}

#[derive(Clone, Serialize)]
pub struct SyncPeerPayload {
    pub device_id: String,
    pub display_name: String,
    pub online: bool,
}

#[derive(Clone, Serialize)]
pub struct SyncPinPayload {
    pub pin: String,
    pub expires_at: i64,
}

pub fn start_sync(app: AppHandle) -> Arc<SyncState> {
    let device_id = match get_or_create_device_id(&app) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("[Sync] Failed to get/create device id: {}", error);
            uuid::Uuid::new_v4().to_string()
        }
    };

    let device_name = gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_string();
    let device_name = if device_name.is_empty() {
        "Unknown Device".to_string()
    } else {
        device_name
    };

    ensure_windows_firewall_rules(SYNC_PORT);

    let mdns = match ServiceDaemon::new() {
        Ok(mdns) => Some(mdns),
        Err(error) => {
            eprintln!("[Sync] Failed to create mDNS daemon: {}", error);
            None
        }
    };
    if let Some(mdns) = mdns.as_ref() {
        let properties: [(&str, &str); 2] = [("v", "1"), ("name", device_name.as_str())];
        match ServiceInfo::new(
            SERVICE_TYPE,
            &device_id,
            &format!("{}.local.", device_id),
            (),
            SYNC_PORT,
            properties.as_slice(),
        ) {
            Ok(service) => {
                if let Err(error) = mdns.register(service.enable_addr_auto()) {
                    eprintln!("[Sync] Failed to register mDNS service: {}", error);
                }
            }
            Err(error) => {
                eprintln!("[Sync] Failed to build mDNS service info: {}", error);
            }
        }
    }

    let trusted_peers = get_trusted_peers(&app).unwrap_or_default();

    let sync = Arc::new(SyncState {
        device_id,
        device_name,
        enabled: AtomicBool::new(false),
        generation: AtomicU64::new(0),
        live: RwLock::new(HashMap::new()),
        known_addrs: RwLock::new(HashMap::new()),
        discovered: RwLock::new(HashMap::new()),
        pairing_pin: Mutex::new(None),
        _mdns: mdns,
    });

    let enabled = crate::settings::get_config_sync(app.clone())
        .map(|cfg| cfg.sync.enabled)
        .unwrap_or(false);
    let debug_force_enable = std::env::var("COPI_SYNC_DEBUG_FORCE_ENABLE")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if enabled || debug_force_enable {
        enable_runtime(app.clone(), sync.clone(), trusted_peers);
    }

    sync
}

fn enable_runtime(app: AppHandle, sync: Arc<SyncState>, trusted_peers: Vec<TrustedPeer>) {
    sync.enabled.store(true, Ordering::SeqCst);
    let generation = sync.generation.fetch_add(1, Ordering::SeqCst) + 1;
    eprintln!("[Sync] Runtime enabled (generation={})", generation);

    {
        let app_clone = app.clone();
        let sync_clone = sync.clone();
        tauri::async_runtime::spawn(async move {
            run_server(app_clone, sync_clone, generation).await;
        });
    }

    {
        let app_clone = app.clone();
        let sync_clone = sync.clone();
        tauri::async_runtime::spawn(async move {
            run_browser(app_clone, sync_clone, trusted_peers, generation).await;
        });
    }
}

async fn disable_runtime(sync: Arc<SyncState>) {
    sync.enabled.store(false, Ordering::SeqCst);
    sync.generation.fetch_add(1, Ordering::SeqCst);
    sync.live.write().await.clear();
    sync.known_addrs.write().await.clear();
    sync.discovered.write().await.clear();
    eprintln!("[Sync] Runtime disabled");
}

async fn run_server(app: AppHandle, sync: Arc<SyncState>, generation: u64) {
    let listener = loop {
        if !sync.is_enabled() || sync.current_generation() != generation {
            return;
        }
        match TcpListener::bind(("0.0.0.0", SYNC_PORT)).await {
            Ok(listener) => break listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                eprintln!(
                    "[Sync] Port {} busy, retrying bind for generation {}",
                    SYNC_PORT, generation
                );
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            Err(error) => {
                eprintln!("[Sync] Failed to bind TCP server on {}: {}", SYNC_PORT, error);
                return;
            }
        }
    };

    eprintln!("[Sync] TCP server listening on port {}", SYNC_PORT);

    loop {
        if !sync.is_enabled() || sync.current_generation() != generation {
            break;
        }
        match tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
            Ok(Ok((stream, _))) => {
                if !sync.is_enabled() || sync.current_generation() != generation {
                    break;
                }
                let app_clone = app.clone();
                let sync_clone = sync.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = handle_connection(app_clone, sync_clone, stream, false, None).await;
                });
            }
            Ok(Err(error)) => {
                eprintln!("[Sync] Accept error: {}", error);
            }
            Err(_) => {}
        }
    }
}

async fn run_browser(
    app: AppHandle,
    sync: Arc<SyncState>,
    trusted_peers: Vec<TrustedPeer>,
    generation: u64,
) {
    let Some(mdns) = sync._mdns.as_ref() else {
        eprintln!("[Sync] mDNS unavailable; discovery/browser disabled");
        return;
    };
    let browse_rx = match mdns.browse(SERVICE_TYPE) {
        Ok(rx) => rx,
        Err(error) => {
            eprintln!("[Sync] Failed to start mDNS browse: {}", error);
            return;
        }
    };

    for peer in &trusted_peers {
        let app_clone = app.clone();
        let sync_clone = sync.clone();
        let peer_id = peer.device_id.clone();
        tauri::async_runtime::spawn(async move {
            reconnect_loop(app_clone, sync_clone, peer_id, generation).await;
        });
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(event) = browse_rx.recv() {
            if event_tx.send(event).is_err() {
                break;
            }
        }
    });

    loop {
        if !sync.is_enabled() || sync.current_generation() != generation {
            break;
        }
        let event = match tokio::time::timeout(Duration::from_secs(1), event_rx.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => continue,
        };
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let full = info.get_fullname().to_string();
                let mut peer_id = extract_peer_id(&full);

                if peer_id.is_empty() {
                    if let Some(name) = info.get_property_val_str("id") {
                        peer_id = name.to_string();
                    }
                }

                if peer_id.is_empty() || peer_id == sync.device_id {
                    continue;
                }

                let ip = info
                    .get_addresses()
                    .iter()
                    .copied()
                    .find(|addr| matches!(addr, IpAddr::V4(_)))
                    .or_else(|| info.get_addresses().iter().copied().next());

                let Some(ip) = ip else {
                    continue;
                };

                let peer_addr = SocketAddr::new(ip, info.get_port());
                {
                    sync.known_addrs
                        .write()
                        .await
                        .insert(peer_id.clone(), peer_addr);
                }

                let trusted = is_trusted_peer(&app, &peer_id).unwrap_or(false);
                eprintln!(
                    "[Sync] mDNS resolved peer={} addr={} trusted={}",
                    peer_id, peer_addr, trusted
                );
                if trusted {
                    if !auto_connect_enabled(&app) {
                        eprintln!("[Sync] Auto-connect disabled; not dialing {}", peer_id);
                        continue;
                    }
                    let is_connected = sync.connected_peers().await.contains(&peer_id);
                    if !is_connected {
                        let app_clone = app.clone();
                        let sync_clone = sync.clone();
                        let peer_id_clone = peer_id.clone();
                        tauri::async_runtime::spawn(async move {
                            connect_to_peer(app_clone, sync_clone, peer_id_clone, peer_addr).await;
                        });
                    }
                } else {
                    let display_name = info
                        .get_property_val_str("name")
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| peer_id.clone());
                    let payload = DiscoveredPeer {
                        device_id: peer_id,
                        display_name,
                        addr: peer_addr.to_string(),
                    };
                    sync.discovered
                        .write()
                        .await
                        .insert(payload.device_id.clone(), payload.clone());
                    let _ = app.emit("sync:discovered", payload);
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                let peer_id = extract_peer_id(&fullname);
                if !peer_id.is_empty() {
                    eprintln!("[Sync] mDNS removed peer={}", peer_id);
                    sync.known_addrs.write().await.remove(&peer_id);
                    sync.discovered.write().await.remove(&peer_id);
                }
            }
            _ => {}
        }
    }
}

async fn reconnect_loop(
    app: AppHandle,
    sync: Arc<SyncState>,
    peer_id: String,
    generation: u64,
) {
    loop {
        if !sync.is_enabled() || sync.current_generation() != generation {
            break;
        }
        if sync.connected_peers().await.contains(&peer_id) {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        if !auto_connect_enabled(&app) {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let target_addr = sync.known_addrs.read().await.get(&peer_id).copied();
        if let Some(target_addr) = target_addr {
            eprintln!("[Sync] Reconnect attempt peer={} addr={}", peer_id, target_addr);
            connect_to_peer(app.clone(), sync.clone(), peer_id.clone(), target_addr).await;
            tokio::time::sleep(RECONNECT_BACKOFF).await;
        } else {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn connect_to_peer(app: AppHandle, sync: Arc<SyncState>, peer_id: String, addr: SocketAddr) {
    if !sync.is_enabled() {
        eprintln!("[Sync] connect_to_peer: sync disabled, not connecting to {}", peer_id);
        return;
    }
    eprintln!("[Sync] Dialing peer={} addr={}", peer_id, addr);
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            eprintln!("[Sync] TCP connected to peer={} addr={}", peer_id, addr);
            stream
        }
        Ok(Err(error)) => {
            eprintln!("[Sync] Failed to connect to {} at {}: {}", peer_id, addr, error);
            return;
        }
        Err(_) => {
            eprintln!("[Sync] Connect timeout to {} at {}", peer_id, addr);
            return;
        }
    };

    eprintln!("[Sync] Starting session with peer={} as initiator", peer_id);
    match handle_connection(app, sync, stream, true, Some(peer_id.clone())).await {
        Ok(()) => eprintln!("[Sync] Session with {} ended normally", peer_id),
        Err(e) => eprintln!("[Sync] Session with {} ended with error: {}", peer_id, e),
    }
}

async fn handle_connection(
    app: AppHandle,
    sync: Arc<SyncState>,
    stream: TcpStream,
    initiator: bool,
    expected_peer: Option<String>,
) -> Result<()> {
    match run_session(app.clone(), sync.clone(), stream, initiator, expected_peer).await {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("[Sync] Session error: {}", error);
            Err(error)
        }
    }
}

async fn run_session(
    app: AppHandle,
    sync: Arc<SyncState>,
    stream: TcpStream,
    initiator: bool,
    expected_peer: Option<String>,
) -> Result<()> {
    if !sync.is_enabled() {
        return Err(anyhow!("sync disabled"));
    }
    stream.set_nodelay(true).ok();
    let (read_half, write_half) = stream.into_split();
    let writer = PeerWriter(Arc::new(AsyncMutex::new(write_half)));
    let mut reader = BufReader::new(read_half);
    let our_cursors = build_cursor_map(&app, &sync.device_id)?;

    if initiator {
        writer
            .send(&Msg::Hello {
                device_id: sync.device_id.clone(),
                device_name: sync.device_name.clone(),
                protocol_version: PROTOCOL_VERSION,
                cursors: our_cursors.clone(),
            })
            .await?;
    }

    let mut first_line = String::new();
    let read = reader.read_line(&mut first_line).await?;
    if read == 0 {
        return Err(anyhow!("session closed before handshake"));
    }
    let first_msg: Msg = serde_json::from_str(first_line.trim_end()).context("parse first msg")?;

    let (peer_id, peer_name, peer_cursors) = match first_msg {
        Msg::PairRequest {
            device_id,
            device_name,
            pin,
        } => {
            if sync.verify_pin(&pin) {
                eprintln!("[Sync] Pair request accepted from {} ({})", device_name, device_id);
                save_trusted_peer(&app, &device_id, &device_name)?;
                sync.clear_pin();
                writer
                    .send(&Msg::PairAccept {
                        device_id: sync.device_id.clone(),
                        device_name: sync.device_name.clone(),
                    })
                    .await?;
                let _ = app.emit(
                    "sync:paired",
                    PairedEvent {
                        device_id,
                        display_name: device_name,
                    },
                );
                return Ok(());
            } else {
                eprintln!("[Sync] Pair request rejected from {} ({})", device_name, device_id);
                writer
                    .send(&Msg::PairReject {
                        reason: "Invalid or expired PIN".to_string(),
                    })
                    .await?;
                return Err(anyhow!("invalid pairing pin"));
            }
        }
        Msg::Hello {
            device_id,
            device_name,
            protocol_version,
            cursors,
        } => {
            eprintln!("[Sync] Incoming hello from {} ({})", device_name, device_id);
            if protocol_version != PROTOCOL_VERSION {
                return Err(anyhow!("protocol mismatch {}", protocol_version));
            }
            if !is_trusted_peer(&app, &device_id)? {
                return Err(anyhow!("untrusted peer {}", device_id));
            }
            save_trusted_peer(&app, &device_id, &device_name)?;
            writer
                .send(&Msg::HelloAck {
                    device_id: sync.device_id.clone(),
                    device_name: sync.device_name.clone(),
                    protocol_version: PROTOCOL_VERSION,
                    cursors: our_cursors.clone(),
                })
                .await?;
            (device_id, device_name, cursors)
        }
        Msg::HelloAck {
            device_id,
            device_name,
            protocol_version,
            cursors,
        } => {
            eprintln!("[Sync] Received hello_ack from {} ({})", device_name, device_id);
            if !initiator {
                return Err(anyhow!("received hello_ack as non-initiator"));
            }
            if protocol_version != PROTOCOL_VERSION {
                return Err(anyhow!("protocol mismatch {}", protocol_version));
            }
            if !is_trusted_peer(&app, &device_id)? {
                return Err(anyhow!("untrusted peer {}", device_id));
            }
            save_trusted_peer(&app, &device_id, &device_name)?;
            (device_id, device_name, cursors)
        }
        _ => {
            return Err(anyhow!("unexpected handshake message"));
        }
    };

    if let Some(expected) = expected_peer {
        if expected != peer_id {
            return Err(anyhow!(
                "peer mismatch: expected {}, got {}",
                expected,
                peer_id
            ));
        }
    }

    sync.register_peer(peer_id.clone(), writer.clone()).await;
    eprintln!("[Sync] Session connected peer={} name={} (now registered)", peer_id, peer_name);
    let _ = app.emit(
        "sync:connected",
        PairedEvent {
            device_id: peer_id.clone(),
            display_name: peer_name.clone(),
        },
    );

    let peer_cursor = peer_cursors.get(&sync.device_id).copied().unwrap_or(0);
    eprintln!("[Sync] Peer {} has cursor {} for our device {}", peer_id, peer_cursor, sync.device_id);
    let delta = get_clips_since(&app, &sync.device_id, peer_cursor)?;
    eprintln!("[Sync] Sending {} clips to peer {} (since cursor {})", delta.len(), peer_id, peer_cursor);
    if !delta.is_empty() {
        writer.send(&Msg::ClipBatch { clips: delta }).await?;
        eprintln!("[Sync] Sent clip batch to peer {}", peer_id);
    }

    let mut ping = tokio::time::interval(PING_INTERVAL);
    let session_result = loop {
        let mut line = String::new();
        tokio::select! {
            read_result = reader.read_line(&mut line) => {
                match read_result {
                    Ok(0) => break Ok(()),
                    Ok(_) => {
                        let msg: Msg = match serde_json::from_str(line.trim_end()) {
                            Ok(msg) => msg,
                            Err(error) => {
                                eprintln!("[Sync] Failed to parse message from {}: {}", peer_id, error);
                                break Err(anyhow!("invalid message from peer"));
                            }
                        };
                        if let Err(error) = handle_message(&app, &sync, &peer_id, &writer, msg).await {
                            break Err(error);
                        }
                    }
                    Err(error) => break Err(anyhow!("read error: {}", error)),
                }
            }
            _ = ping.tick() => {
                if let Err(error) = writer.send(&Msg::Ping).await {
                    break Err(error);
                }
            }
        }
    };

    sync.unregister_peer(&peer_id).await;
    eprintln!("[Sync] Session disconnected peer={} name={}", peer_id, peer_name);
    let _ = app.emit(
        "sync:disconnected",
        PairedEvent {
            device_id: peer_id,
            display_name: peer_name,
        },
    );

    session_result
}

async fn handle_message(
    app: &AppHandle,
    sync: &Arc<SyncState>,
    peer_id: &str,
    writer: &PeerWriter,
    msg: Msg,
) -> Result<()> {
    match msg {
        Msg::ClipBatch { clips } => receive_clips(app, sync, peer_id, writer, clips).await,
        Msg::ClipPush { clip } => receive_clips(app, sync, peer_id, writer, vec![clip]).await,
        Msg::BlobRequest { hash } => {
            if let Some(bytes) = get_image_blob(app, &hash)? {
                writer
                    .send(&Msg::BlobData {
                        hash,
                        data: B64.encode(bytes),
                    })
                    .await?;
            }
            Ok(())
        }
        Msg::BlobData { hash, data } => {
            // BlobData can be large (single-line base64 PNG payload).
            // BufReader reads until '\n' and does not impose a fixed line-size limit.
            let bytes = B64.decode(data).context("decode blob data")?;
            let clip_id = save_image_blob_if_missing(app, &hash, &bytes)?;
            if let Some(id) = clip_id {
                let state = app.state::<AppState>();
                if let Err(error) = state.clip_tx.try_send(id) {
                    eprintln!(
                        "[Sync][debug] embed queue full/dropped after blob for clip {}: {}",
                        id, error
                    );
                }
            }
            let _ = app.emit("sync:blob-received", hash);
            Ok(())
        }
        Msg::Ping => writer.send(&Msg::Pong).await,
        Msg::Pong => Ok(()),
        _ => Ok(()),
    }
}

async fn receive_clips(
    app: &AppHandle,
    sync: &Arc<SyncState>,
    peer_id: &str,
    writer: &PeerWriter,
    clips: Vec<WireClip>,
) -> Result<()> {
    eprintln!("[Sync] Receiving {} clips from peer {}", clips.len(), peer_id);
    if !sync.is_enabled() {
        eprintln!("[Sync] Sync disabled, ignoring incoming clips");
        return Ok(());
    }
    let mut max_by_source: HashMap<String, i64> = HashMap::new();
    let mut inserted_any = false;
    let mut insert_count = 0;

    for clip in clips {
        if clip.source_device.is_empty() || clip.source_device == sync.device_id {
            eprintln!("[Sync] Skipping clip from self or empty source: hash={}", clip.hash);
            continue;
        }

        eprintln!("[Sync] Processing clip: hash={} kind={} source={}", clip.hash, clip.kind, clip.source_device);

        max_by_source
            .entry(clip.source_device.clone())
            .and_modify(|ts| *ts = (*ts).max(clip.created_at))
            .or_insert(clip.created_at);

        let source_icon = clip
            .source_app_icon
            .as_ref()
            .and_then(|icon| B64.decode(icon).ok());
        let should_request_blob = {
            let state = app.state::<AppState>();
            let conn = state
                .db_write
                .lock()
                .map_err(|e| anyhow!("lock poisoned: {}", e))?;

            let rows = conn.execute(
                "INSERT OR IGNORE INTO clips (content, content_hash, content_type, source_app, source_app_icon, content_highlighted, ocr_text, language, created_at, pinned, image_data, source_device, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, 0)",
                rusqlite::params![
                    clip.content,
                    clip.hash,
                    clip.kind,
                    clip.source_app,
                    source_icon,
                    clip.content_highlighted,
                    clip.ocr_text,
                    clip.language,
                    clip.created_at,
                    if clip.pinned { 1 } else { 0 },
                    clip.source_device,
                ],
            )?;

            if rows > 0 {
                inserted_any = true;
                insert_count += 1;
                let clip_id = conn.last_insert_rowid();
                eprintln!("[Sync] Inserted clip id={} hash={}", clip_id, clip.hash);
                if let Err(error) = state.clip_tx.try_send(clip_id) {
                    eprintln!(
                        "[Sync][debug] embed queue full/dropped for clip {}: {}",
                        clip_id, error
                    );
                }
            } else {
                eprintln!("[Sync] Clip already exists or ignored: hash={}", clip.hash);
            }

            if clip.kind == "image" {
                if let Some(image_hash) = clip.image_hash.clone() {
                    let needs_blob: Option<i64> = conn
                        .query_row(
                            "SELECT id FROM clips
                             WHERE content_hash = ?1
                               AND deleted = 0
                               AND (image_data IS NULL OR length(image_data) = 0)
                             LIMIT 1",
                            [image_hash],
                            |row| row.get(0),
                        )
                        .optional()?;
                    needs_blob.is_some()
                } else {
                    false
                }
            } else {
                false
            }
        };

        if should_request_blob {
            if let Some(hash) = clip.image_hash {
                writer.send(&Msg::BlobRequest { hash }).await?;
            }
        }
    }

    for (source_device, ts) in max_by_source {
        update_sync_cursor(app, &source_device, ts)?;
    }

    if inserted_any {
        eprintln!("[Sync] Applied {} incoming clips from peer {} and emitted new-clip", insert_count, peer_id);
        let _ = app.emit("new-clip", ());
    } else {
        eprintln!("[Sync] No new clips inserted from peer {}", peer_id);
    }

    Ok(())
}

fn get_image_blob(app: &AppHandle, hash: &str) -> Result<Option<Vec<u8>>> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;
    let image_data = conn
        .query_row(
            "SELECT image_data FROM clips
             WHERE content_hash = ?1
               AND deleted = 0
               AND image_data IS NOT NULL
               AND length(image_data) > 0
             LIMIT 1",
            [hash],
            |row| row.get(0),
        )
        .optional()
        .context("query image blob")?;
    Ok(image_data)
}

fn save_image_blob_if_missing(app: &AppHandle, hash: &str, bytes: &[u8]) -> Result<Option<i64>> {
    let state = app.state::<AppState>();
    let conn = state
        .db_write
        .lock()
        .map_err(|e| anyhow!("lock poisoned: {}", e))?;

    let updated = conn.execute(
        "UPDATE clips
         SET image_data = ?1
         WHERE content_hash = ?2
           AND deleted = 0
           AND (image_data IS NULL OR length(image_data) = 0)",
        rusqlite::params![bytes, hash],
    )?;

    if updated > 0 {
        let clip_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM clips WHERE content_hash = ?1 AND deleted = 0 LIMIT 1",
                [hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(clip_id)
    } else {
        Ok(None)
    }
}

pub fn get_or_create_device_id(app: &AppHandle) -> Result<String> {
    let state = app.state::<AppState>();
    let conn = state
        .db_write
        .lock()
        .map_err(|e| anyhow!("lock poisoned: {}", e))?;

    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'sync_device_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(device_id) = existing {
        return Ok(device_id);
    }

    let device_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES ('sync_device_id', ?1)",
        [device_id.clone()],
    )?;
    Ok(device_id)
}

pub fn get_trusted_peers(app: &AppHandle) -> Result<Vec<TrustedPeer>> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;

    let mut stmt = conn.prepare(
        "SELECT device_id, display_name, last_seen
         FROM sync_peers
         ORDER BY display_name COLLATE NOCASE ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(TrustedPeer {
            device_id: row.get(0)?,
            display_name: row.get(1)?,
        })
    })?;

    let mut peers = Vec::new();
    for row in rows {
        peers.push(row?);
    }
    Ok(peers)
}

pub fn is_trusted_peer(app: &AppHandle, device_id: &str) -> Result<bool> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sync_peers WHERE device_id = ?1 LIMIT 1",
            [device_id],
            |row| row.get(0),
        )
        .optional()?;
    let is_trusted = exists.is_some();
    eprintln!("[Sync] is_trusted_peer device_id={} result={}", device_id, is_trusted);
    Ok(is_trusted)
}

pub fn save_trusted_peer(app: &AppHandle, device_id: &str, display_name: &str) -> Result<()> {
    eprintln!("[Sync] Saving trusted peer: device_id={} display_name={}", device_id, display_name);
    let state = app.state::<AppState>();
    let conn = state
        .db_write
        .lock()
        .map_err(|e| anyhow!("lock poisoned: {}", e))?;
    let rows = conn.execute(
        "INSERT INTO sync_peers(device_id, display_name, last_seen)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(device_id)
         DO UPDATE SET display_name = excluded.display_name, last_seen = excluded.last_seen",
        rusqlite::params![device_id, display_name, now_ts()],
    )?;
    eprintln!("[Sync] Saved trusted peer rows_affected={}", rows);
    Ok(())
}

pub fn remove_trusted_peer(app: &AppHandle, device_id: &str) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state
        .db_write
        .lock()
        .map_err(|e| anyhow!("lock poisoned: {}", e))?;
    conn.execute("DELETE FROM sync_peers WHERE device_id = ?1", [device_id])?;
    conn.execute("DELETE FROM sync_cursors WHERE device_id = ?1", [device_id])?;
    Ok(())
}

pub fn update_sync_cursor(app: &AppHandle, device_id: &str, last_received_ts: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state
        .db_write
        .lock()
        .map_err(|e| anyhow!("lock poisoned: {}", e))?;
    conn.execute(
        "INSERT INTO sync_cursors(device_id, last_received_ts)
         VALUES (?1, ?2)
         ON CONFLICT(device_id)
         DO UPDATE SET last_received_ts = MAX(sync_cursors.last_received_ts, excluded.last_received_ts)",
        rusqlite::params![device_id, last_received_ts],
    )?;
    Ok(())
}

pub fn build_cursor_map(app: &AppHandle, our_device_id: &str) -> Result<HashMap<String, i64>> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;

    let mut cursors = HashMap::new();

    {
        let mut stmt = conn.prepare("SELECT device_id, last_received_ts FROM sync_cursors")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (device_id, ts) = row?;
            cursors.insert(device_id, ts);
        }
    }

    let own_max: i64 = conn.query_row(
        "SELECT COALESCE(MAX(created_at), 0)
         FROM clips
         WHERE deleted = 0
           AND (source_device = '' OR source_device = ?1)",
        [our_device_id],
        |row| row.get(0),
    )?;

    cursors
        .entry(our_device_id.to_string())
        .and_modify(|ts| *ts = (*ts).max(own_max))
        .or_insert(own_max);

    Ok(cursors)
}

pub fn get_clips_since(app: &AppHandle, our_device_id: &str, since: i64) -> Result<Vec<WireClip>> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;

    let mut stmt = conn.prepare(
        "SELECT content_hash,
                created_at,
                COALESCE(source_device, ''),
                content_type,
                COALESCE(content, ''),
                COALESCE(source_app, ''),
                source_app_icon,
                ocr_text,
                content_highlighted,
                language,
                pinned
         FROM clips
         WHERE deleted = 0
           AND created_at > ?1
           AND (source_device = '' OR source_device = ?2)
         ORDER BY created_at ASC
         LIMIT 500",
    )?;

    let rows = stmt.query_map(rusqlite::params![since, our_device_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<Vec<u8>>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, i64>(10)?,
        ))
    })?;

    let mut clips = Vec::new();
    for row in rows {
        let (
            hash,
            created_at,
            source_device,
            kind,
            content,
            source_app,
            source_app_icon,
            ocr_text,
            content_highlighted,
            language,
            pinned,
        ) = row?;

        let send_source = if source_device.is_empty() {
            our_device_id.to_string()
        } else {
            source_device
        };

        let image_hash = if kind == "image" {
            Some(hash.clone())
        } else {
            None
        };

        clips.push(WireClip {
            hash,
            created_at,
            source_device: send_source,
            kind: kind.clone(),
            content: if kind == "image" {
                "[Image]".to_string()
            } else {
                content
            },
            source_app,
            source_app_icon: source_app_icon.map(|icon| B64.encode(icon)),
            ocr_text,
            content_highlighted,
            language,
            pinned: pinned != 0,
            image_hash,
        });
    }

    Ok(clips)
}

fn parse_target_addr(input: &str) -> Result<SocketAddr> {
    if let Ok(addr) = input.parse::<SocketAddr>() {
        return Ok(addr);
    }

    let fallback = format!("{}:{}", input.trim(), SYNC_PORT);
    fallback
        .parse::<SocketAddr>()
        .with_context(|| format!("invalid target addr: {}", input))
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn auto_connect_enabled(app: &AppHandle) -> bool {
    crate::settings::get_config_sync(app.clone())
        .map(|cfg| cfg.sync.enabled && cfg.sync.auto_connect)
        .unwrap_or(true)
}

fn extract_peer_id(fullname: &str) -> String {
    let trimmed = fullname.trim_end_matches('.');
    let svc = SERVICE_TYPE.trim_end_matches('.');

    if let Some(prefix) = trimmed.strip_suffix(&format!(".{}", svc)) {
        return prefix.to_string();
    }
    if let Some(prefix) = trimmed.strip_suffix(svc) {
        return prefix.trim_end_matches('.').to_string();
    }

    trimmed
        .split('.')
        .next()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn ensure_windows_firewall_rules(listen_port: u16) {
    use std::process::Command;

    let tcp_rule = format!("Copi LAN Sync TCP {}", listen_port);
    let mdns_rule = "Copi mDNS UDP 5353";
    let script = format!(
        "$tcp = Get-NetFirewallRule -DisplayName '{tcp_rule}' -ErrorAction SilentlyContinue; if (-not $tcp) {{ New-NetFirewallRule -DisplayName '{tcp_rule}' -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port} -Profile Any | Out-Null }}; $mdns = Get-NetFirewallRule -DisplayName '{mdns_rule}' -ErrorAction SilentlyContinue; if (-not $mdns) {{ New-NetFirewallRule -DisplayName '{mdns_rule}' -Direction Inbound -Action Allow -Protocol UDP -LocalPort 5353 -Profile Any | Out-Null }}",
        tcp_rule = tcp_rule,
        port = listen_port,
        mdns_rule = mdns_rule
    );

    let utf16_bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let b64 = B64.encode(&utf16_bytes);

    if let Ok(output) = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &b64,
        ])
        .output()
    {
        if !output.status.success() {
            eprintln!(
                "[Sync] Firewall rule command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn ensure_windows_firewall_rules(_listen_port: u16) {}

async fn lookup_clip_for_push(
    app: &AppHandle,
    our_device_id: &str,
    key: &str,
) -> Result<Option<(WireClip, Option<Vec<u8>>)>> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;

    let query_one = |sql: &str| -> Result<Option<(WireClip, Option<Vec<u8>>)>> {
        let row = conn
            .query_row(sql, [key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<Vec<u8>>>(11)?,
                ))
            })
            .optional()?;

        if let Some((
            hash,
            created_at,
            source_device,
            kind,
            content,
            source_app,
            source_app_icon,
            ocr_text,
            content_highlighted,
            language,
            pinned,
            image_data,
        )) = row
        {
            let wire = WireClip {
                hash: hash.clone(),
                created_at,
                source_device: if source_device.is_empty() {
                    our_device_id.to_string()
                } else {
                    source_device
                },
                kind: kind.clone(),
                content: if kind == "image" {
                    "[Image]".to_string()
                } else {
                    content
                },
                source_app,
                source_app_icon: source_app_icon.map(|icon| B64.encode(icon)),
                ocr_text,
                content_highlighted,
                language,
                pinned: pinned != 0,
                image_hash: if kind == "image" {
                    Some(hash)
                } else {
                    None
                },
            };
            return Ok(Some((wire, image_data)));
        }

        Ok(None)
    };

    let by_hash = query_one(
        "SELECT content_hash,
                created_at,
                COALESCE(source_device, ''),
                content_type,
                COALESCE(content, ''),
                COALESCE(source_app, ''),
                source_app_icon,
                ocr_text,
                content_highlighted,
                language,
                pinned,
                image_data
         FROM clips
         WHERE content_hash = ?1
           AND deleted = 0
         ORDER BY created_at DESC
         LIMIT 1",
    )?;
    if by_hash.is_some() {
        return Ok(by_hash);
    }

    query_one(
        "SELECT content_hash,
                created_at,
                COALESCE(source_device, ''),
                content_type,
                COALESCE(content, ''),
                COALESCE(source_app, ''),
                source_app_icon,
                ocr_text,
                content_highlighted,
                language,
                pinned,
                image_data
         FROM clips
         WHERE sync_id = ?1
           AND deleted = 0
         ORDER BY created_at DESC
         LIMIT 1",
    )
}

#[tauri::command]
pub async fn sync_get_identity(app: AppHandle) -> Result<SyncIdentityPayload, String> {
    let state = app.state::<AppState>();
    let sync = state
        .sync
        .get()
        .ok_or_else(|| "sync not initialized".to_string())?;
    Ok(SyncIdentityPayload {
        device_id: sync.device_id.clone(),
        device_name: sync.device_name.clone(),
    })
}

#[tauri::command]
pub async fn sync_list_peers(app: AppHandle) -> Result<Vec<SyncPeerPayload>, String> {
    let trusted = get_trusted_peers(&app).map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let Some(sync) = state.sync.get() else {
        return Ok(Vec::new());
    };
    let connected: HashSet<String> = sync.connected_peers().await.into_iter().collect();

    Ok(trusted
        .into_iter()
        .map(|peer| SyncPeerPayload {
            online: connected.contains(&peer.device_id),
            device_id: peer.device_id,
            display_name: if peer.display_name.trim().is_empty() {
                "Unnamed Device".to_string()
            } else {
                peer.display_name
            },
        })
        .collect())
}

#[tauri::command]
pub async fn sync_generate_pin(app: AppHandle) -> Result<SyncPinPayload, String> {
    let state = app.state::<AppState>();
    let sync = state
        .sync
        .get()
        .ok_or_else(|| "sync not initialized".to_string())?;
    if !sync.is_enabled() {
        return Err("sync is disabled".to_string());
    }
    let pin = sync.generate_pin();
    Ok(SyncPinPayload {
        pin,
        expires_at: now_ts() + PIN_TTL.as_secs() as i64,
    })
}

#[tauri::command]
pub async fn sync_get_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let sync = state
        .sync
        .get()
        .ok_or_else(|| "sync not initialized".to_string())?;
    let connected_count = sync.connected_peers().await.len();
    Ok(serde_json::json!({
        "enabled": sync.is_enabled(),
        "connectedCount": connected_count,
        "deviceId": sync.device_id,
        "deviceName": sync.device_name,
    }))
}

#[tauri::command]
pub async fn sync_pair_with(
    app: AppHandle,
    target_addr: String,
    pin: String,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let sync = state
        .sync
        .get()
        .cloned()
        .ok_or_else(|| "sync not initialized".to_string())?;
    if !sync.is_enabled() {
        return Err("sync is disabled".to_string());
    }

    let addr = parse_target_addr(&target_addr).map_err(|e| e.to_string())?;
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout".to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))?;
    stream.set_nodelay(true).ok();
    let (read_half, write_half) = stream.into_split();
    let writer = PeerWriter(Arc::new(AsyncMutex::new(write_half)));

    writer
        .send(&Msg::PairRequest {
            device_id: sync.device_id.clone(),
            device_name: sync.device_name.clone(),
            pin,
        })
        .await
        .map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;
    if read == 0 {
        return Err("pairing connection closed".to_string());
    }

    let msg: Msg = serde_json::from_str(line.trim_end()).map_err(|e| e.to_string())?;
    match msg {
        Msg::PairAccept {
            device_id,
            device_name,
        } => {
            save_trusted_peer(&app, &device_id, &device_name).map_err(|e| e.to_string())?;
            let _ = app.emit(
                "sync:paired",
                PairedEvent {
                    device_id: device_id.clone(),
                    display_name: device_name,
                },
            );

            let app_clone = app.clone();
            let sync_clone = sync.clone();
            tauri::async_runtime::spawn(async move {
                connect_to_peer(app_clone, sync_clone, device_id, addr).await;
            });

            Ok(())
        }
        Msg::PairReject { reason } => Err(reason),
        _ => Err("unexpected pairing response".to_string()),
    }
}

#[tauri::command]
pub async fn sync_remove_peer(
    app: AppHandle,
    device_id: String,
) -> Result<(), String> {
    remove_trusted_peer(&app, &device_id).map_err(|e| e.to_string())?;
    if let Some(sync) = app.state::<AppState>().sync.get() {
        sync.unregister_peer(&device_id).await;
    }
    Ok(())
}

pub async fn on_local_clip_saved(app: &AppHandle, content_hash: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Some(sync) = state.sync.get().cloned() else {
        return;
    };

    let clip = match lookup_clip_for_push(app, &sync.device_id, content_hash).await {
        Ok(Some(clip)) => clip,
        Ok(None) => return,
        Err(error) => {
            eprintln!("[Sync] Failed to load local clip for push: {}", error);
            return;
        }
    };

    let (wire_clip, image_data) = clip;
    let image_hash = wire_clip.image_hash.clone();
    let is_image = wire_clip.kind == "image";

    if let Err(error) = sync.push_clip(wire_clip).await {
        eprintln!("[Sync] Failed to push local clip: {}", error);
    } else {
        eprintln!("[Sync] Pushed local clip hash={}", content_hash);
    }

    if is_image {
        if let (Some(hash), Some(data)) = (image_hash, image_data) {
            if let Err(error) = sync.push_blob(hash, B64.encode(data)).await {
                eprintln!("[Sync] Failed to push image blob: {}", error);
            }
        }
    }
}

#[tauri::command]
pub async fn sync_list_discovered(app: AppHandle) -> Result<Vec<DiscoveredPeer>, String> {
    let state = app.state::<AppState>();
    let sync = state
        .sync
        .get()
        .ok_or_else(|| "sync not initialized".to_string())?;
    if !sync.is_enabled() {
        return Ok(Vec::new());
    }
    let values = sync.discovered.read().await.values().cloned().collect();
    Ok(values)
}

pub fn next_sync_version(app: &AppHandle) -> i64 {
    let state = app.state::<AppState>();
    let conn = match state.db_write.lock() {
        Ok(conn) => conn,
        Err(_) => return 0,
    };

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(value, '0') FROM settings WHERE key = 'sync_version'",
            [],
            |row| {
                let value: String = row.get(0)?;
                Ok(value.parse::<i64>().unwrap_or(0))
            },
        )
        .unwrap_or(0);
    let next = current + 1;
    let _ = conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES ('sync_version', ?1)",
        [next.to_string()],
    );
    next
}

pub fn apply_config_change(
    app: &AppHandle,
    previous: Option<&crate::settings::CopiConfig>,
    next: &crate::settings::CopiConfig,
) {
    let Some(sync) = app.try_state::<AppState>().and_then(|state| state.sync.get().cloned()) else {
        return;
    };

    let was_enabled = previous.map(|cfg| cfg.sync.enabled).unwrap_or(false);
    let is_enabled = next.sync.enabled;

    if was_enabled == is_enabled {
        return;
    }

    if is_enabled {
        let trusted_peers = get_trusted_peers(app).unwrap_or_default();
        enable_runtime(app.clone(), sync, trusted_peers);
    } else {
        tauri::async_runtime::spawn(async move {
            disable_runtime(sync).await;
        });
    }
}

pub fn on_collection_changed(_app: &AppHandle) {}
