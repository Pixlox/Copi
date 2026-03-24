use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::ShortcutState;

mod clipboard;
mod collections;
mod db;
mod embed;
mod hotkey;
mod macos;
mod ocr;
mod privacy;
mod query_parser;
mod search;
mod settings;

pub struct AppState {
    pub db_read: Mutex<rusqlite::Connection>,
    pub db_write: Mutex<rusqlite::Connection>,
    pub model: Option<std::sync::Arc<embed::EmbeddingModel>>,
    pub ocr_engine: Option<Box<dyn ocr::OcrEngine>>,
    pub clip_tx: tokio::sync::mpsc::Sender<i64>,
    pub clipboard_watcher_running: Mutex<bool>,
    pub previous_frontmost_app: Mutex<Option<String>>,
    pub search_generation: AtomicU64,
    pub search_status: Mutex<search::SearchStatusPayload>,
}

pub struct MenuBarState {
    pub tray_icon: Mutex<Option<TrayIcon<tauri::Wry>>>,
}

// ─── NSPanel Definition (EcoPaste pattern) ────────────────────────

#[cfg(target_os = "macos")]
use tauri_nspanel::{
    tauri_panel, CollectionBehavior, ManagerExt, PanelLevel, StyleMask, WebviewWindowExt,
};

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(OverlayPanel {
        config: {
            is_floating_panel: true,
            can_become_key_window: true,
            can_become_main_window: false
        }
    })

    panel_event!(OverlayPanelEventHandler {
        window_did_resign_key(notification: &NSNotification) -> ()
    })
}

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let _ = shortcut;
                    if event.state == ShortcutState::Pressed {
                        toggle_overlay(app);
                    }
                })
                .build(),
        )
        .on_window_event(|_window, event| match event {
            tauri::WindowEvent::Focused(focused) => {
                #[cfg(not(target_os = "macos"))]
                if _window.label() == "overlay" && !*focused {
                    hide_overlay_inner(_window.app_handle(), false);
                }
                #[cfg(target_os = "macos")]
                let _ = focused;
            }
            _ => {}
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let handle = app.handle();
            eprintln!("[Copi] Starting up...");

            // Register NSPanel plugin INSIDE setup (not in builder chain)
            // This is critical for macOS 26 Tahoe — prevents PAC crash
            #[cfg(target_os = "macos")]
            {
                let _ = handle.plugin(tauri_nspanel::init());
                eprintln!("[Copi] NSPanel plugin registered");
            }

            // Desktop plugins
            #[cfg(desktop)]
            {
                handle.plugin(tauri_plugin_updater::Builder::new().build())?;
                handle.plugin(tauri_plugin_dialog::init())?;
                handle.plugin(tauri_plugin_process::init())?;
                handle.plugin(tauri_plugin_autostart::Builder::new().build())?;
            }

            // Initialize database (dual connections for read/write separation)
            let db_conns = db::init_db(handle).expect("Failed to initialize database");

            // Initialize ONNX model
            let model = embed::init_model(handle);
            match &model {
                Ok(m) => eprintln!("[Copi] Model loaded ({}d)", m.dimensions),
                Err(e) => eprintln!("[Copi] Model: {}", e),
            }

            let (clip_tx, clip_rx) = tokio::sync::mpsc::channel::<i64>(512);
            let model_arc = model.ok();

            // Initialize OCR
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

            app.manage(AppState {
                db_read: Mutex::new(db_conns.read),
                db_write: Mutex::new(db_conns.write),
                model: model_arc.clone(),
                ocr_engine,
                clip_tx: clip_tx.clone(),
                clipboard_watcher_running: Mutex::new(true),
                previous_frontmost_app: Mutex::new(None),
                search_generation: AtomicU64::new(0),
                search_status: Mutex::new(search::SearchStatusPayload {
                    phase: if model_arc.is_some() { "warming".into() } else { "idle".into() },
                    queued_items: 0,
                    completed_items: 0,
                    total_items: 0,
                    semantic_ready: model_arc.is_some(),
                }),
            });
            app.manage(MenuBarState {
                tray_icon: Mutex::new(None),
            });

            // Delay heavy historical repair slightly so launch and first search stay responsive.
            let delayed_handle = handle.clone();
            let delayed_clip_tx = clip_tx.clone();
            let delayed_has_model = model_arc.is_some();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                if delayed_has_model {
                    let _ = embed::backfill_embeddings(&delayed_handle, &delayed_clip_tx).await;
                }
            });

            let delayed_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                clipboard::backfill_language_tags(&delayed_handle).await;
            });

            let delayed_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(18)).await;
                clipboard::backfill_image_metadata(&delayed_handle).await;
            });

            // Spawn workers
            let ah = handle.clone();
            tauri::async_runtime::spawn(async move {
                embed::embedding_worker(model_arc, clip_rx, ah).await;
            });
            let ah = handle.clone();
            tauri::async_runtime::spawn(async move {
                clipboard::watch_clipboard(&ah).await;
            });
            let ah = handle.clone();
            tauri::async_runtime::spawn(async move {
                let ah2 = ah.clone();
                let _ = tokio::task::spawn_blocking(move || cleanup_old_clips(&ah2)).await;
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    interval.tick().await;
                    let ah2 = ah.clone();
                    let _ = tokio::task::spawn_blocking(move || cleanup_old_clips(&ah2)).await;
                }
            });

            // Convert overlay to NSPanel (inside setup — EcoPaste pattern)
            #[cfg(target_os = "macos")]
            {
                if let Some(overlay) = handle.get_webview_window("overlay") {
                    match overlay.to_panel::<OverlayPanel>() {
                        Ok(panel) => {
                            panel.set_level(PanelLevel::Dock.value());
                            panel.set_style_mask(
                                StyleMask::empty().nonactivating_panel().resizable().into(),
                            );
                            panel.set_collection_behavior(hidden_overlay_space_behavior().into());
                            panel.set_corner_radius(16.0);
                            panel.set_has_shadow(true);

                            let handler = OverlayPanelEventHandler::new();
                            let app_for_hide = handle.clone();
                            handler.window_did_resign_key(move |_| {
                                hide_overlay_inner(&app_for_hide, false);
                            });
                            panel.set_event_handler(Some(handler.as_ref()));

                            eprintln!("[Copi] NSPanel configured (fullscreen overlay)");
                        }
                        Err(e) => eprintln!("[Copi] NSPanel conversion failed: {:?}", e),
                    }
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
                    eprintln!("[Copi] Vibrancy applied");
                }
                let _ = overlay.center();
            }

            // Tray icon
            let settings_item =
                MenuItem::with_id(handle, "settings", "Settings\u{2026}", true, None::<&str>)?;
            let pause_item =
                MenuItem::with_id(handle, "pause", "Pause Monitoring", true, None::<&str>)?;
            let quit = MenuItem::with_id(handle, "quit", "Quit Copi", true, None::<&str>)?;
            let menu = Menu::with_items(
                handle,
                &[
                    &settings_item,
                    &PredefinedMenuItem::separator(handle)?,
                    &pause_item,
                    &PredefinedMenuItem::separator(handle)?,
                    &quit,
                ],
            )?;

            let mut tray_builder = TrayIconBuilder::with_id("copi-menubar")
                .menu(&menu)
                .tooltip("Copi")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "settings" => {
                        if let Some(w) = app.get_webview_window("settings") {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "pause" => {
                        let state = app.state::<AppState>();
                        let mut running = state.clipboard_watcher_running.lock().unwrap();
                        *running = !*running;
                        if *running {
                            eprintln!("[Tray] Clipboard monitoring resumed");
                        } else {
                            eprintln!("[Tray] Clipboard monitoring paused");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });

            #[cfg(target_os = "macos")]
            {
                tray_builder = tray_builder
                    .icon(build_menubar_icon())
                    .icon_as_template(true);
            }

            #[cfg(not(target_os = "macos"))]
            if let Some(default_icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(default_icon);
            }

            let tray = tray_builder.build(app)?;
            let _ = tray.set_visible(true);
            if let Ok(mut guard) = app.state::<MenuBarState>().tray_icon.lock() {
                *guard = Some(tray);
            }

            register_initial_hotkey(app)?;

            eprintln!("[Copi] Ready. Press hotkey to open overlay.");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search::search_clips,
            search::get_total_clip_count,
            search::get_search_status,
            search::get_image_thumbnail,
            search::get_image_preview,
            search::toggle_pin,
            search::delete_clip,
            search::update_clip_content,
            clipboard::copy_to_clipboard,
            show_overlay,
            hide_overlay,
            settings::get_config,
            settings::set_config,
            settings::get_db_size,
            settings::clear_all_history,
            settings::export_history_json,
            collections::create_collection,
            collections::delete_collection,
            collections::rename_collection,
            collections::list_collections,
            collections::move_clip_to_collection,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ─── Cleanup ──────────────────────────────────────────────────────

fn cleanup_old_clips(app: &tauri::AppHandle) {
    let retention_days = match settings::get_config_sync(app.clone()) {
        Ok(c) => c.general.history_retention_days,
        Err(_) => return,
    };
    if retention_days <= 0 {
        return;
    }
    let cutoff = chrono::Utc::now().timestamp() - (retention_days * 86400);
    let state = app.state::<AppState>();
    let Ok(conn) = state.db_write.try_lock() else {
        return;
    };
    let Ok(_) = conn.execute(
        "DELETE FROM clip_embeddings WHERE rowid IN (SELECT id FROM clips WHERE created_at < ?1 AND pinned = 0)",
        [cutoff],
    ) else {
        return;
    };
    let Ok(count) = conn.execute(
        "DELETE FROM clips WHERE created_at < ?1 AND pinned = 0",
        [cutoff],
    ) else {
        return;
    };
    if count > 0 {
        eprintln!("[Cleanup] Removed {} old clips", count);
    }
}

// ─── Overlay Toggle (EcoPaste pattern: run_on_main_thread) ────────

fn toggle_overlay(app: &tauri::AppHandle) {
    // Check current visibility via NSPanel
    #[cfg(target_os = "macos")]
    {
        if let Ok(panel) = app.get_webview_panel("overlay") {
            if panel.is_visible() {
                hide_overlay_inner(app, false);
            } else {
                show_overlay_inner(app);
            }
            return;
        }
    }
    // Fallback for non-macOS
    if let Some(window) = app.get_webview_window("overlay") {
        if window.is_visible().unwrap_or(false) {
            hide_overlay_inner(app, false);
        } else {
            show_overlay_inner(app);
        }
    }
}

fn show_overlay_inner(app: &tauri::AppHandle) {
    // Save previous frontmost app for paste-on-select
    if let Ok(mut guard) = app.state::<AppState>().previous_frontmost_app.try_lock() {
        *guard = crate::macos::get_frontmost_app_bundle_id();
    }

    #[cfg(target_os = "macos")]
    {
        let app_clone = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Ok(panel) = app_clone.get_webview_panel("overlay") {
                panel.show_and_make_key();
                panel.set_collection_behavior(shown_overlay_space_behavior().into());
            }
            if let Some(window) = app_clone.get_webview_window("overlay") {
                let _ = window.unminimize();
                let _ = window.emit("overlay:shown", ());
            }
        });
        return;
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.show();
        let _ = window.set_always_on_top(true);
        let _ = window.set_focus();
        let _ = window.emit("overlay:shown", ());
    }
}

fn hide_overlay_inner(app: &tauri::AppHandle, paste: bool) {
    #[cfg(target_os = "macos")]
    {
        let app_clone = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Ok(panel) = app_clone.get_webview_panel("overlay") {
                panel.hide();
                panel.set_collection_behavior(hidden_overlay_space_behavior().into());
            }
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.hide();
        }
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

#[cfg(target_os = "macos")]
fn shown_overlay_space_behavior() -> CollectionBehavior {
    CollectionBehavior::new()
        .stationary()
        .can_join_all_spaces()
        .full_screen_auxiliary()
}

#[cfg(target_os = "macos")]
fn hidden_overlay_space_behavior() -> CollectionBehavior {
    CollectionBehavior::new()
        .stationary()
        .move_to_active_space()
        .full_screen_auxiliary()
}

fn register_initial_hotkey(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let configured = settings::get_config_sync(app.handle().clone())
        .map(|config| config.general.hotkey)
        .unwrap_or_else(|_| "alt+space".to_string());

    match hotkey::register_hotkey(app.handle(), &configured) {
        Ok(registered) => {
            eprintln!("[Copi] Hotkey registered: {}", registered);
            Ok(())
        }
        Err(error) => {
            eprintln!("[Copi] Hotkey '{}' failed: {}", configured, error);
            let fallback = "ctrl+shift+space";
            let registered =
                hotkey::register_hotkey(app.handle(), fallback).map_err(|fallback_error| {
                    format!(
                        "failed to register '{}' ({}) and fallback '{}' ({})",
                        configured, error, fallback, fallback_error
                    )
                })?;
            eprintln!("[Copi] Hotkey registered: {} (fallback)", registered);
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
fn build_menubar_icon() -> tauri::image::Image<'static> {
    // Tauri tray icons accept raster data here, so we rasterize the same
    // two-card glyph used by icons/copi-menubar.svg.
    let width = 44usize;
    let height = 44usize;
    let mut rgba = vec![0u8; width * height * 4];

    draw_rounded_rect(&mut rgba, width, height, 16.0, 12.0, 22.0, 26.0, 5.0, 0.4);
    draw_rounded_rect(&mut rgba, width, height, 6.0, 6.0, 22.0, 26.0, 5.0, 1.0);

    tauri::image::Image::new_owned(rgba, width as u32, height as u32)
}

#[cfg(target_os = "macos")]
fn draw_rounded_rect(
    rgba: &mut [u8],
    canvas_width: usize,
    canvas_height: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    alpha: f32,
) {
    for py in 0..canvas_height {
        for px in 0..canvas_width {
            let coverage = rounded_rect_coverage(
                px as f32 + 0.5,
                py as f32 + 0.5,
                x,
                y,
                width,
                height,
                radius,
            );
            if coverage <= 0.0 {
                continue;
            }

            let idx = (py * canvas_width + px) * 4;
            let src_alpha = alpha * coverage;
            let dst_alpha = rgba[idx + 3] as f32 / 255.0;
            let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);

            rgba[idx] = 255;
            rgba[idx + 1] = 255;
            rgba[idx + 2] = 255;
            rgba[idx + 3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[cfg(target_os = "macos")]
fn rounded_rect_coverage(
    px: f32,
    py: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> f32 {
    let half_width = width / 2.0;
    let half_height = height / 2.0;
    let center_x = x + half_width;
    let center_y = y + half_height;
    let dx = (px - center_x).abs() - (half_width - radius);
    let dy = (py - center_y).abs() - (half_height - radius);
    let ax = dx.max(0.0);
    let ay = dy.max(0.0);
    let distance = (ax * ax + ay * ay).sqrt() - radius;

    (0.5 - distance).clamp(0.0, 1.0)
}

// ─── macOS Helpers ────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn restore_previous_app(app: &tauri::AppHandle) {
    if let Some(Some(id)) = app
        .state::<AppState>()
        .previous_frontmost_app
        .try_lock()
        .ok()
        .map(|g| g.clone())
    {
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
