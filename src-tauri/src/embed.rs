use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tokenizers::Tokenizer;

pub struct EmbeddingModel {
    pub session: Mutex<Session>,
    pub tokenizer: Tokenizer,
    pub dimensions: usize,
    pub query_cache: Mutex<HashMap<String, Vec<f32>>>,
}

pub fn init_model(app: &tauri::AppHandle) -> Result<Arc<EmbeddingModel>, String> {
    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        search_dirs.push(resource_dir.join("resources/models"));
    }
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(parent) = exe_dir.parent() {
            search_dirs.push(parent.join("resources/models"));
        }
    }
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        search_dirs.push(std::path::PathBuf::from(manifest_dir).join("resources/models"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        search_dirs.push(cwd.join("src-tauri/resources/models"));
    }

    let mut model_path: Option<std::path::PathBuf> = None;
    let mut tokenizer_path: Option<std::path::PathBuf> = None;

    for dir in &search_dirs {
        let mp = dir.join("gte-multilingual-base.onnx");
        let tp = dir.join("tokenizer.json");
        if mp.exists() && tp.exists() {
            model_path = Some(mp);
            tokenizer_path = Some(tp);
            eprintln!("[Embed] Found model in: {:?}", dir);
            break;
        }
    }

    let model_path = model_path.ok_or_else(|| {
        format!(
            "Model not found. Searched: {:?}\nPlace gte-multilingual-base.onnx and tokenizer.json in resources/models/",
            search_dirs
        )
    })?;
    let tokenizer_path = tokenizer_path.unwrap();

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

    eprintln!("[Embed] ONNX session ready (768d)");

    Ok(Arc::new(EmbeddingModel {
        session: Mutex::new(session),
        tokenizer,
        dimensions: 768,
        query_cache: Mutex::new(HashMap::new()),
    }))
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

    let input_ids_tensor = Tensor::from_array((vec![1i64, seq_len], input_ids.to_vec()))
        .map_err(|e| format!("Tensor failed: {}", e))?;
    let attention_mask_tensor = Tensor::from_array((vec![1i64, seq_len], attention_mask.to_vec()))
        .map_err(|e| format!("Tensor failed: {}", e))?;

    let mut session = model.session.lock().map_err(|e| e.to_string())?;
    let outputs = session
        .run(ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor
        ])
        .map_err(|e| format!("Inference failed: {}", e))?;

    let embedding_output = outputs["sentence_embedding"]
        .try_extract_array::<f32>()
        .map_err(|e| format!("Extract failed: {}", e))?;
    let embedding: Vec<f32> = embedding_output
        .as_slice()
        .ok_or("Empty embedding")?
        .to_vec();

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

    if let Ok(cache) = model.query_cache.lock() {
        if let Some(cached) = cache.get(&key) {
            return Ok(cached.clone());
        }
    }

    let embedding = embed_text(model, query)?;
    if let Ok(mut cache) = model.query_cache.lock() {
        if cache.len() > 128 {
            cache.clear();
        }
        cache.insert(key, embedding.clone());
    }
    Ok(embedding)
}

fn collect_missing_embedding_ids(app: &tauri::AppHandle) -> Vec<i64> {
    let state = app.state::<crate::AppState>();
    let conn = match state.db_read.try_lock() {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    conn.prepare(
        "SELECT c.id FROM clips c
         LEFT JOIN clip_embeddings e ON c.id = e.rowid
         WHERE e.rowid IS NULL
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
    let unembedded = tokio::task::spawn_blocking(move || collect_missing_embedding_ids(&app_handle))
        .await
        .unwrap_or_default();

    let total = unembedded.len();
    if total > 0 {
        eprintln!("[Embed] Queuing {} clips without embeddings", total);
        update_search_status(app, "backfill_embeddings", total, 0, total, false);
        for id in unembedded {
            if clip_tx.send(id).await.is_err() {
                break;
            }
        }
    } else {
        eprintln!("[Embed] All clips already embedded");
        update_search_status(app, "idle", 0, 0, 0, true);
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

    // Process clips sequentially — each on a blocking thread to not starve the async runtime
    while let Some(clip_id) = rx.recv().await {
        let model = model.clone();
        let app = app.clone();

        // Use spawn_blocking so we don't block the async runtime during 17s inference
        let handle = tokio::task::spawn_blocking(move || {
            // 1. Read content (uses read connection)
            let content: Option<String> = {
                let state = app.state::<crate::AppState>();
                let conn = match state.db_read.lock() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                conn.query_row(
                    "SELECT content, ocr_text FROM clips WHERE id = ?",
                    [clip_id],
                    |row| {
                        let content: String = row.get(0).unwrap_or_default();
                        let ocr_text: Option<String> = row.get(1).unwrap_or(None);
                        if content == "[Image]" || content.is_empty() {
                            Ok(ocr_text)
                        } else {
                            Ok(Some(content))
                        }
                    },
                )
                .unwrap_or(None)
            };
            // DB lock released here

            let content = match content {
                Some(c) if !c.is_empty() => c,
                _ => return,
            };

            // 2. Embed (NO DB lock held — runs on blocking thread)
            let embedding = match embed_text(&model, &content) {
                Ok(e) if e.len() == 768 => e,
                Ok(e) => { eprintln!("[Embed] Wrong dims: {} (expected 768)", e.len()); return; }
                Err(e) => { eprintln!("[Embed] Failed clip {}: {}", clip_id, e); return; }
            };

            let vec_bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

            // 3. Write embedding (uses write connection)
            {
                let state = app.state::<crate::AppState>();
                let conn = match state.db_write.lock() {
                    Ok(c) => c,
                    Err(_) => { eprintln!("[Embed] Write lock failed clip {}", clip_id); return; }
                };
                let _ = conn.execute("DELETE FROM clip_embeddings WHERE rowid = ?", [clip_id]);
                if let Err(e) = conn.execute(
                    "INSERT INTO clip_embeddings(rowid, embedding) VALUES (?1, ?2)",
                    rusqlite::params![clip_id, vec_bytes],
                ) {
                    eprintln!("[Embed] Store failed clip {}: {}", clip_id, e);
                } else {
                    let state = app.state::<crate::AppState>();
                    match state.search_status.lock() {
                        Ok(mut status) => {
                            if status.phase == "backfill_embeddings" && status.total_items > 0 {
                                status.completed_items =
                                    (status.completed_items + 1).min(status.total_items);
                                status.queued_items =
                                    status.total_items.saturating_sub(status.completed_items);
                                if status.completed_items >= status.total_items {
                                    status.phase = "idle".to_string();
                                    status.semantic_ready = true;
                                }
                                let _ = app.emit("search-status-updated", status.clone());
                            }
                        }
                        Err(_) => {}
                    };
                }
            }
        });

        // Wait for this clip to finish before processing the next one
        // This prevents multiple 17-second inferences from running concurrently
        let _ = handle.await;
    }
}

fn update_search_status(
    app: &tauri::AppHandle,
    phase: &str,
    queued_items: usize,
    completed_items: usize,
    total_items: usize,
    semantic_ready: bool,
) {
    let state = app.state::<crate::AppState>();
    match state.search_status.lock() {
        Ok(mut status) => {
            status.phase = phase.to_string();
            status.queued_items = queued_items;
            status.completed_items = completed_items;
            status.total_items = total_items;
            status.semantic_ready = semantic_ready;
            let _ = app.emit("search-status-updated", status.clone());
        }
        Err(_) => {}
    };
}
