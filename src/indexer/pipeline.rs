use std::path::Path;
use anyhow::Result;

use std::collections::HashSet;

use crate::db::schema;
use crate::indexer::{file_walker, symbol_extractor, embedding_client, import_extractor};

/// Index an entire project: walk files, extract symbols, batch-generate embeddings, store in SQLite.
///
/// Uses a phased approach for efficiency:
/// 1. Collect files needing (re-)indexing
/// 2. Batch-generate file embeddings (one model call)
/// 3. Extract symbols, batch-generate symbol embeddings (one model call)
/// 4. Write everything in a single SQLite transaction
pub fn index_project(project_path: &Path) -> Result<IndexStats> {
    let conn = schema::open_db(project_path)?;
    let files = file_walker::walk_project(project_path)?;

    let mut stats = IndexStats {
        files_total: files.len(),
        files_indexed: 0,
        files_skipped: 0,
        symbols_extracted: 0,
        embeddings_generated: 0,
    };

    tracing::info!("Indexing {} files...", files.len());

    // ── Phase 1: Collect files that need (re-)indexing ────────────────
    let mut to_index: Vec<PendingFile> = Vec::new();

    for file in &files {
        let content = match std::fs::read_to_string(&file.absolute_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("Skipping {} (read failed: {})", file.relative_path, e);
                stats.files_skipped += 1;
                continue;
            }
        };

        let content_hash = hash_content(&content);

        let existing_hash: Option<String> = conn
            .prepare("SELECT content_hash FROM files WHERE relative_path = ?1")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(rusqlite::params![&file.relative_path], |row| row.get(0))
                    .ok()
            });

        if existing_hash.as_deref() == Some(&content_hash) {
            stats.files_skipped += 1;
            continue;
        }

        to_index.push(PendingFile {
            relative_path: file.relative_path.clone(),
            extension: file.extension.clone(),
            size_bytes: file.size_bytes,
            content,
            content_hash,
        });
    }

    if to_index.is_empty() {
        tracing::info!("All files up to date, nothing to index.");
        return Ok(stats);
    }

    let embed_stats = embed_and_store(&conn, &to_index)?;
    stats.embeddings_generated = embed_stats.embeddings_generated;
    stats.symbols_extracted = embed_stats.symbols_extracted;
    stats.files_indexed = to_index.len();

    // ── Phase 5: Build import graph and compute centrality ──────────
    tracing::info!("Computing import centrality...");
    let known_paths: HashSet<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    let mut all_file_data: Vec<(String, String, String)> = Vec::new();

    // Use content from to_index where available, read the rest from disk
    let indexed_paths: HashSet<String> = to_index.iter().map(|f| f.relative_path.clone()).collect();
    for file in &to_index {
        all_file_data.push((file.relative_path.clone(), file.extension.clone(), file.content.clone()));
    }
    for file in &files {
        if !indexed_paths.contains(&file.relative_path) {
            if let Ok(content) = std::fs::read_to_string(&file.absolute_path) {
                all_file_data.push((file.relative_path.clone(), file.extension.clone(), content));
            }
        }
    }

    let import_counts = import_extractor::build_import_counts(&all_file_data, &known_paths);
    if !import_counts.is_empty() {
        if let Err(e) = schema::update_import_counts(&conn, &import_counts) {
            tracing::warn!("Failed to update import counts: {}", e);
        }
    }

    tracing::info!("Indexing complete!");
    Ok(stats)
}

/// Check if a project has been indexed.
pub fn is_indexed(project_path: &Path) -> bool {
    let db_path = schema::db_path(project_path);
    if !db_path.exists() {
        return false;
    }
    if let Ok(conn) = schema::open_db(project_path) {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap_or(0);
        count > 0
    } else {
        false
    }
}

pub struct IndexStats {
    pub files_total: usize,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub symbols_extracted: usize,
    pub embeddings_generated: usize,
}

impl std::fmt::Display for IndexStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Indexed {}/{} files ({} skipped), {} symbols, {} embeddings",
            self.files_indexed,
            self.files_total,
            self.files_skipped,
            self.symbols_extracted,
            self.embeddings_generated
        )
    }
}

struct PendingFile {
    relative_path: String,
    extension: String,
    size_bytes: u64,
    content: String,
    content_hash: String,
}

/// Shared logic: generate embeddings, extract symbols, and write to DB.
/// Used by both `index_project` and `index_files`.
fn embed_and_store(conn: &rusqlite::Connection, to_index: &[PendingFile]) -> Result<EmbedStats> {
    let mut stats = EmbedStats { embeddings_generated: 0, symbols_extracted: 0 };

    // Batch-generate file embeddings
    let file_summaries: Vec<String> = to_index.iter()
        .map(|f| format!("{} {}", f.relative_path, f.content.chars().take(500).collect::<String>()))
        .collect();
    let summary_refs: Vec<&str> = file_summaries.iter().map(|s| s.as_str()).collect();
    let file_embeddings = embedding_client::try_embed_batch(&summary_refs);
    stats.embeddings_generated += file_embeddings.len();

    // Extract symbols and batch-generate symbol embeddings
    let mut file_symbols: Vec<Vec<PendingSymbol>> = Vec::with_capacity(to_index.len());
    let mut embed_texts: Vec<String> = Vec::new();
    let mut embed_map: Vec<(usize, usize)> = Vec::new();

    for (file_idx, file) in to_index.iter().enumerate() {
        let mut symbols = Vec::new();
        if super::config::has_ast_support(&file.extension) {
            if let Ok(extracted) = symbol_extractor::extract_symbols(&file.content, &file.extension) {
                for sym in extracted {
                    if sym.end_line.saturating_sub(sym.start_line) > 3 {
                        embed_map.push((file_idx, symbols.len()));
                        embed_texts.push(format!("{} {}", sym.name, sym.signature));
                    }
                    symbols.push(PendingSymbol {
                        name: sym.name, kind: sym.kind.to_string(),
                        start_line: sym.start_line, end_line: sym.end_line,
                        signature: sym.signature, embedding: None,
                    });
                }
            }
        }
        file_symbols.push(symbols);
    }

    if !embed_texts.is_empty() {
        tracing::info!("Generating symbol embeddings for {} symbols...", embed_texts.len());
        let refs: Vec<&str> = embed_texts.iter().map(|s| s.as_str()).collect();
        if let Ok(sym_embeddings) = embedding_client::embed_batch(&refs) {
            stats.embeddings_generated += sym_embeddings.len();
            for (emb_idx, embedding) in sym_embeddings.into_iter().enumerate() {
                if let Some(&(fi, si)) = embed_map.get(emb_idx) {
                    if let Some(sym) = file_symbols.get_mut(fi).and_then(|s| s.get_mut(si)) {
                        sym.embedding = Some(embedding);
                    }
                }
            }
        }
    }

    // Write to DB in a single transaction
    let tx = conn.unchecked_transaction()?;
    for (file_idx, file) in to_index.iter().enumerate() {
        let file_embedding = file_embeddings.get(file_idx);
        let file_embedding_compressed = file_embedding.map(|e| {
            let qv = embedding_client::quantize_embedding(e);
            schema::quantized_to_blob(&qv)
        });
        let line_count = file.content.lines().count();
        let file_id = schema::upsert_file(
            &tx, &file.relative_path, &file.extension, file.size_bytes,
            line_count, &file.content_hash,
            file_embedding.map(|e| e.as_slice()),
            file_embedding_compressed.as_deref(),
        )?;
        schema::delete_file_symbols(&tx, file_id)?;
        if let Some(symbols) = file_symbols.get(file_idx) {
            for sym in symbols {
                let sym_compressed = sym.embedding.as_ref().map(|e| {
                    let qv = embedding_client::quantize_embedding(e);
                    schema::quantized_to_blob(&qv)
                });
                schema::insert_symbol(
                    &tx, file_id, &sym.name, &sym.kind,
                    sym.start_line, sym.end_line, &sym.signature,
                    sym.embedding.as_deref(), sym_compressed.as_deref(),
                )?;
                stats.symbols_extracted += 1;
            }
        }
    }
    tx.commit()?;
    Ok(stats)
}

struct EmbedStats {
    embeddings_generated: usize,
    symbols_extracted: usize,
}

struct PendingSymbol {
    name: String,
    kind: String,
    start_line: usize,
    end_line: usize,
    signature: String,
    embedding: Option<Vec<f32>>,
}

/// Index specific files (for incremental re-indexing from file watcher).
pub fn index_files(project_path: &Path, relative_paths: &[String]) -> Result<usize> {
    if relative_paths.is_empty() {
        return Ok(0);
    }

    let conn = schema::open_db(project_path)?;
    let mut to_index: Vec<PendingFile> = Vec::new();

    for rel_path in relative_paths {
        let abs_path = project_path.join(rel_path);
        let content = match std::fs::read_to_string(&abs_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let ext = std::path::Path::new(rel_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        if !super::config::is_source_file(&ext) {
            continue;
        }

        let content_hash = hash_content(&content);
        let existing_hash: Option<String> = conn
            .prepare("SELECT content_hash FROM files WHERE relative_path = ?1")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(rusqlite::params![rel_path], |row| row.get(0)).ok()
            });

        if existing_hash.as_deref() == Some(&content_hash) {
            continue;
        }

        let size = std::fs::metadata(&abs_path).map(|m| m.len()).unwrap_or(0);
        to_index.push(PendingFile {
            relative_path: rel_path.clone(),
            extension: ext,
            size_bytes: size,
            content,
            content_hash,
        });
    }

    if to_index.is_empty() {
        return Ok(0);
    }

    let count = to_index.len();
    tracing::info!("Incremental re-index: {} files changed", count);
    let stats = embed_and_store(&conn, &to_index)?;
    tracing::info!("Incremental re-index complete: {} files, {} symbols", count, stats.symbols_extracted);
    Ok(count)
}

/// Remove a file from the index (called when file is deleted).
pub fn delete_file(project_path: &Path, relative_path: &str) -> Result<()> {
    let conn = schema::open_db(project_path)?;
    schema::delete_file_by_path(&conn, relative_path)?;
    tracing::info!("Removed from index: {}", relative_path);
    Ok(())
}

/// FNV-1a 64-bit hash — stable across Rust versions (unlike DefaultHasher).
fn hash_content(content: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001B3;
    let mut hash = FNV_OFFSET;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}
