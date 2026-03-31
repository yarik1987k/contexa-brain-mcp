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
        // Cap total unique matches to prevent unbounded memory growth
        if matches.len() >= 500 {
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
