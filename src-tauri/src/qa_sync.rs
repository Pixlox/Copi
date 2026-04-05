#![cfg(debug_assertions)]

use anyhow::{anyhow, Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::TryLockError;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::{settings, sync, AppState};

const QA_COLLECTION_NAME: &str = "QA_SYNC_LOCAL_COLL";
const QA_COLLECTION_SYNC_ID: &str = "qa-sync-coll-v1";
const QA_CLIP_1: &str = "qa_pin_sync_clip_v1";
const QA_CLIP_2: &str = "qa_pin_sync_clip_v2";

#[derive(Debug, Deserialize)]
struct QaRequest {
    cmd: String,
    phase: Option<u8>,
    enabled: Option<bool>,
    suffix: Option<String>,
    name: Option<String>,
    color: Option<String>,
}

#[derive(Debug, Serialize)]
struct QaResponse {
    ok: bool,
    message: String,
    state: Option<QaState>,
}

#[derive(Debug, Serialize)]
struct QaState {
    device_id: String,
    sync_enabled: bool,
    metadata_enabled: bool,
    connected_peers: usize,
    collection: Option<QaCollectionState>,
    clips: Vec<QaClipState>,
    collections: Vec<QaCollectionState>,
}

#[derive(Debug, Serialize)]
struct QaCollectionState {
    id: i64,
    name: String,
    color: String,
    sync_id: String,
    deleted: bool,
    sync_version: i64,
    clip_count: i64,
}

#[derive(Debug, Serialize)]
struct QaClipState {
    content: String,
    id: Option<i64>,
    sync_id: Option<String>,
    content_hash: Option<String>,
    pinned: Option<bool>,
    collection_sync_id: Option<String>,
    in_collection: Option<bool>,
    sync_version: Option<i64>,
    source_device: Option<String>,
}

#[derive(Debug)]
struct SeedData {
    collection_id: i64,
    collection_sync_id: String,
    clip1_id: i64,
    clip2_id: i64,
    clip1_key: String,
    clip2_key: String,
}

pub fn start_server_if_enabled(app: AppHandle) {
    let Some(port) = std::env::var("COPI_SYNC_QA_PORT")
        .ok()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
    else {
        return;
    };

    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_server(app, port).await {
            eprintln!("[QA][sync] server stopped: {}", error);
        }
    });
}

async fn run_server(app: AppHandle, port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {}", addr))?;
    eprintln!("[QA][sync] listening on {}", addr);

    loop {
        let (stream, _) = listener.accept().await.context("accept")?;
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = handle_connection(app_clone, stream).await {
                eprintln!("[QA][sync] request failed: {}", error);
            }
        });
    }
}

async fn handle_connection(app: AppHandle, stream: TcpStream) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let read = reader.read_line(&mut line).await.context("read line")?;
    if read == 0 {
        return Ok(());
    }

    let request: QaRequest = serde_json::from_str(line.trim_end()).context("parse request")?;
    let cmd_name = request.cmd.clone();
    let started = Instant::now();
    eprintln!("[QA][sync] cmd={} start", cmd_name);
    let response = match dispatch(&app, request).await {
        Ok(response) => response,
        Err(error) => QaResponse {
            ok: false,
            message: error.to_string(),
            state: None,
        },
    };
    eprintln!(
        "[QA][sync] cmd={} done ok={} elapsed_ms={}",
        cmd_name,
        response.ok,
        started.elapsed().as_millis()
    );

    let mut encoded = serde_json::to_vec(&response).context("serialize response")?;
    encoded.push(b'\n');
    write_half.write_all(&encoded).await.context("write response")?;
    write_half.flush().await.context("flush response")?;
    Ok(())
}

fn lock_db_write<'a>(state: &'a AppState) -> Result<std::sync::MutexGuard<'a, rusqlite::Connection>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match state.db_write.try_lock() {
            Ok(conn) => return Ok(conn),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(anyhow!("qa db_write busy"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::Poisoned(error)) => {
                return Err(anyhow!("db lock poisoned: {}", error));
            }
        }
    }
}

async fn dispatch(app: &AppHandle, request: QaRequest) -> Result<QaResponse> {
    match request.cmd.as_str() {
        "ping" => Ok(QaResponse {
            ok: true,
            message: "pong".to_string(),
            state: Some(collect_state(app).await?),
        }),
        "state" => Ok(QaResponse {
            ok: true,
            message: "state".to_string(),
            state: Some(collect_state(app).await?),
        }),
        "set_metadata_sync" => {
            let enabled = request
                .enabled
                .ok_or_else(|| anyhow!("enabled is required"))?;
            set_metadata_sync(app.clone(), enabled)
                .await
                .map_err(|e| anyhow!(e))?;
            Ok(QaResponse {
                ok: true,
                message: format!("metadata sync set to {}", enabled),
                state: None,
            })
        }
        "seed" => {
            let seed = ensure_seed_data(app)?;
            enqueue_pushes(app, &seed);
            Ok(QaResponse {
                ok: true,
                message: "seed applied".to_string(),
                state: None,
            })
        }
        "phase" => {
            let phase = request.phase.ok_or_else(|| anyhow!("phase is required"))?;
            let seed = apply_phase(app, phase)?;
            enqueue_pushes(app, &seed);
            Ok(QaResponse {
                ok: true,
                message: format!("phase {} applied", phase),
                state: None,
            })
        }
        "delete_collection" => {
            let seed = delete_collection(app)?;
            enqueue_pushes(app, &seed);
            Ok(QaResponse {
                ok: true,
                message: "collection deleted".to_string(),
                state: None,
            })
        }
        "make_collection" => {
            let suffix = request
                .suffix
                .ok_or_else(|| anyhow!("suffix is required"))?;
            let name = request.name.unwrap_or_else(|| format!("QA_SYNC_{}", suffix));
            let color = request.color.unwrap_or_else(|| "#0A84FF".to_string());
            let sync_id = upsert_named_collection(app, &suffix, &name, &color)?;
            Ok(QaResponse {
                ok: true,
                message: format!("collection upserted: {}", sync_id),
                state: None,
            })
        }
        _ => Err(anyhow!("unknown command: {}", request.cmd)),
    }
}

fn qa_collection_sync_id(suffix: &str) -> String {
    format!("qa-sync-extra-{}", suffix)
}

fn upsert_named_collection(app: &AppHandle, suffix: &str, name: &str, color: &str) -> Result<String> {
    let sync_id = qa_collection_sync_id(suffix);
    let state = app.state::<AppState>();
    let conn = lock_db_write(&state)?;
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let origin_device_id: Option<String> = conn
        .query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| row.get(0))
        .optional()?;

    let existing_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE sync_id = ?1 LIMIT 1",
            [sync_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(id) = existing_id {
        conn.execute(
            "UPDATE collections
             SET name = ?1,
                 color = ?2,
                 deleted = 0,
                 sync_version = ?3,
                 origin_device_id = COALESCE(origin_device_id, ?4)
             WHERE id = ?5",
            rusqlite::params![name, color, sync_version, origin_device_id, id],
        )?;
    } else {
        conn.execute(
            "INSERT INTO collections(name, color, created_at, sync_id, sync_version, deleted, origin_device_id)
             VALUES(?1, ?2, ?3, ?4, ?5, 0, ?6)",
            rusqlite::params![name, color, now_ts(), sync_id, sync_version, origin_device_id],
        )?;
    }

    drop(conn);
    sync::on_collection_changed(app);
    let _ = app.emit("collections-changed", ());
    Ok(sync_id)
}

fn enqueue_pushes(app: &AppHandle, seed: &SeedData) {
    sync::on_collection_changed(app);

    let app_clip1 = app.clone();
    let clip1_key = seed.clip1_key.clone();
    tauri::async_runtime::spawn(async move {
        sync::on_local_clip_saved(&app_clip1, &clip1_key).await;
    });

    let app_clip2 = app.clone();
    let clip2_key = seed.clip2_key.clone();
    tauri::async_runtime::spawn(async move {
        sync::on_local_clip_saved(&app_clip2, &clip2_key).await;
    });
}

fn clip_push_key_by_id(conn: &rusqlite::Connection, clip_id: i64) -> Option<String> {
    conn.query_row(
        "SELECT COALESCE(NULLIF(sync_id, ''), content_hash)
         FROM clips
         WHERE id = ?1",
        [clip_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|key| !key.is_empty())
}

fn sha256_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn ensure_seed_data(app: &AppHandle) -> Result<SeedData> {
    let state = app.state::<AppState>();
    let conn = lock_db_write(&state)?;

    let our_device_id: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'sync_device_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    let collection = conn
        .query_row(
            "SELECT id, COALESCE(deleted, 0)
             FROM collections
             WHERE sync_id = ?1
             ORDER BY id DESC
             LIMIT 1",
            [QA_COLLECTION_SYNC_ID],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;

    let collection_id = if let Some((id, deleted)) = collection {
        if deleted != 0 {
            let sync_version = sync::next_sync_version_from_conn(&conn);
            conn.execute(
                "UPDATE collections
                 SET name = ?1,
                     color = '#0A84FF',
                     deleted = 0,
                     sync_version = ?2
                 WHERE id = ?3",
                rusqlite::params![QA_COLLECTION_NAME, sync_version, id],
            )?;
        } else {
            let _ = conn.execute(
                "UPDATE collections
                 SET name = ?1,
                     color = CASE
                         WHEN COALESCE(color, '') = '' THEN '#0A84FF'
                         ELSE color
                     END
                 WHERE id = ?2",
                rusqlite::params![QA_COLLECTION_NAME, id],
            );
        }
        id
    } else {
        let sync_version = sync::next_sync_version_from_conn(&conn);
        conn.execute(
            "INSERT INTO collections(name, color, created_at, sync_id, sync_version, deleted, origin_device_id)
             VALUES(?1, '#0A84FF', ?2, ?3, ?4, 0, ?5)",
            rusqlite::params![
                QA_COLLECTION_NAME,
                now_ts(),
                QA_COLLECTION_SYNC_ID,
                sync_version,
                our_device_id
            ],
        )?;
        conn.last_insert_rowid()
    };
    let collection_sync_id = QA_COLLECTION_SYNC_ID.to_string();

    let clip1_id = ensure_clip_seed(&conn, QA_CLIP_1, our_device_id.as_deref())?;
    let clip2_id = ensure_clip_seed(&conn, QA_CLIP_2, our_device_id.as_deref())?;

    let clip1_key = clip_push_key_by_id(&conn, clip1_id)
        .ok_or_else(|| anyhow!("missing sync push key for clip1"))?;
    let clip2_key = clip_push_key_by_id(&conn, clip2_id)
        .ok_or_else(|| anyhow!("missing sync push key for clip2"))?;

    Ok(SeedData {
        collection_id,
        collection_sync_id,
        clip1_id,
        clip2_id,
        clip1_key,
        clip2_key,
    })
}

fn ensure_clip_seed(
    conn: &rusqlite::Connection,
    text: &str,
    origin_device_id: Option<&str>,
) -> Result<i64> {
    let hash = sha256_text(text);
    let existing = conn
        .query_row(
            "SELECT id, COALESCE(sync_id, ''), COALESCE(deleted, 0)
             FROM clips
             WHERE content_hash = ?1
             LIMIT 1",
            [hash.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;

    if let Some((id, sync_id, deleted)) = existing {
        if sync_id.trim().is_empty() {
            conn.execute(
                "UPDATE clips SET sync_id = ?1 WHERE id = ?2",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id],
            )?;
        }
        if deleted != 0 {
            let sync_version = sync::next_sync_version_from_conn(conn);
            conn.execute(
                "UPDATE clips
                 SET deleted = 0,
                     content = ?1,
                     content_type = 'text',
                     source_app = 'qa-sync',
                     created_at = ?2,
                     sync_version = ?3
                 WHERE id = ?4",
                rusqlite::params![text, now_ts(), sync_version, id],
            )?;
        }
        return Ok(id);
    }

    let sync_version = sync::next_sync_version_from_conn(conn);
    conn.execute(
        "INSERT INTO clips(content, content_hash, content_type, source_app, created_at, pinned, collection_id, collection_sync_id, sync_id, sync_version, deleted, source_device, origin_device_id)
         VALUES(?1, ?2, 'text', 'qa-sync', ?3, 0, NULL, NULL, ?4, ?5, 0, '', ?6)",
        rusqlite::params![
            text,
            hash,
            now_ts(),
            uuid::Uuid::new_v4().to_string(),
            sync_version,
            origin_device_id
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn apply_phase(app: &AppHandle, phase: u8) -> Result<SeedData> {
    if phase != 1 && phase != 2 {
        return Err(anyhow!("phase must be 1 or 2"));
    }

    let seed = ensure_seed_data(app)?;

    let state = app.state::<AppState>();
    let conn = lock_db_write(&state)?;

    let (clip1_pinned, clip1_in_collection, clip2_pinned, clip2_in_collection) = if phase == 1 {
        (1_i64, true, 0_i64, true)
    } else {
        (0_i64, false, 1_i64, true)
    };

    let sync_version_clip1 = sync::next_sync_version_from_conn(&conn);
    conn.execute(
        "UPDATE clips
         SET pinned = ?1,
             collection_id = CASE WHEN ?2 = 1 THEN ?3 ELSE NULL END,
             collection_sync_id = CASE WHEN ?2 = 1 THEN ?4 ELSE NULL END,
             sync_version = ?5
         WHERE id = ?6 AND deleted = 0",
        rusqlite::params![
            clip1_pinned,
            if clip1_in_collection { 1 } else { 0 },
            seed.collection_id,
            seed.collection_sync_id,
            sync_version_clip1,
            seed.clip1_id
        ],
    )?;

    let sync_version_clip2 = sync::next_sync_version_from_conn(&conn);
    conn.execute(
        "UPDATE clips
         SET pinned = ?1,
             collection_id = CASE WHEN ?2 = 1 THEN ?3 ELSE NULL END,
             collection_sync_id = CASE WHEN ?2 = 1 THEN ?4 ELSE NULL END,
             sync_version = ?5
         WHERE id = ?6 AND deleted = 0",
        rusqlite::params![
            clip2_pinned,
            if clip2_in_collection { 1 } else { 0 },
            seed.collection_id,
            seed.collection_sync_id,
            sync_version_clip2,
            seed.clip2_id
        ],
    )?;

    Ok(seed)
}

fn delete_collection(app: &AppHandle) -> Result<SeedData> {
    let seed = ensure_seed_data(app)?;

    let state = app.state::<AppState>();
    let conn = lock_db_write(&state)?;

    let clip_sync_version = sync::next_sync_version_from_conn(&conn);
    conn.execute(
        "UPDATE clips
         SET collection_id = NULL,
             collection_sync_id = NULL,
             sync_version = ?1
         WHERE collection_id = ?2 AND deleted = 0",
        rusqlite::params![clip_sync_version, seed.collection_id],
    )?;

    let collection_sync_version = sync::next_sync_version_from_conn(&conn);
    conn.execute(
        "UPDATE collections
         SET deleted = 1,
             sync_version = ?1
         WHERE id = ?2",
        rusqlite::params![collection_sync_version, seed.collection_id],
    )?;

    Ok(seed)
}

async fn collect_state(app: &AppHandle) -> Result<QaState> {
    let config = settings::get_config_sync(app.clone()).map_err(|e| anyhow!(e))?;

    let sync_state = app
        .try_state::<AppState>()
        .and_then(|state| state.sync.get().cloned());

    let device_id = if let Some(sync) = sync_state.as_ref() {
        sync.device_id.clone()
    } else {
        let state = app.state::<AppState>();
        let conn = state.db_read_pool.get().context("db read pool")?;
        conn.query_row(
            "SELECT value FROM settings WHERE key = 'sync_device_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| "unknown".to_string())
    };

    let connected_peers = if let Some(sync) = sync_state {
        sync.connected_peers().await.len()
    } else {
        0
    };

    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().context("db read pool")?;

    let collection = conn
        .query_row(
            "SELECT c.id,
                    c.name,
                    COALESCE(c.color, '#0A84FF'),
                    COALESCE(c.sync_id, ''),
                    COALESCE(c.deleted, 0),
                    COALESCE(c.sync_version, 0),
                    (SELECT COUNT(*) FROM clips WHERE deleted = 0 AND collection_id = c.id)
             FROM collections c
             WHERE c.sync_id = ?1
             ORDER BY c.id DESC
             LIMIT 1",
            [QA_COLLECTION_SYNC_ID],
            |row| {
                Ok(QaCollectionState {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    sync_id: row.get(3)?,
                    deleted: row.get::<_, i64>(4)? != 0,
                    sync_version: row.get(5)?,
                    clip_count: row.get(6)?,
                })
            },
        )
        .optional()?;

    let mut collections = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT c.id,
                c.name,
                COALESCE(c.color, '#0A84FF'),
                COALESCE(c.sync_id, ''),
                COALESCE(c.deleted, 0),
                COALESCE(c.sync_version, 0),
                (SELECT COUNT(*) FROM clips WHERE deleted = 0 AND collection_id = c.id)
         FROM collections c
         WHERE c.sync_id IS NOT NULL
         ORDER BY c.id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(QaCollectionState {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
            sync_id: row.get(3)?,
            deleted: row.get::<_, i64>(4)? != 0,
            sync_version: row.get(5)?,
            clip_count: row.get(6)?,
        })
    })?;
    for row in rows {
        collections.push(row?);
    }

    let collection_is_active = collection.as_ref().map(|value| !value.deleted).unwrap_or(false);

    let mut clips = Vec::new();
    for content in [QA_CLIP_1, QA_CLIP_2] {
        let row = conn
            .query_row(
                "SELECT id,
                        COALESCE(sync_id, ''),
                        COALESCE(content_hash, ''),
                        COALESCE(pinned, 0),
                        COALESCE(collection_sync_id, ''),
                        COALESCE(sync_version, 0),
                        COALESCE(source_device, '')
                 FROM clips
                 WHERE content = ?1
                   AND deleted = 0
                 ORDER BY id DESC
                 LIMIT 1",
                [content],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;

        if let Some((id, sync_id, content_hash, pinned, clip_collection_sync_id, sync_version, source_device)) = row
        {
            let clip_collection_sync_id = if clip_collection_sync_id.is_empty() {
                None
            } else {
                Some(clip_collection_sync_id.clone())
            };
            clips.push(QaClipState {
                content: content.to_string(),
                id: Some(id),
                sync_id: if sync_id.is_empty() {
                    None
                } else {
                    Some(sync_id)
                },
                content_hash: if content_hash.is_empty() {
                    None
                } else {
                    Some(content_hash)
                },
                pinned: Some(pinned != 0),
                in_collection: Some(
                    collection_is_active
                        && clip_collection_sync_id.as_deref() == Some(QA_COLLECTION_SYNC_ID),
                ),
                collection_sync_id: clip_collection_sync_id,
                sync_version: Some(sync_version),
                source_device: if source_device.is_empty() {
                    None
                } else {
                    Some(source_device)
                },
            });
        } else {
            clips.push(QaClipState {
                content: content.to_string(),
                id: None,
                sync_id: None,
                content_hash: None,
                pinned: None,
                in_collection: None,
                collection_sync_id: None,
                sync_version: None,
                source_device: None,
            });
        }
    }

    Ok(QaState {
        device_id,
        sync_enabled: config.sync.enabled,
        metadata_enabled: config.sync.enabled && config.sync.sync_collections_and_pins,
        connected_peers,
        collection,
        clips,
        collections,
    })
}

async fn set_metadata_sync(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut config = settings::get_config_sync(app.clone())?;
    config.sync.enabled = true;
    config.sync.sync_collections_and_pins = enabled;
    settings::set_config(app, config).await
}
