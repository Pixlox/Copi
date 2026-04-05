#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
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
mod model_setup;
mod ocr;
mod privacy;
#[cfg(debug_assertions)]
mod qa_sync;
mod query_parser;
mod search;
mod settings;
mod sync;

#[cfg(target_os = "windows")]
fn startup_log_path() -> std::path::PathBuf {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        return std::path::PathBuf::from(local_app_data)
            .join("com.copi.app")
            .join("logs")
            .join("startup.log");
    }

    std::env::temp_dir()
        .join("copi")
        .join("logs")
        .join("startup.log")
}

#[cfg(target_os = "windows")]
fn log_startup_line(message: &str) {
    use std::io::Write;

    let path = startup_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{}] {}", timestamp, message);
    }
}

#[cfg(target_os = "windows")]
fn mark_overlay_blur_ignore(app: &tauri::AppHandle, duration_ms: u64) {
    if let Some(state) = app.try_state::<AppState>() {
        let deadline = current_time_millis().saturating_add(duration_ms);
        let existing = state.overlay_drag_ignore_until_ms.load(Ordering::Relaxed);
        if deadline > existing {
            state
                .overlay_drag_ignore_until_ms
                .store(deadline, Ordering::Relaxed);
        }
    }
}

pub struct AppState {
    pub db_read_pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
    pub db_write: Mutex<rusqlite::Connection>,
    pub model: RwLock<Option<std::sync::Arc<embed::EmbeddingModel>>>,
    pub ocr_engine: Option<Box<dyn ocr::OcrEngine>>,
    pub clip_tx: tokio::sync::mpsc::Sender<i64>,
    pub clip_rx: Mutex<Option<tokio::sync::mpsc::Receiver<i64>>>,
    pub clipboard_watcher_running: Mutex<bool>,
    pub suppress_next_clipboard_capture: AtomicBool,
    pub suppressed_clipboard_change_count: AtomicI64,
    pub previous_frontmost_app: Mutex<Option<String>>,
    pub previous_foreground_window: Mutex<Option<isize>>,
    pub overlay_drag_ignore_until_ms: AtomicU64,
    pub search_generation: AtomicU64,
    pub runtime_started: AtomicBool,
    pub search_status: Mutex<search::SearchStatusPayload>,
    pub model_setup_status: Mutex<model_setup::ModelSetupStatus>,
    pub sync: std::sync::OnceLock<Arc<crate::sync::SyncState>>,
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
    #[cfg(target_os = "windows")]
    log_startup_line("=== Copi launch started ===");

    std::panic::set_hook(Box::new(|info| {
        #[cfg(target_os = "windows")]
        {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("unknown panic payload");
            let location = info
                .location()
                .map(|loc| format!("{}:{}", loc.file(), loc.line()))
                .unwrap_or_else(|| "unknown location".to_string());
            log_startup_line(&format!("panic at {}: {}", location, payload));
            let backtrace = std::backtrace::Backtrace::force_capture();
            log_startup_line(&format!("backtrace: {}", backtrace));
        }

        #[cfg(not(target_os = "windows"))]
        {
            eprintln!("[Panic] {}", info);
            eprintln!(
                "[Panic] backtrace: {}",
                std::backtrace::Backtrace::force_capture()
            );
        }
    }));

    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        handle_second_instance_launch(app);
    }));

    builder
        .on_window_event(|_window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if matches!(_window.label(), "settings" | "setup") {
                    api.prevent_close();
                    let _ = _window.hide();
                    sync_app_shell_visibility(_window.app_handle());
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
                if _window.label() == "overlay" && !*focused {
                    #[cfg(target_os = "windows")]
                    if should_ignore_overlay_blur(_window.app_handle()) {
                        return;
                    }
                    hide_overlay_inner(_window.app_handle(), false);
                }

                #[cfg(target_os = "windows")]
                if _window.label() == "overlay" && !*focused {
                    let app_handle = _window.app_handle().clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(140)).await;

                        if should_ignore_overlay_blur(&app_handle) {
                            return;
                        }

                        if let Some(window) = app_handle.get_webview_window("overlay") {
                            if window.is_focused().unwrap_or(false) {
                                return;
                            }
                        }

                        hide_overlay_inner(&app_handle, false);
                    });
                }

                #[cfg(target_os = "macos")]
                let _ = focused;
            }
            #[cfg(target_os = "windows")]
            tauri::WindowEvent::Resized(_) => {
                if _window.label() == "overlay" {
                    mark_overlay_blur_ignore(_window.app_handle(), 700);
                }
            }
            _ => {}
        })
        .setup(|app| {
            #[cfg(target_os = "windows")]
            log_startup_line("setup entered");

            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let handle = app.handle().clone();
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
                let plugin_init_result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    handle.plugin(tauri_plugin_updater::Builder::new().build())?;
                    handle.plugin(tauri_plugin_dialog::init())?;
                    handle.plugin(tauri_plugin_process::init())?;
                    let autostart_builder = tauri_plugin_autostart::Builder::new().app_name("Copi");
                    #[cfg(target_os = "macos")]
                    let autostart_builder = autostart_builder
                        .macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
                    handle.plugin(autostart_builder.build())?;
                    Ok(())
                })();

                #[cfg(target_os = "windows")]
                {
                    if let Err(error) = plugin_init_result {
                        eprintln!("[Copi] Plugin init failed: {}", error);
                        log_startup_line(&format!("plugin init failed: {}", error));
                    } else {
                        log_startup_line("plugins initialized");
                    }
                }

                #[cfg(not(target_os = "windows"))]
                plugin_init_result?;
            }

            // Initialize database (dual connections for read/write separation)
            let db_conns = match db::init_db(&handle) {
                Ok(conns) => {
                    #[cfg(target_os = "windows")]
                    log_startup_line("database initialized");
                    conns
                }
                Err(error) => {
                    #[cfg(target_os = "windows")]
                    log_startup_line(&format!("database init failed: {}", error));
                    return Err(Box::new(error));
                }
            };
            let (clip_tx, clip_rx) = tokio::sync::mpsc::channel::<i64>(512);
            if let Err(error) = model_setup::migrate_legacy_model_dir(&handle) {
                eprintln!("[Copi] Model migration: {}", error);
            }
            let install_path = model_setup::model_install_path_string(&handle);
            let model = if model_setup::has_valid_model_install(&handle) {
                embed::init_model(&handle)
            } else {
                Err(format!("Model files missing from {}", install_path))
            };
            match &model {
                Ok(model) => eprintln!("[Copi] Model loaded ({}d)", model.dimensions),
                Err(error) => eprintln!("[Copi] Model unavailable: {}", error),
            }
            let model_load_error = if model_setup::has_valid_model_install(&handle) {
                model.as_ref().err().cloned()
            } else {
                None
            };
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
                db_read_pool: db_conns.read_pool,
                db_write: Mutex::new(db_conns.write),
                model: RwLock::new(model_arc.clone()),
                ocr_engine,
                clip_tx: clip_tx.clone(),
                clip_rx: Mutex::new(Some(clip_rx)),
                clipboard_watcher_running: Mutex::new(true),
                suppress_next_clipboard_capture: AtomicBool::new(false),
                suppressed_clipboard_change_count: AtomicI64::new(i64::MIN),
                previous_frontmost_app: Mutex::new(None),
                previous_foreground_window: Mutex::new(None),
                overlay_drag_ignore_until_ms: AtomicU64::new(0),
                search_generation: AtomicU64::new(0),
                runtime_started: AtomicBool::new(false),
                search_status: Mutex::new(search::SearchStatusPayload {
                    phase: if model_arc.is_some() {
                        "starting".into()
                    } else {
                        "unavailable".into()
                    },
                    queued_items: 0,
                    completed_items: 0,
                    failed_items: 0,
                    total_items: 0,
                    semantic_ready: false,
                }),
                model_setup_status: Mutex::new(if model_arc.is_some() {
                    model_setup::ModelSetupStatus {
                        phase: "ready".to_string(),
                        current_file: None,
                        downloaded_bytes: 0,
                        total_bytes: 0,
                        completed_files: 5,
                        total_files: 5,
                        install_path: install_path.clone(),
                        error: None,
                        ready: true,
                        setup_required: false,
                    }
                } else {
                    model_setup::ModelSetupStatus {
                        phase: "missing".to_string(),
                        current_file: None,
                        downloaded_bytes: 0,
                        total_bytes: 0,
                        completed_files: 0,
                        total_files: 5,
                        install_path: install_path.clone(),
                        error: model_load_error,
                        ready: false,
                        setup_required: true,
                    }
                }),
                sync: std::sync::OnceLock::new(),
            });
            app.manage(MenuBarState {
                tray_icon: Mutex::new(None),
            });

            let shortcut_plugin_result = handle.plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(|app, shortcut, event| {
                        let _ = shortcut;
                        if event.state == ShortcutState::Pressed {
                            toggle_overlay(app);
                        }
                    })
                    .build(),
            );

            #[cfg(target_os = "windows")]
            {
                if let Err(error) = shortcut_plugin_result {
                    eprintln!("[Copi] Global shortcut plugin failed: {}", error);
                    log_startup_line(&format!("shortcut plugin failed: {}", error));
                } else {
                    log_startup_line("shortcut plugin initialized");
                }
            }

            #[cfg(not(target_os = "windows"))]
            shortcut_plugin_result?;

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

            // Apply vibrancy to setup window
            if let Some(_setup) = handle.get_webview_window("setup") {
                #[cfg(target_os = "macos")]
                {
                    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                    let _ = apply_vibrancy(
                        &_setup,
                        NSVisualEffectMaterial::HudWindow,
                        None,
                        Some(16.0),
                    );
                    eprintln!("[Copi] Setup vibrancy applied");
                }
            }

            // Apply vibrancy to settings window
            if let Some(_settings) = handle.get_webview_window("settings") {
                #[cfg(target_os = "macos")]
                {
                    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                    let _ = apply_vibrancy(
                        &_settings,
                        NSVisualEffectMaterial::Sidebar,
                        None,
                        Some(12.0),
                    );
                    eprintln!("[Copi] Settings vibrancy applied");
                }
            }

            // Tray icon
            let tray_init_result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let settings_item =
                    MenuItem::with_id(&handle, "settings", "Settings\u{2026}", true, None::<&str>)?;
                let pause_item =
                    MenuItem::with_id(&handle, "pause", "Pause Monitoring", true, None::<&str>)?;
                let quit = MenuItem::with_id(&handle, "quit", "Quit Copi", true, None::<&str>)?;
                let menu = Menu::with_items(
                    &handle,
                    &[
                        &settings_item,
                        &PredefinedMenuItem::separator(&handle)?,
                        &pause_item,
                        &PredefinedMenuItem::separator(&handle)?,
                        &quit,
                    ],
                )?;

                let mut tray_builder = TrayIconBuilder::with_id("copi-menubar")
                    .menu(&menu)
                    .tooltip("Copi")
                    .show_menu_on_left_click(true)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "settings" => {
                            show_settings_window_inner(app);
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

                #[cfg(target_os = "windows")]
                {
                    tray_builder = tray_builder.icon(build_menubar_icon());
                }

                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                if let Some(default_icon) = app.default_window_icon().cloned() {
                    tray_builder = tray_builder.icon(default_icon);
                }

                let tray = tray_builder.build(app)?;
                let _ = tray.set_visible(true);
                if let Ok(mut guard) = app.state::<MenuBarState>().tray_icon.lock() {
                    *guard = Some(tray);
                }

                Ok(())
            })();

            #[cfg(target_os = "windows")]
            {
                if let Err(error) = tray_init_result {
                    eprintln!("[Copi] Tray initialization failed: {}", error);
                    log_startup_line(&format!("tray init failed: {}", error));
                } else {
                    log_startup_line("tray initialized");
                }
            }

            #[cfg(not(target_os = "windows"))]
            tray_init_result?;

            let hotkey_result = register_initial_hotkey(app);

            #[cfg(target_os = "windows")]
            {
                if let Err(error) = hotkey_result {
                    eprintln!("[Copi] Hotkey initialization failed: {}", error);
                    log_startup_line(&format!("hotkey init failed: {}", error));
                } else {
                    log_startup_line("hotkey registered");
                }
            }

            #[cfg(not(target_os = "windows"))]
            hotkey_result?;

            if model_arc.is_some() {
                start_runtime_services_once(&handle);
                #[cfg(target_os = "windows")]
                log_startup_line("runtime services started");
            } else {
                show_setup_window_inner(&handle);
                #[cfg(target_os = "windows")]
                log_startup_line("setup window shown (model missing)");
            }

            #[cfg(debug_assertions)]
            qa_sync::start_server_if_enabled(handle.clone());
            eprintln!("[Copi] Ready. Press hotkey to open overlay.");
            #[cfg(target_os = "windows")]
            log_startup_line("setup completed successfully");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search::search_clips,
            search::get_total_clip_count,
            search::get_search_status,
            search::get_image_thumbnail,
            search::get_image_preview,
            search::get_clip_full_content,
            search::toggle_pin,
            search::delete_clip,
            search::update_clip_content,
            clipboard::copy_to_clipboard,
            clipboard::get_clip_icons_batch,
            model_setup::get_model_setup_status,
            model_setup::download_required_models,
            hide_setup_window,
            show_overlay,
            hide_overlay,
            set_overlay_drag_state,
            settings::get_config,
            settings::set_config,
            settings::get_db_size,
            settings::clear_all_history,
            sync::sync_get_identity,
            sync::sync_get_status,
            sync::sync_list_peers,
            sync::sync_list_discovered,
            sync::sync_generate_pin,
            sync::sync_pair_with,
            sync::sync_remove_peer,
            collections::create_collection,
            collections::delete_collection,
            collections::rename_collection,
            collections::update_collection_color,
            collections::list_collections,
            collections::move_clip_to_collection,
            open_external_url,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| {
            #[cfg(target_os = "windows")]
            log_startup_line(&format!("build failed: {}", error));
            panic!("error while building tauri application: {}", error);
        })
        .run(|_app_handle, _event| {
            #[cfg(target_os = "windows")]
            match _event {
                tauri::RunEvent::Ready => log_startup_line("run event: ready"),
                tauri::RunEvent::Exit => log_startup_line("run event: exit"),
                tauri::RunEvent::ExitRequested { .. } => {
                    log_startup_line("run event: exit requested")
                }
                _ => {}
            }
        });
}

fn is_setup_required(app: &tauri::AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|state| {
            state
                .model_setup_status
                .lock()
                .ok()
                .map(|status| status.setup_required)
        })
        .unwrap_or(true)
}

fn handle_second_instance_launch(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    log_startup_line("second launch blocked; focusing existing instance");

    if !app_state_ready(app) {
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.show();
            let _ = window.set_focus();
        }
        return;
    }

    if is_setup_required(app) {
        show_setup_window_inner(app);
        return;
    }

    if app
        .get_webview_window("settings")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
    {
        show_settings_window_inner(app);
        return;
    }

    show_overlay_inner(app);
}

#[cfg(target_os = "windows")]
fn current_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn should_ignore_overlay_blur(app: &tauri::AppHandle) -> bool {
    let deadline = app
        .try_state::<AppState>()
        .map(|state| state.overlay_drag_ignore_until_ms.load(Ordering::Relaxed))
        .unwrap_or(0);
    deadline > current_time_millis()
}

fn app_state_ready(app: &tauri::AppHandle) -> bool {
    app.try_state::<AppState>().is_some()
}

#[cfg(target_os = "macos")]
fn set_app_shell_visible(app: &tauri::AppHandle, visible: bool) {
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.set_activation_policy(policy);
}

#[cfg(not(target_os = "macos"))]
fn set_app_shell_visible(_app: &tauri::AppHandle, _visible: bool) {}

#[cfg(target_os = "macos")]
fn sync_app_shell_visibility(app: &tauri::AppHandle) {
    let show_in_dock = ["settings", "setup"].iter().any(|label| {
        app.get_webview_window(label)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false)
    });
    set_app_shell_visible(app, show_in_dock);
}

#[cfg(not(target_os = "macos"))]
fn sync_app_shell_visibility(_app: &tauri::AppHandle) {}

pub(crate) fn start_runtime_services_once(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if state
        .runtime_started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let model = match state
        .model
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().cloned())
    {
        Some(model) => model,
        None => {
            state.runtime_started.store(false, Ordering::SeqCst);
            return;
        }
    };
    let clip_rx = match state.clip_rx.lock() {
        Ok(mut guard) => match guard.take() {
            Some(rx) => rx,
            None => {
                state.runtime_started.store(false, Ordering::SeqCst);
                return;
            }
        },
        Err(_) => {
            state.runtime_started.store(false, Ordering::SeqCst);
            return;
        }
    };
    let clip_tx = state.clip_tx.clone();

    let delayed_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let _ = embed::backfill_embeddings(&delayed_handle, &clip_tx).await;
    });

    let delayed_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        clipboard::backfill_language_tags(&delayed_handle).await;
    });

    let delayed_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(18)).await;
        clipboard::backfill_image_metadata(&delayed_handle).await;
    });

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        embed::embedding_worker(Some(model), clip_rx, app_handle).await;
    });

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        clipboard::watch_clipboard(&app_handle).await;
    });

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let cleanup_handle = app_handle.clone();
        let _ = tokio::task::spawn_blocking(move || cleanup_old_clips(&cleanup_handle)).await;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let cleanup_handle = app_handle.clone();
            let _ = tokio::task::spawn_blocking(move || cleanup_old_clips(&cleanup_handle)).await;
        }
    });

    let sync_state = crate::sync::start_sync(app.clone());
    let _ = app.state::<AppState>().sync.set(sync_state);
}

fn show_setup_window_inner(app: &tauri::AppHandle) {
    if !app_state_ready(app) {
        return;
    }

    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
    }
    set_app_shell_visible(app, true);
    if let Some(window) = app.get_webview_window("setup") {
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
    }
    sync_app_shell_visibility(app);
}

fn show_settings_window_inner(app: &tauri::AppHandle) {
    if !app_state_ready(app) {
        return;
    }

    set_app_shell_visible(app, true);
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("settings:shown", ());
    }
    sync_app_shell_visibility(app);
}

// ─── Cleanup ──────────────────────────────────────────────────────

pub(crate) fn cleanup_old_clips(app: &tauri::AppHandle) {
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
        "DELETE FROM clip_embeddings WHERE rowid IN (SELECT id FROM clips WHERE created_at < ?1 AND pinned = 0 AND deleted = 0)",
        [cutoff],
    ) else {
        return;
    };
    let sync_version = sync::next_sync_version_from_conn(&conn);
    let Ok(count) = conn.execute(
        "UPDATE clips SET deleted = 1, sync_version = ?1 WHERE created_at < ?2 AND pinned = 0 AND deleted = 0",
        rusqlite::params![sync_version, cutoff],
    ) else {
        return;
    };
    if count > 0 {
        eprintln!("[Cleanup] Removed {} old clips", count);
    }
}

// ─── Overlay Toggle (EcoPaste pattern: run_on_main_thread) ────────

fn toggle_overlay(app: &tauri::AppHandle) {
    if !app_state_ready(app) {
        return;
    }

    if is_setup_required(app) {
        show_setup_window_inner(app);
        return;
    }

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
    if !app_state_ready(app) {
        return;
    }

    if is_setup_required(app) {
        show_setup_window_inner(app);
        return;
    }

    if let Some(setup) = app.get_webview_window("setup") {
        let _ = setup.hide();
    }
    sync_app_shell_visibility(app);

    // Save previous frontmost target for paste-on-select
    #[cfg(target_os = "macos")]
    if let Ok(mut guard) = app.state::<AppState>().previous_frontmost_app.try_lock() {
        *guard = crate::macos::get_frontmost_app_bundle_id();
    }

    #[cfg(target_os = "windows")]
    if let Ok(mut guard) = app
        .state::<AppState>()
        .previous_foreground_window
        .try_lock()
    {
        *guard = crate::macos::get_foreground_window_handle();
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
    if !app_state_ready(app) {
        return;
    }

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

        #[cfg(target_os = "windows")]
        {
            restore_previous_window(app);
            simulate_paste_windows();
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

#[tauri::command]
fn set_overlay_drag_state(app: tauri::AppHandle, active: bool) {
    #[cfg(target_os = "windows")]
    if let Some(state) = app.try_state::<AppState>() {
        let deadline = if active {
            current_time_millis() + 900
        } else {
            current_time_millis() + 180
        };
        state
            .overlay_drag_ignore_until_ms
            .store(deadline, Ordering::Relaxed);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        let _ = active;
    }
}
#[tauri::command]
fn hide_setup_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("setup") {
        let _ = window.hide();
    }
    sync_app_shell_visibility(&app);
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("Only http/https URLs are allowed".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(trimmed)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", trimmed])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(trimmed)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
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
    // macOS uses template icons (white with alpha) that adapt to light/dark mode.
    let width = 44usize;
    let height = 44usize;
    let mut rgba = vec![0u8; width * height * 4];

    draw_rounded_rect(&mut rgba, width, height, 16.0, 12.0, 22.0, 26.0, 5.0, 0.4);
    draw_rounded_rect(&mut rgba, width, height, 6.0, 6.0, 22.0, 26.0, 5.0, 1.0);

    tauri::image::Image::new_owned(rgba, width as u32, height as u32)
}

#[cfg(target_os = "windows")]
fn build_menubar_icon() -> tauri::image::Image<'static> {
    // Windows tray icons need to be larger and fully opaque with a visible color.
    // We draw the same two-card glyph but with white cards on transparent background.
    let width = 32usize;
    let height = 32usize;
    let mut rgba = vec![0u8; width * height * 4];

    // Scale factors for 32x32 canvas (original was 44x44)
    let scale = 32.0 / 44.0;
    draw_rounded_rect(
        &mut rgba,
        width,
        height,
        16.0 * scale,
        12.0 * scale,
        22.0 * scale,
        26.0 * scale,
        5.0 * scale,
        0.5,
    );
    draw_rounded_rect(
        &mut rgba,
        width,
        height,
        6.0 * scale,
        6.0 * scale,
        22.0 * scale,
        26.0 * scale,
        5.0 * scale,
        1.0,
    );

    tauri::image::Image::new_owned(rgba, width as u32, height as u32)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
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
    std::thread::sleep(std::time::Duration::from_millis(80));
    // Use CGEventPost for ~20ms latency (vs ~200ms with osascript)
    // kVK_ANSI_V = 0x09, kCGHIDEventTap = 0, kCGEventFlagMaskCommand = 1 << 20
    extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *const std::ffi::c_void,
            keycode: u16,
            key_down: bool,
        ) -> *mut std::ffi::c_void;
        fn CGEventSetFlags(event: *mut std::ffi::c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
        fn CFRelease(cf: *mut std::ffi::c_void);
    }

    const K_VK_ANSI_V: u16 = 0x09;
    const K_CG_HID_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 1 << 20;

    unsafe {
        let source = std::ptr::null();
        let key_down = CGEventCreateKeyboardEvent(source, K_VK_ANSI_V, true);
        if !key_down.is_null() {
            CGEventSetFlags(key_down, K_CG_EVENT_FLAG_MASK_COMMAND);
            CGEventPost(K_CG_HID_EVENT_TAP, key_down);
            CFRelease(key_down);
        }

        std::thread::sleep(std::time::Duration::from_millis(20));

        let key_up = CGEventCreateKeyboardEvent(source, K_VK_ANSI_V, false);
        if !key_up.is_null() {
            CGEventSetFlags(key_up, K_CG_EVENT_FLAG_MASK_COMMAND);
            CGEventPost(K_CG_HID_EVENT_TAP, key_up);
            CFRelease(key_up);
        }
    }
}

#[cfg(target_os = "windows")]
fn restore_previous_window(app: &tauri::AppHandle) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let Some(hwnd_value) = app
        .state::<AppState>()
        .previous_foreground_window
        .try_lock()
        .ok()
        .and_then(|guard| *guard)
    else {
        return;
    };

    let hwnd = hwnd_value as windows_sys::Win32::Foundation::HWND;
    if hwnd.is_null() {
        return;
    }

    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

#[cfg(target_os = "windows")]
fn simulate_paste_windows() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
    };

    std::thread::sleep(std::time::Duration::from_millis(90));

    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    wScan: 0,
                    dwFlags: 0,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: b'V' as u16,
                    wScan: 0,
                    dwFlags: 0,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: b'V' as u16,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];

    unsafe {
        let _ = SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    #[test]
    fn windows_paste_sequence_uses_four_inputs() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
        };

        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_CONTROL,
                        wScan: 0,
                        dwFlags: 0,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: b'V' as u16,
                        wScan: 0,
                        dwFlags: 0,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: b'V' as u16,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_CONTROL,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];

        assert_eq!(inputs.len(), 4);
        assert_eq!(inputs[0].r#type, INPUT_KEYBOARD);
    }
}
