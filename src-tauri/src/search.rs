use crate::query_parser::{self, Ordering, ParsedQuery};
use crate::sync;
use crate::AppState;
use rusqlite::{OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::Ordering as AtomicOrdering;
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchStatusPayload {
    pub phase: String,
    pub queued_items: usize,
    pub completed_items: usize,
    pub failed_items: usize,
    pub total_items: usize,
    pub semantic_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipResult {
    pub id: i64,
    pub content: String,
    pub content_type: String,
    pub source_app: String,
    pub created_at: i64,
    pub pinned: bool,
    pub content_highlighted: Option<String>,
    pub ocr_text: Option<String>,
    pub copy_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchUpdatedPayload {
    query: String,
    filter: String,
    collection_id: Option<i64>,
    results: Vec<ClipResult>,
}

#[derive(Debug, Clone, Copy)]
enum SearchPhase {
    Fast,
    Semantic,
}

#[derive(Default, Clone)]
struct ScoreEntry {
    score: f64,
}

const SEL: &str = "id, content, content_type, source_app, created_at, pinned, content_highlighted, ocr_text, COALESCE(copy_count, 0)";

fn row_to_clip(r: &rusqlite::Row) -> rusqlite::Result<ClipResult> {
    Ok(ClipResult {
        id: r.get(0)?,
        content: trunc(&r.get::<_, String>(1).unwrap_or_default()),
        content_type: r.get(2)?,
        source_app: r.get(3)?,
        created_at: r.get(4)?,
        pinned: r.get::<_, i64>(5)? != 0,
        content_highlighted: r.get(6)?,
        ocr_text: r.get(7).unwrap_or(None),
        copy_count: r.get(8).unwrap_or(0),
    })
}

#[tauri::command]
pub async fn search_clips(
    app: tauri::AppHandle,
    query: String,
    filter: String,
    collection_id: Option<i64>,
) -> Result<Vec<ClipResult>, String> {
    let token = match app.try_state::<AppState>() {
        Some(state) => {
            state
                .search_generation
                .fetch_add(1, AtomicOrdering::Relaxed)
                + 1
        }
        None => return Ok(Vec::new()),
    };

    let fast_results = {
        let app_handle = app.clone();
        let query_clone = query.clone();
        let filter_clone = filter.clone();
        tokio::task::spawn_blocking(move || {
            search_sync(
                &app_handle,
                &query_clone,
                &filter_clone,
                collection_id,
                SearchPhase::Fast,
            )
        })
        .await
        .map_err(|e| e.to_string())??
    };

    schedule_semantic_update(&app, token, query.clone(), filter.clone(), collection_id);

    Ok(fast_results)
}

#[tauri::command]
pub async fn get_total_clip_count(app: tauri::AppHandle) -> Result<i64, String> {
    tokio::task::spawn_blocking(move || {
        let state = app
            .try_state::<AppState>()
            .ok_or_else(|| "App state not ready yet".to_string())?;
        let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
        conn.query_row("SELECT COUNT(*) FROM clips WHERE deleted = 0", [], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_search_status(app: tauri::AppHandle) -> Result<SearchStatusPayload, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let status = state.search_status.lock().map_err(|e| e.to_string())?;
    Ok(status.clone())
}

#[tauri::command]
pub async fn toggle_pin(app: tauri::AppHandle, clip_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let updated = conn.execute(
        "UPDATE clips SET pinned = CASE WHEN pinned = 1 THEN 0 ELSE 1 END, sync_version = ?1 WHERE id = ?2 AND deleted = 0",
        rusqlite::params![sync::next_sync_version(&app), clip_id],
    )
    .map_err(|e| e.to_string())?;
    let mut sync_key: Option<String> = None;
    if updated > 0 {
        if let Ok(sync_id) =
            conn.query_row("SELECT sync_id FROM clips WHERE id = ?1", [clip_id], |r| {
                r.get::<_, String>(0)
            })
        {
            sync_key = Some(sync_id);
        }
    }
    drop(conn);
    if let Some(sync_key) = sync_key {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            sync::on_local_clip_saved(&app_clone, &sync_key).await;
        });
    }
    let _ = app.emit("clips-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_clip(app: tauri::AppHandle, clip_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let sync_version = sync::next_sync_version(&app);
    conn.execute("DELETE FROM clip_embeddings WHERE rowid = ?", [clip_id])
        .ok();
    let updated = conn
        .execute(
            "UPDATE clips SET deleted = 1, sync_version = ?1 WHERE id = ?2 AND deleted = 0",
            rusqlite::params![sync_version, clip_id],
        )
        .map_err(|e| e.to_string())?;
    let mut sync_key: Option<String> = None;
    if updated > 0 {
        if let Ok(sync_id) =
            conn.query_row("SELECT sync_id FROM clips WHERE id = ?1", [clip_id], |r| {
                r.get::<_, String>(0)
            })
        {
            sync_key = Some(sync_id);
        }
    }
    drop(conn);
    if let Some(sync_key) = sync_key {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            sync::on_local_clip_saved(&app_clone, &sync_key).await;
        });
    }
    let _ = app.emit("clips-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn update_clip_content(
    app: tauri::AppHandle,
    clip_id: i64,
    new_content: String,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(new_content.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let content_type = if new_content.starts_with("http") {
        "url"
    } else if new_content.contains('\n')
        && (new_content.contains('{')
            || new_content.contains("fn ")
            || new_content.contains("function "))
    {
        "code"
    } else {
        "text"
    };

    let detected_language = query_parser::detect_language(&new_content).map(str::to_string);
    let state = app.state::<AppState>();
    let conn = state.db_write.lock().map_err(|e| e.to_string())?;
    let sync_version = sync::next_sync_version(&app);
    let updated = conn.execute(
        "UPDATE clips
         SET content = ?1, content_hash = ?2, content_type = ?3, language = COALESCE(?5, language), sync_version = ?6
         WHERE id = ?4 AND deleted = 0",
        rusqlite::params![
            new_content,
            hash,
            content_type,
            clip_id,
            detected_language,
            sync_version
        ],
    )
    .map_err(|e| e.to_string())?;
    let mut sync_key: Option<String> = None;
    if updated > 0 {
        if let Ok(sync_id) =
            conn.query_row("SELECT sync_id FROM clips WHERE id = ?1", [clip_id], |r| {
                r.get::<_, String>(0)
            })
        {
            sync_key = Some(sync_id);
        }
    }
    drop(conn);
    if let Some(sync_key) = sync_key {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            sync::on_local_clip_saved(&app_clone, &sync_key).await;
        });
    }
    let tx = state.clip_tx.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tx.send(clip_id).await {
            eprintln!(
                "[EmbedQueue] Failed to enqueue edited clip {}: {}",
                clip_id, e
            );
        }
    });
    let _ = app.emit("clips-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn get_image_thumbnail(
    app: tauri::AppHandle,
    clip_id: i64,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    let result: Option<(Vec<u8>, Vec<u8>, i64, i64)> = conn
        .query_row(
            "SELECT COALESCE(image_thumbnail, X''), image_data, image_width, image_height
             FROM clips
             WHERE id = ? AND content_type = 'image' AND deleted = 0",
            [clip_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match result {
        Some((thumb, _, _, _)) if !thumb.is_empty() => Ok(Some(b64(&thumb))),
        Some((_, raw, width, height)) if !raw.is_empty() => {
            drop(conn);
            let (rgba, w, h) =
                decode_stored_image(&raw, width, height).ok_or("Failed to decode image")?;
            Ok(gen_thumb(&rgba, w, h, 64).map(|data| b64(&data)))
        }
        _ => Ok(None),
    }
}

#[tauri::command]
pub async fn get_image_preview(
    app: tauri::AppHandle,
    clip_id: i64,
    max_size: u32,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    let result: Option<(Vec<u8>, i64, i64)> = conn
        .query_row(
            "SELECT image_data, image_width, image_height
             FROM clips
             WHERE id = ? AND content_type = 'image' AND deleted = 0",
            [clip_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match result {
        Some((raw, width, height)) if !raw.is_empty() => {
            drop(conn);
            let (rgba, w, h) =
                decode_stored_image(&raw, width, height).ok_or("Failed to decode image")?;
            Ok(gen_thumb(&rgba, w, h, max_size).map(|data| b64(&data)))
        }
        _ => Ok(None),
    }
}

#[tauri::command]
pub async fn get_clip_full_content(app: tauri::AppHandle, clip_id: i64) -> Result<String, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT content FROM clips WHERE id = ? AND deleted = 0",
        [clip_id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

fn schedule_semantic_update(
    app: &tauri::AppHandle,
    token: u64,
    query: String,
    filter: String,
    collection_id: Option<i64>,
) {
    let has_model = app
        .try_state::<AppState>()
        .and_then(|state| {
            state
                .model
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().cloned())
        })
        .is_some();
    if !has_model {
        return;
    }
    if query.trim().len() < 2 {
        return;
    }
    let parsed = query_parser::parse_query(&query);
    if parsed.query_is_empty_after_parse {
        return;
    }
    if parsed.semantic.trim().is_empty() {
        return;
    }
    // Allow semantic search if we have meaningful text OR temporal/source/type filters
    let has_filters =
        parsed.has_temporal || !parsed.source_apps.is_empty() || parsed.content_type.is_some();
    if parsed.semantic.trim().len() < 3 && !has_filters {
        return;
    }
    if parsed.keywords.is_empty() && !has_filters && parsed.semantic.trim().len() < 3 {
        return;
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let state = app_handle.state::<AppState>();
        if state.search_generation.load(AtomicOrdering::Relaxed) != token {
            return;
        }

        let results = tokio::task::spawn_blocking({
            let app_handle = app_handle.clone();
            let query = query.clone();
            let filter = filter.clone();
            move || {
                search_sync(
                    &app_handle,
                    &query,
                    &filter,
                    collection_id,
                    SearchPhase::Semantic,
                )
            }
        })
        .await;

        let Ok(Ok(results)) = results else {
            return;
        };

        let state = app_handle.state::<AppState>();
        if state.search_generation.load(AtomicOrdering::Relaxed) != token {
            return;
        }

        let payload = SearchUpdatedPayload {
            query,
            filter,
            collection_id,
            results,
        };
        let _ = app_handle.emit("search-updated", payload);
    });
}

fn search_sync(
    app: &tauri::AppHandle,
    query: &str,
    filter: &str,
    collection_id: Option<i64>,
    phase: SearchPhase,
) -> Result<Vec<ClipResult>, String> {
    let state = app.state::<AppState>();
    let conn = state.db_read_pool.get().map_err(|e| e.to_string())?;

    if query.trim().is_empty() {
        return do_empty_search(&conn, filter, collection_id);
    }

    let parsed = query_parser::parse_query(query);

    if let Some(ordering) = parsed.ordering.as_ref() {
        return do_ordering(&conn, ordering, filter, collection_id);
    }

    if parsed.query_is_empty_after_parse || parsed.semantic.is_empty() {
        return do_filter_search(&conn, &parsed, filter, collection_id);
    }

    let (semantic_results, semantic_results_relaxed) = match phase {
        SearchPhase::Fast => (None, None),
        SearchPhase::Semantic => {
            let model = state
                .model
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().cloned());
            if let Some(model) = model.as_ref() {
                let strict =
                    do_vector_candidates(&conn, model, &parsed, filter, collection_id, true);
                let relaxed = if parsed.has_temporal {
                    Some(do_vector_candidates(
                        &conn,
                        model,
                        &parsed,
                        filter,
                        collection_id,
                        false,
                    ))
                } else {
                    None
                };
                (Some(strict), relaxed)
            } else {
                (None, None)
            }
        }
    };

    do_ranked_search(
        &conn,
        &parsed,
        filter,
        collection_id,
        semantic_results.as_deref(),
        semantic_results_relaxed.as_deref(),
    )
}

fn do_empty_search(
    conn: &rusqlite::Connection,
    filter: &str,
    collection_id: Option<i64>,
) -> Result<Vec<ClipResult>, String> {
    let mut conditions = vec!["deleted = 0".to_string()];
    let mut params: Vec<Box<dyn ToSql>> = vec![];

    match filter {
        "all" => {}
        "pinned" => conditions.push("pinned = 1".to_string()),
        value => {
            conditions.push("content_type = ?".to_string());
            params.push(Box::new(value.to_string()));
        }
    }

    if let Some(col_id) = collection_id {
        conditions.push("collection_id = ?".to_string());
        params.push(Box::new(col_id));
    }

    let sql = format!(
        "SELECT {SEL} FROM clips WHERE {} ORDER BY created_at DESC LIMIT 50",
        conditions.join(" AND ")
    );
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|param| param.as_ref()).collect();
    query_rows(conn, &sql, &param_refs)
}

fn do_filter_search(
    conn: &rusqlite::Connection,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
) -> Result<Vec<ClipResult>, String> {
    let strict = run_filter_search(conn, parsed, filter, collection_id, true)?;
    if !strict.is_empty() || !parsed.has_temporal {
        return Ok(strict);
    }

    run_filter_search(conn, parsed, filter, collection_id, false)
}

fn run_filter_search(
    conn: &rusqlite::Connection,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
    include_temporal: bool,
) -> Result<Vec<ClipResult>, String> {
    let mut conditions = vec!["deleted = 0".to_string()];
    let mut params: Vec<Box<dyn ToSql>> = vec![];
    apply_filters(
        &mut conditions,
        &mut params,
        parsed,
        filter,
        collection_id,
        "",
        include_temporal,
    );
    if !parsed.languages.is_empty() {
        let placeholders = (0..parsed.languages.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        conditions.push(format!("language IN ({placeholders})"));
        for language in &parsed.languages {
            params.push(Box::new(language.clone()));
        }
    }

    let sql = if include_temporal {
        format!(
            "SELECT {SEL} FROM clips WHERE {} ORDER BY pinned DESC, COALESCE(copy_count, 0) DESC, created_at DESC LIMIT 50",
            conditions.join(" AND ")
        )
    } else if let Some(center) = temporal_center(parsed) {
        params.push(Box::new(center));
        format!(
            "SELECT {SEL} FROM clips WHERE {} ORDER BY pinned DESC, ABS(created_at - ?) ASC, COALESCE(copy_count, 0) DESC, created_at DESC LIMIT 50",
            conditions.join(" AND ")
        )
    } else {
        format!(
            "SELECT {SEL} FROM clips WHERE {} ORDER BY pinned DESC, COALESCE(copy_count, 0) DESC, created_at DESC LIMIT 50",
            conditions.join(" AND ")
        )
    };
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|param| param.as_ref()).collect();
    query_rows(conn, &sql, &param_refs)
}

fn do_ordering(
    conn: &rusqlite::Connection,
    ordering: &Ordering,
    filter: &str,
    collection_id: Option<i64>,
) -> Result<Vec<ClipResult>, String> {
    let order = match ordering {
        Ordering::Newest | Ordering::SecondNewest => "created_at DESC",
        Ordering::Oldest => "created_at ASC",
    };
    let limit = if *ordering == Ordering::SecondNewest {
        2
    } else {
        1
    };
    let mut conditions = vec!["deleted = 0".to_string()];
    let mut params: Vec<Box<dyn ToSql>> = vec![];

    match filter {
        "all" => {}
        "pinned" => conditions.push("pinned = 1".to_string()),
        value => {
            conditions.push("content_type = ?".to_string());
            params.push(Box::new(value.to_string()));
        }
    }
    if let Some(col_id) = collection_id {
        conditions.push("collection_id = ?".to_string());
        params.push(Box::new(col_id));
    }

    let sql = format!(
        "SELECT {SEL} FROM clips WHERE {} ORDER BY {order} LIMIT {limit}",
        conditions.join(" AND ")
    );
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|param| param.as_ref()).collect();
    let mut results = query_rows(conn, &sql, &param_refs)?;
    if *ordering == Ordering::SecondNewest && results.len() == 2 {
        return Ok(vec![results.remove(1)]);
    }
    Ok(results)
}

fn do_ranked_search(
    conn: &rusqlite::Connection,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
    semantic_candidates: Option<&[(i64, f64)]>,
    semantic_candidates_relaxed: Option<&[(i64, f64)]>,
) -> Result<Vec<ClipResult>, String> {
    let (mut scores, mut temporal_relaxed) = collect_ranked_scores(
        conn,
        parsed,
        filter,
        collection_id,
        semantic_candidates,
        true,
    )?;

    if scores.is_empty() && parsed.has_temporal {
        let fallback_semantic = semantic_candidates_relaxed.or(semantic_candidates);
        let (fallback_scores, _) = collect_ranked_scores(
            conn,
            parsed,
            filter,
            collection_id,
            fallback_semantic,
            false,
        )?;
        scores = fallback_scores;
        temporal_relaxed = true;
    }

    if scores.is_empty() {
        return Ok(vec![]);
    }

    let initial_ids = sort_scored_ids(scores.clone(), 80);
    let clips = fetch_clips_by_ids(conn, &initial_ids)?;
    apply_clip_boosts(&mut scores, &clips, parsed, temporal_relaxed);

    let ids = sort_scored_ids(scores, 50);
    fetch_clips_by_ids(conn, &ids)
}

fn do_fts_candidates(
    conn: &rusqlite::Connection,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
    include_temporal: bool,
) -> Result<Vec<(i64, f64)>, String> {
    let query = fts_query(parsed);
    if query.is_empty() {
        return Ok(vec![]);
    }

    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(query)];
    let mut conditions = vec!["clips_fts MATCH ?".to_string(), "c.deleted = 0".to_string()];
    apply_filters(
        &mut conditions,
        &mut params,
        parsed,
        filter,
        collection_id,
        "c.",
        include_temporal,
    );
    let sql = format!(
        "SELECT c.id, -bm25(clips_fts, 1.0, 1.3) AS score
         FROM clips_fts
         JOIN clips c ON c.id = clips_fts.rowid
         WHERE {}
         ORDER BY score DESC, c.pinned DESC, COALESCE(c.copy_count, 0) DESC, c.created_at DESC
         LIMIT 60",
        conditions.join(" AND ")
    );
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|param| param.as_ref()).collect();
    query_id_scores(conn, &sql, &param_refs)
}

fn do_vector_candidates(
    conn: &rusqlite::Connection,
    model: &std::sync::Arc<crate::embed::EmbeddingModel>,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
    include_temporal: bool,
) -> Vec<(i64, f64)> {
    let semantic_query = build_semantic_query_text(parsed);
    let Ok(embedding) = crate::embed::embed_query(model, &semantic_query) else {
        return vec![];
    };
    if embedding.len() != model.dimensions {
        return vec![];
    }

    let bytes: Vec<u8> = embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(bytes), Box::new(60_i64)];
    let mut conditions = vec![
        "e.embedding MATCH ?".to_string(),
        "e.k = ?".to_string(),
        "c.deleted = 0".to_string(),
    ];
    apply_filters(
        &mut conditions,
        &mut params,
        parsed,
        filter,
        collection_id,
        "c.",
        include_temporal,
    );

    let sql = format!(
        "SELECT c.id, e.distance
         FROM clip_embeddings e
         JOIN clips c ON c.id = e.rowid
         WHERE {}
         ORDER BY e.distance ASC
         LIMIT 60",
        conditions.join(" AND ")
    );
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|param| param.as_ref()).collect();
    query_id_scores(conn, &sql, &param_refs).unwrap_or_default()
}

fn build_semantic_query_text(parsed: &ParsedQuery) -> String {
    let mut text = parsed.semantic.clone();
    if let Some(content_type) = parsed.content_type.as_deref() {
        text.push(' ');
        text.push_str(content_type);
    }
    for app in &parsed.source_apps {
        text.push(' ');
        text.push_str(app);
    }
    for lang in &parsed.languages {
        text.push(' ');
        text.push_str(lang);
    }
    text.trim().to_string()
}

fn apply_filters(
    conditions: &mut Vec<String>,
    params: &mut Vec<Box<dyn ToSql>>,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
    prefix: &str,
    include_temporal: bool,
) {
    let content_type = parsed
        .content_type
        .as_ref()
        .cloned()
        .or_else(|| match filter {
            "all" | "pinned" => None,
            other => Some(other.to_string()),
        });
    if let Some(content_type) = content_type {
        conditions.push(format!("{prefix}content_type = ?"));
        params.push(Box::new(content_type));
    }

    if parsed.is_pinned == Some(true) || filter == "pinned" {
        conditions.push(format!("{prefix}pinned = 1"));
    }

    if include_temporal {
        if let Some(after) = parsed.temporal_after {
            conditions.push(format!("{prefix}created_at >= ?"));
            params.push(Box::new(after));
        }
        if let Some(before) = parsed.temporal_before {
            conditions.push(format!("{prefix}created_at <= ?"));
            params.push(Box::new(before));
        }
    }

    if !parsed.source_apps.is_empty() {
        let app_conditions: Vec<String> = parsed
            .source_apps
            .iter()
            .map(|_| format!("LOWER({prefix}source_app) LIKE ?"))
            .collect();
        conditions.push(format!("({})", app_conditions.join(" OR ")));
        for app in &parsed.source_apps {
            params.push(Box::new(format!("%{}%", app.to_lowercase())));
        }
    }

    if let Some(min_length) = parsed.min_length {
        conditions.push(format!(
            "LENGTH(CASE WHEN {prefix}content_type = 'image' THEN COALESCE({prefix}ocr_text, '') ELSE {prefix}content END) >= ?"
        ));
        params.push(Box::new(min_length as i64));
    }

    if parsed.is_multiline == Some(true) {
        conditions.push(format!(
            "(INSTR({prefix}content, CHAR(10)) > 0 OR INSTR(COALESCE({prefix}ocr_text, ''), CHAR(10)) > 0)"
        ));
    }

    if let Some(collection_id) = collection_id {
        conditions.push(format!("{prefix}collection_id = ?"));
        params.push(Box::new(collection_id));
    }
}

fn add_ranked_scores(scores: &mut HashMap<i64, ScoreEntry>, ranked: &[(i64, f64)], weight: f64) {
    for (idx, (id, raw_score)) in ranked.iter().enumerate() {
        let rank_score = weight / (idx as f64 + 8.0);
        let normalized = weight * raw_score.max(0.0) * 0.04;
        scores.entry(*id).or_default().score += rank_score + normalized;
    }
}

fn add_distance_scores(scores: &mut HashMap<i64, ScoreEntry>, ranked: &[(i64, f64)], weight: f64) {
    let Some((_, best_distance)) = ranked.first() else {
        return;
    };
    let best_distance = *best_distance;
    for (idx, (id, distance)) in ranked.iter().enumerate() {
        let rank_score = weight / (idx as f64 + 10.0);
        let distance_score = weight * (1.0 / (1.0 + (distance - best_distance).max(0.0)));
        scores.entry(*id).or_default().score += rank_score + distance_score;
    }
}

fn sort_scored_ids(scores: HashMap<i64, ScoreEntry>, limit: usize) -> Vec<i64> {
    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .score
            .partial_cmp(&left.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.0.cmp(&left.0))
    });
    ranked.into_iter().take(limit).map(|(id, _)| id).collect()
}

fn query_rows(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<ClipResult>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, row_to_clip)
        .map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

fn query_id_scores(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<(i64, f64)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for row in rows {
        let (id, score) = row.map_err(|e| e.to_string())?;
        results.push((id, score));
    }
    Ok(results)
}

fn fetch_clips_by_ids(conn: &rusqlite::Connection, ids: &[i64]) -> Result<Vec<ClipResult>, String> {
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!("SELECT {SEL} FROM clips WHERE deleted = 0 AND id IN ({placeholders})");
    let params: Vec<&dyn ToSql> = ids.iter().map(|id| id as &dyn ToSql).collect();
    let rows = query_rows(conn, &sql, &params)?;
    let mut by_id: HashMap<i64, ClipResult> =
        rows.into_iter().map(|clip| (clip.id, clip)).collect();

    let mut ordered = Vec::new();
    for id in ids {
        if let Some(clip) = by_id.remove(id) {
            ordered.push(clip);
        }
    }
    Ok(ordered)
}

fn fts_query(parsed: &ParsedQuery) -> String {
    let mut tokens = if !parsed.keywords.is_empty() {
        parsed.keywords.clone()
    } else {
        parsed
            .semantic
            .split_whitespace()
            .map(|part| part.to_lowercase())
            .collect()
    };
    if tokens.is_empty() {
        return String::new();
    }
    for token in &mut tokens {
        token.retain(|c| c.is_alphanumeric() || c == '_' || c == '-');
    }
    tokens.retain(|token| !token.is_empty());
    if tokens.is_empty() {
        return String::new();
    }
    tokens.truncate(12);

    let mut query_parts = Vec::new();
    if parsed.semantic.contains(' ') {
        let phrase = parsed
            .semantic
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '_' || *c == '-')
            .collect::<String>();
        let phrase = phrase.trim();
        if !phrase.is_empty() {
            query_parts.push(format!("\"{phrase}\""));
        }
    }
    for token in tokens {
        query_parts.push(format!("{token}*"));
    }
    query_parts.join(" OR ")
}

fn collect_ranked_scores(
    conn: &rusqlite::Connection,
    parsed: &ParsedQuery,
    filter: &str,
    collection_id: Option<i64>,
    semantic_candidates: Option<&[(i64, f64)]>,
    include_temporal: bool,
) -> Result<(HashMap<i64, ScoreEntry>, bool), String> {
    let fts = do_fts_candidates(conn, parsed, filter, collection_id, include_temporal)?;

    let mut scores: HashMap<i64, ScoreEntry> = HashMap::new();
    add_ranked_scores(&mut scores, &fts, 4.4);
    if let Some(vec_candidates) = semantic_candidates {
        add_distance_scores(&mut scores, vec_candidates, 5.0);
    }

    Ok((scores, !include_temporal))
}

fn apply_clip_boosts(
    scores: &mut HashMap<i64, ScoreEntry>,
    clips: &[ClipResult],
    parsed: &ParsedQuery,
    temporal_relaxed: bool,
) {
    let now = chrono::Local::now().timestamp();
    let semantic_lower = parsed.semantic.to_lowercase();
    let temporal_center = temporal_center(parsed);

    // Query length factor: shorter queries lean more on semantic
    let query_word_count = parsed.semantic.split_whitespace().count();
    let semantic_boost_factor: f64 = if query_word_count <= 2 { 1.5 } else { 1.0 };

    for clip in clips {
        let text = format!(
            "{} {} {}",
            clip.content.to_lowercase(),
            clip.ocr_text.as_deref().unwrap_or("").to_lowercase(),
            clip.source_app.to_lowercase()
        );
        let entry = scores.entry(clip.id).or_default();
        let mut keyword_hits = 0.0;

        for keyword in parsed.keywords.iter().take(16) {
            if text.contains(keyword) {
                keyword_hits += 1.0;
                entry.score += 0.7;
            }
        }

        if !parsed.keywords.is_empty() {
            entry.score += (keyword_hits / parsed.keywords.len() as f64) * 3.2;
        }

        if !semantic_lower.is_empty() && text.contains(&semantic_lower) {
            entry.score += 2.8 * semantic_boost_factor;
        }

        if let Some(content_type) = parsed.content_type.as_deref() {
            if clip.content_type == content_type {
                entry.score += 1.8;
            }
        }

        if !parsed.source_apps.is_empty()
            && parsed
                .source_apps
                .iter()
                .any(|app| clip.source_app.to_lowercase().contains(&app.to_lowercase()))
        {
            entry.score += 1.6;
        }

        if parsed.content_type.as_deref() == Some("image")
            && clip
                .ocr_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        {
            entry.score += 0.5;
        }

        if clip.pinned {
            entry.score += 0.18;
        }
        entry.score += ((clip.copy_count as f64) + 1.0).ln() * 0.18;

        // Soft recency decay: Gaussian with 7-day half-life
        let age_days = (now - clip.created_at).max(0) as f64 / 86400.0;
        entry.score += 2.0 * (-0.693 * age_days / 7.0).exp();

        if temporal_relaxed {
            if let Some(center) = temporal_center {
                let distance = (clip.created_at - center).abs() as f64;
                entry.score += 2.4 / (1.0 + distance / 86_400.0);
            }
        }
    }
}

fn temporal_center(parsed: &ParsedQuery) -> Option<i64> {
    match (parsed.temporal_after, parsed.temporal_before) {
        (Some(after), Some(before)) => Some(after + ((before - after) / 2)),
        (Some(after), None) => Some(after),
        (None, Some(before)) => Some(before),
        (None, None) => None,
    }
}

fn trunc(text: &str) -> String {
    if text.len() <= 500 {
        return text.to_string();
    }
    match text.char_indices().nth(500) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

fn b64(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let a = chunk[0] as u32;
        let b = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let c = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (a << 16) | (b << 8) | c;
        encoded.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        encoded.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            CHARS[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            CHARS[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

/// Decode stored image data (PNG or legacy raw RGBA) into (raw_rgba_bytes, width, height).
fn decode_stored_image(data: &[u8], db_width: i64, db_height: i64) -> Option<(Vec<u8>, u32, u32)> {
    if data.is_empty() {
        return None;
    }
    // Try PNG decode first
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    if let Ok(mut reader) = decoder.read_info() {
        let mut buf = vec![0u8; reader.output_buffer_size()];
        if let Ok(info) = reader.next_frame(&mut buf) {
            return Some((buf[..info.buffer_size()].to_vec(), info.width, info.height));
        }
    }
    // Fallback: legacy raw RGBA
    if db_width > 0 && db_height > 0 {
        Some((data.to_vec(), db_width as u32, db_height as u32))
    } else {
        None
    }
}

fn gen_thumb(data: &[u8], width: u32, height: u32, max: u32) -> Option<Vec<u8>> {
    let scale = if width > max || height > max {
        max as f32 / width.max(height) as f32
    } else {
        1.0
    };
    let new_width = (width as f32 * scale) as u32;
    let new_height = (height as f32 * scale) as u32;
    if new_width == 0 || new_height == 0 {
        return None;
    }

    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, new_width, new_height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        if let Ok(mut writer) = encoder.write_header() {
            let mut resized = vec![0u8; (new_width * new_height * 4) as usize];
            for y in 0..new_height {
                for x in 0..new_width {
                    let src_x = (x as f32 / scale) as u32;
                    let src_y = (y as f32 / scale) as u32;
                    let src_idx = ((src_y * width + src_x) as usize) * 4;
                    let dst_idx = ((y * new_width + x) as usize) * 4;
                    if src_idx + 3 < data.len() && dst_idx + 3 < resized.len() {
                        resized[dst_idx..dst_idx + 4].copy_from_slice(&data[src_idx..src_idx + 4]);
                    }
                }
            }
            let _ = writer.write_image_data(&resized);
        }
    }

    if png.is_empty() {
        None
    } else {
        Some(png)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::Instant;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE clips (
                id INTEGER PRIMARY KEY,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                content_type TEXT NOT NULL,
                source_app TEXT DEFAULT '',
                source_app_icon BLOB,
                content_highlighted TEXT,
                ocr_text TEXT,
                image_data BLOB,
                image_thumbnail BLOB,
                image_width INTEGER DEFAULT 0,
                image_height INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                pinned INTEGER DEFAULT 0,
                collection_id INTEGER,
                language TEXT,
                copy_count INTEGER DEFAULT 0,
                deleted INTEGER DEFAULT 0,
                sync_version INTEGER DEFAULT 0
            );
            CREATE VIRTUAL TABLE clips_fts USING fts5(content, ocr_text);
            CREATE INDEX idx_test_clips_created ON clips(created_at DESC);
            ",
        )
        .unwrap();
        conn
    }

    fn insert_clip(
        conn: &Connection,
        id: i64,
        content: &str,
        content_type: &str,
        source_app: &str,
        created_at: i64,
        ocr_text: Option<&str>,
    ) {
        insert_clip_with_language(
            conn,
            id,
            content,
            content_type,
            source_app,
            created_at,
            ocr_text,
            None,
        );
    }

    fn insert_clip_with_language(
        conn: &Connection,
        id: i64,
        content: &str,
        content_type: &str,
        source_app: &str,
        created_at: i64,
        ocr_text: Option<&str>,
        language: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO clips (
                id, content, content_hash, content_type, source_app, ocr_text, created_at, pinned, language, copy_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, NULL, 0)",
            rusqlite::params![
                id,
                content,
                format!("hash-{id}"),
                content_type,
                source_app,
                ocr_text,
                created_at
            ],
        )
        .unwrap();
        if let Some(language) = language {
            conn.execute(
                "UPDATE clips SET language = ?1 WHERE id = ?2",
                rusqlite::params![language, id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO clips_fts(rowid, content, ocr_text) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, content, ocr_text.unwrap_or("")],
        )
        .unwrap();
    }

    #[test]
    fn meaning_query_finds_boarding_pass() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Boarding pass for QF12 to Los Angeles. Gate A12. Terminal 3.",
            "text",
            "Mail",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Grocery list for tonight",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("flight info");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|clip| clip.id), Some(1));
    }

    #[test]
    fn natural_filter_query_finds_slack_url() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "https://slack.com/archives/C123/p456",
            "url",
            "Slack",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "https://example.com/docs",
            "url",
            "Safari",
            1_720_000_000,
            None,
        );

        let parsed = query_parser::parse_query("url from slack yesterday");
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|clip| clip.id), Some(1));
    }

    #[test]
    fn code_block_query_finds_code_clip() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "fn main() {\n    println!(\"hello\");\n}",
            "code",
            "Code",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "hello there",
            "text",
            "Messages",
            1_710_000_050,
            None,
        );

        let parsed = query_parser::parse_query("code block");
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|clip| clip.id), Some(1));
    }

    #[test]
    fn empty_query_defaults_to_latest_first() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "older pinned item",
            "text",
            "Notes",
            1_710_000_000,
            None,
        );
        conn.execute(
            "UPDATE clips SET pinned = 1, copy_count = 99 WHERE id = 1",
            [],
        )
        .unwrap();
        insert_clip(
            &conn,
            2,
            "newest item",
            "text",
            "Notes",
            1_720_000_000,
            None,
        );

        let results = do_empty_search(&conn, "all", None).unwrap();
        assert_eq!(results.first().map(|clip| clip.id), Some(2));
    }

    #[test]
    fn ocr_text_is_ranked_for_image_searches() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "[Image]",
            "image",
            "Photos",
            1_710_000_000,
            Some("Boarding pass for SQ221 gate B7 terminal 1"),
        );
        insert_clip(
            &conn,
            2,
            "[Image]",
            "image",
            "Photos",
            1_710_000_100,
            Some("Random screenshot of a recipe"),
        );

        let parsed = query_parser::parse_query("boarding pass screenshot");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|clip| clip.id), Some(1));
    }

    #[test]
    fn language_and_type_filters_find_multilingual_images() {
        let conn = setup_test_db();
        insert_clip_with_language(
            &conn,
            1,
            "[Image]",
            "image",
            "Photos",
            1_710_000_000,
            Some("搭乗券 ゲート B7"),
            Some("ja"),
        );
        insert_clip_with_language(
            &conn,
            2,
            "[Image]",
            "image",
            "Photos",
            1_710_000_100,
            Some("boarding pass gate B7"),
            Some("en"),
        );

        let parsed = query_parser::parse_query("japanese screenshot");
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|clip| clip.id), Some(1));
    }

    #[test]
    #[ignore]
    fn search_stress_benchmark() {
        let conn = setup_test_db();
        let now = 1_720_000_000_i64;
        for id in 1..=5000_i64 {
            let content = format!("noise clip {id} lorem ipsum dolor sit amet {id}");
            let content_type = if id % 9 == 0 {
                "url"
            } else if id % 13 == 0 {
                "code"
            } else {
                "text"
            };
            let source_app = if id % 7 == 0 { "Slack" } else { "Notes" };
            insert_clip(
                &conn,
                id,
                &content,
                content_type,
                source_app,
                now - id,
                None,
            );
        }
        insert_clip(
            &conn,
            9001,
            "Boarding pass for SQ221 with gate B7 and terminal details",
            "text",
            "Mail",
            now + 1,
            None,
        );
        insert_clip(
            &conn,
            9002,
            "https://slack.com/archives/C123/p12345",
            "url",
            "Slack",
            now - 86_400,
            None,
        );
        insert_clip(
            &conn,
            9003,
            "fn render_card() {\n    return view;\n}",
            "code",
            "Code",
            now,
            None,
        );

        let queries = [
            "flight info",
            "url from slack",
            "code block",
            "boarding pass",
            "url from slack yesterday",
        ];

        let started = Instant::now();
        for _ in 0..25 {
            for query in queries {
                let parsed = query_parser::parse_query(query);
                if parsed.query_is_empty_after_parse || parsed.semantic.is_empty() {
                    let _ = do_filter_search(&conn, &parsed, "all", None).unwrap();
                } else {
                    let _ = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
                }
            }
        }
        let elapsed = started.elapsed();
        eprintln!(
            "[search-stress] {} searches over 5003 clips in {:?} ({:.2} ms/search)",
            25 * queries.len(),
            elapsed,
            elapsed.as_secs_f64() * 1000.0 / (25.0 * queries.len() as f64)
        );
        assert!(elapsed.as_millis() < 15_000);
    }

    #[test]
    #[ignore]
    fn search_large_dataset_benchmark() {
        let conn = setup_test_db();
        let now = 1_720_000_000_i64;
        for id in 1..=20_000_i64 {
            let content = format!("noise clip {id} lorem ipsum dolor sit amet {id}");
            let content_type = if id % 11 == 0 {
                "url"
            } else if id % 17 == 0 {
                "code"
            } else if id % 23 == 0 {
                "image"
            } else {
                "text"
            };
            let source_app = if id % 7 == 0 {
                "Slack"
            } else if id % 5 == 0 {
                "Mail"
            } else {
                "Notes"
            };
            let ocr = if content_type == "image" {
                Some("Boarding pass receipt code snippet link screenshot")
            } else {
                None
            };
            insert_clip(&conn, id, &content, content_type, source_app, now - id, ocr);
        }
        insert_clip(
            &conn,
            30_001,
            "Boarding pass for SQ221 with gate B7 and terminal details",
            "text",
            "Mail",
            now + 1,
            None,
        );
        insert_clip(
            &conn,
            30_002,
            "https://slack.com/archives/C123/p12345",
            "url",
            "Slack",
            now - 86_400,
            None,
        );
        insert_clip(
            &conn,
            30_003,
            "[Image]",
            "image",
            "Photos",
            now,
            Some("搭乗券 ゲート B7"),
        );

        let queries = [
            "flight info",
            "url from slack",
            "code block",
            "boarding pass",
            "url from slack yesterday",
            "japanese screenshot",
        ];

        let started = Instant::now();
        for _ in 0..20 {
            for query in queries {
                let parsed = query_parser::parse_query(query);
                if parsed.query_is_empty_after_parse || parsed.semantic.is_empty() {
                    let _ = do_filter_search(&conn, &parsed, "all", None).unwrap();
                } else {
                    let _ = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
                }
            }
        }
        let elapsed = started.elapsed();
        eprintln!(
            "[search-large] {} searches over 20003 clips in {:?} ({:.2} ms/search)",
            20 * queries.len(),
            elapsed,
            elapsed.as_secs_f64() * 1000.0 / (20.0 * queries.len() as f64)
        );
        assert!(elapsed.as_millis() < 20_000);
    }

    #[test]
    fn intent_query_finds_related_clips() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Your verification code is 123456",
            "text",
            "Messages",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Hello world",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("auth code");
        // Verify intent expansion produces correct keywords
        assert!(
            parsed.keywords.contains(&"verification".to_string()),
            "should expand to 'verification'"
        );
        assert!(
            parsed.keywords.contains(&"token".to_string()),
            "should expand to 'token'"
        );
        // Verify semantic is enriched for embedding
        assert!(
            parsed.semantic.contains("verification"),
            "semantic should contain 'verification'"
        );
    }

    #[test]
    fn receipt_query_finds_invoice() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Invoice #1234 - Total: $50.00 - Thank you for your payment",
            "text",
            "Mail",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Meeting at 3pm",
            "text",
            "Calendar",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("receipt");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn error_query_finds_stack_trace() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "panic: index out of range\nat main.rs:42\nstack trace:\n  0: main",
            "code",
            "Terminal",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Hello world",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("error");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn temporal_only_semantic_search() {
        let conn = setup_test_db();
        let now = 1_720_000_000_i64;
        let yesterday = now - 86400;
        insert_clip(
            &conn,
            1,
            "Meeting notes from yesterday",
            "text",
            "Notes",
            yesterday,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Meeting notes from last month",
            "text",
            "Notes",
            now - 86400 * 30,
            None,
        );

        let parsed = query_parser::parse_query("from yesterday");
        // Should return clip 1 based on temporal filter alone
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn source_filtered_url_query() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "https://slack.com/archives/C123/p456",
            "url",
            "Slack",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "https://example.com/docs",
            "url",
            "Safari",
            1_720_000_000,
            None,
        );

        let parsed = query_parser::parse_query("url from slack");
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn code_block_query_finds_code() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "fn main() {\n    println!(\"hello\");\n}",
            "code",
            "Code",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "hello there",
            "text",
            "Messages",
            1_710_000_050,
            None,
        );

        let parsed = query_parser::parse_query("code block");
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn language_filtered_screenshot_query() {
        let conn = setup_test_db();
        insert_clip_with_language(
            &conn,
            1,
            "[Image]",
            "image",
            "Photos",
            1_710_000_000,
            Some("搭乗券 ゲート B7"),
            Some("ja"),
        );
        insert_clip(
            &conn,
            2,
            "[Image]",
            "image",
            "Photos",
            1_710_000_100,
            Some("Boarding pass Gate A12"),
        );

        let parsed = query_parser::parse_query("japanese screenshot");
        let results = do_filter_search(&conn, &parsed, "all", None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn todo_list_query_finds_tasks() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Buy groceries, Call dentist, Send email - my todo list for today",
            "text",
            "Notes",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Meeting notes for today",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("todo list");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn zoom_link_query_finds_meeting_url() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "https://zoom.us/j/123456789?pwd=abc123",
            "url",
            "Calendar",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "https://example.com/article",
            "url",
            "Safari",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("zoom link");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn meeting_notes_query_finds_agenda() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Meeting agenda: Q3 review, action items, followup on deployment",
            "text",
            "Notion",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Random text note",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("meeting notes");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn tracking_number_query_finds_shipment() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Your order has been shipped! Tracking: 1Z999AA10123456784 - FedEx delivery",
            "text",
            "Mail",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Meeting at 3pm",
            "text",
            "Calendar",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("tracking number");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn password_query_finds_credentials() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Username: admin@example.com\nPassword: secure123",
            "text",
            "Notes",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "Meeting notes",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("password");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn semantic_query_text_includes_filters() {
        let parsed = query_parser::parse_query("url from slack in japanese");
        let built = build_semantic_query_text(&parsed);
        assert!(built.contains("slack"));
        assert!(built.contains("url"));
        assert!(built.contains("ja"));
    }

    #[test]
    fn privacy_policy_query_finds_policy_text() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Privacy policy update: data retention and compliance terms",
            "text",
            "Notion",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "random shopping note",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("that thing about the privacy policy");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn staging_server_query_finds_ip_and_ssh() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "ssh ubuntu@10.42.8.19 # staging server",
            "code",
            "Terminal",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "ssh root@prod.internal",
            "code",
            "Terminal",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("that ip address for the staging server");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }

    #[test]
    fn apology_email_query_finds_draft() {
        let conn = setup_test_db();
        insert_clip(
            &conn,
            1,
            "Hi team, sorry for the delay. I apologize and will send a followup by EOD.",
            "text",
            "Mail",
            1_710_000_000,
            None,
        );
        insert_clip(
            &conn,
            2,
            "meeting agenda for friday",
            "text",
            "Notes",
            1_710_000_100,
            None,
        );

        let parsed = query_parser::parse_query("that apology email I wrote");
        let results = do_ranked_search(&conn, &parsed, "all", None, None, None).unwrap();
        assert_eq!(results.first().map(|c| c.id), Some(1));
    }
}
