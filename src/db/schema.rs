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
    // Enable foreign key enforcement (SQLite has this OFF by default)
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    // Set busy timeout to handle concurrent access gracefully
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;

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

        -- Full-text search for symbols (used by search_codebase)
        CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(name, signature);

        -- Indexes
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(relative_path);
        ",
    )?;

    // Migrations: run in a transaction to prevent partial schema updates
    let needs_files_compressed: bool = conn
        .prepare("SELECT embedding_compressed FROM files LIMIT 0")
        .is_err();
    let needs_mem_compressed: bool = conn
        .prepare("SELECT embedding_compressed FROM memories LIMIT 0")
        .is_err();

    if needs_files_compressed || needs_mem_compressed {
        let tx = conn.unchecked_transaction()?;
        if needs_files_compressed {
            tx.execute_batch(
                "ALTER TABLE files ADD COLUMN embedding_compressed BLOB;
                 ALTER TABLE symbols ADD COLUMN embedding_compressed BLOB;",
            )?;
        }
        if needs_mem_compressed {
            tx.execute_batch(
                "ALTER TABLE memories ADD COLUMN embedding_compressed BLOB;",
            )?;
        }
        tx.commit()?;
    }

    // Migration: add import_count column for centrality ranking
    let needs_import_count: bool = conn
        .prepare("SELECT import_count FROM files LIMIT 0")
        .is_err();
    if needs_import_count {
        conn.execute_batch("ALTER TABLE files ADD COLUMN import_count INTEGER DEFAULT 0;")?;
    }

    Ok(conn)
}

/// Delete a file and its symbols by relative path.
pub fn delete_file_by_path(conn: &Connection, relative_path: &str) -> Result<()> {
    let file_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM files WHERE relative_path = ?1",
            rusqlite::params![relative_path],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = file_id {
        delete_file_symbols(conn, id)?;
        conn.execute("DELETE FROM files WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// Update import counts for all files in a single transaction.
pub fn update_import_counts(conn: &Connection, counts: &std::collections::HashMap<String, u32>) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE files SET import_count = 0", [])?;
    for (path, count) in counts {
        tx.execute(
            "UPDATE files SET import_count = ?1 WHERE relative_path = ?2",
            rusqlite::params![*count as i64, path],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Store a file entry with its embedding and optional compressed embedding.
pub fn upsert_file(
    conn: &Connection,
    relative_path: &str,
    extension: &str,
    size_bytes: u64,
    line_count: usize,
    content_hash: &str,
    embedding: Option<&[f32]>,
    embedding_compressed: Option<&[u8]>,
) -> Result<i64> {
    let embedding_blob = embedding.map(|e| {
        e.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    });

    conn.execute(
        "INSERT INTO files (relative_path, extension, size_bytes, line_count, content_hash, embedding, embedding_compressed, last_indexed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
         ON CONFLICT(relative_path) DO UPDATE SET
            extension = ?2, size_bytes = ?3, line_count = ?4,
            content_hash = ?5, embedding = ?6, embedding_compressed = ?7, last_indexed = datetime('now')",
        rusqlite::params![
            relative_path,
            extension,
            size_bytes as i64,
            line_count as i64,
            content_hash,
            embedding_blob,
            embedding_compressed,
        ],
    )?;

    // last_insert_rowid returns 0 on UPDATE, so always query the actual ID
    let file_id: i64 = conn.query_row(
        "SELECT id FROM files WHERE relative_path = ?1",
        rusqlite::params![relative_path],
        |row| row.get(0),
    )?;

    Ok(file_id)
}

/// Store a symbol entry with its embedding and optional compressed embedding.
pub fn insert_symbol(
    conn: &Connection,
    file_id: i64,
    name: &str,
    kind: &str,
    start_line: usize,
    end_line: usize,
    signature: &str,
    embedding: Option<&[f32]>,
    embedding_compressed: Option<&[u8]>,
) -> Result<()> {
    let embedding_blob = embedding.map(|e| {
        e.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    });

    let symbol_id = {
        conn.execute(
            "INSERT INTO symbols (file_id, name, kind, start_line, end_line, signature, embedding, embedding_compressed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                file_id,
                name,
                kind,
                start_line as i64,
                end_line as i64,
                signature,
                embedding_blob,
                embedding_compressed,
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
/// Cleans up FTS entries first (subquery needs symbols rows to still exist).
pub fn delete_file_symbols(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)",
        [file_id],
    )?;
    conn.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
    Ok(())
}

/// Read an embedding blob back into a Vec<f32>.
/// Returns empty vec if blob length is not a multiple of 4 (corrupted data).
pub fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    if blob.len() % 4 != 0 {
        tracing::warn!("Embedding blob has invalid length {} (not a multiple of 4), skipping", blob.len());
        return Vec::new();
    }
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Store a memory with its embedding and optional compressed embedding.
pub fn insert_memory(
    conn: &Connection,
    content: &str,
    category: &str,
    tags: &str,
    embedding: Option<&[f32]>,
    embedding_compressed: Option<&[u8]>,
) -> Result<i64> {
    let embedding_blob = embedding.map(|e| {
        e.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    });

    conn.execute(
        "INSERT INTO memories (content, category, tags, embedding, embedding_compressed)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![content, category, tags, embedding_blob, embedding_compressed],
    )?;

    let memory_id = conn.last_insert_rowid();
    Ok(memory_id)
}

// ── TurboQuant compressed embedding serialization ───────────────────

use crate::turboquant::{QuantizedVector, QuantMode};

/// Serialize a QuantizedVector to a compact blob.
/// Format: [version:1][bits:1][mode:1][original_dim:u16 LE][padded_dim:u16 LE]
///         [norm:f32 LE][residual_norm:f32 LE][mse_len:u16 LE][mse_data...]
///         [qjl_len:u16 LE][qjl_data...]
pub fn quantized_to_blob(qv: &QuantizedVector) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32 + qv.mse_indices.len());
    buf.push(1u8); // version
    buf.push(qv.bits);
    buf.push(match qv.mode { QuantMode::Fast => 0, QuantMode::Unbiased => 1 });
    buf.extend_from_slice(&(qv.original_dim as u16).to_le_bytes());
    buf.extend_from_slice(&(qv.padded_dim as u16).to_le_bytes());
    buf.extend_from_slice(&qv.norm.to_le_bytes());
    buf.extend_from_slice(&qv.residual_norm.to_le_bytes());
    buf.extend_from_slice(&(qv.mse_indices.len() as u16).to_le_bytes());
    buf.extend_from_slice(&qv.mse_indices);
    let qjl = qv.qjl_bits.as_deref().unwrap_or(&[]);
    buf.extend_from_slice(&(qjl.len() as u16).to_le_bytes());
    buf.extend_from_slice(qjl);
    buf
}

/// Deserialize a QuantizedVector from a blob.
/// Returns None for invalid/corrupted data instead of panicking.
pub fn blob_to_quantized(blob: &[u8]) -> Option<QuantizedVector> {
    // Header: version(1) + bits(1) + mode(1) + original_dim(2) + padded_dim(2)
    //       + norm(4) + residual_norm(4) + mse_len(2) = 17 bytes minimum
    if blob.len() < 17 { return None; }
    let version = blob[0];
    if version != 1 { return None; }
    let bits = blob[1];
    if !(1..=4).contains(&bits) { return None; }
    let mode = match blob[2] { 0 => QuantMode::Fast, 1 => QuantMode::Unbiased, _ => return None };
    let original_dim = u16::from_le_bytes([blob[3], blob[4]]) as usize;
    let padded_dim = u16::from_le_bytes([blob[5], blob[6]]) as usize;
    if original_dim == 0 || padded_dim == 0 || padded_dim < original_dim { return None; }
    let norm = f32::from_le_bytes([blob[7], blob[8], blob[9], blob[10]]);
    let residual_norm = f32::from_le_bytes([blob[11], blob[12], blob[13], blob[14]]);
    let mse_len = u16::from_le_bytes([blob[15], blob[16]]) as usize;
    let mse_start: usize = 17;
    let mse_end = mse_start.checked_add(mse_len)?;
    if blob.len() < mse_end.checked_add(2)? { return None; }
    let mse_indices = blob[mse_start..mse_end].to_vec();
    let qjl_len = u16::from_le_bytes([blob[mse_end], blob[mse_end + 1]]) as usize;
    let qjl_start = mse_end + 2;
    let qjl_end = qjl_start.checked_add(qjl_len)?;
    let qjl_bits = if qjl_len > 0 && blob.len() >= qjl_end {
        Some(blob[qjl_start..qjl_end].to_vec())
    } else if qjl_len > 0 {
        return None; // Declared QJL data but blob too short
    } else {
        None
    };
    Some(QuantizedVector {
        mse_indices,
        norm,
        qjl_bits,
        residual_norm,
        bits,
        mode,
        original_dim,
        padded_dim,
    })
}
