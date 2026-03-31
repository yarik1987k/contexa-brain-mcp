use std::path::Path;
use anyhow::Result;

use crate::context::scoring;
use crate::indexer::{file_walker, symbol_extractor, embedding_client};

use super::types::*;

/// Live scan fallback when no index exists.
pub fn search_live(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let mut output = String::new();
    let query_lower = query.to_lowercase();
    let files = file_walker::walk_project(project_path)?;

    let query_embedding = embedding_client::try_embed_text(query);

    // Phase 1: keyword + symbol scoring (no embedding calls)
    struct Candidate {
        relative_path: String,
        score: f32,
        context_lines: Vec<ContextLine>,
        symbol_matches: Vec<String>,
        summary: String, // for batch embedding
    }

    let mut candidates: Vec<Candidate> = Vec::new();

    for file in &files {
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

        if file.relative_path.to_lowercase().contains(&query_lower) {
            score += scoring::SEARCH_EXACT_NAME_BONUS;
        }

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

        let has_ast = crate::indexer::config::has_ast_support(&file.extension);
        if has_ast {
            if let Ok(symbols) = symbol_extractor::extract_symbols(&content, &file.extension) {
                for sym in &symbols {
                    if is_word_match(&sym.name.to_lowercase(), &query_lower) {
                        score += scoring::SEARCH_SUBSTRING_NAME_BONUS;
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

        if score > 0.0 {
            let summary = format!("{}\n{}", file.relative_path, content.chars().take(300).collect::<String>());
            candidates.push(Candidate {
                relative_path: file.relative_path.clone(),
                score, context_lines, symbol_matches, summary,
            });
            if candidates.len() >= 500 {
                break;
            }
        }
    }

    // Phase 2: batch embed all candidate summaries at once
    if let Some(ref qe) = query_embedding {
        if !candidates.is_empty() {
            let summaries: Vec<&str> = candidates.iter().map(|c| c.summary.as_str()).collect();
            let embeddings = embedding_client::try_embed_batch(&summaries);
            for (i, candidate) in candidates.iter_mut().enumerate() {
                if let Some(fe) = embeddings.get(i) {
                    let sim = embedding_client::cosine_similarity(qe, fe);
                    if sim > scoring::SEARCH_SYMBOL_SIM_THRESHOLD {
                        candidate.score += sim * scoring::SEARCH_SYMBOL_SIM_WEIGHT;
                    }
                }
            }
        }
    }

    let mut matches: Vec<SearchMatch> = candidates.into_iter().map(|c| SearchMatch {
        relative_path: c.relative_path,
        score: c.score,
        context_lines: c.context_lines,
        symbol_matches: c.symbol_matches,
    }).collect();

    matches.sort_by(|a, b| crate::context::relevance_scorer::cmp_score_desc(a.score, b.score));
    matches.truncate(max_results as usize);

    format_results(&matches, query, token_budget, &mut output)?;
    Ok(output)
}

/// Check if query appears as a word boundary match (not substring of unrelated word).
fn is_word_match(haystack: &str, needle: &str) -> bool {
    if let Some(pos) = haystack.find(needle) {
        let before_ok = pos == 0 || !haystack.as_bytes()[pos - 1].is_ascii_alphanumeric();
        let after_pos = pos + needle.len();
        let after_ok = after_pos >= haystack.len() || !haystack.as_bytes()[after_pos].is_ascii_alphanumeric();
        // Also allow camelCase/snake_case boundaries
        let after_ok = after_ok || haystack.as_bytes().get(after_pos).map(|b| *b == b'_' || b.is_ascii_uppercase()).unwrap_or(true);
        let before_ok = before_ok || haystack.as_bytes().get(pos.wrapping_sub(1)).map(|b| *b == b'_').unwrap_or(true);
        before_ok || after_ok
    } else {
        false
    }
}
