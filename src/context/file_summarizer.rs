use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

use crate::context::{token_estimator, relevance_scorer, scoring};
use crate::indexer::symbol_extractor::{self, Symbol};
use crate::indexer::embedding_client;

fn truncate_signature(sig: &str, max_chars: usize) -> String {
    if sig.len() <= max_chars {
        return sig.to_string();
    }
    let cut = sig[..max_chars]
        .rfind(|c: char| c == ',' || c == ')')
        .unwrap_or(max_chars);
    format!("{}...", &sig[..cut])
}

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
    let mut output = String::new();

    // Header
    writeln!(&mut output, "# {}\n", path.display())?;

    // If no query or non-AST file, fall back to structural summary
    let has_ast = crate::indexer::config::has_ast_support(ext);

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
    let query_embedding = embedding_client::try_embed_text(query);

    // Batch-score all symbols (single model call instead of N calls)
    let scores = relevance_scorer::score_symbols_batch(&symbols, query, query_embedding.as_deref());

    let mut scored: Vec<(f32, &Symbol)> = scores
        .into_iter()
        .zip(symbols.iter())
        .map(|(score, sym)| (score, sym))
        .collect();

    // Sort by relevance (highest first)
    scored.sort_by(|a, b| relevance_scorer::cmp_score_desc(a.0, b.0));

    // Compact imports: count + module names only
    let imports = extract_imports(content, ext);
    if !imports.is_empty() {
        let import_lines: Vec<&str> = imports.lines().collect();
        let modules = extract_module_names(&imports, ext);
        if modules.is_empty() {
            writeln!(&mut output, "Imports: {}\n", import_lines.len())?;
        } else {
            writeln!(&mut output, "Imports({}): {}\n", import_lines.len(), modules.join(", "))?;
        }
    }

    let mut remaining_budget = token_budget.saturating_sub(token_estimator::estimate_tokens(&output) as u32);

    // Pack symbols into budget
    for (score, sym) in &scored {
        if remaining_budget < scoring::MIN_BUDGET_TOKENS {
            break;
        }

        if *score > scoring::RELEVANCE_HIGH_THRESHOLD {
            // High relevance: include code body (stripped + truncated)
            let stripped_code = strip_comments(&sym.code, ext);
            let stripped_lines: Vec<&str> = stripped_code.lines().collect();
            let code_to_show = if stripped_lines.len() > scoring::MAX_BODY_LINES_IN_SUMMARY + 10 {
                let truncated: String = stripped_lines[..scoring::MAX_BODY_LINES_IN_SUMMARY].join("\n");
                let remaining = stripped_lines.len() - scoring::MAX_BODY_LINES_IN_SUMMARY;
                format!("{}\n// ... +{} more lines", truncated, remaining)
            } else {
                stripped_code
            };
            let code_tokens = token_estimator::estimate_tokens(&code_to_show) as u32;

            if code_tokens <= remaining_budget {
                writeln!(
                    &mut output,
                    "[{}] {} L{}-{}:\n```{}\n{}\n```\n",
                    sym.kind.short(), sym.name, sym.start_line, sym.end_line,
                    ext, code_to_show
                )?;
                remaining_budget = remaining_budget.saturating_sub(code_tokens + 10);
            } else {
                // Code too large for budget — include signature only
                let sig_line = format!(
                    "[{}] {} L{}-{}: {}",
                    sym.kind.short(), sym.name, sym.start_line, sym.end_line, truncate_signature(&sym.signature, 100)
                );
                let sig_tokens = token_estimator::estimate_tokens(&sig_line) as u32;
                if sig_tokens <= remaining_budget {
                    writeln!(&mut output, "{}", sig_line)?;
                    remaining_budget = remaining_budget.saturating_sub(sig_tokens);
                }
            }
        } else if *score > scoring::RELEVANCE_MEDIUM_THRESHOLD {
            // Medium relevance: signature only
            let sig_line = format!(
                "- [{}] {} L{}-{}: {}",
                sym.kind.short(), sym.name, sym.start_line, sym.end_line, truncate_signature(&sym.signature, 100)
            );
            let sig_tokens = token_estimator::estimate_tokens(&sig_line) as u32;
            if sig_tokens <= remaining_budget {
                writeln!(&mut output, "{}", sig_line)?;
                remaining_budget = remaining_budget.saturating_sub(sig_tokens);
            }
        }
        // Low relevance (< 0.05): omit entirely
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

    // Compact imports: count + module names
    let imports = extract_imports(content, ext);
    if !imports.is_empty() {
        let import_lines: Vec<&str> = imports.lines().collect();
        let modules = extract_module_names(&imports, ext);
        if modules.is_empty() {
            writeln!(output, "Imports: {}\n", import_lines.len())?;
        } else {
            writeln!(output, "Imports({}): {}\n", import_lines.len(), modules.join(", "))?;
        }
    }

    // Try AST symbols
    let has_ast = crate::indexer::config::has_ast_support(ext);

    if has_ast {
        if let Ok(symbols) = symbol_extractor::extract_symbols(content, ext) {
            if !symbols.is_empty() {
                // Symbol list follows directly
                for sym in &symbols {
                    writeln!(
                        output,
                        "[{}] {} L{}-{}: {}",
                        sym.kind.short(), sym.name, sym.start_line, sym.end_line, truncate_signature(&sym.signature, 100)
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

/// Strip single-line comments and blank lines from code to save tokens.
/// Preserves JSDoc/docstrings on the first line only (they're often the function signature).
fn strip_comments(code: &str, ext: &str) -> String {
    let comment_prefix: &[&str] = match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "rs" | "go" | "c" | "cpp" | "h" => &["//"],
        "py" | "pyi" => &["#"],
        _ => return code.to_string(),
    };
    code.lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() { return false; }
            // Keep lines that start with code, not comments
            !comment_prefix.iter().any(|p| trimmed.starts_with(p))
                // But keep lines like `// ... +N more lines` (our own truncation markers)
                || trimmed.starts_with("// ...")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract a quoted module name from a JS/TS import or require line
fn extract_quoted_module(line: &str) -> Option<&str> {
    // Find first quote character
    for quote in ['\'', '"'] {
        if let Some(start) = line.find(quote) {
            let rest = &line[start+1..];
            if let Some(end) = rest.find(quote) {
                return Some(&rest[..end]);
            }
        }
    }
    None
}

/// Extract short module names from import lines (e.g., "express", "mongoose", "react")
fn extract_module_names(imports: &str, ext: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in imports.lines() {
        let trimmed = line.trim();
        let module = match ext {
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
                // import X from 'module' or require('module')
                if let Some(m) = extract_quoted_module(trimmed) {
                    m
                } else { continue }
            }
            "py" | "pyi" => {
                if trimmed.starts_with("from ") {
                    trimmed[5..].split_whitespace().next().unwrap_or("")
                } else if trimmed.starts_with("import ") {
                    trimmed[7..].split(',').next().unwrap_or("").trim()
                } else { continue }
            }
            "rs" => {
                if trimmed.starts_with("use ") {
                    trimmed[4..].split("::").next().unwrap_or("").trim_end_matches(';')
                } else { continue }
            }
            "go" => {
                if let Some(pos) = trimmed.find('"') {
                    let rest = &trimmed[pos+1..];
                    rest.split('"').next().unwrap_or("").rsplit('/').next().unwrap_or("")
                } else { continue }
            }
            _ => continue,
        };
        // Take just the package name (strip path prefixes like ./ or ../)
        let short = module.trim_start_matches("./").trim_start_matches("../").rsplit('/').next().unwrap_or(module);
        if !short.is_empty() && seen.insert(short.to_string()) {
            names.push(short.to_string());
        }
    }
    names
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
