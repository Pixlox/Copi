use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::AppState;

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
    conn.execute(
        "INSERT INTO collections (name, color, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![name, color, now],
    )
    .map_err(|e| e.to_string())?;

    let id = conn.last_insert_rowid();
    drop(conn);
    let _ = app.emit("collections-changed", ());
    Ok(id)
}

#[tauri::command]
pub fn delete_collection(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;

    // Move clips to no collection
    conn.execute(
        "UPDATE clips SET collection_id = NULL WHERE collection_id = ?",
        [id],
    )
    .map_err(|e| e.to_string())?;

    conn.execute("DELETE FROM collections WHERE id = ?", [id])
        .map_err(|e| e.to_string())?;

    drop(conn);
    let _ = app.emit("collections-changed", ());
    Ok(())
}

#[tauri::command]
pub fn rename_collection(app: tauri::AppHandle, id: i64, name: String) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE collections SET name = ?1 WHERE id = ?2",
        rusqlite::params![name, id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
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
                    (SELECT COUNT(*) FROM clips WHERE collection_id = c.id) as clip_count
             FROM collections c ORDER BY c.name",
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
    conn.execute(
        "UPDATE collections SET color = ?1 WHERE id = ?2",
        rusqlite::params![color, id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
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
    conn.execute(
        "UPDATE clips SET collection_id = ?1 WHERE id = ?2",
        rusqlite::params![collection_id, clip_id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    let _ = app.emit("clips-changed", ());
    let _ = app.emit("collections-changed", ());
    Ok(())
}
