use lru::LruCache;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tokenizers::Tokenizer;

const EMBED_DIMS: usize = 384;
pub const EMBEDDING_MODEL_SIGNATURE: &str = "multilingual-e5-small-384-v1";
const QUERY_CACHE_MAX: usize = 2048;
const WORKER_CONCURRENCY: usize = 4;

pub struct EmbeddingModel {
    pub session: Mutex<Session>,
    pub tokenizer: Tokenizer,
    pub dimensions: usize,
    pub query_cache: Mutex<LruCache<String, Vec<f32>>>,
}

pub fn init_model(app: &tauri::AppHandle) -> Result<Arc<EmbeddingModel>, String> {
    let install_dir = crate::model_setup::model_install_dir(app)?;
    if !crate::model_setup::validate_model_install_dir(&install_dir) {
        return Err(format!(
            "Model files are missing or incomplete in {}",
            install_dir.to_string_lossy()
        ));
    }
    load_model_from_dir(&install_dir)
}

pub fn load_model_from_dir(dir: &Path) -> Result<Arc<EmbeddingModel>, String> {
    let model_path = dir.join("model_O4.onnx");
    let tokenizer_path = dir.join("tokenizer.json");

    ort::init().commit();

    let parallelism = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4);
    let intra_threads = parallelism.clamp(1, 4);
    let mut builder = Session::builder().map_err(|e| format!("Session builder failed: {}", e))?;
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Optimization config failed: {}", e))?;
    builder = builder
        .with_intra_threads(intra_threads)
        .map_err(|e| format!("Thread config failed: {}", e))?;
    builder = builder
        .with_inter_threads(1)
        .map_err(|e| format!("Thread config failed: {}", e))?;

    let session = builder
        .commit_from_file(&model_path)
        .map_err(|e| format!("Failed to load model: {}", e))?;

    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| format!("Failed to load tokenizer: {}", e))?;

    eprintln!("[Embed] ONNX session ready (multilingual-e5-small, 384d, LRU cache)");

    Ok(Arc::new(EmbeddingModel {
        session: Mutex::new(session),
        tokenizer,
        dimensions: EMBED_DIMS,
        query_cache: Mutex::new(LruCache::new(NonZeroUsize::new(QUERY_CACHE_MAX).unwrap())),
    }))
}

pub fn classify_query_intent(query_embedding: &[f32]) -> Vec<(String, f32)> {
    let mut scores: Vec<(String, f32)> = crate::centroids::ALL
        .iter()
        .map(|(name, centroid)| {
            let similarity = cosine_similarity(query_embedding, *centroid);
            ((*name).to_string(), similarity)
        })
        .collect();

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    scores.into_iter().filter(|(_, s)| *s > 0.6).collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

pub fn embed_text(model: &EmbeddingModel, text: &str) -> Result<Vec<f32>, String> {
    let encoding = model
        .tokenizer
        .encode(text, true)
        .map_err(|e| format!("Tokenize failed: {}", e))?;

    let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
    let attention_mask: Vec<i64> = encoding
        .get_attention_mask()
        .iter()
        .map(|&m| m as i64)
        .collect();

    let max_len = 512.min(input_ids.len());
    let input_ids = &input_ids[..max_len];
    let attention_mask = &attention_mask[..max_len];
    let seq_len = input_ids.len() as i64;

    let token_type_ids: Vec<i64> = vec![0i64; max_len];

    let input_ids_tensor = Tensor::from_array((vec![1i64, seq_len], input_ids.to_vec()))
        .map_err(|e| format!("Tensor failed: {}", e))?;
    let attention_mask_tensor = Tensor::from_array((vec![1i64, seq_len], attention_mask.to_vec()))
        .map_err(|e| format!("Tensor failed: {}", e))?;
    let token_type_ids_tensor = Tensor::from_array((vec![1i64, seq_len], token_type_ids))
        .map_err(|e| format!("Tensor failed: {}", e))?;

    let mut session = model.session.lock().map_err(|e| e.to_string())?;
    let outputs = session
        .run(ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor,
            "token_type_ids" => token_type_ids_tensor
        ])
        .map_err(|e| format!("Inference failed: {}", e))?;

    let hidden_output = outputs["last_hidden_state"]
        .try_extract_array::<f32>()
        .map_err(|e| format!("Extract failed: {}", e))?;

    let dims = hidden_output.dim();
    let seq_len_actual = dims[1];
    let hidden_size = dims[2];
    if hidden_size != EMBED_DIMS {
        return Err(format!(
            "Unexpected hidden size {} (expected {})",
            hidden_size, EMBED_DIMS
        ));
    }

    let slice = hidden_output.as_slice().ok_or("Empty output")?;
    let mut embedding = vec![0.0f32; hidden_size];
    let mut count = 0usize;

    for (t, token_embedding) in slice
        .chunks_exact(hidden_size)
        .take(seq_len_actual)
        .enumerate()
    {
        if t < attention_mask.len() && attention_mask[t] == 1 {
            for (dst, value) in embedding.iter_mut().zip(token_embedding.iter()) {
                *dst += *value;
            }
            count += 1;
        }
    }

    if count > 0 {
        let inv = 1.0 / count as f32;
        for value in &mut embedding {
            *value *= inv;
        }
    }

    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    let normalized = if norm > 0.0 {
        embedding.iter().map(|x| x / norm).collect()
    } else {
        embedding
    };

    Ok(normalized)
}

pub fn embed_query(model: &EmbeddingModel, query: &str) -> Result<Vec<f32>, String> {
    let key = query.trim().to_lowercase();
    if key.is_empty() {
        return Ok(vec![]);
    }

    if let Ok(mut cache) = model.query_cache.lock() {
        if let Some(cached) = cache.get(&key) {
            return Ok(cached.clone());
        }
    }

    let prefixed = format!("query: {}", query);
    let embedding = embed_text(model, &prefixed)?;

    if let Ok(mut cache) = model.query_cache.lock() {
        cache.put(key, embedding.clone());
    }
    Ok(embedding)
}

fn collect_missing_embedding_ids(app: &tauri::AppHandle) -> Vec<i64> {
    let state = app.state::<crate::AppState>();
    let conn = match state.db_read_pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    conn.prepare(
        "SELECT c.id FROM clips c
        LEFT JOIN clip_embeddings e ON c.id = e.rowid
        WHERE e.rowid IS NULL
        AND c.deleted = 0
        AND (c.content_type != 'image' OR c.ocr_text IS NOT NULL)
        AND (c.content != '' OR c.ocr_text IS NOT NULL)
        ORDER BY c.created_at DESC",
    )
    .ok()
    .map(|mut stmt| {
        stmt.query_map([], |row| row.get(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<i64>>())
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

/// Backfill: enqueue all clips that don't have embeddings yet.
pub async fn backfill_embeddings(
    app: &tauri::AppHandle,
    clip_tx: &tokio::sync::mpsc::Sender<i64>,
) -> usize {
    let app_handle = app.clone();
    let unembedded =
        tokio::task::spawn_blocking(move || collect_missing_embedding_ids(&app_handle))
            .await
            .unwrap_or_default();

    let total = unembedded.len();
    if total > 0 {
        eprintln!("[Embed] Queuing {} clips without embeddings", total);
        update_search_status(app, "indexing", total, 0, 0, total, false);
        let mut enqueued = 0usize;
        for id in unembedded {
            if clip_tx.send(id).await.is_err() {
                eprintln!(
                    "[Embed] Queue closed while backfilling at item {}",
                    enqueued + 1
                );
                break;
            }
            enqueued += 1;
        }
        if enqueued == 0 {
            update_search_status(app, "error", 0, 0, 0, 0, false);
        } else if enqueued < total {
            update_search_status(app, "indexing", enqueued, 0, 0, enqueued, false);
        }
    } else {
        eprintln!("[Embed] All clips already embedded");
        update_search_status(app, "idle", 0, 0, 0, 0, true);
    }
    total
}

pub async fn embedding_worker(
    model: Option<Arc<EmbeddingModel>>,
    mut rx: tokio::sync::mpsc::Receiver<i64>,
    app: tauri::AppHandle,
) {
    let model = match model {
        Some(m) => m,
        None => {
            eprintln!("[Embed] No model loaded, worker idle");
            while rx.recv().await.is_some() {}
            return;
        }
    };

    let semaphore = Arc::new(tokio::sync::Semaphore::new(WORKER_CONCURRENCY));

    while let Some(clip_id) = rx.recv().await {
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
        };
        let model = model.clone();
        let app = app.clone();

        tokio::task::spawn_blocking(move || {
            let success = !matches!(
                embed_single_clip(&model, &app, clip_id),
                EmbedOutcome::Failed
            );
            update_search_progress(&app, success);
            drop(permit);
        });
    }
}

enum EmbedOutcome {
    Stored,
    Skipped,
    Failed,
}

fn embed_single_clip(model: &EmbeddingModel, app: &tauri::AppHandle, clip_id: i64) -> EmbedOutcome {
    let (content, source_app, content_type): (Option<String>, String, String) = {
        let state = app.state::<crate::AppState>();
        let conn = match state.db_read_pool.get() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[Embed] Pool error for clip {}: {}", clip_id, e);
                return EmbedOutcome::Failed;
            }
        };
        match conn.query_row(
            "SELECT content, ocr_text, source_app, content_type FROM clips WHERE id = ? AND deleted = 0",
            [clip_id],
            |row| {
                let content: String = row.get(0).unwrap_or_default();
                let ocr_text: Option<String> = row.get(1).unwrap_or(None);
                let source_app: String = row.get(2).unwrap_or_default();
                let content_type: String = row.get(3).unwrap_or_default();
                if content == "[Image]" || content.is_empty() {
                    Ok((ocr_text, source_app, content_type))
                } else {
                    Ok((Some(content), source_app, content_type))
                }
            },
        ) {
            Ok(data) => data,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                eprintln!("[Embed] Skipping clip {}: clip no longer exists", clip_id);
                return EmbedOutcome::Skipped;
            }
            Err(error) => {
                eprintln!("[Embed] Read failed for clip {}: {}", clip_id, error);
                return EmbedOutcome::Failed;
            }
        }
    };

    let content = match content {
        Some(c) if !c.is_empty() => c,
        _ => {
            eprintln!(
                "[Embed] Skipping clip {}: no embeddable text for content type '{}'",
                clip_id, content_type
            );
            return EmbedOutcome::Skipped;
        }
    };

    let enriched = if !source_app.is_empty() {
        match content_type.as_str() {
            "image" => format!("passage: This is an image from {}: {}", source_app, content),
            "code" => format!("passage: This is code from {}: {}", source_app, content),
            "url" => format!("passage: This is a link from {}: {}", source_app, content),
            _ => format!("passage: Copied from {}: {}", source_app, content),
        }
    } else {
        format!("passage: {}", content)
    };

    let embedding = match embed_text(model, &enriched) {
        Ok(e) if e.len() == EMBED_DIMS => e,
        Ok(e) => {
            eprintln!(
                "[Embed] Wrong dims for clip {}: {} (expected {})",
                clip_id,
                e.len(),
                EMBED_DIMS
            );
            return EmbedOutcome::Failed;
        }
        Err(e) => {
            eprintln!("[Embed] Failed clip {}: {}", clip_id, e);
            return EmbedOutcome::Failed;
        }
    };

    let vec_bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

    let state = app.state::<crate::AppState>();
    let conn = match state.db_write.lock() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("[Embed] Write lock failed clip {}", clip_id);
            return EmbedOutcome::Failed;
        }
    };
    let _ = conn.execute("DELETE FROM clip_embeddings WHERE rowid = ?", [clip_id]);
    match conn.execute(
        "INSERT INTO clip_embeddings(rowid, embedding) VALUES (?1, ?2)",
        rusqlite::params![clip_id, vec_bytes],
    ) {
        Ok(_) => EmbedOutcome::Stored,
        Err(e) => {
            eprintln!("[Embed] Store failed clip {}: {}", clip_id, e);
            EmbedOutcome::Failed
        }
    }
}

fn update_search_progress(app: &tauri::AppHandle, success: bool) {
    let state = app.state::<crate::AppState>();
    let lock = state.search_status.lock();
    if let Ok(mut status) = lock {
        if !advance_search_status(&mut status, success) {
            return;
        }
        if status.phase == "idle" {
            eprintln!(
                "[Embed] Indexing complete: {} success, {} failed",
                status.completed_items, status.failed_items
            );
        }
        let _ = app.emit("search-status-updated", status.clone());
    }
}

fn advance_search_status(status: &mut crate::search::SearchStatusPayload, success: bool) -> bool {
    if status.phase != "indexing" || status.total_items == 0 {
        return false;
    }

    if success {
        status.completed_items = (status.completed_items + 1).min(status.total_items);
    } else {
        status.failed_items = (status.failed_items + 1).min(status.total_items);
    }
    let processed = (status.completed_items + status.failed_items).min(status.total_items);
    status.queued_items = status.total_items.saturating_sub(processed);
    if processed >= status.total_items {
        status.phase = "idle".to_string();
        status.semantic_ready = true;
    }
    true
}

fn update_search_status(
    app: &tauri::AppHandle,
    phase: &str,
    queued_items: usize,
    completed_items: usize,
    failed_items: usize,
    total_items: usize,
    semantic_ready: bool,
) {
    let state = app.state::<crate::AppState>();
    let lock = state.search_status.lock();
    if let Ok(mut status) = lock {
        status.phase = phase.to_string();
        status.queued_items = queued_items;
        status.completed_items = completed_items;
        status.failed_items = failed_items;
        status.total_items = total_items;
        status.semantic_ready = semantic_ready;
        let _ = app.emit("search-status-updated", status.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_near_duplicates(embeddings: &[(i64, Vec<f32>)], threshold: f32) -> Vec<Vec<i64>> {
        let mut groups: Vec<Vec<i64>> = Vec::new();
        let mut used: std::collections::HashSet<i64> = std::collections::HashSet::new();

        for (i, (id_a, emb_a)) in embeddings.iter().enumerate() {
            if used.contains(id_a) {
                continue;
            }

            let mut group = vec![*id_a];
            used.insert(*id_a);

            for (id_b, emb_b) in embeddings.iter().skip(i + 1) {
                if used.contains(id_b) {
                    continue;
                }

                if cosine_similarity(emb_a, emb_b) >= threshold {
                    group.push(*id_b);
                    used.insert(*id_b);
                }
            }

            if group.len() > 1 {
                groups.push(group);
            }
        }

        groups
    }

    fn cluster_embeddings(
        embeddings: &[(i64, Vec<f32>)],
        min_similarity: f32,
    ) -> Vec<(Vec<i64>, Vec<f32>)> {
        if embeddings.is_empty() {
            return vec![];
        }

        let dims = embeddings[0].1.len();
        let mut clusters: Vec<(Vec<i64>, Vec<f32>)> = Vec::new();

        for (id, embedding) in embeddings {
            let mut assigned = false;

            for (cluster_ids, centroid) in &mut clusters {
                if cosine_similarity(embedding, centroid) < min_similarity {
                    continue;
                }

                cluster_ids.push(*id);
                let n = cluster_ids.len() as f32;
                for (idx, value) in embedding.iter().enumerate().take(dims) {
                    centroid[idx] = (centroid[idx] * (n - 1.0) + *value) / n;
                }
                assigned = true;
                break;
            }

            if !assigned {
                clusters.push((vec![*id], embedding.clone()));
            }
        }

        clusters
    }

    #[test]
    fn advance_status_counts_success_and_failure() {
        let mut status = crate::search::SearchStatusPayload {
            phase: "indexing".into(),
            queued_items: 3,
            completed_items: 0,
            failed_items: 0,
            total_items: 3,
            semantic_ready: false,
        };

        assert!(advance_search_status(&mut status, true));
        assert_eq!(status.completed_items, 1);
        assert_eq!(status.failed_items, 0);
        assert_eq!(status.queued_items, 2);
        assert_eq!(status.phase, "indexing");

        assert!(advance_search_status(&mut status, false));
        assert_eq!(status.completed_items, 1);
        assert_eq!(status.failed_items, 1);
        assert_eq!(status.queued_items, 1);
    }

    #[test]
    fn advance_status_finishes_when_processed_reaches_total() {
        let mut status = crate::search::SearchStatusPayload {
            phase: "indexing".into(),
            queued_items: 1,
            completed_items: 0,
            failed_items: 0,
            total_items: 1,
            semantic_ready: false,
        };

        assert!(advance_search_status(&mut status, false));
        assert_eq!(status.failed_items, 1);
        assert_eq!(status.queued_items, 0);
        assert_eq!(status.phase, "idle");
        assert!(status.semantic_ready);
    }

    #[test]
    fn advance_status_is_noop_when_not_indexing() {
        let mut status = crate::search::SearchStatusPayload {
            phase: "idle".into(),
            queued_items: 0,
            completed_items: 0,
            failed_items: 0,
            total_items: 0,
            semantic_ready: true,
        };

        assert!(!advance_search_status(&mut status, true));
        assert_eq!(status.completed_items, 0);
        assert_eq!(status.failed_items, 0);
    }

    #[test]
    fn cosine_similarity_range() {
        // Same vectors should have similarity 1.0
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        // Orthogonal vectors should have similarity 0.0
        let c = vec![1.0, 0.0];
        let d = vec![0.0, 1.0];
        assert!(cosine_similarity(&c, &d).abs() < 0.001);

        // Opposite vectors should have similarity -1.0
        let e = vec![1.0, 0.0];
        let f = vec![-1.0, 0.0];
        assert!((cosine_similarity(&e, &f) - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn near_duplicate_detection() {
        let embeddings = vec![
            (1, vec![1.0, 0.0, 0.0, 0.0]),
            (2, vec![0.99, 0.01, 0.0, 0.0]), // Near duplicate of 1
            (3, vec![0.0, 1.0, 0.0, 0.0]),
            (4, vec![0.0, 0.99, 0.01, 0.0]), // Near duplicate of 3
        ];

        let duplicates = find_near_duplicates(&embeddings, 0.95);
        assert_eq!(duplicates.len(), 2);
        assert!(duplicates.iter().any(|g| g.contains(&1) && g.contains(&2)));
        assert!(duplicates.iter().any(|g| g.contains(&3) && g.contains(&4)));
    }

    #[test]
    fn cluster_embeddings_groups_similar() {
        let embeddings = vec![
            (1, vec![1.0, 0.0]),
            (2, vec![0.95, 0.05]),
            (3, vec![0.0, 1.0]),
            (4, vec![0.05, 0.95]),
        ];

        let clusters = cluster_embeddings(&embeddings, 0.9);
        assert_eq!(clusters.len(), 2);
    }
}
