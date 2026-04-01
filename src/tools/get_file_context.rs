use std::fmt::Write;
use std::path::Path;
use anyhow::{Result, bail};

use crate::context::{token_estimator, file_summarizer};
use crate::indexer::symbol_extractor;

fn truncate_sig(sig: &str, max_chars: usize) -> String {
    if sig.len() <= max_chars { return sig.to_string(); }
    let cut = sig[..max_chars].rfind(|c: char| c == ',' || c == ')').unwrap_or(max_chars);
    format!("{}...", &sig[..cut])
}

/// Read a file with smart token optimization based on mode.
///
/// Modes:
/// - "full": entire file (truncated at budget)
/// - "summary": imports + AST signatures (no query awareness)
/// - "smart": query-aware — includes full bodies for relevant symbols, signatures for others
/// - "symbols": compact symbol list with line numbers
pub fn read_file_context(path: &Path, mode: &str, token_budget: u32, query: Option<&str>) -> Result<String> {
    if !path.exists() {
        bail!("File not found: {}", path.display());
    }

    // Check file size before reading — reject files over 10MB
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > 10 * 1024 * 1024 {
        bail!(
            "File too large ({:.1}MB). Use get_symbol to extract specific functions.",
            metadata.len() as f64 / (1024.0 * 1024.0)
        );
    }

    let content = std::fs::read_to_string(path)?;

    match mode {
        "full" => read_full(&content, path, token_budget),
        "summary" => file_summarizer::smart_summarize(&content, path, token_budget, None),
        "smart" => file_summarizer::smart_summarize(&content, path, token_budget, query),
        "symbols" => read_symbols(&content, path),
        _ => bail!("Unknown mode: '{}'. Use 'full', 'summary', 'smart', or 'symbols'.", mode),
    }
}

fn read_full(content: &str, path: &Path, token_budget: u32) -> Result<String> {
    let mut output = String::new();
    let line_count = content.lines().count();
    let char_budget = token_estimator::tokens_to_chars(token_budget);

    writeln!(&mut output, "# {} ({} lines)", path.display(), line_count)?;
    writeln!(&mut output)?;

    if content.len() > char_budget {
        let truncated: String = content.chars().take(char_budget).collect();
        writeln!(&mut output, "{}", truncated)?;
        writeln!(&mut output)?;
        writeln!(
            &mut output,
            "... [TRUNCATED — showing ~{} of ~{} tokens. Use 'smart' mode or get_symbol for specific functions]",
            token_budget,
            token_estimator::estimate_tokens(content)
        )?;
    } else {
        write!(&mut output, "{}", content)?;
    }

    Ok(output)
}

fn read_symbols(content: &str, path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let line_count = content.lines().count();
    let mut output = String::new();

    writeln!(&mut output, "# Symbols in {} ({} lines)\n", path.display(), line_count)?;

    let has_ast = crate::indexer::config::has_ast_support(ext);

    if has_ast {
        match symbol_extractor::extract_symbols(content, ext) {
            Ok(symbols) if !symbols.is_empty() => {
                for sym in &symbols {
                    let short_sig = truncate_sig(&sym.signature, 100);
                    writeln!(
                        &mut output,
                        "L{}-{} [{}] {}: {}",
                        sym.start_line, sym.end_line, sym.kind.short(), sym.name, short_sig
                    )?;
                }
                return Ok(output);
            }
            _ => {}
        }
    }

    // Fallback
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if is_symbol_line(trimmed, ext) {
            writeln!(&mut output, "L{}: {}", i + 1, trimmed)?;
        }
    }

    Ok(output)
}

fn is_symbol_line(trimmed: &str, ext: &str) -> bool {
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            trimmed.starts_with("export ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("enum ")
        }
        "py" | "pyi" => {
            trimmed.starts_with("def ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("async def ")
        }
        "rs" => {
            trimmed.starts_with("pub ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("trait ")
                || trimmed.starts_with("impl ")
        }
        "go" => trimmed.starts_with("func ") || trimmed.starts_with("type "),
        _ => false,
    }
}
