use rusqlite::{Connection, OptionalExtension, Result};
use sqlite_vec::sqlite3_vec_init;
use std::path::PathBuf;
use tauri::Manager;

pub struct DbConnections {
    pub read_pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
    pub write: Connection,
}

fn resolve_db_path(app: &tauri::AppHandle) -> PathBuf {
    if let Ok(dir) = app.path().app_data_dir() {
        return dir.join("copi.db");
    }

    if let Ok(dir) = app.path().app_local_data_dir() {
        return dir.join("copi.db");
    }

    std::env::temp_dir().join("copi").join("copi.db")
}

pub fn init_db(app: &tauri::AppHandle) -> Result<DbConnections> {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }

    let db_path = resolve_db_path(app);

    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Writer connection — holds the WAL lock for writes
    let write = Connection::open(&db_path)?;
    write.pragma_update(None, "journal_mode", "WAL")?;
    write.pragma_update(None, "synchronous", "NORMAL")?;
    write.pragma_update(None, "busy_timeout", 5000)?;
    write.pragma_update(None, "temp_store", "MEMORY")?;

    // Read pool — 4 concurrent read connections leveraging WAL mode
    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path)
        .with_flags(
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_init(|c| {
            c.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            c.pragma_update(None, "busy_timeout", 5000)
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            c.pragma_update(None, "temp_store", "MEMORY")
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            Ok(())
        });
    let read_pool = r2d2::Pool::builder()
        .max_size(4)
        .build(manager)
        .map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                Some(format!("Failed to create read pool: {}", e)),
            )
        })?;

    // Schema + migrations (use writer connection)
    write.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS collections (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            color TEXT,
            created_at INTEGER NOT NULL,
            -- Sync columns
            sync_id TEXT UNIQUE,
            sync_version INTEGER DEFAULT 0,
            deleted INTEGER DEFAULT 0,
            origin_device_id TEXT
        );

        CREATE TABLE IF NOT EXISTS clips (
            id INTEGER PRIMARY KEY,
            content TEXT NOT NULL,
            content_hash TEXT UNIQUE NOT NULL,
            content_type TEXT NOT NULL CHECK(content_type IN ('text', 'url', 'code', 'image')),
            source_app TEXT DEFAULT '',
            source_app_icon BLOB,
            content_highlighted TEXT,
            ocr_text TEXT,
            image_data BLOB,
            image_thumbnail BLOB,
            image_width INTEGER DEFAULT 0,
            image_height INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            pinned INTEGER DEFAULT 0,
            collection_id INTEGER REFERENCES collections(id),
            -- Sync columns
            sync_id TEXT UNIQUE,
            sync_version INTEGER DEFAULT 0,
            deleted INTEGER DEFAULT 0,
            origin_device_id TEXT
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sync_peers (
            device_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL DEFAULT '',
            last_seen INTEGER NOT NULL DEFAULT 0,
            last_addr TEXT
        );

        CREATE TABLE IF NOT EXISTS sync_cursors (
            device_id TEXT PRIMARY KEY,
            last_received_ts INTEGER NOT NULL DEFAULT 0
        );

        -- Sync: Device identity (this device)
        CREATE TABLE IF NOT EXISTS device_info (
            device_id TEXT PRIMARY KEY,
            device_name TEXT NOT NULL,
            platform TEXT NOT NULL,
            private_key BLOB NOT NULL,
            public_key BLOB NOT NULL,
            created_at INTEGER NOT NULL
        );

        -- Sync: Paired devices (other devices we sync with)
        CREATE TABLE IF NOT EXISTS paired_devices (
            device_id TEXT PRIMARY KEY,
            device_name TEXT NOT NULL,
            platform TEXT NOT NULL,
            public_key BLOB NOT NULL,
            paired_at INTEGER NOT NULL,
            last_seen INTEGER,
            last_sync_version INTEGER DEFAULT 0,
            last_sent_version INTEGER DEFAULT 0
        );

        -- Sync: Pending operations queue (for offline sync)
        CREATE TABLE IF NOT EXISTS sync_queue (
            id INTEGER PRIMARY KEY,
            operation TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            entity_sync_id TEXT NOT NULL,
            payload BLOB NOT NULL,
            created_at INTEGER NOT NULL
        );

        -- Sync indexes are created in run_migrations() after legacy columns are added.
        ",
    )?;

    // FTS5 table with prefix indexes for search-as-you-type
    let fts_needs_rebuild: bool = write
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='clips_fts'",
            [],
            |row| {
                let sql: String = row.get(0).unwrap_or_default();
                Ok(!sql.contains("prefix='2 3 4'"))
            },
        )
        .unwrap_or(true);

    if fts_needs_rebuild {
        write.execute_batch("DROP TABLE IF EXISTS clips_fts")?;
    }

    write.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS clips_fts USING fts5(
            content,
            ocr_text,
            content='clips',
            content_rowid='id',
            prefix='2 3 4'
        );

        CREATE TRIGGER IF NOT EXISTS clips_ai AFTER INSERT ON clips BEGIN
            INSERT INTO clips_fts(rowid, content, ocr_text)
            VALUES (new.id, new.content, COALESCE(new.ocr_text, ''));
        END;

        CREATE TRIGGER IF NOT EXISTS clips_ad AFTER DELETE ON clips BEGIN
            INSERT INTO clips_fts(clips_fts, rowid, content, ocr_text)
            VALUES ('delete', old.id, old.content, COALESCE(old.ocr_text, ''));
        END;

        CREATE TRIGGER IF NOT EXISTS clips_au AFTER UPDATE ON clips BEGIN
            INSERT INTO clips_fts(clips_fts, rowid, content, ocr_text)
            VALUES ('delete', old.id, old.content, COALESCE(old.ocr_text, ''));
            INSERT INTO clips_fts(rowid, content, ocr_text)
            VALUES (new.id, new.content, COALESCE(new.ocr_text, ''));
        END;
        ",
    )?;

    // Vector embeddings table — 384 dims for multilingual-e5-small
    let needs_recreate: bool = write
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='clip_embeddings'",
            [],
            |row| {
                let sql: String = row.get(0).unwrap_or_default();
                Ok(!sql.contains("float[384]"))
            },
        )
        .unwrap_or(true);

    if needs_recreate {
        write.execute("DROP TABLE IF EXISTS clip_embeddings", [])?;
        write.execute(
            "CREATE VIRTUAL TABLE clip_embeddings USING vec0(embedding float[384])",
            [],
        )?;
    }

    run_migrations(&write)?;
    write.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_clips_sort ON clips(pinned DESC, copy_count DESC, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_created_at ON clips(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_active_created ON clips(deleted, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_collection_created ON clips(collection_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_content_type_created ON clips(content_type, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_language_created ON clips(language, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_source_app_nocase ON clips(source_app COLLATE NOCASE);
        ",
    )?;

    const SEARCH_SCHEMA_VERSION_KEY: &str = "search_schema_version";
    const SEARCH_SCHEMA_VERSION: &str = "v4"; // bumped for e5-small model switch
    let recorded_schema_version: Option<String> = write
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SEARCH_SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let should_force_rebuild = fts_needs_rebuild
        || recorded_schema_version
            .as_deref()
            .map(|value| value != SEARCH_SCHEMA_VERSION)
            .unwrap_or(true);

    if should_force_rebuild {
        write.execute_batch("INSERT INTO clips_fts(clips_fts) VALUES('rebuild');")?;
        write.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES (?1, ?2)",
            [SEARCH_SCHEMA_VERSION_KEY, SEARCH_SCHEMA_VERSION],
        )?;
        eprintln!("[DB] FTS5 index rebuilt and search schema version refreshed");
    }

    eprintln!(
        "[DB] Database ready at {} (pool={}, WAL mode)",
        db_path.to_string_lossy(),
        4
    );

    Ok(DbConnections { read_pool, write })
}

fn run_migrations(conn: &Connection) -> Result<()> {
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('clips')")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let needed = [
        ("source_app_icon", "BLOB"),
        ("content_highlighted", "TEXT"),
        ("ocr_text", "TEXT"),
        ("image_data", "BLOB"),
        ("image_thumbnail", "BLOB"),
        ("image_width", "INTEGER DEFAULT 0"),
        ("image_height", "INTEGER DEFAULT 0"),
        ("pinned", "INTEGER DEFAULT 0"),
        ("language", "TEXT"),
        ("copy_count", "INTEGER DEFAULT 0"),
        // Sync columns
        ("sync_id", "TEXT"),
        ("sync_version", "INTEGER DEFAULT 0"),
        ("deleted", "INTEGER DEFAULT 0"),
        ("origin_device_id", "TEXT"),
        ("source_device", "TEXT NOT NULL DEFAULT ''"),
    ];

    for (col, col_type) in &needed {
        if !columns.iter().any(|c| c == col) {
            conn.execute(
                &format!("ALTER TABLE clips ADD COLUMN {} {}", col, col_type),
                [],
            )?;
        }
    }

    // Migrate collections table for sync columns
    let collection_columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('collections')")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let collection_sync_cols = [
        ("sync_id", "TEXT"),
        ("sync_version", "INTEGER DEFAULT 0"),
        ("deleted", "INTEGER DEFAULT 0"),
        ("origin_device_id", "TEXT"),
    ];

    for (col, col_type) in &collection_sync_cols {
        if !collection_columns.iter().any(|c| c == col) {
            conn.execute(
                &format!("ALTER TABLE collections ADD COLUMN {} {}", col, col_type),
                [],
            )?;
        }
    }

    let paired_columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('paired_devices')")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if !paired_columns.iter().any(|c| c == "last_sent_version") {
        conn.execute(
            "ALTER TABLE paired_devices ADD COLUMN last_sent_version INTEGER DEFAULT 0",
            [],
        )?;
    }

    // Add last_addr column to sync_peers for cross-platform reconnection
    let sync_peers_columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('sync_peers')")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if !sync_peers_columns.iter().any(|c| c == "last_addr") {
        conn.execute("ALTER TABLE sync_peers ADD COLUMN last_addr TEXT", [])?;
    }

    conn.execute_batch(
        "
        CREATE UNIQUE INDEX IF NOT EXISTS idx_clips_sync_id ON clips(sync_id) WHERE sync_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_clips_sync_version ON clips(sync_version);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_collections_sync_id ON collections(sync_id) WHERE sync_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_collections_sync_version ON collections(sync_version);
        CREATE INDEX IF NOT EXISTS idx_clips_deleted ON clips(deleted) WHERE deleted = 1;
        CREATE INDEX IF NOT EXISTS idx_collections_deleted ON collections(deleted) WHERE deleted = 1;
        CREATE INDEX IF NOT EXISTS idx_sync_queue_created ON sync_queue(created_at);
        CREATE INDEX IF NOT EXISTS idx_paired_devices_last_seen ON paired_devices(last_seen);
        CREATE INDEX IF NOT EXISTS idx_clips_source_device ON clips(source_device, created_at DESC);
        ",
    )?;

    const PIN_SYSTEM_MIGRATION_KEY: &str = "pin_system_v1_migrated";
    let pin_migration_done = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [PIN_SYSTEM_MIGRATION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .is_some();

    if !pin_migration_done {
        conn.execute("UPDATE clips SET pinned = 0", [])?;
        conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES (?1, '1')",
            [PIN_SYSTEM_MIGRATION_KEY],
        )?;
    }

    // Migrate existing raw RGBA images to PNG (saves ~10-20x space)
    // Detect: image_data length > (image_width * image_height * 2) = likely raw RGBA
    migrate_raw_images_to_png(conn)?;

    // Generate sync_id for existing clips and collections that don't have one
    migrate_sync_ids(conn)?;

    Ok(())
}

/// Generate sync_id (UUIDv4) for existing clips and collections
fn migrate_sync_ids(conn: &Connection) -> Result<()> {
    const MIGRATION_KEY: &str = "sync_ids_v1_migrated";
    let done: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [MIGRATION_KEY],
            |row| row.get(0),
        )
        .optional()?;

    if done.is_some() {
        return Ok(());
    }

    // Get all clips without sync_id
    let clip_ids: Vec<i64> = conn
        .prepare("SELECT id FROM clips WHERE sync_id IS NULL")?
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let clip_count = clip_ids.len();
    for id in clip_ids {
        let sync_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "UPDATE clips SET sync_id = ?1 WHERE id = ?2",
            rusqlite::params![sync_id, id],
        )?;
    }

    // Get all collections without sync_id
    let collection_ids: Vec<i64> = conn
        .prepare("SELECT id FROM collections WHERE sync_id IS NULL")?
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let collection_count = collection_ids.len();
    for id in collection_ids {
        let sync_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "UPDATE collections SET sync_id = ?1 WHERE id = ?2",
            rusqlite::params![sync_id, id],
        )?;
    }

    conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES (?1, '1')",
        [MIGRATION_KEY],
    )?;

    let total = clip_count + collection_count;
    if total > 0 {
        eprintln!(
            "[DB] Generated sync_ids for {} clips and {} collections",
            clip_count, collection_count
        );
    }

    Ok(())
}

fn migrate_raw_images_to_png(conn: &Connection) -> Result<()> {
    const MIGRATION_KEY: &str = "raw_rgba_to_png_v1_migrated";
    let done: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [MIGRATION_KEY],
            |row| row.get(0),
        )
        .optional()?;

    if done.is_some() {
        return Ok(());
    }

    // Find images that are likely raw RGBA
    let mut stmt = conn.prepare(
        "SELECT id, image_data, image_width, image_height FROM clips
         WHERE content_type = 'image' AND image_data IS NOT NULL AND image_width > 0 AND image_height > 0"
    )?;

    let images: Vec<(i64, Vec<u8>, i64, i64)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut migrated = 0usize;
    for (id, data, width, height) in images {
        let expected_rgba_size = (width * height * 4) as usize;
        // If data size matches raw RGBA, convert to PNG
        if data.len() >= expected_rgba_size && data.len() <= expected_rgba_size + 1024 {
            if let Some(png_bytes) = rgba_to_png_bytes(&data, width as usize, height as usize) {
                let _ = conn.execute(
                    "UPDATE clips SET image_data = ?1 WHERE id = ?2",
                    rusqlite::params![png_bytes, id],
                );
                migrated += 1;
            }
        }
    }

    conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES (?1, '1')",
        [MIGRATION_KEY],
    )?;

    if migrated > 0 {
        eprintln!("[DB] Migrated {} raw RGBA images to PNG", migrated);
    }

    Ok(())
}

fn rgba_to_png_bytes(bytes: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
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
