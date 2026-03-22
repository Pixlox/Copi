use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::Manager;

use crate::query_parser;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipResult {
    pub id: i64,
    pub content: String,
    pub content_type: String,
    pub source_app: String,
    pub created_at: i64,
    pub pinned: bool,
    pub source_app_icon: Option<String>,
    pub content_highlighted: Option<String>,
}

#[tauri::command]
pub fn search_clips(
    app: tauri::AppHandle,
    query: String,
    filter: String,
) -> Result<Vec<ClipResult>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    if query.trim().is_empty() {
        return search_empty(&conn, &filter);
    }

    let parsed = query_parser::parse_query(&query);
    let search_query = if parsed.semantic.is_empty() {
        &query
    } else {
        &parsed.semantic
    };

    // Build WHERE clauses
    let temporal_clause = match (parsed.temporal_after, parsed.temporal_before) {
        (Some(after), Some(before)) => {
            format!(
                " AND c.created_at >= {} AND c.created_at <= {}",
                after, before
            )
        }
        (Some(after), None) => format!(" AND c.created_at >= {}", after),
        (None, Some(before)) => format!(" AND c.created_at <= {}", before),
        (None, None) => String::new(),
    };

    let source_app_clause = parsed
        .source_app
        .as_ref()
        .map(|app_name| {
            format!(
                " AND LOWER(c.source_app) LIKE '%{}%'",
                app_name.to_lowercase()
            )
        })
        .unwrap_or_default();

    let effective_filter = parsed.content_type.as_deref().unwrap_or(&filter);

    // Run all search strategies
    let fts_results = search_fts(
        &conn,
        search_query,
        effective_filter,
        &temporal_clause,
        &source_app_clause,
    )
    .unwrap_or_default();

    let vec_results = if let Some(ref model) = state.model {
        search_vectors(
            model,
            &conn,
            search_query,
            effective_filter,
            &temporal_clause,
            &source_app_clause,
        )
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    let like_results = if fts_results.len() + vec_results.len() < 10 {
        search_like(
            &conn,
            search_query,
            effective_filter,
            &temporal_clause,
            &source_app_clause,
        )
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Merge with Reciprocal Rank Fusion
    let fused = reciprocal_rank_fusion(vec![fts_results, vec_results, like_results]);

    Ok(fused)
}

#[tauri::command]
pub fn get_total_clip_count(app: tauri::AppHandle) -> Result<i64, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.query_row("SELECT COUNT(*) FROM clips", [], |row| row.get(0))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_image_thumbnail(app: tauri::AppHandle, clip_id: i64) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    let thumb: Option<Vec<u8>> = conn
        .query_row(
            "SELECT source_app_icon FROM clips WHERE id = ? AND content_type = 'image'",
            [clip_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    match thumb {
        Some(bytes) if !bytes.is_empty() => {
            // Encode as base64
            let encoded = base64_encode(&bytes);
            Ok(Some(encoded))
        }
        _ => Ok(None),
    }
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        result.push(if chunk.len() > 1 {
            CHARS[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            CHARS[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    result
}

// ─── Search Strategies ────────────────────────────────────────────

fn search_empty(conn: &rusqlite::Connection, filter: &str) -> Result<Vec<ClipResult>, String> {
    let sql = match filter {
        "all" => "SELECT id, content, content_type, source_app, created_at, pinned, content_highlighted FROM clips ORDER BY pinned DESC, created_at DESC LIMIT 50",
        "pinned" => "SELECT id, content, content_type, source_app, created_at, pinned, content_highlighted FROM clips WHERE pinned = 1 ORDER BY created_at DESC LIMIT 50",
        f => &format!("SELECT id, content, content_type, source_app, created_at, pinned, content_highlighted FROM clips WHERE content_type = '{}' ORDER BY pinned DESC, created_at DESC LIMIT 50", f),
    };
    query_rows(conn, sql, [])
}

fn search_fts(
    conn: &rusqlite::Connection,
    query: &str,
    filter: &str,
    temporal: &str,
    source_app: &str,
) -> Result<Vec<ClipResult>, String> {
    let fts_query = build_fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let type_clause = build_type_clause(filter);
    let pinned_clause = if filter == "pinned" {
        " AND c.pinned = 1"
    } else {
        ""
    };

    let sql = format!(
        "SELECT c.id, c.content, c.content_type, c.source_app, c.created_at, c.pinned, c.content_highlighted
         FROM clips_fts fts
         JOIN clips c ON c.id = fts.rowid
         WHERE clips_fts MATCH ?1{}{}{}{}
         ORDER BY fts.rank
         LIMIT 50",
        type_clause, pinned_clause, temporal, source_app
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&fts_query], |row| row_to_clip(row))
        .map_err(|e| e.to_string())?;
    collect_rows(rows)
}

fn search_vectors(
    model: &crate::embed::EmbeddingModel,
    conn: &rusqlite::Connection,
    query: &str,
    filter: &str,
    temporal: &str,
    source_app: &str,
) -> Result<Vec<ClipResult>, String> {
    let query_vec = crate::embed::embed_query(model, query)?;
    if query_vec.len() != 768 {
        return Ok(Vec::new());
    }

    let vec_bytes: Vec<u8> = query_vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    let type_clause = build_type_clause(filter);
    let pinned_clause = if filter == "pinned" {
        " AND c.pinned = 1"
    } else {
        ""
    };

    // CTE pattern: get KNN candidates first, then filter — avoids sqlite-vec JOIN issues
    let sql = format!(
        "WITH knn AS (
            SELECT rowid, distance FROM clip_embeddings
            WHERE embedding MATCH ?1 AND k = 200
         )
         SELECT c.id, c.content, c.content_type, c.source_app, c.created_at, c.pinned, c.content_highlighted
         FROM knn vec
         JOIN clips c ON c.id = vec.rowid
         WHERE 1=1{}{}{}{}
         ORDER BY vec.distance
         LIMIT 50",
        type_clause, pinned_clause, temporal, source_app
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([vec_bytes], |row| row_to_clip(row))
        .map_err(|e| e.to_string())?;
    collect_rows(rows)
}

fn search_like(
    conn: &rusqlite::Connection,
    query: &str,
    filter: &str,
    temporal: &str,
    source_app: &str,
) -> Result<Vec<ClipResult>, String> {
    let pattern = format!("%{}%", query);
    let type_clause = if filter != "all" && filter != "pinned" {
        format!(" AND content_type = '{}'", filter)
    } else {
        String::new()
    };
    let pinned_clause = if filter == "pinned" {
        " AND pinned = 1"
    } else {
        ""
    };

    let sql = format!(
        "SELECT id, content, content_type, source_app, created_at, pinned, content_highlighted
         FROM clips
         WHERE content LIKE ?1{}{}{}{}
         ORDER BY created_at DESC
         LIMIT 30",
        type_clause, pinned_clause, temporal, source_app
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&pattern], |row| row_to_clip(row))
        .map_err(|e| e.to_string())?;
    collect_rows(rows)
}

// ─── Helpers ──────────────────────────────────────────────────────

fn build_fts_query(query: &str) -> String {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    if words.len() == 1 {
        return format!("{}*", words[0]);
    }
    let mut parts: Vec<String> = Vec::new();
    for (i, word) in words.iter().enumerate() {
        if i == words.len() - 1 {
            parts.push(format!("{}*", word));
        } else {
            parts.push(word.to_string());
        }
    }
    parts.join(" ")
}

fn build_type_clause(filter: &str) -> String {
    match filter {
        "all" | "pinned" => String::new(),
        f => format!(" AND c.content_type = '{}'", f),
    }
}

fn row_to_clip(row: &rusqlite::Row) -> rusqlite::Result<ClipResult> {
    Ok(ClipResult {
        id: row.get(0)?,
        content: truncate(&row.get::<_, String>(1).unwrap_or_default()),
        content_type: row.get(2)?,
        source_app: row.get(3)?,
        created_at: row.get(4)?,
        pinned: row.get::<_, i64>(5)? != 0,
        source_app_icon: None,
        content_highlighted: row.get(6)?,
    })
}

fn query_rows<P: rusqlite::Params>(
    conn: &rusqlite::Connection,
    sql: &str,
    params: P,
) -> Result<Vec<ClipResult>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |row| row_to_clip(row))
        .map_err(|e| e.to_string())?;
    collect_rows(rows)
}

fn collect_rows(
    rows: rusqlite::MappedRows<impl FnMut(&rusqlite::Row) -> rusqlite::Result<ClipResult>>,
) -> Result<Vec<ClipResult>, String> {
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

fn truncate(content: &str) -> String {
    if content.len() > 500 {
        // Find a safe char boundary at or before byte 500
        let end = content
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 500)
            .last()
            .unwrap_or(0);
        format!("{}…", &content[..end])
    } else {
        content.to_string()
    }
}

// ─── Reciprocal Rank Fusion ───────────────────────────────────────

fn reciprocal_rank_fusion(result_lists: Vec<Vec<ClipResult>>) -> Vec<ClipResult> {
    let k: f64 = 60.0;
    let mut scores: HashMap<i64, f64> = HashMap::new();
    let mut clip_map: HashMap<i64, ClipResult> = HashMap::new();

    for list in &result_lists {
        for (rank, clip) in list.iter().enumerate() {
            let rrf_score = 1.0 / (k + rank as f64 + 1.0);
            *scores.entry(clip.id).or_insert(0.0) += rrf_score;
            clip_map.entry(clip.id).or_insert_with(|| clip.clone());
        }
    }

    let mut ranked: Vec<(i64, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    ranked
        .into_iter()
        .filter_map(|(id, _)| clip_map.remove(&id))
        .collect()
}
