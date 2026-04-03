Step 0 — Read Everything First

Read the entire src-tauri/src/ directory, src-tauri/Cargo.toml, src-tauri/Cargo.lock, and src-tauri/tauri.conf.json in full before writing a single character. Build a mental model of: how AppState is structured, how the DB write lock works (Mutex<rusqlite::Connection>), how the read pool works (r2d2::Pool), how clips are inserted (both insert_clip and insert_image_clip in clipboard.rs), and how start_runtime_services_once in main.rs spawns async tasks. Do not proceed until you have read all of it.


Step 1 — Add Dependencies

Open src-tauri/Cargo.toml. Add exactly these four crates to [dependencies] and nothing else. Do not change any existing dependency. Do not add features that aren't listed here.

mdns-sd = "0.11"
uuid = { version = "1", features = ["v4"] } — check if uuid is already in Cargo.lock (it is, at version 1.x) — if so, just add the v4 feature to whatever form is already declared, don't add a second entry
base64 = "0.22" — check if base64 is already declared — it is in the lock at 0.22.1 — if it's not in Cargo.toml already, add it; if it is, leave it alone
gethostname = "1.1" — check Cargo.lock first; it's already there as a transitive dep, so just declare it in Cargo.toml

After editing, run cargo check --manifest-path src-tauri/Cargo.toml from the repo root. Fix any errors. Do not proceed until this command exits 0.


Step 2 — Extend the Database Schema

Open src-tauri/src/db.rs. You need to make three additions. Read the file completely first.
Addition 1: In the main execute_batch call that creates the initial schema, add two new CREATE TABLE IF NOT EXISTS statements after the existing settings table creation — one called sync_peers with columns device_id TEXT PRIMARY KEY, display_name TEXT NOT NULL DEFAULT '', last_seen INTEGER NOT NULL DEFAULT 0; and one called sync_cursors with columns device_id TEXT PRIMARY KEY, last_received_ts INTEGER NOT NULL DEFAULT 0.
Addition 2: In the run_migrations function, find the needed array of (column_name, column_type) pairs. Add one more entry: ("source_device", "TEXT NOT NULL DEFAULT ''"). This column tracks which device a clip originated from. Existing clips will correctly default to empty string, meaning "this device".
Addition 3: In the same run_migrations function, after the existing index creation execute_batch, add one more index: CREATE INDEX IF NOT EXISTS idx_clips_source_device ON clips(source_device, created_at DESC).
Run cargo check --manifest-path src-tauri/Cargo.toml. Fix any errors. Do not proceed until this exits 0. Then run the app briefly and confirm in the logs that the DB initializes without panic.


Step 3 — Create sync.rs

Create a new file src-tauri/src/sync.rs. This is the core sync engine. Before writing, re-read src-tauri/src/main.rs (specifically AppState and start_runtime_services_once), src-tauri/src/clipboard.rs (specifically insert_clip and insert_image_clip), and src-tauri/src/db.rs (specifically how the write lock and read pool are used). You must match the exact locking patterns already in use — never hold the write Mutex across an await, never call .unwrap() on a lock that might be poisoned in a way that crashes the server.
The file must implement:
Constants: SYNC_PORT: u16 = 51827, service type string "_copi._tcp.local.", protocol version 1u8, PIN TTL of 120 seconds, reconnect backoff of 30 seconds, connect timeout of 5 seconds, ping interval of 60 seconds.
Wire protocol: A single Msg enum, serde tagged with #[serde(tag = "t", rename_all = "snake_case")], serialized as newline-delimited JSON. Variants: Hello (sent by initiator, contains device_id, device_name, protocol_version, and a HashMap<String, i64> of cursors mapping source_device_id to max created_at held), HelloAck (same fields, sent by responder), PairRequest (device_id, device_name, pin), PairAccept (device_id, device_name), PairReject (reason string), ClipBatch (vec of WireClip), ClipPush (single WireClip), BlobRequest (hash string), BlobData (hash string, data as base64 string), Ping, Pong.
WireClip struct: Fields: hash (content_hash), created_at, source_device, kind (content_type), content, source_app, source_app_icon (Option<String> base64), ocr_text (Option<String>), content_highlighted (Option<String>), language (Option<String>), pinned (bool), image_hash (Option<String>, only set for images — the image_data blob is never included inline). Use #[serde(skip_serializing_if = "Option::is_none")] on all Option fields.
SyncState struct: Fields: device_id: String, device_name: String, live: RwLock<HashMap<String, PeerWriter>> (the connected peers), pairing_pin: Mutex<Option<(String, Instant)>>, _mdns: ServiceDaemon (kept alive). All fields pub except live, pairing_pin, _mdns. Implement methods: push_clip, push_blob, register_peer, unregister_peer, connected_peers, generate_pin, verify_pin, clear_pin.
PeerWriter newtype: struct PeerWriter(Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>). Implement Clone. Implement an async send(&self, msg: &Msg) -> anyhow::Result<()> that serializes to JSON, appends \n, and writes all bytes. Never hold this lock across anything that might deadlock.
start_sync(app: AppHandle) -> Arc<SyncState>: This is the public entry point. It must: (1) read or create the device_id from the settings table using the key "sync_device_id" — acquire the write lock, read, if missing generate a UUID v4 and insert, release the lock; (2) get the hostname via gethostname::gethostname(); (3) create the ServiceDaemon; (4) register the mDNS service with instance name = device_id, service type = _copi._tcp.local., port = SYNC_PORT, TXT properties v=1 and name={device_name}; (5) read trusted peers from sync_peers table; (6) construct and Arc-wrap the SyncState; (7) spawn run_server task; (8) spawn run_browser task passing the mdns daemon's browse receiver and the trusted peer list; (9) return the Arc.
run_server: Binds TcpListener to 0.0.0.0:SYNC_PORT. Loops accepting connections. Each accepted connection spawns handle_connection(app, sync, stream, false). Log the port on success, log a clear error and return if bind fails.
run_browser: Calls mdns.browse(SERVICE_TYPE). Maintains a Arc<RwLock<HashMap<String, SocketAddr>>> of known peer addresses. For each peer in the initial trusted list, spawns a reconnect_loop task. Processes mDNS events in a loop: on ServiceResolved, extract peer instance name (strip the service type suffix), skip if it's our own device_id, extract the first IPv4 address (fall back to any address), update the known map, check if peer is trusted (read-only DB query), if trusted and not already connected spawn a connect_to_peer task, if untrusted emit "sync:discovered" event to frontend with device_id, display_name (from TXT name property), and address string. On ServiceRemoved, remove from known map.
reconnect_loop: Loops forever. Sleeps 5 seconds if already connected. If not connected and address is known in the shared map, calls connect_to_peer (which runs the full session and returns when it ends). After connect_to_peer returns, sleeps RECONNECT_BACKOFF before next attempt.
handle_connection / connect_to_peer: connect_to_peer wraps TcpStream::connect in a timeout, calls handle_connection. handle_connection calls run_session and logs the result.
run_session: The core. Sets TCP_NODELAY. Splits stream into read/write halves. Wraps write half in PeerWriter. Creates BufReader on read half.
Handshake phase: build cursor map (query DB for max created_at per source_device). If initiator, send Hello first. Read the first line. Parse it. If it's a PairRequest, verify PIN, if valid save to sync_peers, send PairAccept, emit "sync:paired" event, return Ok. If invalid, send PairReject, return Err. If it's a Hello (we're the responder), verify peer is trusted, send HelloAck with our cursors. If it's a HelloAck (we're the initiator), verify peer is trusted. Extract peer_id and peer_cursors from whichever variant matched.
Register peer in live map. Emit "sync:connected" event.
Delta sync: query DB for clips from our own device (source_device = '' OR = our device_id) newer than peer's cursor for our device_id. If any, send as ClipBatch.
Main loop: tokio::select! between next line from peer and ping interval tick. On receiving a line, parse and dispatch via handle_message. On ping tick, send Ping. Break loop on any read error or Ok(None).
Teardown: unregister peer, emit "sync:disconnected".
handle_message: Dispatches ClipBatch and ClipPush to receive_clips. Handles BlobRequest by querying image_data from DB and sending BlobData. Handles BlobData by decoding base64 and updating the DB clip's image_data where it's currently NULL, then enqueuing the clip_id for embedding via state.clip_tx.try_send(id), then emitting "sync:blob-received". Handles Ping by sending Pong. Ignores Pong.
receive_clips: Iterates clips. Skips any where source_device is empty or equals our own device_id (prevents echo). For each clip, acquire write lock, execute INSERT OR IGNORE INTO clips (...) with all wire fields. After insert, if rows_affected > 0: get the new clip_id, enqueue for embedding. If kind is "image" and image_hash is set, check if image_data is NULL, if so send BlobRequest back to peer. After processing all clips, update sync cursor to max(created_at) seen, emit "new-clip" event.
DB helper functions (all pub): get_or_create_device_id, get_trusted_peers, is_trusted_peer, save_trusted_peer, remove_trusted_peer, update_sync_cursor, build_cursor_map, get_clips_since. Implement these using the same locking patterns as the rest of the codebase — state.db_read_pool.get() for reads, state.db_write.lock().unwrap() for writes, never hold write lock across await.
Tauri commands (all pub, #[tauri::command]): sync_get_identity (returns device_id and device_name as JSON), sync_list_peers (returns paired devices with online status by checking sync.live), sync_generate_pin (calls sync.generate_pin()), sync_pair_with(target_addr: String, pin: String) (connects and sends PairRequest), sync_remove_peer(device_id: String) (removes from DB and disconnects).
on_local_clip_saved(app: &AppHandle, content_hash: &str): Public async function. Gets sync from AppState OnceLock. Queries the clip by content_hash. Builds a WireClip. If kind is "image", sets image_hash = Some(hash), sets content = "[Image]". Calls sync.push_clip(clip).await. For images also separately calls sync.push_blob(hash, base64_encoded_image_data).await if image_data is not null.
Event payload structs: DiscoveredPeer { device_id, display_name, addr } and PairedEvent { device_id, display_name }, both implementing Serialize + Clone.
Run cargo check --manifest-path src-tauri/Cargo.toml. Fix every warning that is an error. Fix every actual error. Do not proceed until this exits 0.


Step 4 — Wire sync into AppState

Open src-tauri/src/main.rs. Read it entirely. Make exactly these changes and no others:
Change 1: Add mod sync; near the top with the other mod declarations.
Change 2: Add pub sync: std::sync::OnceLock<Arc<crate::sync::SyncState>> to the AppState struct. It must be pub.
Change 3: In the app.manage(AppState { ... }) block, add sync: std::sync::OnceLock::new() to the struct literal.
Change 4: In start_runtime_services_once, after all the existing tokio::spawn calls (embedding worker, clipboard watcher, cleanup loop), add two lines: first let sync_state = crate::sync::start_sync(app.clone());, then let _ = app.state::<AppState>().sync.set(sync_state);. The let _ is intentional — if sync is already set (should never happen but defensive), silently ignore.
Change 5: In the invoke_handler! macro call, add these five new commands to the list: sync::sync_get_identity, sync::sync_list_peers, sync::sync_generate_pin, sync::sync_pair_with, sync::sync_remove_peer.
Run cargo check --manifest-path src-tauri/Cargo.toml. Fix all errors. Do not proceed until exits 0.


Step 5 — Hook Sync into Clipboard Capture

Open src-tauri/src/clipboard.rs. Read insert_clip and insert_image_clip completely.
In insert_clip: Find the line let _ = app.emit("new-clip", ());. Immediately after it, add a tauri::async_runtime::spawn block that clones the app handle and the hash string, then calls crate::sync::on_local_clip_saved(&app_clone, &hash_str).await. This must be a spawn (fire-and-forget) so it never blocks the insert path.
In insert_image_clip: Find the equivalent let _ = app.emit("new-clip", ());. Immediately after it, add the same tauri::async_runtime::spawn block calling crate::sync::on_local_clip_saved. The on_local_clip_saved implementation already handles fetching the image blob separately, so no additional code is needed here.
Important: Do not move, restructure, or delete any existing code. Only add the two spawn blocks.
Run cargo check --manifest-path src-tauri/Cargo.toml. Fix all errors. Do not proceed until exits 0.


Step 6 — Windows Firewall Rules

Create a new directory wix/ at the repository root (same level as src and src-tauri). Inside it, create firewall.wxs.
The file must be valid WiX XML that declares a ComponentGroup named CopiSyncFirewall inside a Fragment. The component group must contain one Component with two FirewallException elements from the FirewallExtension namespace: one for TCP port 51827 with Scope="localSubnet" and IgnoreFailure="yes", and one for UDP port 5353 (mDNS) with the same scope and ignore settings.
Then open src-tauri/tauri.conf.json. Find the "bundle" object. Add a "windows" key if it doesn't exist. Inside it, add a "wix" object with two keys: "fragmentPaths" set to ["../../wix/firewall.wxs"] and "componentGroupRefs" set to ["CopiSyncFirewall"]. Do not touch any other keys in the bundle config.
Verification: run cargo tauri build --target x86_64-pc-windows-msvc -- --dry-run if on Windows, or simply validate the JSON is well-formed with python3 -c "import json; json.load(open('src-tauri/tauri.conf.json'))". Fix any issues.


Step 7 — Frontend: Add Sync Section to Settings

Open src/settings/Settings.tsx. Read the entire file. You need to add a Sync section that integrates cleanly with the existing sidebar layout pattern.
Change 1: Add "sync" to the Section type union.
Change 2: Add { id: "sync" as Section, label: "Sync", icon: Wifi } to the SECTIONS array. Import Wifi from lucide-react.
Change 3: Add a SyncSection React component. It must: manage state for identity (device_id, device_name), paired peers list, discovered-but-unpaired peers list, current PIN display, and pairing-in-progress UI. Subscribe to Tauri events "sync:paired", "sync:connected", "sync:disconnected" to refresh the peers list, and "sync:discovered" to append to the discovered list. Use the existing SettingCard, SettingRow, SettingDivider components for all layout — do not introduce new CSS classes. The PIN display should show the 6 digits prominently with letter-spacing. Each discovered device should show its name, address, and an input field for entering the PIN from the other device plus a Pair button. Paired devices should show online/offline status and a Remove button. Follow the exact same patterns as other section components in the file (GeneralSection, DataSection, etc.).
Change 4: In the main section render block (the series of {activeSection === "xxx" && <XxxSection />} expressions), add {activeSection === "sync" && <SyncSection />}.
Run npm run typecheck from the repo root. Fix all TypeScript errors. Do not proceed until exits 0.


Step 8 — Full Integration Test (Checklist)

Run the app in development mode with npm run tauri dev. Open the developer console. Verify each of these in order:

Console shows [Sync] TCP server listening on port 51827 — if not, the server failed to bind; check for port conflict and fix.
Settings window opens, Sync section is visible in the sidebar, clicking it shows the section content without errors.
Clicking "Generate PIN" calls the command and displays a 6-digit number.
Copy a piece of text. Console should not show any sync-related panic or error.
If a second device is available on the same network running the app, verify the first device emits a sync:discovered event (visible in frontend console or Tauri event log) and the device appears in the "Found on this network" section.

If any of steps 1–4 fail, read the full error, identify which step introduced the regression, return to that step's prompt, and fix it before re-running this checklist.


Step 9 — End-to-End Pairing Test

With two devices on the same LAN, both running the app:

On Device A, open Settings → Sync → Generate PIN. Note the 6 digits.
Device B should show Device A in "Found on this network" within 5 seconds (mDNS resolution time). If it doesn't appear after 15 seconds, check that both devices are on the same subnet and that no software firewall is blocking UDP 5353.
On Device B, enter the PIN from step 1 into Device A's entry and click Pair.
Both devices should now show each other in the paired list marked Online.
Copy text on Device A. Within 2 seconds it should appear in Device B's clipboard history.
Copy text on Device B. Within 2 seconds it should appear in Device A's clipboard history.
Close the app on Device A (simulate "leaving home"). Device B should mark Device A as Offline.
Relaunch Device A. Within 5 seconds of mDNS resolution, both devices should reconnect automatically and sync any clips created while disconnected.

If step 5 or 6 fails, add logging to receive_clips to confirm the TCP message is arriving and the INSERT OR IGNORE is executing. If the INSERT is silently ignored (duplicate hash), the dedup is working correctly — check that you're copying different content.
If step 8 fails (no reconnect), verify reconnect_loop is running for the peer and that mDNS fires ServiceResolved when Device A comes back. Add a log line at the top of reconnect_loop's connect attempt to confirm it's executing.


Step 10 — Production Hardening Pass

Before considering this done, audit the following and fix any issues found:

No .unwrap() on the write lock in receive_clips — if the mutex is poisoned and you unwrap, the entire async task panics silently. Use .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))? or the equivalent.
try_send on the embed queue — you use try_send to enqueue clip IDs for embedding. If the channel is full (512 items), try_send silently drops. This is acceptable — add a tracing::debug! log when it returns Err so it's diagnosable.
Image blob size — a PNG can be several MB. BlobData sends it as a base64 string in a single JSON line. Verify your BufReader line buffer can handle this. tokio::io::BufReader has no line length limit by default but will read until \n — this is fine. Document this in a comment near BlobData.
Peer count — receive_clips sends BlobRequest only to the peer that sent the clip. This is correct. Confirm this is the case in your implementation.
Cursor accuracy — build_cursor_map must return the correct max created_at for each source_device. Write a mental test: if device A sends 10 clips at ts=100-110, device B stores them, and then they reconnect — device B's cursor for device A should be 110, so the next delta sync sends 0 clips. Trace through your implementation and confirm this is true.
source_device column migration — existing clips have source_device = ''. In build_cursor_map, your query for "our own clips" must correctly match both source_device = '' AND source_device = our_device_id. Confirm the SQL handles both cases.
mDNS instance name — the mDNS instance name (the part before ._copi._tcp.local.) must be the device_id UUID and nothing else. Verify run_browser's parsing strips the suffix correctly and doesn't include dots or underscores from the service type in the extracted peer_id.

Run cargo check one final time after any fixes. Run npm run typecheck. Both must exit 0.
