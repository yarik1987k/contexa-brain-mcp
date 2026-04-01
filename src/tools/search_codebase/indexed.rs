use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;

use crate::context::scoring;
use crate::db::schema;
use crate::indexer::embedding_client;

use super::types::*;

fn truncate_sig(sig: &str, max_chars: usize) -> String {
    if sig.len() <= max_chars { return sig.to_string(); }
    let cut = sig[..max_chars].rfind(|c: char| c == ',' || c == ')').unwrap_or(max_chars);
    format!("{}...", &sig[..cut])
}

/// Fast search using the pre-built index.
pub fn search_indexed(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let conn = schema::open_db(project_path)?;
    let mut output = String::new();
    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut match_index: HashMap<String, usize> = HashMap::new();
    let query_lower = query.to_lowercase();

    // Generate query embedding once
    let query_embedding = embedding_client::try_embed_text(query);

    // Split query into words for multi-word matching (used across search phases)
    let query_words: Vec<&str> = query_lower.split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect();

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
        let fts_rows: Vec<_> = match fts_stmt.query_map(rusqlite::params![&fts_query], SymbolRow::from_row) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                tracing::warn!("FTS query failed (falling back to LIKE): {}", e);
                Vec::new()
            }
        };

        let mut like_rows: Vec<SymbolRow> = Vec::new();
        {
            let mut like_stmt = conn.prepare(
                "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, s.embedding,
                        f.relative_path
                 FROM symbols s JOIN files f ON s.file_id = f.id
                 WHERE LOWER(s.name) LIKE ?1
                 ORDER BY s.name
                 LIMIT 100"
            )?;
            let mut like_seen = std::collections::HashSet::new();
            for word in &query_words {
                let pattern = format!("%{}%", word);
                let word_rows: Vec<SymbolRow> = like_stmt.query_map(rusqlite::params![&pattern], SymbolRow::from_row)?
                    .filter_map(|r| r.ok()).collect();
                for row in word_rows {
                    let key = format!("{}:{}:{}", row.name, row.file_path, row.start_line);
                    if like_seen.insert(key) {
                        like_rows.push(row);
                    }
                }
            }
        }

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
            let name_lower = row.name.to_lowercase();

            if name_lower == query_lower {
                score += scoring::SEARCH_EXACT_NAME_BONUS;
            } else if name_lower.contains(&query_lower) {
                score += scoring::SEARCH_SUBSTRING_NAME_BONUS;
            } else if query_words.len() > 1 {
                // Score by fraction of query words matched in symbol name
                let matched = query_words.iter().filter(|w| name_lower.contains(*w)).count();
                if matched > 0 {
                    let fraction = matched as f32 / query_words.len() as f32;
                    score += fraction * scoring::SEARCH_SUBSTRING_NAME_BONUS;
                }
            }

            if let (Some(ref qe), Some(ref se)) = (&query_embedding, &row.embedding) {
                let sim = embedding_client::cosine_similarity(qe, se);
                if sim > scoring::SEARCH_SYMBOL_SIM_THRESHOLD {
                    score += sim * scoring::SEARCH_SYMBOL_SIM_WEIGHT;
                }
            }

            if score > 0.0 {
                // Boost definitions, penalize import-only matches
                let name_is_destructured = name_lower.starts_with('{') || name_lower.starts_with('[');
                let sig_is_require = row.signature.contains("require(");

                let is_definition = !name_is_destructured && !sig_is_require
                    && matches!(row.kind.as_str(),
                        "function" | "async function" | "Function" | "AsyncFunction"
                        | "class" | "Class" | "method" | "Method"
                        | "struct" | "Struct" | "trait" | "Trait"
                        | "impl" | "Impl" | "enum" | "Enum"
                        | "interface" | "Interface" | "type" | "TypeAlias");
                let is_import_symbol = name_is_destructured || sig_is_require;
                let is_export = row.kind == "export" || row.kind == "Export";
                let is_small_export = is_export && (row.end_line - row.start_line) < 3;

                if is_definition {
                    score *= 1.5;
                } else if is_import_symbol || is_small_export {
                    score *= 0.2;
                }

                let sig = truncate_sig(&row.signature, 80);
                let sym_line = format!(
                    "[{}] {} L{}-{}: {}",
                    crate::indexer::symbol_extractor::abbreviate_kind(&row.kind),
                    row.name, row.start_line, row.end_line, sig
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

    // 3. Search files by path name match — try full query first, then individual words
    {
        let mut stmt = conn.prepare(
            "SELECT relative_path FROM files WHERE LOWER(relative_path) LIKE ?1"
        )?;
        let pattern = format!("%{}%", query_lower);
        let paths: Vec<String> = stmt.query_map(rusqlite::params![&pattern], |row| {
            row.get(0)
        })?.filter_map(|r| r.ok()).collect();

        for path in &paths {
            upsert_match(&mut matches, &mut match_index, path, scoring::SEARCH_PATH_MATCH_BONUS, None, None);
        }

        // Also match individual query words against file paths (with prefix matching)
        {
            let mut all_files_stmt = conn.prepare(
                "SELECT relative_path FROM files"
            )?;
            let all_paths: Vec<String> = all_files_stmt.query_map([], |row| {
                row.get(0)
            })?.filter_map(|r| r.ok()).collect();

            for file_path in &all_paths {
                let path_lower = file_path.to_lowercase();
                // Match words with prefix support: "authentication" matches "auth" in path
                let matched = query_words.iter().filter(|w| {
                    path_lower.contains(*w)
                        || w.len() >= 4 && path_lower.contains(&w[..4]) // prefix match (4+ chars)
                }).count();
                if matched > 0 {
                    let fraction = matched as f32 / query_words.len().max(1) as f32;
                    upsert_match(&mut matches, &mut match_index, file_path, fraction * scoring::SEARCH_PATH_MATCH_BONUS, None, None);
                }
            }
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

    // 5. Post-scoring: check if query words appear in implementation code vs only in imports.
    // Files where query terms only appear in import/require lines are consumers, not implementations.
    // Read a wider set of candidates before truncating so we can re-rank.
    matches.sort_by(|a, b| crate::context::relevance_scorer::cmp_score_desc(a.score, b.score));
    let analysis_count = (max_results as usize * 3).min(matches.len()); // analyze top 3x candidates

    for m in matches.iter_mut().take(analysis_count) {
        let file_path = project_path.join(&m.relative_path);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            let mut def_lines: Vec<ContextLine> = Vec::new();
            let mut import_lines: Vec<ContextLine> = Vec::new();
            let mut def_count = 0u32;
            let mut import_count = 0u32;

            for (i, line) in content.lines().enumerate() {
                let line_lower = line.to_lowercase();
                // Use prefix matching too: "auth" matches "authentication"
                let matches_query = query_words.iter().any(|w| {
                    line_lower.contains(*w)
                        || (w.len() >= 4 && line_lower.contains(&w[..4]))
                });

                if matches_query {
                    let trimmed = line.trim();
                    let is_import = trimmed.starts_with("import ")
                        || (trimmed.starts_with("const {") && trimmed.contains("require("))
                        || (trimmed.starts_with("const ") && trimmed.contains("require(") && trimmed.contains('}'))
                        || trimmed.starts_with("from ")
                        || trimmed.starts_with("#include");

                    if is_import {
                        import_count += 1;
                        if import_lines.len() < 1 {
                            import_lines.push(ContextLine { line_num: i + 1, content: trimmed.to_string() });
                        }
                    } else {
                        def_count += 1;
                        if def_lines.len() < 1 {
                            def_lines.push(ContextLine { line_num: i + 1, content: trimmed.to_string() });
                        }
                    }
                }
            }

            // Adjust score based on definition-vs-import ratio
            if def_count == 0 && import_count > 0 {
                // File only references query terms in imports — likely a consumer
                m.score *= 0.4;
            } else if def_count > 0 && import_count == 0 {
                // File uses query terms in actual code, not imports — likely an implementation
                m.score *= 1.3;
            }

            // Set context lines: prefer definitions
            if m.context_lines.is_empty() {
                for ctx in def_lines.into_iter().chain(import_lines.into_iter()).take(1) {
                    m.context_lines.push(ctx);
                }
            }
        }
    }

    // Re-sort after score adjustments and truncate to final count
    matches.sort_by(|a, b| crate::context::relevance_scorer::cmp_score_desc(a.score, b.score));
    matches.truncate(max_results as usize);

    format_results(&matches, query, token_budget, &mut output)?;
    Ok(output)
}
