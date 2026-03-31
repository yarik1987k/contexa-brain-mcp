use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

use crate::context::token_estimator;
use crate::db::schema;
use crate::indexer::{file_walker, symbol_extractor, embedding_client};

/// Search the codebase using the persistent index (fast) with fallback to live scan.
///
/// Priority order:
/// 1. Query the indexed SQLite database (symbols + file embeddings) — instant
/// 2. Fall back to live file scan only if index doesn't exist
pub fn search(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    // Cap max_results to prevent abuse
    let max_results = max_results.min(50);
    let token_budget = token_budget.min(100_000);

    let db_path = schema::db_path(project_path);
    if db_path.exists() {
        match search_indexed(project_path, query, max_results, token_budget) {
            Ok(result) if !result.contains("No results found") => return Ok(result),
            _ => {} // Fall through to live scan
        }
    }

    search_live(project_path, query, max_results, token_budget)
}

/// Fast search using the pre-built index.
fn search_indexed(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let conn = schema::open_db(project_path)?;
    let mut output = String::new();
    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut match_index: HashMap<String, usize> = HashMap::new();
    let query_lower = query.to_lowercase();

    // Generate query embedding once
    let query_embedding = embedding_client::embed_text(query).ok();

    // 1. Search symbols — FTS5 MATCH (fast, ranked) then LIKE fallback (substring)
    {
        // Primary: FTS5 full-text search on symbol names and signatures
        let fts_query = format!("\"{}\"*", query_lower.replace('"', "\"\""));
        let mut fts_stmt = conn.prepare(
            "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, s.embedding,
                    f.relative_path, f.embedding as file_embedding
             FROM symbols_fts fts
             JOIN symbols s ON s.id = fts.rowid
             JOIN files f ON s.file_id = f.id
             WHERE symbols_fts MATCH ?1
             LIMIT 100"
        )?;
        let fts_rows: Vec<_> = fts_stmt.query_map(rusqlite::params![&fts_query], |row| {
            let sym_blob: Option<Vec<u8>> = row.get(5)?;
            let file_blob: Option<Vec<u8>> = row.get(7)?;
            Ok(SymbolRow {
                name: row.get(0)?,
                kind: row.get(1)?,
                start_line: row.get::<_, i64>(2)? as usize,
                end_line: row.get::<_, i64>(3)? as usize,
                signature: row.get(4)?,
                embedding: sym_blob.map(|b| schema::blob_to_embedding(&b)),
                file_path: row.get(6)?,
                file_embedding: file_blob.map(|b| schema::blob_to_embedding(&b)),
            })
        }).ok().map(|r| r.filter_map(|r| r.ok()).collect()).unwrap_or_default();

        // Fallback: LIKE for substring matches FTS may miss
        let mut like_stmt = conn.prepare(
            "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, s.embedding,
                    f.relative_path, f.embedding as file_embedding
             FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE LOWER(s.name) LIKE ?1
             ORDER BY s.name
             LIMIT 100"
        )?;
        let pattern = format!("%{}%", query_lower);
        let like_rows: Vec<_> = like_stmt.query_map(rusqlite::params![&pattern], |row| {
            let sym_blob: Option<Vec<u8>> = row.get(5)?;
            let file_blob: Option<Vec<u8>> = row.get(7)?;
            Ok(SymbolRow {
                name: row.get(0)?,
                kind: row.get(1)?,
                start_line: row.get::<_, i64>(2)? as usize,
                end_line: row.get::<_, i64>(3)? as usize,
                signature: row.get(4)?,
                embedding: sym_blob.map(|b| schema::blob_to_embedding(&b)),
                file_path: row.get(6)?,
                file_embedding: file_blob.map(|b| schema::blob_to_embedding(&b)),
            })
        })?.filter_map(|r| r.ok()).collect();

        // Merge FTS + LIKE results, deduplicating by (name, file_path, start_line)
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

            // Exact name match
            if row.name.to_lowercase() == query_lower {
                score += 5.0;
            } else if row.name.to_lowercase().contains(&query_lower) {
                score += 3.0;
            }

            // Semantic similarity on symbol embedding
            if let (Some(ref qe), Some(ref se)) = (&query_embedding, &row.embedding) {
                let sim = embedding_client::cosine_similarity(qe, se);
                if sim > 0.3 {
                    score += sim * 5.0;
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

        // Stream rows one at a time instead of collecting all into RAM
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
                // Fast path: TurboQuant similarity on packed indices (no f32 allocation)
                if let Some(qv) = schema::blob_to_quantized(blob) {
                    tq.fast_cosine_similarity(&query_rotated, query_norm, &qv)
                } else {
                    continue;
                }
            } else if let Some(ref blob) = raw_blob {
                // Fallback: raw f32 embedding (backward compat with old DBs)
                let file_emb = schema::blob_to_embedding(blob);
                embedding_client::cosine_similarity(qe, &file_emb)
            } else {
                continue;
            };

            if sim > 0.35 {
                upsert_match(&mut matches, &mut match_index, &path, sim * 3.0, None, None);
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
            upsert_match(&mut matches, &mut match_index, &path, 4.0, None, None);
        }
    }

    // For top matches, add keyword context lines from actual files
    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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

/// Insert or update a match entry using O(1) HashMap lookup instead of linear scan.
fn upsert_match(
    matches: &mut Vec<SearchMatch>,
    index: &mut HashMap<String, usize>,
    path: &str,
    score_delta: f32,
    context_line: Option<ContextLine>,
    symbol_match: Option<String>,
) {
    if let Some(&idx) = index.get(path) {
        let entry = &mut matches[idx];
        entry.score += score_delta;
        if let Some(sym) = symbol_match {
            if entry.symbol_matches.len() < 5 {
                entry.symbol_matches.push(sym);
            }
        }
        if let Some(ctx) = context_line {
            if entry.context_lines.len() < 3 {
                entry.context_lines.push(ctx);
            }
        }
    } else {
        let idx = matches.len();
        index.insert(path.to_string(), idx);
        matches.push(SearchMatch {
            relative_path: path.to_string(),
            score: score_delta,
            context_lines: context_line.into_iter().collect(),
            symbol_matches: symbol_match.into_iter().collect(),
        });
    }
}

/// Live scan fallback when no index exists.
fn search_live(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let mut output = String::new();
    let mut matches: Vec<SearchMatch> = Vec::new();
    let query_lower = query.to_lowercase();
    let files = file_walker::walk_project(project_path)?;

    // Generate query embedding once (not per-file)
    let query_embedding = embedding_client::embed_text(query).ok();

    for file in &files {
        // Skip files over 1MB to avoid DoS
        if file.size_bytes > 1_000_000 {
            continue;
        }

        let content = match std::fs::read_to_string(&file.absolute_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut score: f32 = 0.0;
        let mut context_lines: Vec<ContextLine> = Vec::new();
        let mut symbol_matches: Vec<String> = Vec::new();

        // Filename match
        if file.relative_path.to_lowercase().contains(&query_lower) {
            score += 5.0;
        }

        // Line-level keyword search
        for (i, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                score += 1.0;
                if context_lines.len() < 3 {
                    context_lines.push(ContextLine {
                        line_num: i + 1,
                        content: line.trim().to_string(),
                    });
                }
            }
        }

        // Symbol-level search
        let has_ast = crate::indexer::config::has_ast_support(&file.extension);

        if has_ast {
            if let Ok(symbols) = symbol_extractor::extract_symbols(&content, &file.extension) {
                for sym in &symbols {
                    if sym.name.to_lowercase().contains(&query_lower) {
                        score += 3.0;
                        if symbol_matches.len() < 5 {
                            symbol_matches.push(format!(
                                "[{}] {} (L{}-L{}): {}",
                                sym.kind, sym.name, sym.start_line, sym.end_line, sym.signature
                            ));
                        }
                    }
                }
            }
        }

        // Semantic search — only generate embedding for files with some keyword signal
        // This avoids the O(files) embedding cost for files with zero relevance
        if score > 0.0 {
            if let Some(ref qe) = query_embedding {
                let file_summary = format!("{}\n{}", file.relative_path, content.chars().take(300).collect::<String>());
                if let Ok(fe) = embedding_client::embed_text(&file_summary) {
                    let sim = embedding_client::cosine_similarity(qe, &fe);
                    if sim > 0.3 {
                        score += sim * 5.0;
                    }
                }
            }
        }

        if score > 0.0 {
            matches.push(SearchMatch {
                relative_path: file.relative_path.clone(),
                score,
                context_lines,
                symbol_matches,
            });
        }
    }

    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    matches.truncate(max_results as usize);

    format_results(&matches, query, token_budget, &mut output)?;
    Ok(output)
}

fn format_results(matches: &[SearchMatch], query: &str, token_budget: u32, output: &mut String) -> Result<()> {
    if matches.is_empty() {
        writeln!(output, "No results found for: '{}'", query)?;
        return Ok(());
    }

    writeln!(output, "Found {} results for '{}':\n", matches.len(), query)?;

    let mut budget_used = token_estimator::estimate_tokens(output) as u32;

    for m in matches {
        let mut entry = String::new();
        writeln!(&mut entry, "## {} (score: {:.1})", m.relative_path, m.score)?;

        for sym in &m.symbol_matches {
            writeln!(&mut entry, "  SYMBOL: {}", sym)?;
        }

        for ctx in &m.context_lines {
            writeln!(&mut entry, "  L{}: {}", ctx.line_num, ctx.content)?;
        }
        writeln!(&mut entry)?;

        let entry_tokens = token_estimator::estimate_tokens(&entry) as u32;
        if budget_used + entry_tokens > token_budget {
            writeln!(output, "... [TRUNCATED — token budget reached]")?;
            break;
        }

        output.push_str(&entry);
        budget_used += entry_tokens;
    }

    Ok(())
}

struct SearchMatch {
    relative_path: String,
    score: f32,
    context_lines: Vec<ContextLine>,
    symbol_matches: Vec<String>,
}

struct ContextLine {
    line_num: usize,
    content: String,
}

struct SymbolRow {
    name: String,
    kind: String,
    start_line: usize,
    end_line: usize,
    signature: String,
    embedding: Option<Vec<f32>>,
    file_path: String,
    file_embedding: Option<Vec<f32>>,
}
