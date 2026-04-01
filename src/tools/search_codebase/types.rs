use std::collections::HashMap;
use std::fmt::Write;
use anyhow::Result;

use crate::context::token_estimator;

pub(super) struct SearchMatch {
    pub relative_path: String,
    pub score: f32,
    pub context_lines: Vec<ContextLine>,
    pub symbol_matches: Vec<String>,
}

pub(super) struct ContextLine {
    pub line_num: usize,
    pub content: String,
}

pub(super) struct SymbolRow {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub embedding: Option<Vec<f32>>,
    pub file_path: String,
}

impl SymbolRow {
    /// Map a rusqlite row to a SymbolRow. Expects columns:
    /// 0=name, 1=kind, 2=start_line(i64), 3=end_line(i64), 4=signature, 5=embedding(blob), 6=file_path
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        let sym_blob: Option<Vec<u8>> = row.get(5)?;
        Ok(Self {
            name: row.get(0)?,
            kind: row.get(1)?,
            start_line: row.get::<_, i64>(2)?.max(0) as usize,
            end_line: row.get::<_, i64>(3)?.max(0) as usize,
            signature: row.get(4)?,
            embedding: sym_blob.map(|b| crate::db::schema::blob_to_embedding(&b)),
            file_path: row.get(6)?,
        })
    }
}

/// Insert or update a match entry using O(1) HashMap lookup instead of linear scan.
pub(super) fn upsert_match(
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
            if entry.symbol_matches.len() < 3 {
                entry.symbol_matches.push(sym);
            }
        }
        if let Some(ctx) = context_line {
            if entry.context_lines.len() < 1 {
                entry.context_lines.push(ctx);
            }
        }
    } else {
        // Cap total unique matches to prevent unbounded memory growth
        if matches.len() >= crate::context::scoring::MAX_SEARCH_MATCHES {
            return;
        }
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

pub(super) fn format_results(matches: &[SearchMatch], query: &str, token_budget: u32, output: &mut String) -> Result<()> {
    // Warn user if semantic search is unavailable
    if !crate::indexer::embedding_client::is_model_available() {
        writeln!(output, "Warning: No semantic search — keyword matching only.\n")?;
    }

    if matches.is_empty() {
        writeln!(output, "No results for '{}'", query)?;
        return Ok(());
    }

    let mut budget_used = token_estimator::estimate_tokens(output) as u32;

    for m in matches {
        let mut entry = String::new();

        if !m.symbol_matches.is_empty() {
            for sym in &m.symbol_matches {
                writeln!(&mut entry, "{}: {}", m.relative_path, sym)?;
            }
        } else if !m.context_lines.is_empty() {
            for ctx in &m.context_lines {
                writeln!(&mut entry, "{} L{}: {}", m.relative_path, ctx.line_num, ctx.content.trim_start())?;
            }
        } else {
            writeln!(&mut entry, "{}", m.relative_path)?;
        }

        let entry_tokens = token_estimator::estimate_tokens(&entry) as u32;
        if budget_used + entry_tokens > token_budget {
            writeln!(output, "... [TRUNCATED]")?;
            break;
        }

        output.push_str(&entry);
        budget_used += entry_tokens;
    }

    Ok(())
}
