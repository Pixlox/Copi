use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

const MODEL_NAME: &str = "multilingual-e5-small";

const MODEL_FILES: [(&str, &str); 5] = [
    (
        "config.json",
        "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/config.json",
    ),
    (
        "model_O4.onnx",
        "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/model_O4.onnx",
    ),
    (
        "sentencepiece.bpe.model",
        "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/sentencepiece.bpe.model",
    ),
    (
        "tokenizer.json",
        "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/tokenizer.json",
    ),
    (
        "tokenizer_config.json",
        "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/tokenizer_config.json",
    ),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSetupStatus {
    pub phase: String,
    pub current_file: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub completed_files: usize,
    pub total_files: usize,
    pub install_path: String,
    pub error: Option<String>,
    pub ready: bool,
    pub setup_required: bool,
}

pub fn model_install_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|e| format!("Failed to resolve local data directory: {}", e))
        .map(|dir| dir.join("models").join(MODEL_NAME))
}

fn legacy_model_install_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|e| format!("Failed to resolve local data directory: {}", e))
        .map(|dir| {
            dir.join("models")
                .join("intfloat")
                .join(MODEL_NAME)
                .join("onnx")
        })
}

pub fn migrate_legacy_model_dir(app: &tauri::AppHandle) -> Result<(), String> {
    let install_dir = model_install_dir(app)?;
    if validate_model_install_dir(&install_dir) {
        return Ok(());
    }

    let legacy_dir = legacy_model_install_dir(app)?;
    if !validate_model_install_dir(&legacy_dir) {
        return Ok(());
    }

    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create model parent directory: {}", e))?;
    }
    if install_dir.exists() {
        std::fs::remove_dir_all(&install_dir)
            .map_err(|e| format!("Failed to replace model directory: {}", e))?;
    }
    std::fs::rename(&legacy_dir, &install_dir)
        .map_err(|e| format!("Failed to migrate model directory: {}", e))?;
    eprintln!(
        "[Embed] Migrated model directory from {} to {}",
        legacy_dir.to_string_lossy(),
        install_dir.to_string_lossy()
    );
    Ok(())
}

pub fn model_install_path_string(app: &tauri::AppHandle) -> String {
    model_install_dir(app)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn validate_model_install_dir(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|(filename, _)| {
        let path = dir.join(filename);
        path.is_file()
            && path
                .metadata()
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false)
    })
}

pub fn has_valid_model_install(app: &tauri::AppHandle) -> bool {
    model_install_dir(app)
        .map(|dir| validate_model_install_dir(&dir))
        .unwrap_or(false)
}

pub fn set_missing_status(app: &tauri::AppHandle, error: Option<String>) {
    let install_path = model_install_path_string(app);
    mutate_status(app, move |status| {
        status.phase = "missing".to_string();
        status.current_file = None;
        status.downloaded_bytes = 0;
        status.total_bytes = 0;
        status.completed_files = 0;
        status.total_files = MODEL_FILES.len();
        status.install_path = install_path;
        status.error = error;
        status.ready = false;
        status.setup_required = true;
    });
}

pub fn set_ready_status(app: &tauri::AppHandle) {
    let install_path = model_install_path_string(app);
    mutate_status(app, move |status| {
        status.phase = "ready".to_string();
        status.current_file = None;
        status.downloaded_bytes = 0;
        status.total_bytes = 0;
        status.completed_files = MODEL_FILES.len();
        status.total_files = MODEL_FILES.len();
        status.install_path = install_path;
        status.error = None;
        status.ready = true;
        status.setup_required = false;
    });
}

pub fn mutate_status<F>(app: &tauri::AppHandle, mutate: F)
where
    F: FnOnce(&mut ModelSetupStatus),
{
    let state = app.state::<crate::AppState>();
    let payload = match state.model_setup_status.lock() {
        Ok(mut status) => {
            mutate(&mut status);
            status.clone()
        }
        Err(_) => return,
    };
    let _ = app.emit("model-setup-updated", payload);
}

fn set_search_status_starting(app: &tauri::AppHandle) {
    let state = app.state::<crate::AppState>();
    let payload = match state.search_status.lock() {
        Ok(mut status) => {
            status.phase = "starting".to_string();
            status.queued_items = 0;
            status.completed_items = 0;
            status.failed_items = 0;
            status.total_items = 0;
            status.semantic_ready = false;
            status.clone()
        }
        Err(_) => return,
    };
    let _ = app.emit("search-status-updated", payload);
}

fn load_installed_model(app: &tauri::AppHandle, install_dir: &Path) -> Result<(), String> {
    let model = crate::embed::load_model_from_dir(install_dir)?;
    let state = app.state::<crate::AppState>();
    let mut guard = state.model.write().map_err(|e| e.to_string())?;
    *guard = Some(model);
    drop(guard);

    set_search_status_starting(app);
    set_ready_status(app);
    crate::start_runtime_services_once(app);
    Ok(())
}

async fn download_required_models_inner(
    app: &tauri::AppHandle,
    install_dir: &Path,
) -> Result<(), String> {
    let parent = install_dir
        .parent()
        .ok_or_else(|| "Invalid model install path".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create model directory: {}", e))?;

    let temp_dir = install_dir.with_extension("partial");
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)
            .map_err(|e| format!("Failed to clean previous partial download: {}", e))?;
    }
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create partial model directory: {}", e))?;

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| format!("Failed to create download client: {}", e))?;

    let install_path = install_dir.to_string_lossy().into_owned();
    for (index, (filename, url)) in MODEL_FILES.iter().enumerate() {
        let filename = (*filename).to_string();
        mutate_status(app, {
            let filename = filename.clone();
            let install_path = install_path.clone();
            move |status| {
                status.phase = "downloading".to_string();
                status.current_file = Some(filename);
                status.downloaded_bytes = 0;
                status.total_bytes = 0;
                status.completed_files = index;
                status.total_files = MODEL_FILES.len();
                status.install_path = install_path;
                status.error = None;
                status.ready = false;
                status.setup_required = true;
            }
        });

        let mut response = client
            .get(*url)
            .send()
            .await
            .map_err(|e| format!("Failed to download {}: {}", filename, e))?
            .error_for_status()
            .map_err(|e| format!("Failed to download {}: {}", filename, e))?;

        let total_bytes = response.content_length().unwrap_or(0);
        let temp_path = temp_dir.join(format!("{}.part", filename));
        let final_temp_path = temp_dir.join(&filename);
        let mut file = std::fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to write {}: {}", filename, e))?;

        let mut downloaded_bytes = 0u64;
        let mut last_emit = Instant::now();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("Failed while downloading {}: {}", filename, e))?
        {
            file.write_all(&chunk)
                .map_err(|e| format!("Failed to save {}: {}", filename, e))?;
            downloaded_bytes += chunk.len() as u64;

            if last_emit.elapsed() >= Duration::from_millis(80) {
                mutate_status(app, {
                    let filename = filename.clone();
                    let install_path = install_path.clone();
                    move |status| {
                        status.phase = "downloading".to_string();
                        status.current_file = Some(filename);
                        status.downloaded_bytes = downloaded_bytes;
                        status.total_bytes = total_bytes;
                        status.completed_files = index;
                        status.total_files = MODEL_FILES.len();
                        status.install_path = install_path;
                        status.error = None;
                        status.ready = false;
                        status.setup_required = true;
                    }
                });
                last_emit = Instant::now();
            }
        }

        file.flush()
            .map_err(|e| format!("Failed to finalize {}: {}", filename, e))?;
        std::fs::rename(&temp_path, &final_temp_path)
            .map_err(|e| format!("Failed to finalize {}: {}", filename, e))?;

        mutate_status(app, {
            let filename = filename.clone();
            let install_path = install_path.clone();
            move |status| {
                status.phase = "downloading".to_string();
                status.current_file = Some(filename);
                status.downloaded_bytes = total_bytes.max(downloaded_bytes);
                status.total_bytes = total_bytes;
                status.completed_files = index + 1;
                status.total_files = MODEL_FILES.len();
                status.install_path = install_path;
                status.error = None;
                status.ready = false;
                status.setup_required = true;
            }
        });
    }

    mutate_status(app, {
        let install_path = install_path.clone();
        move |status| {
            status.phase = "installing".to_string();
            status.current_file = None;
            status.downloaded_bytes = 0;
            status.total_bytes = 0;
            status.completed_files = MODEL_FILES.len();
            status.total_files = MODEL_FILES.len();
            status.install_path = install_path;
            status.error = None;
            status.ready = false;
            status.setup_required = true;
        }
    });

    if install_dir.exists() {
        std::fs::remove_dir_all(install_dir)
            .map_err(|e| format!("Failed to replace existing model files: {}", e))?;
    }
    std::fs::rename(&temp_dir, install_dir)
        .map_err(|e| format!("Failed to install model files: {}", e))?;

    load_installed_model(app, install_dir)
}

#[tauri::command]
pub async fn get_model_setup_status(app: tauri::AppHandle) -> Result<ModelSetupStatus, String> {
    let state = app
        .try_state::<crate::AppState>()
        .ok_or_else(|| "App state not ready yet".to_string())?;
    let status = state.model_setup_status.lock().map_err(|e| e.to_string())?;
    Ok(status.clone())
}

#[tauri::command]
pub async fn download_required_models(app: tauri::AppHandle) -> Result<(), String> {
    {
        let state = app
            .try_state::<crate::AppState>()
            .ok_or_else(|| "App state not ready yet".to_string())?;
        let status = state.model_setup_status.lock().map_err(|e| e.to_string())?;
        if matches!(
            status.phase.as_str(),
            "downloading" | "installing" | "ready"
        ) {
            return Ok(());
        }
    }

    let install_dir = model_install_dir(&app)?;
    let result = download_required_models_inner(&app, &install_dir).await;
    if let Err(error) = &result {
        let _ = std::fs::remove_dir_all(install_dir.with_extension("partial"));
        set_missing_status(&app, Some(error.clone()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("copi-{}-{}", label, nanos))
    }

    #[test]
    fn valid_model_install_requires_all_files() {
        let dir = unique_temp_dir("model-valid");
        std::fs::create_dir_all(&dir).unwrap();
        for (filename, _) in MODEL_FILES {
            std::fs::write(dir.join(filename), b"ok").unwrap();
        }

        assert!(validate_model_install_dir(&dir));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_invalidates_install() {
        let dir = unique_temp_dir("model-missing");
        std::fs::create_dir_all(&dir).unwrap();
        for (index, (filename, _)) in MODEL_FILES.iter().enumerate() {
            if index == 0 {
                continue;
            }
            std::fs::write(dir.join(filename), b"ok").unwrap();
        }

        assert!(!validate_model_install_dir(&dir));

        let _ = std::fs::remove_dir_all(dir);
    }
}
