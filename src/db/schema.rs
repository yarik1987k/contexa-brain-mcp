use std::path::Path;
use anyhow::Result;
use rusqlite::Connection;

/// Get the path to the project's context-brain database.
pub fn db_path(project_path: &Path) -> std::path::PathBuf {
    project_path.join(".context-brain.db")
}

/// Open a connection and ensure all tables exist.
pub fn open_db(project_path: &Path) -> Result<Connection> {
    let path = db_path(project_path);
    let conn = Connection::open(&path)?;

    // Enable WAL mode for better concurrent performance
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    conn.execute_batch(
        "
        -- File index
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            relative_path TEXT UNIQUE NOT NULL,
            extension TEXT,
            size_bytes INTEGER,
            line_count INTEGER,
            content_hash TEXT,
            embedding BLOB,
            last_indexed TEXT DEFAULT (datetime('now'))
        );

        -- Symbol index
        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            start_line INTEGER,
            end_line INTEGER,
            signature TEXT,
            embedding BLOB
        );

        -- Memories
        CREATE TABLE IF NOT EXISTS memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            category TEXT DEFAULT 'general',
            tags TEXT DEFAULT '',
            created_at TEXT DEFAULT (datetime('now')),
            embedding BLOB
        );

        -- Sessions
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            started_at TEXT NOT NULL,
            summary TEXT,
            last_active TEXT
        );

        -- Full-text search for files
        CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(relative_path);

        -- Full-text search for symbols
        CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(name, signature);

        -- Full-text search for memories
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(content, tags);

        -- Indexes
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(relative_path);
        ",
    )?;

    Ok(conn)
}

/// Store a file entry with its embedding.
pub fn upsert_file(
    conn: &Connection,
    relative_path: &str,
    extension: &str,
    size_bytes: u64,
    line_count: usize,
    content_hash: &str,
    embedding: Option<&[f32]>,
) -> Result<i64> {
    let embedding_blob = embedding.map(|e| {
        e.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    });

    conn.execute(
        "INSERT INTO files (relative_path, extension, size_bytes, line_count, content_hash, embedding, last_indexed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
         ON CONFLICT(relative_path) DO UPDATE SET
            extension = ?2, size_bytes = ?3, line_count = ?4,
            content_hash = ?5, embedding = ?6, last_indexed = datetime('now')",
        rusqlite::params![
            relative_path,
            extension,
            size_bytes as i64,
            line_count as i64,
            content_hash,
            embedding_blob,
        ],
    )?;

    let file_id = conn.last_insert_rowid();

    // Update FTS
    conn.execute(
        "INSERT OR REPLACE INTO files_fts(rowid, relative_path) VALUES (?1, ?2)",
        rusqlite::params![file_id, relative_path],
    )?;

    Ok(file_id)
}

/// Store a symbol entry with its embedding.
pub fn insert_symbol(
    conn: &Connection,
    file_id: i64,
    name: &str,
    kind: &str,
    start_line: usize,
    end_line: usize,
    signature: &str,
    embedding: Option<&[f32]>,
) -> Result<()> {
    let embedding_blob = embedding.map(|e| {
        e.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    });

    let symbol_id = {
        conn.execute(
            "INSERT INTO symbols (file_id, name, kind, start_line, end_line, signature, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                file_id,
                name,
                kind,
                start_line as i64,
                end_line as i64,
                signature,
                embedding_blob,
            ],
        )?;
        conn.last_insert_rowid()
    };

    // Update FTS
    conn.execute(
        "INSERT INTO symbols_fts(rowid, name, signature) VALUES (?1, ?2, ?3)",
        rusqlite::params![symbol_id, name, signature],
    )?;

    Ok(())
}

/// Delete all symbols for a file (before re-indexing).
pub fn delete_file_symbols(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
    Ok(())
}

/// Read an embedding blob back into a Vec<f32>.
pub fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Store a memory with its embedding.
pub fn insert_memory(
    conn: &Connection,
    content: &str,
    category: &str,
    tags: &str,
    embedding: Option<&[f32]>,
) -> Result<i64> {
    let embedding_blob = embedding.map(|e| {
        e.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    });

    conn.execute(
        "INSERT INTO memories (content, category, tags, embedding)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![content, category, tags, embedding_blob],
    )?;

    let memory_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO memories_fts(rowid, content, tags) VALUES (?1, ?2, ?3)",
        rusqlite::params![memory_id, content, tags],
    )?;

    Ok(memory_id)
}
