use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::sync;
use crate::AppState;

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

#[derive(Debug, Clone, Serialize)]
pub struct CollectionInfo {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub clip_count: i64,
    pub created_at: i64,
}

#[tauri::command]
pub fn create_collection(
    app: tauri::AppHandle,
    name: String,
    color: String,
) -> Result<i64, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let sync_id = uuid::Uuid::new_v4().to_string();
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let origin_device_id: Option<String> = conn
        .query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();
    conn.execute(
        "INSERT INTO collections (name, color, created_at, sync_id, sync_version, deleted, origin_device_id) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
        rusqlite::params![name, color, now, sync_id, sync_version, origin_device_id],
    )
    .map_err(|e| e.to_string())?;

    let id = conn.last_insert_rowid();
    drop(conn);
    sync::on_collection_changed(&app);
    let _ = app.emit("collections-changed", ());
    Ok(id)
}

#[tauri::command]
pub fn delete_collection(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;

    let sync_version = sync::next_sync_version_from_conn(&conn);

    let clip_sync_ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM clips WHERE collection_id = ?1 AND deleted = 0")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([id], |row| row.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows
            .filter_map(|r| r.ok())
            .filter_map(|clip_id| clip_push_key_by_id(&conn, clip_id))
            .collect()
    };

    conn.execute(
        "UPDATE clips SET collection_id = NULL, collection_sync_id = NULL, sync_version = ?1 WHERE collection_id = ?2 AND deleted = 0",
        rusqlite::params![sync_version, id],
    )
    .map_err(|e| e.to_string())?;

    let updated = conn
        .execute(
            "UPDATE collections SET deleted = 1, sync_version = ?1 WHERE id = ?2 AND deleted = 0",
            rusqlite::params![sync_version, id],
        )
        .map_err(|e| e.to_string())?;

    drop(conn);
    if updated > 0 {
        sync::on_collection_changed(&app);
    }
    for sync_id in clip_sync_ids {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            sync::on_local_clip_saved(&app_clone, &sync_id).await;
        });
    }
    let _ = app.emit("collections-changed", ());
    Ok(())
}

#[tauri::command]
pub fn rename_collection(app: tauri::AppHandle, id: i64, name: String) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let updated = conn
        .execute(
            "UPDATE collections SET name = ?1, sync_version = ?2 WHERE id = ?3 AND deleted = 0",
            rusqlite::params![name, sync_version, id],
        )
        .map_err(|e| e.to_string())?;

    drop(conn);
    if updated > 0 {
        sync::on_collection_changed(&app);
    }
    let _ = app.emit("collections-changed", ());
    Ok(())
}

#[tauri::command]
pub fn list_collections(app: tauri::AppHandle) -> Result<Vec<CollectionInfo>, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, COALESCE(c.color, '#0A84FF'), c.created_at,
                    (SELECT COUNT(*) FROM clips WHERE collection_id = c.id AND deleted = 0) as clip_count
             FROM collections c WHERE c.deleted = 0 ORDER BY c.name",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(CollectionInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                created_at: row.get(3)?,
                clip_count: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

#[tauri::command]
pub fn update_collection_color(
    app: tauri::AppHandle,
    id: i64,
    color: String,
) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let updated = conn
        .execute(
            "UPDATE collections SET color = ?1, sync_version = ?2 WHERE id = ?3 AND deleted = 0",
            rusqlite::params![color, sync_version, id],
        )
        .map_err(|e| e.to_string())?;

    drop(conn);
    if updated > 0 {
        sync::on_collection_changed(&app);
    }
    let _ = app.emit("collections-changed", ());
    Ok(())
}

#[tauri::command]
pub fn move_clip_to_collection(
    app: tauri::AppHandle,
    clip_id: i64,
    collection_id: Option<i64>,
) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let updated = conn
        .execute(
            "UPDATE clips
             SET collection_id = ?1,
                 collection_sync_id = CASE
                     WHEN ?1 IS NULL THEN NULL
                     ELSE (SELECT sync_id FROM collections WHERE id = ?1 AND deleted = 0)
                 END,
                 sync_version = ?2
             WHERE id = ?3 AND deleted = 0",
            rusqlite::params![collection_id, sync_version, clip_id],
        )
        .map_err(|e| e.to_string())?;

    let clip_sync_id: Option<String> = if updated > 0 {
        clip_push_key_by_id(&conn, clip_id)
    } else {
        None
    };

    drop(conn);
    if updated > 0 {
        sync::on_collection_changed(&app);
    }
    if let Some(sync_id) = clip_sync_id {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            sync::on_local_clip_saved(&app_clone, &sync_id).await;
        });
    }
    let _ = app.emit("clips-changed", ());
    let _ = app.emit("collections-changed", ());
    Ok(())
}
