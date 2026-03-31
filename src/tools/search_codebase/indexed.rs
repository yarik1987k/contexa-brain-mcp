use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;

use crate::context::scoring;
use crate::db::schema;
use crate::indexer::embedding_client;

use super::types::*;

/// Fast search using the pre-built index.
pub fn search_indexed(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let conn = schema::open_db(project_path)?;
    let mut output = String::new();
    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut match_index: HashMap<String, usize> = HashMap::new();
    let query_lower = query.to_lowercase();

    // Generate query embedding once
    let query_embedding = embedding_client::try_embed_text(query);

    // 1. Search symbols — FTS5 MATCH (fast, ranked) then LIKE fallback (substring)
    {
        let fts_query = format!("\"{}\"*", query_lower.replace('"', "\"\""));
        let mut fts_stmt = conn.prepare(
            "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, s.embedding,
                    f.relative_path
             FROM symbols_fts fts
             JOIN symbols s ON s.id = fts.rowid
             JOIN files f ON s.file_id = f.id
             WHERE symbols_fts MATCH ?1
             LIMIT 100"
        )?;
        let fts_rows: Vec<_> = match fts_stmt.query_map(rusqlite::params![&fts_query], |row| {
            let sym_blob: Option<Vec<u8>> = row.get(5)?;
            Ok(SymbolRow {
                name: row.get(0)?,
                kind: row.get(1)?,
                start_line: row.get::<_, i64>(2)?.max(0) as usize,
                end_line: row.get::<_, i64>(3)?.max(0) as usize,
                signature: row.get(4)?,
                embedding: sym_blob.map(|b| schema::blob_to_embedding(&b)),
                file_path: row.get(6)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                tracing::warn!("FTS query failed (falling back to LIKE): {}", e);
                Vec::new()
            }
        };

        let mut like_stmt = conn.prepare(
            "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, s.embedding,
                    f.relative_path
             FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE LOWER(s.name) LIKE ?1
             ORDER BY s.name
             LIMIT 100"
        )?;
        let pattern = format!("%{}%", query_lower);
        let like_rows: Vec<_> = like_stmt.query_map(rusqlite::params![&pattern], |row| {
            let sym_blob: Option<Vec<u8>> = row.get(5)?;
            Ok(SymbolRow {
                name: row.get(0)?,
                kind: row.get(1)?,
                start_line: row.get::<_, i64>(2)?.max(0) as usize,
                end_line: row.get::<_, i64>(3)?.max(0) as usize,
                signature: row.get(4)?,
                embedding: sym_blob.map(|b| schema::blob_to_embedding(&b)),
                file_path: row.get(6)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        // Merge FTS + LIKE results, deduplicating
        let mut seen = std::collections::HashSet::new();
        let mut rows: Vec<&SymbolRow> = Vec::new();
        for row in fts_rows.iter().chain(like_rows.iter()) {
            let key = format!("{}:{}:{}", row.name, row.file_path, row.start_line);
            if seen.insert(key) {
                rows.push(row);
            }
        }

        for row in &rows {
            let mut score: f32 = 0.0;

            if row.name.to_lowercase() == query_lower {
                score += scoring::SEARCH_EXACT_NAME_BONUS;
            } else if row.name.to_lowercase().contains(&query_lower) {
                score += scoring::SEARCH_SUBSTRING_NAME_BONUS;
            }

            if let (Some(ref qe), Some(ref se)) = (&query_embedding, &row.embedding) {
                let sim = embedding_client::cosine_similarity(qe, se);
                if sim > scoring::SEARCH_SYMBOL_SIM_THRESHOLD {
                    score += sim * scoring::SEARCH_SYMBOL_SIM_WEIGHT;
                }
            }

            if score > 0.0 {
                let sym_line = format!(
                    "[{}] {} (L{}-L{}): {}",
                    row.kind, row.name, row.start_line, row.end_line, row.signature
                );
                upsert_match(&mut matches, &mut match_index, &row.file_path, score, None, Some(sym_line));
            }
        }
    }

    // 2. Search files by embedding similarity — streaming, prefers compressed
    if let Some(ref qe) = query_embedding {
        let tq = embedding_client::get_turboquant();
        let (query_rotated, query_norm) = tq.prepare_query(qe);

        let mut stmt = conn.prepare(
            "SELECT relative_path, embedding_compressed, embedding
             FROM files
             WHERE embedding_compressed IS NOT NULL OR embedding IS NOT NULL"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?;

        for row_result in rows {
            let (path, compressed_blob, raw_blob) = match row_result {
                Ok(r) => r,
                Err(_) => continue,
            };

            let sim = if let Some(ref blob) = compressed_blob {
                if let Some(qv) = schema::blob_to_quantized(blob) {
                    tq.fast_cosine_similarity(&query_rotated, query_norm, &qv)
                } else {
                    continue;
                }
            } else if let Some(ref blob) = raw_blob {
                let file_emb = schema::blob_to_embedding(blob);
                embedding_client::cosine_similarity(qe, &file_emb)
            } else {
                continue;
            };

            if sim > scoring::SEARCH_FILE_SIM_THRESHOLD {
                upsert_match(&mut matches, &mut match_index, &path, sim * scoring::SEARCH_FILE_SIM_WEIGHT, None, None);
            }
        }
    }

    // 3. Search files by path name match
    {
        let mut stmt = conn.prepare(
            "SELECT relative_path FROM files WHERE LOWER(relative_path) LIKE ?1"
        )?;
        let pattern = format!("%{}%", query_lower);
        let paths: Vec<String> = stmt.query_map(rusqlite::params![&pattern], |row| {
            row.get(0)
        })?.filter_map(|r| r.ok()).collect();

        for path in paths {
            upsert_match(&mut matches, &mut match_index, &path, scoring::SEARCH_PATH_MATCH_BONUS, None, None);
        }
    }

    // 4. Apply import-centrality boost
    {
        let mut stmt = conn.prepare(
            "SELECT relative_path, COALESCE(import_count, 0) FROM files WHERE import_count > 0"
        )?;
        let import_counts: HashMap<String, i64> = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?.filter_map(|r| r.ok()).collect();

        if !import_counts.is_empty() {
            let max_count = import_counts.values().copied().max().unwrap_or(1).max(1) as f32;
            for m in &mut matches {
                if let Some(&count) = import_counts.get(&m.relative_path) {
                    let normalized = (count as f32).ln_1p() / max_count.ln_1p();
                    m.score += normalized * scoring::SEARCH_CENTRALITY_MAX_BOOST;
                }
            }
        }
    }

    // For top matches, add keyword context lines from actual files
    matches.sort_by(|a, b| crate::context::relevance_scorer::cmp_score_desc(a.score, b.score));
    matches.truncate(max_results as usize);

    for m in &mut matches {
        if m.context_lines.is_empty() {
            let file_path = project_path.join(&m.relative_path);
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                for (i, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&query_lower) && m.context_lines.len() < 3 {
                        m.context_lines.push(ContextLine {
                            line_num: i + 1,
                            content: line.trim().to_string(),
                        });
                    }
                }
            }
        }
    }

    format_results(&matches, query, token_budget, &mut output)?;
    Ok(output)
}
