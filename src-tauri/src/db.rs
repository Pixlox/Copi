use rusqlite::{Connection, OptionalExtension, Result};
use sqlite_vec::sqlite3_vec_init;
use tauri::Manager;

pub struct DbConnections {
    pub read: Connection,
    pub write: Connection,
}

pub fn init_db(app: &tauri::AppHandle) -> Result<DbConnections> {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }

    let db_path = app
        .path()
        .app_data_dir()
        .expect("Failed to get app data dir")
        .join("copi.db");

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Writer connection — holds the WAL lock for writes
    let write = Connection::open(&db_path)?;
    write.pragma_update(None, "journal_mode", "WAL")?;
    write.pragma_update(None, "synchronous", "NORMAL")?;
    write.pragma_update(None, "busy_timeout", 5000)?;
    write.pragma_update(None, "temp_store", "MEMORY")?;

    // Reader connection — reads from WAL snapshot, never blocks on writes
    let read = Connection::open(&db_path)?;
    read.pragma_update(None, "journal_mode", "WAL")?;
    read.pragma_update(None, "busy_timeout", 5000)?;
    read.pragma_update(None, "temp_store", "MEMORY")?;

    // Schema + migrations (use writer connection)
    write.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS collections (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            color TEXT,
            created_at INTEGER NOT NULL
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
            collection_id INTEGER REFERENCES collections(id)
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;

    // FTS5 table with prefix indexes for search-as-you-type
    // Check if we need to rebuild (missing prefix config)
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

    // Vector embeddings table
    let needs_recreate: bool = write
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='clip_embeddings'",
            [],
            |row| {
                let sql: String = row.get(0).unwrap_or_default();
                Ok(!sql.contains("float[768]"))
            },
        )
        .unwrap_or(true);

    if needs_recreate {
        write.execute("DROP TABLE IF EXISTS clip_embeddings", [])?;
        write.execute(
            "CREATE VIRTUAL TABLE clip_embeddings USING vec0(embedding float[768])",
            [],
        )?;
    }

    run_migrations(&write)?;
    write.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_clips_sort ON clips(pinned DESC, copy_count DESC, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_created_at ON clips(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_collection_created ON clips(collection_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_content_type_created ON clips(content_type, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_language_created ON clips(language, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_source_app_nocase ON clips(source_app COLLATE NOCASE);
        ",
    )?;

    const SEARCH_SCHEMA_VERSION_KEY: &str = "search_schema_version";
    const SEARCH_SCHEMA_VERSION: &str = "v3";
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

    eprintln!("[DB] Database ready (dual connections, WAL mode)");

    Ok(DbConnections { read, write })
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
    ];

    for (col, col_type) in &needed {
        if !columns.iter().any(|c| c == col) {
            conn.execute(
                &format!("ALTER TABLE clips ADD COLUMN {} {}", col, col_type),
                [],
            )?;
        }
    }

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

    Ok(())
}
