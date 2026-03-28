use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

use crate::indexer::{file_walker, symbol_extractor, embedding_client};

/// Search the codebase using keyword matching + symbol-aware search + semantic similarity.
pub fn search(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let char_budget = (token_budget as usize) * 4;
    let mut output = String::new();
    let mut matches: Vec<SearchMatch> = Vec::new();

    let query_lower = query.to_lowercase();
    let files = file_walker::walk_project(project_path)?;

    // Generate query embedding for semantic search
    let query_embedding = embedding_client::embed_text(query).ok();

    for file in &files {
        let content = match std::fs::read_to_string(&file.absolute_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut score: f32 = 0.0;
        let mut context_lines: Vec<ContextLine> = Vec::new();
        let mut symbol_matches: Vec<String> = Vec::new();

        // Penalize versioned/duplicate directories (tags/, releases/, node_modules/, vendor/)
        let path_lower = file.relative_path.to_lowercase();
        let depth_penalty: f32 = if path_lower.contains("/tags/") || path_lower.contains("/releases/")
            || path_lower.contains("/trunk/") || path_lower.contains("/vendor/")
            || path_lower.contains("node_modules/") || path_lower.contains(".svn")
        {
            0.3
        } else {
            1.0
        };

        // Boost source code files over docs/templates/configs
        let code_boost: f32 = match file.extension.as_str() {
            "js" | "ts" | "jsx" | "tsx" | "rs" | "py" => 1.3,
            "md" | "txt" | "json" | "yml" | "yaml" | "toml" => 0.8,
            "php" | "html" | "css" => 0.7,
            _ => 1.0,
        };

        // 1. Semantic similarity on rich file summary (highest signal)
        if let Some(ref qe) = query_embedding {
            // Build a rich summary: path + comments + symbol names + first content
            let mut file_summary = format!("{}\n", file.relative_path);

            // Extract JSDoc/comment blocks (first 3) — these describe what the file does
            let mut comment_count = 0;
            for line in content.lines() {
                let trimmed = line.trim();
                if (trimmed.starts_with("/**") || trimmed.starts_with(" *") || trimmed.starts_with("//"))
                    && !trimmed.starts_with("#!/")
                {
                    file_summary.push_str(trimmed);
                    file_summary.push('\n');
                    if trimmed.starts_with("/**") {
                        comment_count += 1;
                    }
                    if comment_count >= 3 {
                        break;
                    }
                }
            }

            // Add symbol names for semantic matching (e.g., "quantize dequantize batchQuantizedSearch")
            let ext = &file.extension;
            let has_ast = matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "py" | "pyi" | "rs");
            if has_ast {
                if let Ok(syms) = symbol_extractor::extract_symbols(&content, ext) {
                    let sym_names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
                    file_summary.push_str(&sym_names.join(" "));
                    file_summary.push('\n');
                }
            }

            // Add first 300 chars of content as fallback
            file_summary.push_str(&content.chars().take(300).collect::<String>());

            if let Ok(file_embedding) = embedding_client::embed_text(&file_summary) {
                let sim = embedding_client::cosine_similarity(qe, &file_embedding);
                if sim > 0.25 {
                    score += sim * 10.0; // semantic match is high value
                }
            }
        }

        // 2. Filename match
        if file.relative_path.to_lowercase().contains(&query_lower) {
            score += 5.0;
        }

        // 3. Line-level keyword search
        for (i, line) in content.lines().enumerate() {
            let line_lower = line.to_lowercase();
            if line_lower.contains(&query_lower) {
                score += 1.0;
                if line.contains(query) {
                    score += 0.5;
                }
                if context_lines.len() < 3 {
                    context_lines.push(ContextLine {
                        line_num: i + 1,
                        content: line.trim().to_string(),
                    });
                }
            }
        }

        // 4. Symbol-level search (tree-sitter)
        let ext = &file.extension;
        let has_ast = matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "py" | "pyi" | "rs");

        if has_ast {
            if let Ok(symbols) = symbol_extractor::extract_symbols(&content, ext) {
                for sym in &symbols {
                    let name_lower = sym.name.to_lowercase();
                    let sig_lower = sym.signature.to_lowercase();

                    // Keyword match on symbol
                    if name_lower.contains(&query_lower) || sig_lower.contains(&query_lower) {
                        score += 3.0;
                        symbol_matches.push(format!(
                            "[{}] {} (L{}-L{}): {}",
                            sym.kind, sym.name, sym.start_line, sym.end_line, sym.signature
                        ));
                    }

                    // Semantic match on symbol name + signature
                    if let Some(ref qe) = query_embedding {
                        let sym_text = format!("{} {}", sym.name, sym.signature);
                        if let Ok(sym_embedding) = embedding_client::embed_text(&sym_text) {
                            let sim = embedding_client::cosine_similarity(qe, &sym_embedding);
                            if sim > 0.4 {
                                score += sim * 5.0;
                                if !symbol_matches.iter().any(|s| s.contains(&sym.name)) {
                                    symbol_matches.push(format!(
                                        "[{}] {} (L{}-L{}, {:.0}% semantic): {}",
                                        sym.kind, sym.name, sym.start_line, sym.end_line,
                                        sim * 100.0, sym.signature
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        score *= depth_penalty * code_boost;

        if score > 0.0 {
            matches.push(SearchMatch {
                relative_path: file.relative_path.clone(),
                score,
                context_lines,
                symbol_matches,
            });
        }
    }

    // Sort by score descending
    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    matches.truncate(max_results as usize);

    if matches.is_empty() {
        writeln!(&mut output, "No results found for: '{}'", query)?;
        return Ok(output);
    }

    writeln!(&mut output, "Found {} results for '{}':\n", matches.len(), query)?;

    for m in &matches {
        writeln!(&mut output, "## {} (score: {:.1})", m.relative_path, m.score)?;

        for sym in &m.symbol_matches {
            writeln!(&mut output, "  SYMBOL: {}", sym)?;
        }

        for ctx in &m.context_lines {
            writeln!(&mut output, "  L{}: {}", ctx.line_num, ctx.content)?;
        }
        writeln!(&mut output)?;

        if output.len() > char_budget {
            writeln!(&mut output, "... [TRUNCATED — token budget reached]")?;
            break;
        }
    }

    Ok(output)
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
