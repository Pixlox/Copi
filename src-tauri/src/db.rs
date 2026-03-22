use rusqlite::{Connection, Result};
use sqlite_vec::sqlite3_vec_init;
use tauri::Manager;

pub fn init_db(app: &tauri::AppHandle) -> Result<Connection> {
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

    let conn = Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;

    conn.execute_batch(
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

        CREATE VIRTUAL TABLE IF NOT EXISTS clips_fts USING fts5(
            content,
            ocr_text,
            content='clips',
            content_rowid='id'
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

    // Drop and recreate vec0 table — handles dimension changes (384→768)
    conn.execute("DROP TABLE IF EXISTS clip_embeddings", [])?;
    conn.execute(
        "CREATE VIRTUAL TABLE clip_embeddings USING vec0(embedding float[768])",
        [],
    )?;

    run_migrations(&conn)?;

    eprintln!("[DB] Database ready, vec0 table at 768 dimensions");

    Ok(conn)
}

fn run_migrations(conn: &Connection) -> Result<()> {
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('clips')")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let needed = [
        ("content_highlighted", "TEXT"),
        ("ocr_text", "TEXT"),
        ("image_data", "BLOB"),
        ("image_width", "INTEGER DEFAULT 0"),
        ("image_height", "INTEGER DEFAULT 0"),
    ];

    for (col, col_type) in &needed {
        if !columns.iter().any(|c| c == col) {
            conn.execute(
                &format!("ALTER TABLE clips ADD COLUMN {} {}", col, col_type),
                [],
            )?;
        }
    }

    Ok(())
}
