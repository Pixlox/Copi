//! Sync Protocol Messages
//!
//! Defines all message types exchanged between devices during sync.
//! Messages are serialized as JSON and sent through the encrypted transport.

use serde::{Deserialize, Serialize};

use super::pairing::PairingMessage;

/// Top-level sync message wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncMessage {
    /// Pairing-related messages
    Pairing(PairingMessage),

    /// Request sync state from peer (what's your latest version?)
    SyncRequest(SyncRequest),

    /// Response with sync state
    SyncState(SyncState),

    /// Push operations to peer (Phase 1: metadata without full images)
    PushOperations(PushOperations),

    /// Acknowledge receipt of operations
    Ack(AckMessage),

    /// Push full image data for a clip (Phase 2: images)
    PushImageData(ImageDataMessage),

    /// Request image data for clips (by sync_id)
    RequestImages(RequestImagesMessage),

    /// Heartbeat to keep connection alive
    Heartbeat,

    /// Graceful disconnect
    Disconnect,
}

impl SyncMessage {
    /// Serialize to JSON bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Request the peer's sync state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    /// Our current sync version (highest version we have)
    pub our_version: i64,
    /// Entity types we want to sync
    pub entity_types: Vec<EntityType>,
}

/// Peer's sync state response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// Peer's current sync version
    pub version: i64,
    /// Number of clips
    pub clip_count: u32,
    /// Number of collections
    pub collection_count: u32,
    /// Versions by entity type (for delta sync)
    pub entity_versions: Vec<(EntityType, i64)>,
}

/// Push operations to peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushOperations {
    /// The operations to apply
    pub operations: Vec<SyncOperation>,
    /// Expected version after applying (for conflict detection)
    pub target_version: i64,
}

/// Acknowledgement message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckMessage {
    /// Whether the operations were applied successfully
    pub success: bool,
    /// New sync version after applying
    pub new_version: Option<i64>,
    /// Error message if failed
    pub error: Option<String>,
    /// IDs of operations that conflicted (if any)
    pub conflicts: Vec<String>,
    /// Clips that need image data (sync_ids) - used after Phase 1
    #[serde(default)]
    pub needs_images: Vec<String>,
}

/// Push full image data for a clip (Phase 2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageDataMessage {
    /// The clip's sync_id
    pub sync_id: String,
    /// Full image data (PNG/JPEG bytes)
    pub image_data: Vec<u8>,
    /// Optional: source app icon
    pub source_app_icon: Option<Vec<u8>>,
}

/// Request image data for specific clips
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestImagesMessage {
    /// List of clip sync_ids to request images for
    pub sync_ids: Vec<String>,
}

/// Type of entity being synced
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Clip,
    Collection,
    ClipEmbedding,
}

/// A single sync operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SyncOperation {
    /// Create or update a clip
    UpsertClip(ClipData),

    /// Delete a clip
    DeleteClip { sync_id: String, version: i64 },

    /// Create or update a collection
    UpsertCollection(CollectionData),

    /// Delete a collection
    DeleteCollection { sync_id: String, version: i64 },

    /// Update clip embedding
    UpsertEmbedding {
        clip_sync_id: String,
        embedding: Vec<f32>,
        version: i64,
    },

    /// Move clip to collection
    MoveClipToCollection {
        clip_sync_id: String,
        collection_sync_id: Option<String>,
        version: i64,
    },

    /// Pin/unpin clip
    SetClipPinned {
        sync_id: String,
        pinned: bool,
        version: i64,
    },
}

impl SyncOperation {
    /// Get the sync version of this operation
    pub fn version(&self) -> i64 {
        match self {
            SyncOperation::UpsertClip(data) => data.sync_version,
            SyncOperation::DeleteClip { version, .. } => *version,
            SyncOperation::UpsertCollection(data) => data.sync_version,
            SyncOperation::DeleteCollection { version, .. } => *version,
            SyncOperation::UpsertEmbedding { version, .. } => *version,
            SyncOperation::MoveClipToCollection { version, .. } => *version,
            SyncOperation::SetClipPinned { version, .. } => *version,
        }
    }
}

/// Full clip data for sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipData {
    pub sync_id: String,
    pub sync_version: i64,
    pub content: String,
    pub content_hash: String,
    pub content_type: String,
    pub source_app: Option<String>,
    pub source_app_icon: Option<Vec<u8>>,
    pub content_highlighted: Option<String>,
    pub ocr_text: Option<String>,
    pub image_data: Option<Vec<u8>>,
    pub image_thumbnail: Option<Vec<u8>>,
    pub image_width: Option<i32>,
    pub image_height: Option<i32>,
    pub language: Option<String>,
    pub created_at: i64,
    pub pinned: bool,
    pub copy_count: i32,
    pub collection_sync_id: Option<String>,
    pub origin_device_id: String,
    /// Embedding vector (384 dimensions for multilingual-e5-small)
    pub embedding: Option<Vec<f32>>,
}

/// Full collection data for sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionData {
    pub sync_id: String,
    pub sync_version: i64,
    pub name: String,
    pub color: Option<String>,
    pub created_at: i64,
    pub origin_device_id: Option<String>,
}

/// Conflict resolution strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStrategy {
    /// Last write wins (based on sync_version)
    LastWriteWins,
    /// Keep both versions (create duplicate)
    KeepBoth,
    /// Prefer local version
    PreferLocal,
    /// Prefer remote version
    PreferRemote,
}

impl Default for ConflictStrategy {
    fn default() -> Self {
        Self::LastWriteWins
    }
}
