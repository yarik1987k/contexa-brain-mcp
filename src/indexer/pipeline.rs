use std::path::Path;
use anyhow::Result;

use crate::db::schema;
use crate::indexer::{file_walker, symbol_extractor, embedding_client};

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

    eprintln!("[context-brain] Indexing {} files...", files.len());

    // ── Phase 1: Collect files that need (re-)indexing ────────────────
    let mut to_index: Vec<PendingFile> = Vec::new();

    for file in &files {
        let content = match std::fs::read_to_string(&file.absolute_path) {
            Ok(c) => c,
            Err(_) => {
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
        eprintln!("[context-brain] All files up to date, nothing to index.");
        return Ok(stats);
    }

    // ── Phase 2: Batch-generate file embeddings ──────────────────────
    eprintln!(
        "[context-brain] Generating file embeddings for {} files...",
        to_index.len()
    );
    let file_summaries: Vec<String> = to_index
        .iter()
        .map(|f| {
            format!(
                "{} {}",
                f.relative_path,
                f.content.chars().take(500).collect::<String>()
            )
        })
        .collect();
    let summary_refs: Vec<&str> = file_summaries.iter().map(|s| s.as_str()).collect();
    let file_embeddings = embedding_client::embed_batch(&summary_refs).unwrap_or_default();
    stats.embeddings_generated += file_embeddings.len();

    // ── Phase 3: Extract symbols, batch-generate symbol embeddings ───
    // embed_map tracks which symbols need embeddings: (file_idx, symbol_idx)
    let mut file_symbols: Vec<Vec<PendingSymbol>> = Vec::with_capacity(to_index.len());
    let mut embed_texts: Vec<String> = Vec::new();
    let mut embed_map: Vec<(usize, usize)> = Vec::new();

    for (file_idx, file) in to_index.iter().enumerate() {
        let mut symbols = Vec::new();
        if super::config::has_ast_support(&file.extension) {
            if let Ok(extracted) =
                symbol_extractor::extract_symbols(&file.content, &file.extension)
            {
                for sym in extracted {
                    let needs_embedding = sym.end_line.saturating_sub(sym.start_line) > 3;
                    if needs_embedding {
                        embed_map.push((file_idx, symbols.len()));
                        embed_texts.push(format!("{} {}", sym.name, sym.signature));
                    }
                    symbols.push(PendingSymbol {
                        name: sym.name,
                        kind: sym.kind.to_string(),
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        signature: sym.signature,
                        embedding: None,
                    });
                }
            }
        }
        file_symbols.push(symbols);
    }

    if !embed_texts.is_empty() {
        eprintln!(
            "[context-brain] Generating symbol embeddings for {} symbols...",
            embed_texts.len()
        );
        let refs: Vec<&str> = embed_texts.iter().map(|s| s.as_str()).collect();
        if let Ok(sym_embeddings) = embedding_client::embed_batch(&refs) {
            stats.embeddings_generated += sym_embeddings.len();
            for (emb_idx, embedding) in sym_embeddings.into_iter().enumerate() {
                if let Some(&(file_idx, sym_idx)) = embed_map.get(emb_idx) {
                    if let Some(sym) =
                        file_symbols.get_mut(file_idx).and_then(|s| s.get_mut(sym_idx))
                    {
                        sym.embedding = Some(embedding);
                    }
                }
            }
        }
    }

    // ── Phase 4: Write everything in a single transaction ────────────
    eprintln!("[context-brain] Writing to database...");
    let tx = conn.unchecked_transaction()?;

    for (file_idx, file) in to_index.iter().enumerate() {
        if (file_idx + 1) % 50 == 0 || file_idx == 0 {
            eprintln!(
                "[context-brain] Progress: {}/{} files",
                file_idx + 1,
                to_index.len()
            );
        }

        let file_embedding = file_embeddings.get(file_idx);
        let file_embedding_compressed = file_embedding.map(|e| {
            let qv = embedding_client::quantize_embedding(e);
            schema::quantized_to_blob(&qv)
        });

        let line_count = file.content.lines().count();

        let file_id = schema::upsert_file(
            &tx,
            &file.relative_path,
            &file.extension,
            file.size_bytes,
            line_count,
            &file.content_hash,
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
                    &tx,
                    file_id,
                    &sym.name,
                    &sym.kind,
                    sym.start_line,
                    sym.end_line,
                    &sym.signature,
                    sym.embedding.as_deref(),
                    sym_compressed.as_deref(),
                )?;

                stats.symbols_extracted += 1;
            }
        }
    }

    tx.commit()?;

    stats.files_indexed = to_index.len();
    eprintln!("[context-brain] Indexing complete!");
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

struct PendingSymbol {
    name: String,
    kind: String,
    start_line: usize,
    end_line: usize,
    signature: String,
    embedding: Option<Vec<f32>>,
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
