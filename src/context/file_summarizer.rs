use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

use crate::context::token_estimator;
use crate::context::relevance_scorer;
use crate::indexer::symbol_extractor::{self, Symbol};
use crate::indexer::embedding_client;

/// Smart file summarization that returns only the most relevant parts within a token budget.
///
/// Unlike the old summary mode (imports + all signatures), this:
/// 1. Extracts all symbols via AST
/// 2. Scores each symbol for relevance to the query
/// 3. Includes full function bodies for high-relevance symbols
/// 4. Includes only signatures for medium-relevance symbols
/// 5. Omits low-relevance symbols entirely
/// 6. Packs results within the token budget, highest relevance first
pub fn smart_summarize(
    content: &str,
    path: &Path,
    token_budget: u32,
    query: Option<&str>,
) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let line_count = content.lines().count();
    let mut output = String::new();

    // Header
    writeln!(&mut output, "# {} ({} lines)\n", path.display(), line_count)?;

    // If no query or non-AST file, fall back to structural summary
    let has_ast = matches!(
        ext,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs"
        | "py" | "pyi"
        | "rs"
        | "go"
        | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx"
    );

    if !has_ast || query.is_none() {
        return structural_summary(content, path, token_budget, ext, &mut output);
    }

    let query = query.unwrap();

    // Extract symbols
    let symbols = match symbol_extractor::extract_symbols(content, ext) {
        Ok(s) if !s.is_empty() => s,
        _ => return structural_summary(content, path, token_budget, ext, &mut output),
    };

    // Generate query embedding once
    let query_embedding = embedding_client::embed_text(query).ok();

    // Batch-score all symbols (single model call instead of N calls)
    let scores = relevance_scorer::score_symbols_batch(&symbols, query, query_embedding.as_deref());

    let mut scored: Vec<(f32, &Symbol)> = scores
        .into_iter()
        .zip(symbols.iter())
        .map(|(score, sym)| (score, sym))
        .collect();

    // Sort by relevance (highest first)
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Always include imports (cheap, provides context)
    let imports = extract_imports(content, ext);
    if !imports.is_empty() {
        writeln!(&mut output, "## Imports\n{}\n", imports)?;
    }

    let mut remaining_budget = token_budget.saturating_sub(token_estimator::estimate_tokens(&output) as u32);

    // Pack symbols into budget
    let mut full_count = 0usize;
    let mut sig_count = 0usize;

    for (score, sym) in &scored {
        if remaining_budget < 20 {
            break;
        }

        let lines_span = sym.end_line.saturating_sub(sym.start_line) + 1;

        if *score > 0.25 {
            // High relevance: include full code body
            let code_tokens = token_estimator::estimate_tokens(&sym.code) as u32;

            if code_tokens <= remaining_budget {
                writeln!(
                    &mut output,
                    "## [{}] {} (L{}-L{}, relevance: {:.0}%)\n```{}\n{}\n```\n",
                    sym.kind, sym.name, sym.start_line, sym.end_line,
                    score * 100.0, ext, sym.code
                )?;
                remaining_budget = remaining_budget.saturating_sub(code_tokens + 10);
                full_count += 1;
            } else {
                // Code too large for budget — include signature only
                let sig_line = format!(
                    "- [{}] **{}** (L{}-L{}, {} lines): `{}`",
                    sym.kind, sym.name, sym.start_line, sym.end_line, lines_span, sym.signature
                );
                let sig_tokens = token_estimator::estimate_tokens(&sig_line) as u32;
                if sig_tokens <= remaining_budget {
                    writeln!(&mut output, "{}", sig_line)?;
                    remaining_budget = remaining_budget.saturating_sub(sig_tokens);
                    sig_count += 1;
                }
            }
        } else if *score > 0.05 {
            // Medium relevance: signature only
            let sig_line = format!(
                "- [{}] **{}** (L{}-L{}, {} lines): `{}`",
                sym.kind, sym.name, sym.start_line, sym.end_line, lines_span, sym.signature
            );
            let sig_tokens = token_estimator::estimate_tokens(&sig_line) as u32;
            if sig_tokens <= remaining_budget {
                writeln!(&mut output, "{}", sig_line)?;
                remaining_budget = remaining_budget.saturating_sub(sig_tokens);
                sig_count += 1;
            }
        }
        // Low relevance (< 0.05): omit entirely
    }

    // Footer with stats
    let omitted = symbols.len().saturating_sub(full_count + sig_count);
    if omitted > 0 {
        writeln!(&mut output, "\n_({} full, {} signatures, {} omitted as low-relevance)_", full_count, sig_count, omitted)?;
    }

    Ok(output)
}

/// Structural summary: imports + all symbol signatures (no query-based filtering).
/// Used when there's no query context or for non-AST files.
fn structural_summary(
    content: &str,
    _path: &Path,
    token_budget: u32,
    ext: &str,
    output: &mut String,
) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let char_budget = token_estimator::tokens_to_chars(token_budget);

    // Imports
    let imports = extract_imports(content, ext);
    if !imports.is_empty() {
        writeln!(output, "## Imports\n{}\n", imports)?;
    }

    // Try AST symbols
    let has_ast = matches!(
        ext,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs"
        | "py" | "pyi" | "rs" | "go"
        | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx"
    );

    if has_ast {
        if let Ok(symbols) = symbol_extractor::extract_symbols(content, ext) {
            if !symbols.is_empty() {
                writeln!(output, "## Symbols ({} found)\n", symbols.len())?;
                for sym in &symbols {
                    let span = sym.end_line.saturating_sub(sym.start_line) + 1;
                    writeln!(
                        output,
                        "- [{}] **{}** (L{}-L{}, {} lines): `{}`",
                        sym.kind, sym.name, sym.start_line, sym.end_line, span, sym.signature
                    )?;
                    if output.len() > char_budget {
                        writeln!(output, "\n... [TRUNCATED — budget reached]")?;
                        break;
                    }
                }
            }
        }
    } else {
        // Fallback: definition lines
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if is_definition_line(trimmed, ext) {
                writeln!(output, "L{}: {}", i + 1, trimmed)?;
                if output.len() > char_budget {
                    writeln!(output, "\n... [TRUNCATED — budget reached]")?;
                    break;
                }
            }
        }
    }

    // Truncate if over budget
    if output.len() > char_budget {
        output.truncate(char_budget);
        output.push_str("\n... [TRUNCATED]");
    }

    Ok(output.clone())
}

fn extract_imports(content: &str, ext: &str) -> String {
    content
        .lines()
        .filter(|l| is_import_line(l.trim(), ext))
        .collect::<Vec<&str>>()
        .join("\n")
}

fn is_import_line(line: &str, ext: &str) -> bool {
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            line.starts_with("import ")
                || (line.starts_with("const ") && line.contains("require("))
        }
        "py" | "pyi" => line.starts_with("import ") || line.starts_with("from "),
        "rs" => line.starts_with("use ") || line.starts_with("mod "),
        "go" => line.starts_with("import "),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => line.starts_with("#include"),
        _ => false,
    }
}

fn is_definition_line(line: &str, ext: &str) -> bool {
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            line.starts_with("export ")
                || line.starts_with("function ")
                || line.starts_with("class ")
                || line.starts_with("const ")
                || line.starts_with("async function ")
        }
        "py" | "pyi" => line.starts_with("def ") || line.starts_with("class "),
        "rs" => {
            line.starts_with("pub ") || line.starts_with("fn ") || line.starts_with("struct ")
                || line.starts_with("enum ") || line.starts_with("trait ") || line.starts_with("impl ")
        }
        _ => false,
    }
}
