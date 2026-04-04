use arboard::{Clipboard, ImageData};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

use crate::{
    macos::{get_app_icon_png, get_clipboard_source_app, get_frontmost_app_info, FrontmostApp},
    sync, AppState,
};

struct ClipboardImagePayload {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}

struct ClipboardFilePayload {
    path: PathBuf,
    file_name: String,
    file_size: i64,
    file_data: Option<Vec<u8>>,
}

const FILE_AUTO_SYNC_MAX_BYTES: i64 = crate::sync::FILE_AUTO_SYNC_MAX_BYTES;

// ─── Watch Clipboard ──────────────────────────────────────────────

pub async fn watch_clipboard(app: &tauri::AppHandle) {
    let mut clipboard = match Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[Clip] Failed to init: {}", e);
            return;
        }
    };

    let mut last_text_hash = String::new();
    let mut last_image_hash = String::new();
    let mut last_file_hash = String::new();
    let mut last_non_copi_app: Option<FrontmostApp> = None;
    let mut last_change_count: Option<i64> = None;

    loop {
        // Check if paused
        let paused = {
            let state = app.state::<AppState>();
            let running = *state.clipboard_watcher_running.lock().unwrap();
            !running
        };
        if paused {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }

        // PERF 6: Only read clipboard when changeCount changes (zero CPU when idle)
        let current_change_count = crate::macos::get_pasteboard_change_count();
        if current_change_count >= 0 {
            if Some(current_change_count) == last_change_count {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            last_change_count = Some(current_change_count);
        }

        // ── Source App Detection ──────────────────────────────────
        // Priority: 1) Pasteboard source (most accurate - from clipboard metadata)
        //           2) Current frontmost app (if not Copi)
        //           3) Last known non-Copi app (fallback)
        let pasteboard_source = get_clipboard_source_app();
        let current_frontmost = get_frontmost_app_info();
        if let Some(frontmost) = current_frontmost.clone() {
            if !frontmost.is_copi() && !frontmost.is_empty() {
                last_non_copi_app = Some(frontmost);
            }
        }
        let source_app = pasteboard_source
            .filter(|app| !app.is_copi() && !app.is_empty())
            .or_else(|| current_frontmost.filter(|app| !app.is_copi() && !app.is_empty()))
            .or_else(|| last_non_copi_app.clone())
            .unwrap_or_default();

        // ── File clipboard ────────────────────────────────────────
        if let Ok(file_list) = clipboard.get().file_list() {
            if !file_list.is_empty() {
                let mut hash_input = String::new();
                for path in &file_list {
                    hash_input.push_str(path.to_string_lossy().as_ref());
                    hash_input.push('\n');
                }
                let file_hash = compute_hash(&format!("file-list:{}", hash_input));
                if file_hash != last_file_hash {
                    last_file_hash = file_hash.clone();
                    queue_file_capture(app, file_list, file_hash, source_app.clone());
                }
                tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                continue;
            }
        }

        // ── Text clipboard ────────────────────────────────────────
        if let Ok(text) = clipboard.get_text() {
            let file_paths = parse_file_uri_list(&text);
            if !file_paths.is_empty() {
                let mut hash_input = String::new();
                for path in &file_paths {
                    hash_input.push_str(path.to_string_lossy().as_ref());
                    hash_input.push('\n');
                }
                let file_hash = compute_hash(&format!("file-uri-list:{}", hash_input));
                if file_hash != last_file_hash {
                    last_file_hash = file_hash.clone();
                    queue_file_capture(app, file_paths, file_hash, source_app.clone());
                }
                tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                continue;
            }

            let hash = compute_hash(&text);
            if hash != last_text_hash && !text.trim().is_empty() {
                last_text_hash = hash.clone();

                if !crate::privacy::should_capture(&text, app) {
                    tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                    continue;
                }

                queue_text_capture(app, text, hash, source_app.clone());
            }
        }

        // ── Image clipboard ───────────────────────────────────────
        let img_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match clipboard.get_image() {
                Ok(image_data) => {
                    let pixels = image_data.bytes.as_ref();
                    if pixels.is_empty() {
                        return;
                    }
                    let hash = compute_hash_bytes(pixels);
                    if hash == last_text_hash {
                        return;
                    }
                    if hash == last_image_hash {
                        return;
                    }
                    last_image_hash = hash.clone();
                    let payload = ClipboardImagePayload {
                        width: image_data.width,
                        height: image_data.height,
                        bytes: pixels.to_vec(),
                    };
                    queue_image_capture(app, payload, hash, source_app.clone());
                }
                Err(_) => {} // No image on clipboard — normal
            }
        }));

        if let Err(e) = img_result {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into());
            eprintln!("[Image] Processing failed: {}", msg);
        }

        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
    }
}

// ─── Backfill Image Metadata ──────────────────────────────────────

pub async fn backfill_image_metadata(app: &tauri::AppHandle) {
    let mut retry_count = 0u32;
    const MAX_RETRIES: u32 = 5;
    let mut no_progress_rounds = 0u32;
    let mut any_updated = false;
    loop {
        let app_handle = app.clone();
        let repair_ids = tokio::task::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            let conn = match state.db_read_pool.get() {
                Ok(conn) => conn,
                Err(e) => return Err(format!("{}", e)),
            };
            conn.prepare(
                "SELECT id
                 FROM clips
                 WHERE content_type = 'image'
                   AND length(COALESCE(image_data, X'')) > 0
                   AND (
                     ocr_text IS NULL OR TRIM(ocr_text) = ''
                     OR image_thumbnail IS NULL OR length(image_thumbnail) = 0
                   )
                 ORDER BY created_at DESC
                 LIMIT 24",
            )
            .ok()
            .map(|mut stmt| {
                stmt.query_map([], |row| row.get(0))
                    .map(|rows| rows.filter_map(|row| row.ok()).collect::<Vec<i64>>())
                    .unwrap_or_default()
            })
            .ok_or("query_failed".to_string())
        })
        .await
        .unwrap_or(Err("spawn_failed".to_string()));

        let repair_ids = match repair_ids {
            Ok(ids) => {
                retry_count = 0;
                ids
            }
            Err(_) => {
                retry_count += 1;
                if retry_count > MAX_RETRIES {
                    eprintln!("[Backfill] Giving up after {} retries", MAX_RETRIES);
                    break;
                }
                let delay = std::time::Duration::from_secs(2u64.pow(retry_count));
                eprintln!("[Backfill] Retry {} in {:?}", retry_count, delay);
                tokio::time::sleep(delay).await;
                continue;
            }
        };

        if repair_ids.is_empty() {
            break;
        }

        let mut updated_in_batch = false;
        for clip_id in repair_ids {
            let app_for_task = app.clone();
            let updated =
                tokio::task::spawn_blocking(move || repair_image_clip(&app_for_task, clip_id))
                    .await;
            if updated.unwrap_or(false) {
                updated_in_batch = true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        }

        if updated_in_batch {
            any_updated = true;
            no_progress_rounds = 0;
        } else {
            no_progress_rounds += 1;
            if no_progress_rounds >= 2 {
                eprintln!(
                    "[Backfill] Stopping image metadata backfill after repeated no-progress rounds"
                );
                break;
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    }

    if any_updated {
        let _ = app.emit("clips-changed", ());
    }
}

// ─── Backfill Language Tags ───────────────────────────────────────

pub async fn backfill_language_tags(app: &tauri::AppHandle) {
    let mut any_updated = false;
    loop {
        let app_handle = app.clone();
        let updated = tokio::task::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            let rows: Vec<(i64, String)> = {
                let conn = match state.db_read_pool.get() {
                    Ok(conn) => conn,
                    Err(_) => return 0_usize,
                };
                conn.prepare(
                    "SELECT id,
                            CASE
                              WHEN content_type = 'image' THEN COALESCE(ocr_text, '')
                              ELSE content
                            END AS language_source
                     FROM clips
                     WHERE deleted = 0 AND (language IS NULL OR TRIM(language) = '')
                     ORDER BY created_at DESC
                     LIMIT 250",
                )
                .ok()
                .map(|mut stmt| {
                    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                        .map(|rows| rows.filter_map(|row| row.ok()).collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .unwrap_or_default()
            };

            let updates: Vec<(i64, &'static str)> = rows
                .into_iter()
                .filter_map(|(id, text)| {
                    crate::query_parser::detect_language(&text).map(|lang| (id, lang))
                })
                .collect();

            if updates.is_empty() {
                return 0_usize;
            }

            let conn = match state.db_write.lock() {
                Ok(conn) => conn,
                Err(_) => return 0_usize,
            };
            let mut applied = 0;
            for (clip_id, language) in updates {
                if conn
                    .execute(
                        "UPDATE clips SET language = ?1 WHERE id = ?2 AND deleted = 0 AND (language IS NULL OR TRIM(language) = '')",
                        rusqlite::params![language, clip_id],
                    )
                    .ok()
                    .unwrap_or(0)
                    > 0
                {
                    applied += 1;
                }
            }
            applied
        })
        .await
        .unwrap_or(0);

        if updated == 0 {
            break;
        }

        any_updated = true;
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }

    if any_updated {
        let _ = app.emit("clips-changed", ());
    }
}

// ─── Copy to Clipboard ────────────────────────────────────────────
// PERF 5: Split read/write — read from pool, set clipboard, async copy_count

#[tauri::command]
pub async fn copy_to_clipboard(app: tauri::AppHandle, clip_id: i64) -> Result<(), String> {
    // Phase 1: read data from pool (no write lock held)
    let (
        content_type,
        content,
        image_bytes,
        width,
        height,
        is_file,
        file_name,
        file_data,
        file_path,
    ) = {
        let conn = app
            .state::<AppState>()
            .db_read_pool
            .get()
            .map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT content_type,
                    COALESCE(content, ''),
                    COALESCE(image_data, X''),
                    COALESCE(image_width, 0),
                    COALESCE(image_height, 0),
                    COALESCE(is_file, 0),
                    COALESCE(file_name, ''),
                    COALESCE(file_data, X''),
                    COALESCE(file_path, '')
             FROM clips
             WHERE id = ? AND deleted = 0",
            [clip_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, String>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?
    };
    // Connection returned to pool

    // Phase 2: set clipboard (no DB lock held)
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;

    if is_file {
        let mut target_path = if !file_path.trim().is_empty() {
            PathBuf::from(file_path.trim())
        } else {
            PathBuf::new()
        };

        if target_path.as_os_str().is_empty() || !target_path.exists() {
            if file_data.is_empty() {
                return Err("File bytes unavailable for this clip".to_string());
            }
            let cache_root = app
                .path()
                .app_cache_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("copi"));
            let files_dir = cache_root.join("file-clips");
            let _ = std::fs::create_dir_all(&files_dir);

            let name = if file_name.trim().is_empty() {
                format!("clip-file-{}", clip_id)
            } else {
                file_name.clone()
            };
            target_path = files_dir.join(name);
            std::fs::write(&target_path, &file_data).map_err(|e| e.to_string())?;
        }

        clipboard
            .set()
            .file_list(&[target_path])
            .map_err(|e| format!("Failed to set file list: {}", e))?;
    } else if content_type == "image" {
        // PERF 1: Decode PNG back to RGBA
        if let Some((raw_bytes, w, h)) = png_to_rgba(&image_bytes) {
            let image = ImageData {
                width: w,
                height: h,
                bytes: Cow::Owned(raw_bytes),
            };
            clipboard
                .set_image(image)
                .map_err(|e| format!("Failed to set image: {}", e))?;
        } else if !image_bytes.is_empty() && width > 0 && height > 0 {
            // Fallback: might be legacy raw RGBA
            let image = ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Owned(image_bytes),
            };
            clipboard
                .set_image(image)
                .map_err(|e| format!("Failed to set image: {}", e))?;
        } else {
            return Err("Image data is empty".to_string());
        }
    } else {
        clipboard.set_text(&content).map_err(|e| e.to_string())?;
    }

    // Phase 3: increment copy_count and emit event — emit AFTER write completes
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(conn) = app_clone.state::<AppState>().db_write.lock() {
                let _ = conn.execute(
                    "UPDATE clips SET copy_count = COALESCE(copy_count, 0) + 1 WHERE id = ? AND deleted = 0",
                    [clip_id],
                );
            }
            let _ = app_clone.emit("clips-changed", ());
        })
        .await;
    });

    Ok(())
}

// ─── Batch Icon Retrieval (PERF 3) ────────────────────────────────

#[derive(Clone, serde::Serialize)]
pub struct ClipIconData {
    pub thumbnail: Option<String>,
    pub app_icon: Option<String>,
}

#[tauri::command]
pub async fn get_clip_icons_batch(
    app: tauri::AppHandle,
    clip_ids: Vec<i64>,
) -> Result<HashMap<i64, ClipIconData>, String> {
    if clip_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = app
        .state::<AppState>()
        .db_read_pool
        .get()
        .map_err(|e| e.to_string())?;

    let placeholders = clip_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!(
        "SELECT id, image_thumbnail, source_app_icon FROM clips WHERE deleted = 0 AND id IN ({})",
        placeholders
    );

    let mut result = HashMap::new();
    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(clip_ids.iter()), |row| {
            let id: i64 = row.get(0)?;
            let thumb: Option<Vec<u8>> = row.get(1).unwrap_or(None);
            let icon: Option<Vec<u8>> = row.get(2).unwrap_or(None);
            Ok((
                id,
                ClipIconData {
                    thumbnail: thumb.filter(|b| !b.is_empty()).map(|b| b64(&b)),
                    app_icon: icon.filter(|b| !b.is_empty()).map(|b| b64(&b)),
                },
            ))
        })
        .map_err(|e| e.to_string())?;

    for row in rows.filter_map(|r| r.ok()) {
        result.insert(row.0, row.1);
    }
    Ok(result)
}

// ─── Helpers ──────────────────────────────────────────────────────

fn compute_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn compute_hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
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
        if chunk.len() > 1 {
            encoded.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn decode_file_uri_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if !trimmed.starts_with("file://") {
        return None;
    }

    let rest = &trimmed[7..];
    let (authority, path_part) = if rest.starts_with('/') {
        ("", rest)
    } else {
        match rest.split_once('/') {
            Some((host, tail)) => (host, tail),
            None => (rest, ""),
        }
    };
    let authority_decoded = percent_decode_uri(authority);
    let path_decoded = percent_decode_uri(path_part);

    #[cfg(target_os = "windows")]
    {
        if !authority_decoded.is_empty() && authority_decoded != "localhost" {
            let tail = path_decoded.trim_start_matches('/').replace('/', "\\");
            if tail.is_empty() {
                return None;
            }
            return Some(PathBuf::from(format!(
                "\\\\{}\\{}",
                authority_decoded, tail
            )));
        }

        let normalized = path_decoded.trim_start_matches('/').replace('/', "\\");
        if normalized.is_empty() {
            return None;
        }
        Some(PathBuf::from(normalized))
    }

    #[cfg(not(target_os = "windows"))]
    {
        if !authority_decoded.is_empty() && authority_decoded != "localhost" {
            let full = format!(
                "//{}/{}",
                authority_decoded,
                path_decoded.trim_start_matches('/')
            );
            if full.len() <= 2 {
                return None;
            }
            return Some(PathBuf::from(full));
        }

        let decoded = if path_decoded.starts_with('/') {
            path_decoded
        } else {
            format!("/{}", path_decoded)
        };
        if decoded.is_empty() {
            return None;
        }
        Some(PathBuf::from(decoded))
    }
}

fn percent_decode_uri(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = (bytes[i + 1] as char).to_digit(16);
            let h2 = (bytes[i + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (h1, h2) {
                out.push(((a << 4) as u8) | (b as u8));
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_file_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(decode_file_uri_path)
        .filter(|path| path.exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_uri_decodes_spaces() {
        let decoded = percent_decode_uri("/tmp/my%20file.txt");
        assert_eq!(decoded, "/tmp/my file.txt");
    }

    #[test]
    fn parse_file_uri_list_only_keeps_existing_paths() {
        let temp = std::env::temp_dir().join("copi_clipboard_uri_test_existing.txt");
        std::fs::write(&temp, b"ok").unwrap();

        let existing_uri = format!("file://{}", temp.to_string_lossy());
        let text = format!(
            "{}\nfile:///definitely-not-existing-copi-path",
            existing_uri
        );
        let paths = parse_file_uri_list(&text);

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], temp);

        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn decode_file_uri_path_handles_localhost() {
        let temp = std::env::temp_dir().join("copi_clipboard_uri_test_localhost.txt");
        std::fs::write(&temp, b"ok").unwrap();

        let uri = format!("file://localhost{}", temp.to_string_lossy());
        let decoded = decode_file_uri_path(&uri).unwrap();

        assert_eq!(decoded, temp);
        let _ = std::fs::remove_file(&temp);
    }
}

// PERF 1: PNG encoding/decoding
fn rgba_to_png(bytes: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        if let Ok(mut writer) = encoder.write_header() {
            if writer.write_image_data(bytes).is_err() {
                return None;
            }
        } else {
            return None;
        }
    }
    if png_bytes.is_empty() {
        None
    } else {
        Some(png_bytes)
    }
}

fn png_to_rgba(png_bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    if png_bytes.is_empty() {
        return None;
    }
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let w = info.width as usize;
    let h = info.height as usize;
    Some((buf[..info.buffer_size()].to_vec(), w, h))
}

// ─── Queue / Process ──────────────────────────────────────────────

fn queue_text_capture(
    app: &tauri::AppHandle,
    text: String,
    hash: String,
    source_app: FrontmostApp,
) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            process_text_capture(&app_handle, text, hash, source_app)
        })
        .await;
    });
}

fn process_text_capture(
    app: &tauri::AppHandle,
    text: String,
    hash: String,
    source_app: FrontmostApp,
) {
    let content_type = detect_content_type(&text, None);
    let highlighted = if content_type == "code" {
        Some(highlight_code(&text))
    } else {
        None
    };
    let language = crate::query_parser::detect_language(&text);

    insert_clip(
        app,
        &text,
        &hash,
        &content_type,
        &source_app,
        highlighted.as_deref(),
        language,
    );
}

fn queue_image_capture(
    app: &tauri::AppHandle,
    payload: ClipboardImagePayload,
    hash: String,
    source_app: FrontmostApp,
) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            process_image_capture(&app_handle, payload, hash, source_app)
        })
        .await;
    });
}

fn process_image_capture(
    app: &tauri::AppHandle,
    payload: ClipboardImagePayload,
    hash: String,
    source_app: FrontmostApp,
) {
    let image = ImageData {
        width: payload.width,
        height: payload.height,
        bytes: Cow::Owned(payload.bytes),
    };
    let thumbnail = image_to_thumbnail(&image);
    let ocr_text = run_ocr(
        app,
        image.bytes.as_ref(),
        image.width as u32,
        image.height as u32,
    );
    let language = ocr_text
        .as_ref()
        .and_then(|text| crate::query_parser::detect_language(text));

    // PERF 1: Encode to PNG before storing (10-20x smaller than raw RGBA)
    let png_data = rgba_to_png(image.bytes.as_ref(), image.width, image.height);

    insert_image_clip(
        app,
        &image,
        png_data.as_deref(),
        thumbnail.as_deref(),
        &hash,
        &source_app,
        ocr_text.as_deref(),
        language,
    );
}

fn queue_file_capture(
    app: &tauri::AppHandle,
    file_list: Vec<PathBuf>,
    hash: String,
    source_app: FrontmostApp,
) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            process_file_capture(&app_handle, file_list, hash, source_app)
        })
        .await;
    });
}

fn process_file_capture(
    app: &tauri::AppHandle,
    file_list: Vec<PathBuf>,
    fallback_hash: String,
    source_app: FrontmostApp,
) {
    let path = match file_list.into_iter().find(|p| p.is_file()) {
        Some(path) => path,
        None => return,
    };

    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(_) => return,
    };
    let file_size = meta.len() as i64;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "file".to_string());

    let file_data = if file_size > 0 && file_size <= FILE_AUTO_SYNC_MAX_BYTES {
        std::fs::read(&path).ok()
    } else {
        None
    };

    let hash = if let Some(bytes) = file_data.as_ref() {
        compute_hash_bytes(bytes)
    } else {
        fallback_hash
    };

    let payload = ClipboardFilePayload {
        path,
        file_name,
        file_size,
        file_data,
    };

    insert_file_clip(app, &payload, &hash, &source_app);
}

fn repair_image_clip(app: &tauri::AppHandle, clip_id: i64) -> bool {
    let state = app.state::<AppState>();
    let (stored_bytes, width, height, existing_ocr, has_thumbnail): (
        Vec<u8>,
        i64,
        i64,
        Option<String>,
        bool,
    ) = {
        let conn = match state.db_read_pool.get() {
            Ok(conn) => conn,
            Err(_) => return false,
        };
        match conn.query_row(
            "SELECT image_data, image_width, image_height, ocr_text, length(COALESCE(image_thumbnail, X'')) > 0
             FROM clips
             WHERE id = ? AND content_type = 'image'",
            [clip_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ) {
            Ok(data) => data,
            Err(_) => return false,
        }
    };

    if stored_bytes.is_empty() {
        return false;
    }

    // Data may be PNG (new) or raw RGBA (legacy). Decode PNG if needed for OCR/thumbnail.
    let (raw_rgba, w, h) = if let Some((decoded, dw, dh)) = png_to_rgba(&stored_bytes) {
        (decoded, dw, dh)
    } else if width > 0 && height > 0 {
        // Legacy raw RGBA
        (stored_bytes, width as usize, height as usize)
    } else {
        return false;
    };

    let image = ImageData {
        width: w,
        height: h,
        bytes: Cow::Owned(raw_rgba),
    };
    let thumbnail = if has_thumbnail {
        None
    } else {
        image_to_thumbnail(&image)
    };
    let ocr_text = if existing_ocr
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
    {
        existing_ocr.clone()
    } else {
        run_ocr(app, image.bytes.as_ref(), w as u32, h as u32)
    };
    let language = ocr_text
        .as_ref()
        .and_then(|text| crate::query_parser::detect_language(text));

    let conn = match state.db_write.lock() {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    let ocr_for_db = ocr_text.as_deref().filter(|text| !text.trim().is_empty());
    let thumb_bytes = thumbnail.as_deref().unwrap_or(&[]);
    let updated = conn
        .execute(
            "UPDATE clips
             SET ocr_text = COALESCE(?1, ocr_text),
                 language = COALESCE(?2, language),
                 image_thumbnail = CASE
                     WHEN length(COALESCE(image_thumbnail, X'')) = 0 AND length(?3) > 0 THEN ?3
                     ELSE image_thumbnail
                 END,
                 image_width = CASE
                     WHEN COALESCE(image_width, 0) <= 0 AND ?4 > 0 THEN ?4
                     ELSE image_width
                 END,
                 image_height = CASE
                     WHEN COALESCE(image_height, 0) <= 0 AND ?5 > 0 THEN ?5
                     ELSE image_height
                 END
             WHERE id = ?6",
            rusqlite::params![
                ocr_for_db,
                language,
                thumb_bytes,
                w as i64,
                h as i64,
                clip_id
            ],
        )
        .ok()
        .unwrap_or(0)
        > 0;

    drop(conn);
    if updated && ocr_for_db.is_some() {
        enqueue_embedding(&state, clip_id, "repair_image_clip");
    }
    updated
}

fn run_ocr(app: &tauri::AppHandle, bytes: &[u8], width: u32, height: u32) -> Option<String> {
    if width < 3 || height < 3 {
        return None;
    }

    let state = app.state::<AppState>();
    let ocr = state.ocr_engine.as_ref()?;
    match ocr.recognize_text(bytes, width, height) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        Ok(_) => None,
        Err(error) => {
            let msg = error.to_string();
            if msg.contains("image is too small") {
                return None;
            }
            eprintln!("[OCR] Failed: {}", msg);
            None
        }
    }
}

// ─── Content Type Detection (CLASSIFY 1) ──────────────────────────

fn detect_content_type(content: &str, _source_app: Option<&str>) -> String {
    let trimmed = content.trim();

    // URL: starts with http(s):// and no newlines
    if !content.contains('\n')
        && (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
    {
        return "url".to_string();
    }

    // Single line is never code
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 2 {
        return "text".to_string();
    }

    let mut code_score: u32 = 0;

    for line in &lines {
        let lt = line.trim();

        // Strong indicators (+2)
        if lt.contains("fn ") && lt.contains('(') {
            code_score += 2;
        }
        if lt.contains("function ") && lt.contains('(') {
            code_score += 2;
        }
        if lt.contains("def ") && lt.contains('(') {
            code_score += 2;
        }
        if lt.contains("class ") && (lt.contains('{') || lt.contains(':')) {
            code_score += 2;
        }
        if lt.contains("async fn ") {
            code_score += 2;
        }
        if lt.starts_with("import {") || (lt.starts_with("from ") && lt.contains(" import ")) {
            code_score += 2;
        }
        if lt.starts_with("#include") {
            code_score += 2;
        }
        if lt.starts_with("```") {
            code_score += 2;
        }

        // Medium indicators (+1)
        if lt.starts_with("pub ")
            && (lt.contains("fn ")
                || lt.contains("struct ")
                || lt.contains("enum ")
                || lt.contains("impl ")
                || lt.contains("trait "))
        {
            code_score += 1;
        }
        if lt.ends_with('{')
            && (lt.contains("if ")
                || lt.contains("for ")
                || lt.contains("while ")
                || lt.contains("match ")
                || lt.contains("switch "))
        {
            code_score += 1;
        }
        if lt.ends_with(';') && lt.len() > 3 {
            code_score += 1;
        }
        if lt.contains("=>") && (lt.contains('(') || lt.contains("const ")) {
            code_score += 1;
        }
        if lt.contains(": ")
            && (lt.contains("String")
                || lt.contains("Vec<")
                || lt.contains("Option<")
                || lt.contains("i32")
                || lt.contains("i64")
                || lt.contains("f64")
                || lt.contains("bool"))
        {
            code_score += 1;
        }
        if lt == "}" || lt == "};" {
            code_score += 1;
        }
    }

    // Require at least 3 points for code classification
    if code_score >= 3 {
        return "code".to_string();
    }

    "text".to_string()
}

// ─── Code Highlighting ────────────────────────────────────────────

fn highlight_code(code: &str) -> String {
    use syntect::easy::HighlightLines;
    use syntect::highlighting::ThemeSet;
    use syntect::html::styled_line_to_highlighted_html;
    use syntect::parsing::SyntaxSet;

    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = ps
        .find_syntax_by_extension("txt")
        .unwrap_or_else(|| ps.find_syntax_plain_text());
    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let mut html = String::from("<pre style=\"margin:0\">");
    for line in code.lines() {
        if let Ok(regions) = h.highlight_line(line, &ps) {
            if let Ok(frag) =
                styled_line_to_highlighted_html(&regions, syntect::html::IncludeBackground::No)
            {
                html.push_str(&frag);
            }
        }
        html.push('\n');
    }
    html.push_str("</pre>");
    html
}

fn fetch_app_icon(state: &tauri::State<'_, AppState>, source_app: &FrontmostApp) -> Vec<u8> {
    if source_app.name.is_empty() || source_app.is_copi() {
        return Vec::new();
    }
    let _ = state;
    get_app_icon_png(source_app).unwrap_or_default()
}

fn enqueue_embedding(state: &tauri::State<'_, AppState>, clip_id: i64, reason: &'static str) {
    let tx = state.clip_tx.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tx.send(clip_id).await {
            eprintln!(
                "[EmbedQueue] Failed to enqueue clip {} ({}): {}",
                clip_id, reason, e
            );
        }
    });
}

// ─── Insert Clips ─────────────────────────────────────────────────

fn insert_clip(
    app: &tauri::AppHandle,
    content: &str,
    hash: &str,
    content_type: &str,
    source_app: &FrontmostApp,
    highlighted: Option<&str>,
    language: Option<&str>,
) {
    let state = app.state::<AppState>();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let capped = if content.len() > 100_000 {
        &content[..content
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 100_000)
            .last()
            .unwrap_or(0)]
    } else {
        content
    };

    let icon = fetch_app_icon(&state, source_app);
    let conn = state.db_write.lock().unwrap();
    let sync_id = uuid::Uuid::new_v4().to_string();
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let origin_device_id: Option<String> = conn
        .query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();

    let result = conn.execute(
        "INSERT INTO clips (content, content_hash, content_type, source_app, source_app_icon, content_highlighted, language, created_at, sync_id, sync_version, deleted, origin_device_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11)
         ON CONFLICT(content_hash) DO UPDATE SET
            source_app = CASE
                WHEN excluded.source_app <> '' THEN excluded.source_app
                ELSE clips.source_app
            END,
            source_app_icon = CASE
                WHEN length(excluded.source_app_icon) > 0 THEN excluded.source_app_icon
                ELSE clips.source_app_icon
            END,
            content_highlighted = COALESCE(excluded.content_highlighted, clips.content_highlighted),
            created_at = excluded.created_at,
            sync_id = COALESCE(clips.sync_id, excluded.sync_id),
            sync_version = excluded.sync_version,
            deleted = 0,
            origin_device_id = COALESCE(clips.origin_device_id, excluded.origin_device_id)",
        rusqlite::params![
            capped,
            hash,
            content_type,
            source_app.name,
            icon,
            highlighted,
            language,
            now,
            sync_id,
            sync_version,
            origin_device_id
        ],
    );

    if result.is_ok() {
        let clip_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM clips WHERE content_hash = ? AND deleted = 0",
                [hash],
                |row| row.get(0),
            )
            .ok();
        drop(conn);
        if let Some(clip_id) = clip_id {
            enqueue_embedding(&state, clip_id, "insert_clip");
        }
        let _ = app.emit("new-clip", ());
        let app_clone = app.clone();
        let hash_str = hash.to_string();
        tauri::async_runtime::spawn(async move {
            crate::sync::on_local_clip_saved(&app_clone, &hash_str).await;
        });
    }
}

fn insert_image_clip(
    app: &tauri::AppHandle,
    image_data: &ImageData,
    png_data: Option<&[u8]>,
    thumbnail: Option<&[u8]>,
    hash: &str,
    source_app: &FrontmostApp,
    ocr_text: Option<&str>,
    language: Option<&str>,
) {
    let state = app.state::<AppState>();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // PERF 1: Store PNG instead of raw RGBA
    let store_bytes = png_data.unwrap_or(image_data.bytes.as_ref());
    let width = image_data.width as i64;
    let height = image_data.height as i64;
    let thumb = thumbnail.unwrap_or(&[]);

    let icon = fetch_app_icon(&state, source_app);

    let conn = state.db_write.lock().unwrap();
    let sync_id = uuid::Uuid::new_v4().to_string();
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let origin_device_id: Option<String> = conn
        .query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();
    let result = conn.execute(
        "INSERT INTO clips (content, content_hash, content_type, source_app, source_app_icon, ocr_text, language, image_data, image_thumbnail, image_width, image_height, created_at, sync_id, sync_version, deleted, origin_device_id)
         VALUES ('[Image]', ?1, 'image', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13)
         ON CONFLICT(content_hash) DO UPDATE SET
            source_app = CASE
                WHEN excluded.source_app <> '' THEN excluded.source_app
                ELSE clips.source_app
            END,
            source_app_icon = CASE
                WHEN length(excluded.source_app_icon) > 0 THEN excluded.source_app_icon
                ELSE clips.source_app_icon
            END,
            ocr_text = COALESCE(excluded.ocr_text, clips.ocr_text),
            language = COALESCE(excluded.language, clips.language),
            image_data = COALESCE(excluded.image_data, clips.image_data),
            image_thumbnail = CASE
                WHEN length(excluded.image_thumbnail) > 0 THEN excluded.image_thumbnail
                ELSE clips.image_thumbnail
            END,
            image_width = CASE
                WHEN excluded.image_width > 0 THEN excluded.image_width
                ELSE clips.image_width
            END,
            image_height = CASE
                WHEN excluded.image_height > 0 THEN excluded.image_height
                ELSE clips.image_height
            END,
            created_at = excluded.created_at,
            sync_id = COALESCE(clips.sync_id, excluded.sync_id),
            sync_version = excluded.sync_version,
            deleted = 0,
            origin_device_id = COALESCE(clips.origin_device_id, excluded.origin_device_id)",
        rusqlite::params![
            hash,
            source_app.name,
            icon,
            ocr_text,
            language,
            store_bytes,
            thumb,
            width,
            height,
            now,
            sync_id,
            sync_version,
            origin_device_id
        ],
    );

    if result.is_ok() {
        let clip_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM clips WHERE content_hash = ? AND deleted = 0",
                [hash],
                |row| row.get(0),
            )
            .ok();
        drop(conn);
        if let (Some(clip_id), true) = (clip_id, ocr_text.is_some()) {
            enqueue_embedding(&state, clip_id, "insert_image_clip");
        }
        let _ = app.emit("new-clip", ());
        let app_clone = app.clone();
        let hash_str = hash.to_string();
        tauri::async_runtime::spawn(async move {
            crate::sync::on_local_clip_saved(&app_clone, &hash_str).await;
        });
    }
}

fn insert_file_clip(
    app: &tauri::AppHandle,
    payload: &ClipboardFilePayload,
    hash: &str,
    source_app: &FrontmostApp,
) {
    let state = app.state::<AppState>();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let icon = fetch_app_icon(&state, source_app);
    let file_path = payload.path.to_string_lossy().to_string();

    let conn = state.db_write.lock().unwrap();
    let sync_id = uuid::Uuid::new_v4().to_string();
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let origin_device_id: Option<String> = conn
        .query_row("SELECT device_id FROM device_info LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();

    let result = conn.execute(
        "INSERT INTO clips (content, content_hash, content_type, source_app, source_app_icon, created_at, sync_id, sync_version, deleted, origin_device_id, is_file, file_name, file_size, file_data, file_path)
         VALUES (?1, ?2, 'text', ?3, ?4, ?5, ?6, ?7, 0, ?8, 1, ?9, ?10, ?11, ?12)
         ON CONFLICT(content_hash) DO UPDATE SET
            source_app = CASE
                WHEN excluded.source_app <> '' THEN excluded.source_app
                ELSE clips.source_app
            END,
            source_app_icon = CASE
                WHEN length(excluded.source_app_icon) > 0 THEN excluded.source_app_icon
                ELSE clips.source_app_icon
            END,
            created_at = excluded.created_at,
            sync_id = COALESCE(clips.sync_id, excluded.sync_id),
            sync_version = excluded.sync_version,
            deleted = 0,
            origin_device_id = COALESCE(clips.origin_device_id, excluded.origin_device_id),
            is_file = 1,
            file_name = COALESCE(excluded.file_name, clips.file_name),
            file_size = CASE WHEN excluded.file_size > 0 THEN excluded.file_size ELSE clips.file_size END,
            file_data = COALESCE(excluded.file_data, clips.file_data),
            file_path = COALESCE(excluded.file_path, clips.file_path)",
        rusqlite::params![
            payload.file_name,
            hash,
            source_app.name,
            icon,
            now,
            sync_id,
            sync_version,
            origin_device_id,
            payload.file_name,
            payload.file_size,
            payload.file_data,
            file_path,
        ],
    );

    if result.is_ok() {
        drop(conn);
        let _ = app.emit("new-clip", ());
        if payload.file_data.is_some() {
            let app_clone = app.clone();
            let hash_str = hash.to_string();
            tauri::async_runtime::spawn(async move {
                crate::sync::on_local_clip_saved(&app_clone, &hash_str).await;
            });
        }
    }
}

fn image_to_thumbnail(image_data: &ImageData) -> Option<Vec<u8>> {
    let width = image_data.width as u32;
    let height = image_data.height as u32;
    let bytes = image_data.bytes.as_ref();

    let scale = if width > 200 || height > 200 {
        200.0 / width.max(height) as f32
    } else {
        1.0
    };

    let new_width = (width as f32 * scale) as u32;
    let new_height = (height as f32 * scale) as u32;

    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, new_width, new_height);
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
                    if src_idx + 3 < bytes.len() && dst_idx + 3 < resized.len() {
                        resized[dst_idx..dst_idx + 4].copy_from_slice(&bytes[src_idx..src_idx + 4]);
                    }
                }
            }
            let _ = writer.write_image_data(&resized);
        }
    }

    if png_data.is_empty() {
        None
    } else {
        Some(png_data)
    }
}
