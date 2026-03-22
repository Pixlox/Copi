use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

mod clipboard;
mod db;
mod embed;
mod hotkey;
mod ocr;
mod privacy;
mod query_parser;
mod search;
mod settings;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub model: Option<std::sync::Arc<embed::EmbeddingModel>>,
    pub ocr_engine: Option<Box<dyn ocr::OcrEngine>>,
    pub clip_tx: tokio::sync::mpsc::Sender<i64>,
    pub clipboard_watcher_running: Mutex<bool>,
    pub previous_frontmost_app: Mutex<Option<String>>,
}

// ─── NSPanel Definition (EcoPaste pattern) ────────────────────────

#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, Panel, PanelLevel, StyleMask, WebviewWindowExt};

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(OverlayPanel {
        config: {
            is_floating_panel: true,
            can_become_key_window: true,
            can_become_main_window: false,
            hides_on_deactivate: true,
        }
    })
}

// ─── Main ─────────────────────────────────────────────────────────

fn main() {
    let mut builder = tauri::Builder::default();

    // Register NSPanel plugin (macOS only)
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        if shortcut.matches(Modifiers::ALT, Code::Space) {
                            toggle_overlay(app);
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle();

            eprintln!("[Copi] Starting up...");

            // Initialize database
            let conn = db::init_db(handle).expect("Failed to initialize database");

            // Initialize ONNX embedding model
            let model = embed::init_model(handle);
            match &model {
                Ok(m) => eprintln!("[Copi] Model loaded ({}d)", m.dimensions),
                Err(e) => eprintln!("[Copi] Model: {}", e),
            }

            let (clip_tx, clip_rx) = tokio::sync::mpsc::channel::<i64>(200);
            let model_arc = model.ok();

            // Initialize OCR engine
            let ocr_engine = match ocr::init_ocr_engine() {
                Ok(engine) => {
                    eprintln!("[OCR] Engine initialized");
                    Some(engine)
                }
                Err(e) => {
                    eprintln!("[OCR] Not available: {}", e);
                    None
                }
            };

            let state = AppState {
                db: Mutex::new(conn),
                model: model_arc.clone(),
                ocr_engine,
                clip_tx: clip_tx.clone(),
                clipboard_watcher_running: Mutex::new(true),
                previous_frontmost_app: Mutex::new(None),
            };
            app.manage(state);

            // Register global hotkey
            let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
            match app.global_shortcut().register(shortcut) {
                Ok(_) => eprintln!("[Copi] Hotkey: Option+Space"),
                Err(e) => eprintln!("[Copi] Hotkey failed: {}", e),
            }

            // Backfill existing clips that don't have embeddings
            if model_arc.is_some() {
                embed::backfill_embeddings(handle, &clip_tx);
            }

            // Spawn embedding worker
            let ah = handle.clone();
            tauri::async_runtime::spawn(async move {
                embed::embedding_worker(model_arc, clip_rx, ah).await;
            });

            // Spawn clipboard watcher
            let ah = handle.clone();
            tauri::async_runtime::spawn(async move {
                clipboard::watch_clipboard(&ah).await;
            });

            // ── Convert overlay to NSPanel (macOS only, EcoPaste pattern) ──
            #[cfg(target_os = "macos")]
            {
                if let Some(overlay) = handle.get_webview_window("overlay") {
                    match overlay.to_panel::<OverlayPanel>() {
                        Ok(panel) => {
                            panel.set_level(PanelLevel::Dock.value());
                            panel.set_style_mask(
                                StyleMask::empty()
                                    .nonactivating_panel()
                                    .into(),
                            );
                            panel.set_collection_behavior(
                                tauri_nspanel::CollectionBehavior::new()
                                    .stationary()
                                    .move_to_active_space()
                                    .full_screen_auxiliary()
                                    .into(),
                            );
                    panel.set_hides_on_deactivate(true);
                    panel.set_corner_radius(16.0);
                    panel.set_has_shadow(true);
                            eprintln!("[Copi] NSPanel configured (fullscreen overlay)");
                        }
                        Err(e) => {
                            eprintln!("[Copi] NSPanel conversion FAILED: {:?}", e);
                        }
                    }
                } else {
                    eprintln!("[Copi] Overlay window NOT FOUND");
                }
            }

            // Apply vibrancy with rounded corners
            if let Some(overlay) = handle.get_webview_window("overlay") {
                #[cfg(target_os = "macos")]
                {
                    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                    let _ = apply_vibrancy(
                        &overlay,
                        NSVisualEffectMaterial::HudWindow,
                        None,
                        Some(12.0),
                    );
                    eprintln!("[Copi] Vibrancy applied (radius: 12px)");
                }
                let _ = overlay.center();
            }

            // Tray icon
            let open_item = MenuItem::with_id(handle, "open", "Open Copi", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(handle, "settings", "Settings\u{2026}", true, None::<&str>)?;
            let quit_item =
                MenuItem::with_id(handle, "quit", "Quit Copi", true, None::<&str>)?;

            let menu = Menu::with_items(handle, &[
                &open_item,
                &PredefinedMenuItem::separator(handle)?,
                &settings_item,
                &PredefinedMenuItem::separator(handle)?,
                &quit_item,
            ])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Copi \u{2014} Clipboard Manager")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => toggle_overlay(app),
                    "settings" => {
                        if let Some(win) = app.get_webview_window("settings") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            eprintln!("[Copi] Ready. Option+Space to open.");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search::search_clips,
            search::get_total_clip_count,
            search::get_image_thumbnail,
            clipboard::copy_to_clipboard,
            show_overlay,
            hide_overlay,
            settings::get_config,
            settings::set_config,
            settings::get_db_size,
            settings::clear_all_history,
            settings::export_history_json,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ─── Overlay Toggle ───────────────────────────────────────────────

fn toggle_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let is_visible = window.is_visible().unwrap_or(false);
        if is_visible {
            hide_overlay_inner(app, false);
        } else {
            show_overlay_inner(app);
        }
    }
}

fn show_overlay_inner(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let frontmost = get_frontmost_app_bundle_id();
        let state = app.state::<AppState>();
        *state.previous_frontmost_app.lock().unwrap() = frontmost;
    }

    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.show();
        let _ = window.set_always_on_top(true);
        let _ = window.set_focus();
        let _ = window.eval("setTimeout(() => document.querySelector('input')?.focus(), 50)");
    }
}

fn hide_overlay_inner(app: &tauri::AppHandle, paste: bool) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
    }

    if paste {
        #[cfg(target_os = "macos")]
        {
            restore_previous_app(app);
            simulate_paste();
        }
    }
}

#[tauri::command]
fn show_overlay(app: tauri::AppHandle) {
    show_overlay_inner(&app);
}

#[tauri::command]
fn hide_overlay(app: tauri::AppHandle, paste: bool) {
    hide_overlay_inner(&app, paste);
}

// ─── macOS Helpers ────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn get_frontmost_app_bundle_id() -> Option<String> {
    use std::process::Command;
    Command::new("osascript")
        .arg("-e")
        .arg("id of application (path to frontmost application as text)")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

#[cfg(not(target_os = "macos"))]
fn get_frontmost_app_bundle_id() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn restore_previous_app(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let bundle_id = state.previous_frontmost_app.lock().unwrap().clone();
    if let Some(id) = bundle_id {
        let _ = std::process::Command::new("open")
            .arg("-b")
            .arg(&id)
            .spawn();
    }
}

#[cfg(target_os = "macos")]
fn simulate_paste() {
    std::thread::sleep(std::time::Duration::from_millis(50));
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to keystroke \"v\" using command down")
        .spawn();
}


