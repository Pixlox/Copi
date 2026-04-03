//! LAN Sync Module for Copi

pub mod discovery;
pub mod engine;
pub mod protocol;

// Re-export next_sync_version for backward compatibility
pub use engine::next_sync_version_public as next_sync_version;

// Re-export functions that need to be called from other modules
pub use engine::{
    apply_config_change,
    initialize_sync_if_enabled,
    on_local_clip_saved,
    on_collection_changed,
};

// Tauri commands are referenced directly as sync::engine::func_name in main.rs
