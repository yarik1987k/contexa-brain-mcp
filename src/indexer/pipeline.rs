use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use anyhow::Result;

use crate::db::schema;
use crate::indexer::{file_walker, symbol_extractor, embedding_client};

/// Index an entire project: walk files, extract symbols, generate embeddings, store in SQLite.
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

    for (i, file) in files.iter().enumerate() {
        if (i + 1) % 50 == 0 || i == 0 {
            eprintln!("[context-brain] Progress: {}/{} files", i + 1, files.len());
        }

        let content = match std::fs::read_to_string(&file.absolute_path) {
            Ok(c) => c,
            Err(_) => {
                stats.files_skipped += 1;
                continue;
            }
        };

        // Content hash for change detection
        let content_hash = hash_content(&content);

        // Check if already indexed with same hash
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

        // Generate file embedding
        let file_summary = format!(
            "{} {}",
            file.relative_path,
            content.chars().take(500).collect::<String>()
        );
        let file_embedding = embedding_client::embed_text(&file_summary).ok();
        if file_embedding.is_some() {
            stats.embeddings_generated += 1;
        }

        let line_count = content.lines().count();

        // Upsert file
        let file_id = schema::upsert_file(
            &conn,
            &file.relative_path,
            &file.extension,
            file.size_bytes,
            line_count,
            &content_hash,
            file_embedding.as_deref(),
        )?;

        // Delete old symbols for this file
        schema::delete_file_symbols(&conn, file_id)?;

        // Extract and store symbols
        let has_ast = matches!(
            file.extension.as_str(),
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "py" | "pyi" | "rs"
        );

        if has_ast {
            if let Ok(symbols) = symbol_extractor::extract_symbols(&content, &file.extension) {
                for sym in &symbols {
                    // Generate symbol embedding for significant symbols (> 3 lines)
                    let sym_embedding = if sym.end_line - sym.start_line > 3 {
                        let sym_text = format!("{} {}", sym.name, sym.signature);
                        let emb = embedding_client::embed_text(&sym_text).ok();
                        if emb.is_some() {
                            stats.embeddings_generated += 1;
                        }
                        emb
                    } else {
                        None
                    };

                    schema::insert_symbol(
                        &conn,
                        file_id,
                        &sym.name,
                        &sym.kind.to_string(),
                        sym.start_line,
                        sym.end_line,
                        &sym.signature,
                        sym_embedding.as_deref(),
                    )?;

                    stats.symbols_extracted += 1;
                }
            }
        }

        stats.files_indexed += 1;
    }

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

fn hash_content(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
