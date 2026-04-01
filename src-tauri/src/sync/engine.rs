//! Sync Engine
//!
//! Orchestrates sync operations between devices:
//! - Manages connections to paired devices
//! - Processes incoming sync operations
//! - Pushes local changes to peers
//! - Handles conflict resolution
//! - Auto-reconnects when devices come back online

use rusqlite::{Connection, OptionalExtension};

use super::protocol::{ClipData, CollectionData, ConflictStrategy, SyncOperation};
use super::SyncResult;

/// Global sync version counter key in settings
const SYNC_VERSION_KEY: &str = "sync_version";

/// The sync engine manages all sync operations
pub struct SyncEngine;

impl SyncEngine {
    /// Get the current global sync version
    pub fn get_sync_version(conn: &Connection) -> SyncResult<i64> {
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [SYNC_VERSION_KEY],
                |row| row.get(0),
            )
            .optional()?;

        Ok(version.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    /// Increment and return the new sync version
    pub fn next_sync_version(conn: &Connection) -> SyncResult<i64> {
        let current = Self::get_sync_version(conn)?;
        let next = current + 1;

        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![SYNC_VERSION_KEY, next.to_string()],
        )?;

        Ok(next)
    }
    /// Get operations newer than the given version
    pub fn get_operations_since(
        conn: &Connection,
        since_version: i64,
        device_id: &str,
        limit: usize,
        include_embeddings: bool,
    ) -> SyncResult<Vec<SyncOperation>> {
        let mut operations = Vec::new();

        // Get clips modified since the version
        let mut clip_stmt = conn.prepare(
            "SELECT sync_id, sync_version, content, content_hash, content_type,
                    source_app, source_app_icon, content_highlighted, ocr_text,
                    image_data, image_thumbnail, image_width, image_height,
                    language, created_at, pinned, copy_count, collection_id, origin_device_id, deleted
             FROM clips
             WHERE sync_version > ?1 AND sync_id IS NOT NULL
             ORDER BY sync_version ASC
             LIMIT ?2",
        )?;

        let clips = clip_stmt.query_map(rusqlite::params![since_version, limit], |row| {
            let deleted: i64 = row.get(19)?;
            let sync_id: String = row.get(0)?;
            let sync_version: i64 = row.get(1)?;

            if deleted == 1 {
                Ok(SyncOperation::DeleteClip {
                    sync_id,
                    version: sync_version,
                })
            } else {
                // Get collection sync_id if present
                let collection_id: Option<i64> = row.get(17)?;
                let collection_sync_id: Option<String> = if let Some(cid) = collection_id {
                    conn.query_row(
                        "SELECT sync_id FROM collections WHERE id = ?1",
                        [cid],
                        |r| r.get(0),
                    )
                    .ok()
                } else {
                    None
                };

                Ok(SyncOperation::UpsertClip(ClipData {
                    sync_id,
                    sync_version,
                    content: row.get(2)?,
                    content_hash: row.get(3)?,
                    content_type: row.get(4)?,
                    source_app: row.get(5)?,
                    source_app_icon: None, // Exclude app icons from sync (too large)
                    content_highlighted: row.get(7)?,
                    ocr_text: row.get(8)?,
                    image_data: None, // Exclude full images from sync - only sync thumbnails
                    image_thumbnail: row.get(10)?,
                    image_width: row.get(11)?,
                    image_height: row.get(12)?,
                    language: row.get(13)?,
                    created_at: row.get(14)?,
                    pinned: row.get::<_, i64>(15)? == 1,
                    copy_count: row.get(16)?,
                    collection_sync_id,
                    origin_device_id: row
                        .get::<_, Option<String>>(18)?
                        .unwrap_or_else(|| device_id.to_string()),
                    embedding: None, // Will be fetched separately
                }))
            }
        })?;

        for clip in clips {
            if let Ok(op) = clip {
                if include_embeddings {
                    if let SyncOperation::UpsertClip(ref data) = op {
                        let clip_id: Option<i64> = conn
                            .query_row(
                                "SELECT id FROM clips WHERE sync_id = ?1",
                                [&data.sync_id],
                                |row| row.get(0),
                            )
                            .optional()?;
                        if let Some(clip_id) = clip_id {
                            let embedding_blob: Option<Vec<u8>> = conn
                                .query_row(
                                    "SELECT embedding FROM clip_embeddings WHERE rowid = ?1",
                                    [clip_id],
                                    |row| row.get(0),
                                )
                                .optional()?;
                            if let Some(blob) = embedding_blob {
                                if blob.len() % 4 == 0 {
                                    let embedding: Vec<f32> = blob
                                        .chunks_exact(4)
                                        .map(|chunk| {
                                            f32::from_le_bytes([
                                                chunk[0], chunk[1], chunk[2], chunk[3],
                                            ])
                                        })
                                        .collect();
                                    operations.push(SyncOperation::UpsertEmbedding {
                                        clip_sync_id: data.sync_id.clone(),
                                        embedding,
                                        version: data.sync_version,
                                    });
                                }
                            }
                        }
                    }
                }
                operations.push(op);
            }
        }

        // Get collections modified since the version
        let mut coll_stmt = conn.prepare(
            "SELECT sync_id, sync_version, name, color, created_at, deleted, origin_device_id
             FROM collections
             WHERE sync_version > ?1 AND sync_id IS NOT NULL
             ORDER BY sync_version ASC
             LIMIT ?2",
        )?;

        let collections = coll_stmt.query_map(
            rusqlite::params![since_version, limit - operations.len()],
            |row| {
                let deleted: i64 = row.get(5)?;
                let sync_id: String = row.get(0)?;
                let sync_version: i64 = row.get(1)?;

                if deleted == 1 {
                    Ok(SyncOperation::DeleteCollection {
                        sync_id,
                        version: sync_version,
                    })
                } else {
                    Ok(SyncOperation::UpsertCollection(CollectionData {
                        sync_id,
                        sync_version,
                        name: row.get(2)?,
                        color: row.get(3)?,
                        created_at: row.get(4)?,
                        origin_device_id: row.get(6)?,
                    }))
                }
            },
        )?;

        for coll in collections {
            if let Ok(op) = coll {
                operations.push(op);
            }
        }

        // Sort by version
        operations.sort_by_key(|op| op.version());

        Ok(operations)
    }

    /// Get image data for specific clips (Phase 2 of sync)
    /// Returns a list of (sync_id, image_data, source_app_icon)
    pub fn get_image_data_for_clips(
        conn: &Connection,
        sync_ids: &[String],
    ) -> SyncResult<Vec<super::protocol::ImageDataMessage>> {
        if sync_ids.is_empty() {
            return Ok(vec![]);
        }

        let placeholders = sync_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT sync_id, image_data, source_app_icon 
             FROM clips 
             WHERE sync_id IN ({}) AND image_data IS NOT NULL",
            placeholders
        );

        let mut stmt = conn.prepare(&query)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            sync_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

        let results = stmt.query_map(params.as_slice(), |row| {
            Ok(super::protocol::ImageDataMessage {
                sync_id: row.get(0)?,
                image_data: row.get(1)?,
                source_app_icon: row.get(2)?,
            })
        })?;

        let mut images = vec![];
        for result in results {
            if let Ok(img) = result {
                images.push(img);
            }
        }

        Ok(images)
    }

    /// Get clips that have image_data but haven't been synced to a device yet
    /// Returns sync_ids of clips with images that need Phase 2 sync
    #[allow(dead_code)]
    pub fn get_clips_needing_image_sync(
        conn: &Connection,
        _device_id: &str,
        since_version: i64,
        limit: usize,
    ) -> SyncResult<Vec<String>> {
        // Get clips that:
        // 1. Have image_data
        // 2. Were created/modified since the version
        // 3. Haven't had their image synced yet (tracked via a separate table or flag)
        let mut stmt = conn.prepare(
            "SELECT sync_id FROM clips 
             WHERE sync_version > ?1 
             AND image_data IS NOT NULL 
             AND sync_id IS NOT NULL
             ORDER BY sync_version ASC
             LIMIT ?2",
        )?;

        let sync_ids: Vec<String> = stmt
            .query_map(rusqlite::params![since_version, limit], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(sync_ids)
    }

    /// Apply a sync operation from a remote peer
    pub fn apply_operation(
        conn: &Connection,
        operation: &SyncOperation,
        conflict_strategy: ConflictStrategy,
    ) -> SyncResult<bool> {
        match operation {
            SyncOperation::UpsertClip(data) => {
                Self::apply_upsert_clip(conn, data, conflict_strategy)
            }
            SyncOperation::DeleteClip { sync_id, version } => {
                Self::apply_delete_clip(conn, sync_id, *version, conflict_strategy)
            }
            SyncOperation::UpsertCollection(data) => {
                Self::apply_upsert_collection(conn, data, conflict_strategy)
            }
            SyncOperation::DeleteCollection { sync_id, version } => {
                Self::apply_delete_collection(conn, sync_id, *version, conflict_strategy)
            }
            SyncOperation::UpsertEmbedding {
                clip_sync_id,
                embedding,
                version,
            } => Self::apply_upsert_embedding(conn, clip_sync_id, embedding, *version),
            SyncOperation::MoveClipToCollection {
                clip_sync_id,
                collection_sync_id,
                version,
            } => Self::apply_move_clip(conn, clip_sync_id, collection_sync_id.as_deref(), *version),
            SyncOperation::SetClipPinned {
                sync_id,
                pinned,
                version,
            } => Self::apply_set_pinned(conn, sync_id, *pinned, *version),
        }
    }

    /// Apply upsert clip operation
    fn apply_upsert_clip(
        conn: &Connection,
        data: &ClipData,
        conflict_strategy: ConflictStrategy,
    ) -> SyncResult<bool> {
        // Check if clip exists
        let existing: Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, sync_version FROM clips WHERE sync_id = ?1",
                [&data.sync_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((id, existing_version)) = existing {
            // Conflict resolution
            if existing_version >= data.sync_version {
                match conflict_strategy {
                    ConflictStrategy::LastWriteWins | ConflictStrategy::PreferRemote => {
                        // Remote is older or same, skip
                        if existing_version > data.sync_version {
                            eprintln!("[Sync] apply_upsert_clip: SKIP sync_id={} - existing_version={} > incoming_version={}", 
                                      data.sync_id, existing_version, data.sync_version);
                            return Ok(false);
                        }
                        // existing_version == data.sync_version: fall through to update
                    }
                    ConflictStrategy::PreferLocal => {
                        eprintln!("[Sync] apply_upsert_clip: SKIP sync_id={} - PreferLocal strategy, existing_version={}", 
                                  data.sync_id, existing_version);
                        return Ok(false);
                    }
                    ConflictStrategy::KeepBoth => {
                        // Create as new clip with different hash
                        // (handled below by inserting)
                    }
                }
            }

            // Get collection ID from sync_id
            let collection_id: Option<i64> = if let Some(ref coll_sync_id) = data.collection_sync_id
            {
                conn.query_row(
                    "SELECT id FROM collections WHERE sync_id = ?1",
                    [coll_sync_id],
                    |row| row.get(0),
                )
                .optional()?
            } else {
                None
            };

            // Update existing clip
            conn.execute(
                "UPDATE clips SET
                    content = ?1, content_hash = ?2, content_type = ?3,
                    source_app = ?4, source_app_icon = ?5, content_highlighted = ?6,
                    ocr_text = ?7, image_data = ?8, image_thumbnail = ?9,
                    image_width = ?10, image_height = ?11, language = ?12,
                    pinned = ?13, copy_count = ?14, collection_id = ?15,
                    origin_device_id = ?16, sync_version = ?17, deleted = 0
                 WHERE id = ?18",
                rusqlite::params![
                    data.content,
                    data.content_hash,
                    data.content_type,
                    data.source_app,
                    data.source_app_icon,
                    data.content_highlighted,
                    data.ocr_text,
                    data.image_data,
                    data.image_thumbnail,
                    data.image_width,
                    data.image_height,
                    data.language,
                    data.pinned as i64,
                    data.copy_count,
                    collection_id,
                    data.origin_device_id,
                    data.sync_version,
                    id,
                ],
            )?;
        } else {
            // Get collection ID from sync_id
            let collection_id: Option<i64> = if let Some(ref coll_sync_id) = data.collection_sync_id
            {
                conn.query_row(
                    "SELECT id FROM collections WHERE sync_id = ?1",
                    [coll_sync_id],
                    |row| row.get(0),
                )
                .optional()?
            } else {
                None
            };

            // Insert new clip
            conn.execute(
                "INSERT INTO clips (
                    sync_id, sync_version, content, content_hash, content_type,
                    source_app, source_app_icon, content_highlighted, ocr_text,
                    image_data, image_thumbnail, image_width, image_height,
                    language, created_at, pinned, copy_count, collection_id,
                    origin_device_id, deleted
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 0)",
                rusqlite::params![
                    data.sync_id,
                    data.sync_version,
                    data.content,
                    data.content_hash,
                    data.content_type,
                    data.source_app,
                    data.source_app_icon,
                    data.content_highlighted,
                    data.ocr_text,
                    data.image_data,
                    data.image_thumbnail,
                    data.image_width,
                    data.image_height,
                    data.language,
                    data.created_at,
                    data.pinned as i64,
                    data.copy_count,
                    collection_id,
                    data.origin_device_id,
                ],
            )?;
        }

        Ok(true)
    }

    /// Apply image data to an existing clip (Phase 2 of sync)
    pub fn apply_image_data(
        conn: &Connection,
        image_msg: &super::protocol::ImageDataMessage,
    ) -> SyncResult<bool> {
        // Check if clip exists
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM clips WHERE sync_id = ?1",
                [&image_msg.sync_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if !exists {
            // Clip doesn't exist yet, can't apply image
            return Ok(false);
        }

        // Update clip with image data
        let rows_affected = conn.execute(
            "UPDATE clips SET image_data = ?1, source_app_icon = COALESCE(?2, source_app_icon) WHERE sync_id = ?3",
            rusqlite::params![
                image_msg.image_data,
                image_msg.source_app_icon,
                image_msg.sync_id,
            ],
        )?;

        Ok(rows_affected > 0)
    }

    /// Apply delete clip operation
    fn apply_delete_clip(
        conn: &Connection,
        sync_id: &str,
        version: i64,
        conflict_strategy: ConflictStrategy,
    ) -> SyncResult<bool> {
        // Check current version
        let existing: Option<i64> = conn
            .query_row(
                "SELECT sync_version FROM clips WHERE sync_id = ?1 AND deleted = 0",
                [sync_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(existing_version) = existing {
            if existing_version > version && conflict_strategy == ConflictStrategy::PreferLocal {
                eprintln!("[Sync] apply_delete_clip: SKIP sync_id={} - PreferLocal and existing_version={} > version={}", 
                          sync_id, existing_version, version);
                return Ok(false);
            }

            // Soft delete
            conn.execute(
                "UPDATE clips SET deleted = 1, sync_version = ?1 WHERE sync_id = ?2",
                rusqlite::params![version, sync_id],
            )?;

            Ok(true)
        } else {
            eprintln!(
                "[Sync] apply_delete_clip: SKIP sync_id={} - clip not found or already deleted",
                sync_id
            );
            Ok(false)
        }
    }

    /// Apply upsert collection operation
    fn apply_upsert_collection(
        conn: &Connection,
        data: &CollectionData,
        conflict_strategy: ConflictStrategy,
    ) -> SyncResult<bool> {
        let existing: Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, sync_version FROM collections WHERE sync_id = ?1",
                [&data.sync_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((id, existing_version)) = existing {
            if existing_version >= data.sync_version {
                match conflict_strategy {
                    ConflictStrategy::LastWriteWins | ConflictStrategy::PreferRemote => {
                        if existing_version > data.sync_version {
                            eprintln!("[Sync] apply_upsert_collection: SKIP sync_id={} - existing_version={} > incoming_version={}", 
                                      data.sync_id, existing_version, data.sync_version);
                            return Ok(false);
                        }
                    }
                    ConflictStrategy::PreferLocal => {
                        eprintln!("[Sync] apply_upsert_collection: SKIP sync_id={} - PreferLocal strategy", data.sync_id);
                        return Ok(false);
                    }
                    ConflictStrategy::KeepBoth => {}
                }
            }

            conn.execute(
                "UPDATE collections SET name = ?1, color = ?2, sync_version = ?3, deleted = 0, origin_device_id = COALESCE(?5, origin_device_id)
                 WHERE id = ?4",
                rusqlite::params![data.name, data.color, data.sync_version, id, data.origin_device_id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO collections (sync_id, sync_version, name, color, created_at, deleted, origin_device_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                rusqlite::params![
                    data.sync_id,
                    data.sync_version,
                    data.name,
                    data.color,
                    data.created_at,
                    data.origin_device_id,
                ],
            )?;
        }

        Ok(true)
    }

    /// Apply delete collection operation
    fn apply_delete_collection(
        conn: &Connection,
        sync_id: &str,
        version: i64,
        conflict_strategy: ConflictStrategy,
    ) -> SyncResult<bool> {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT sync_version FROM collections WHERE sync_id = ?1 AND deleted = 0",
                [sync_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(existing_version) = existing {
            if existing_version > version && conflict_strategy == ConflictStrategy::PreferLocal {
                eprintln!("[Sync] apply_delete_collection: SKIP sync_id={} - PreferLocal and existing_version={} > version={}", 
                          sync_id, existing_version, version);
                return Ok(false);
            }

            conn.execute(
                "UPDATE collections SET deleted = 1, sync_version = ?1 WHERE sync_id = ?2",
                rusqlite::params![version, sync_id],
            )?;

            // Also unassign clips from this collection
            conn.execute(
                "UPDATE clips SET collection_id = NULL WHERE collection_id = (
                    SELECT id FROM collections WHERE sync_id = ?1
                 )",
                [sync_id],
            )?;

            Ok(true)
        } else {
            eprintln!("[Sync] apply_delete_collection: SKIP sync_id={} - collection not found or already deleted", sync_id);
            Ok(false)
        }
    }

    /// Apply upsert embedding operation
    fn apply_upsert_embedding(
        conn: &Connection,
        clip_sync_id: &str,
        embedding: &[f32],
        _version: i64,
    ) -> SyncResult<bool> {
        // Get clip ID from sync_id
        let clip_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM clips WHERE sync_id = ?1",
                [clip_sync_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(id) = clip_id {
            // Convert embedding to bytes
            let embedding_bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

            // Insert or replace embedding
            conn.execute(
                "INSERT OR REPLACE INTO clip_embeddings (rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, embedding_bytes],
            )?;

            Ok(true)
        } else {
            eprintln!(
                "[Sync] apply_upsert_embedding: SKIP clip_sync_id={} - clip not found",
                clip_sync_id
            );
            Ok(false)
        }
    }

    /// Apply move clip to collection operation
    fn apply_move_clip(
        conn: &Connection,
        clip_sync_id: &str,
        collection_sync_id: Option<&str>,
        version: i64,
    ) -> SyncResult<bool> {
        let collection_id: Option<i64> = if let Some(coll_sync_id) = collection_sync_id {
            conn.query_row(
                "SELECT id FROM collections WHERE sync_id = ?1",
                [coll_sync_id],
                |row| row.get(0),
            )
            .optional()?
        } else {
            None
        };

        let updated = conn.execute(
            "UPDATE clips SET collection_id = ?1, sync_version = ?2 WHERE sync_id = ?3",
            rusqlite::params![collection_id, version, clip_sync_id],
        )?;

        if updated == 0 {
            eprintln!(
                "[Sync] apply_move_clip: SKIP clip_sync_id={} - clip not found",
                clip_sync_id
            );
        }
        Ok(updated > 0)
    }

    /// Apply set pinned operation
    fn apply_set_pinned(
        conn: &Connection,
        sync_id: &str,
        pinned: bool,
        version: i64,
    ) -> SyncResult<bool> {
        let updated = conn.execute(
            "UPDATE clips SET pinned = ?1, sync_version = ?2 WHERE sync_id = ?3",
            rusqlite::params![pinned as i64, version, sync_id],
        )?;

        if updated == 0 {
            eprintln!(
                "[Sync] apply_set_pinned: SKIP sync_id={} - clip not found",
                sync_id
            );
        }
        Ok(updated > 0)
    }
}
