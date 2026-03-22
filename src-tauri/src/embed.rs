use ort::session::Session;
use ort::value::Tensor;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tokenizers::Tokenizer;

pub struct EmbeddingModel {
    pub session: Mutex<Session>,
    pub tokenizer: Tokenizer,
    pub dimensions: usize,
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

    let session = Session::builder()
        .map_err(|e| format!("Session builder failed: {}", e))?
        .commit_from_file(&model_path)
        .map_err(|e| format!("Failed to load model: {}", e))?;

    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| format!("Failed to load tokenizer: {}", e))?;

    eprintln!("[Embed] ONNX session ready (768d)");

    Ok(Arc::new(EmbeddingModel {
        session: Mutex::new(session),
        tokenizer,
        dimensions: 768,
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
    let attention_mask_tensor =
        Tensor::from_array((vec![1i64, seq_len], attention_mask.to_vec()))
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
    embed_text(model, query)
}

/// Backfill: embed all clips that don't have embeddings yet
pub fn backfill_embeddings(app: &tauri::AppHandle, clip_tx: &tokio::sync::mpsc::Sender<i64>) {
    let state = app.state::<crate::AppState>();
    let conn = match state.db.lock() {
        Ok(c) => c,
        Err(_) => return,
    };

    let unembedded: Vec<i64> = conn
        .prepare(
            "SELECT c.id FROM clips c
             LEFT JOIN clip_embeddings e ON c.id = e.rowid
             WHERE e.rowid IS NULL AND c.content_type != 'image' AND c.content != ''
             ORDER BY c.created_at DESC
             LIMIT 500",
        )
        .ok()
        .map(|mut stmt| {
            stmt.query_map([], |row| row.get(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<i64>>())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    if !unembedded.is_empty() {
        eprintln!(
            "[Embed] Backfilling {} clips without embeddings",
            unembedded.len()
        );
        for id in unembedded {
            let _ = clip_tx.try_send(id);
        }
    } else {
        eprintln!("[Embed] All clips already embedded");
    }
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

    while let Some(clip_id) = rx.recv().await {
        let content: Option<String> = {
            let state = app.state::<crate::AppState>();
            let conn = match state.db.lock() {
                Ok(c) => c,
                Err(_) => continue,
            };
            conn.query_row(
                "SELECT content FROM clips WHERE id = ?",
                [clip_id],
                |row| row.get(0),
            )
            .ok()
        };

        let content = match content {
            Some(c) if !c.is_empty() && c != "[Image]" => c,
            _ => continue,
        };

        match embed_text(&model, &content) {
            Ok(embedding) => {
                if embedding.len() != 768 {
                    eprintln!("[Embed] Wrong dims: {} (expected 768)", embedding.len());
                    continue;
                }

                let vec_bytes: Vec<u8> =
                    embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

                {
                    let state = app.state::<crate::AppState>();
                    let lock_result = state.db.lock();
                    if let Ok(ref conn) = lock_result {
                        if let Err(e) = conn.execute(
                            "INSERT OR REPLACE INTO clip_embeddings(rowid, embedding) VALUES (?1, ?2)",
                            rusqlite::params![clip_id, vec_bytes],
                        ) {
                            eprintln!("[Embed] Store failed clip {}: {}", clip_id, e);
                        }
                    }
                }
            }
            Err(e) => eprintln!("[Embed] Failed clip {}: {}", clip_id, e),
        }
    }
}
