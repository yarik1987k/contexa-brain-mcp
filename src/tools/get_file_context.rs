use std::fmt::Write;
use std::path::Path;
use anyhow::{Result, bail};

use crate::indexer::symbol_extractor;

/// Read a file with smart token optimization based on mode.
pub fn read_file_context(path: &Path, mode: &str, token_budget: u32) -> Result<String> {
    if !path.exists() {
        bail!("File not found: {}", path.display());
    }

    let content = std::fs::read_to_string(path)?;
    let char_budget = (token_budget as usize) * 4; // ~4 chars per token

    match mode {
        "full" => read_full(&content, path, char_budget),
        "summary" => read_summary(&content, path, char_budget),
        "symbols" => read_symbols(&content, path),
        _ => bail!("Unknown mode: '{}'. Use 'full', 'summary', or 'symbols'.", mode),
    }
}

fn read_full(content: &str, path: &Path, char_budget: usize) -> Result<String> {
    let mut output = String::new();
    let line_count = content.lines().count();
    writeln!(&mut output, "# {} ({} lines)", path.display(), line_count)?;
    writeln!(&mut output)?;

    if content.len() > char_budget {
        let truncated: String = content.chars().take(char_budget).collect();
        writeln!(&mut output, "{}", truncated)?;
        writeln!(&mut output)?;
        writeln!(
            &mut output,
            "... [TRUNCATED — showing {}/{} chars. Use a larger token_budget or 'summary' mode]",
            char_budget,
            content.len()
        )?;
    } else {
        write!(&mut output, "{}", content)?;
    }

    Ok(output)
}

fn read_summary(content: &str, path: &Path, char_budget: usize) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut output = String::new();

    writeln!(&mut output, "# {} ({} lines)\n", path.display(), line_count)?;

    // Extract imports
    let imports: Vec<&str> = lines
        .iter()
        .filter(|l| is_import_line(l, ext))
        .copied()
        .collect();

    if !imports.is_empty() {
        writeln!(&mut output, "## Imports")?;
        for imp in &imports {
            writeln!(&mut output, "{}", imp)?;
        }
        writeln!(&mut output)?;
    }

    // Try tree-sitter AST extraction for supported languages
    let has_ast = matches!(
        ext,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs"
        | "py" | "pyi"
        | "rs"
        | "go"
        | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx"
    );

    if has_ast {
        match symbol_extractor::extract_symbols(content, ext) {
            Ok(symbols) if !symbols.is_empty() => {
                writeln!(&mut output, "## Symbols ({} found)\n", symbols.len())?;
                for sym in &symbols {
                    let lines_span = sym.end_line - sym.start_line + 1;
                    writeln!(
                        &mut output,
                        "- [{}] **{}** (L{}-L{}, {} lines): `{}`",
                        sym.kind, sym.name, sym.start_line, sym.end_line, lines_span, sym.signature
                    )?;
                }
            }
            _ => {
                // Fallback to regex-based extraction
                let signatures = extract_signatures_fallback(&lines, ext);
                if !signatures.is_empty() {
                    writeln!(&mut output, "## Signatures\n")?;
                    for sig in &signatures {
                        writeln!(&mut output, "{}", sig)?;
                    }
                }
            }
        }
    } else {
        // Non-AST languages: fallback
        let signatures = extract_signatures_fallback(&lines, ext);
        if !signatures.is_empty() {
            writeln!(&mut output, "## Signatures\n")?;
            for sig in &signatures {
                writeln!(&mut output, "{}", sig)?;
            }
        }
    }

    // Truncate if over budget
    if output.len() > char_budget {
        output.truncate(char_budget);
        output.push_str("\n... [TRUNCATED]");
    }

    Ok(output)
}

fn read_symbols(content: &str, path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let line_count = content.lines().count();
    let mut output = String::new();

    writeln!(&mut output, "# Symbols in {} ({} lines)\n", path.display(), line_count)?;

    // Try tree-sitter first
    let has_ast = matches!(
        ext,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs"
        | "py" | "pyi"
        | "rs"
        | "go"
        | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx"
    );

    if has_ast {
        match symbol_extractor::extract_symbols(content, ext) {
            Ok(symbols) if !symbols.is_empty() => {
                for sym in &symbols {
                    writeln!(
                        &mut output,
                        "L{}-L{} [{}] {}: {}",
                        sym.start_line, sym.end_line, sym.kind, sym.name, sym.signature
                    )?;
                }
                return Ok(output);
            }
            _ => {}
        }
    }

    // Fallback: line-by-line detection
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if is_symbol_line(trimmed, ext) {
            writeln!(&mut output, "L{}: {}", i + 1, trimmed)?;
        }
    }

    Ok(output)
}

fn is_import_line(line: &str, ext: &str) -> bool {
    let trimmed = line.trim();
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            trimmed.starts_with("import ")
                || (trimmed.starts_with("const ") && trimmed.contains("require("))
        }
        "py" | "pyi" => trimmed.starts_with("import ") || trimmed.starts_with("from "),
        "rs" => trimmed.starts_with("use ") || trimmed.starts_with("mod "),
        "go" => trimmed.starts_with("import "),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => trimmed.starts_with("#include"),
        _ => false,
    }
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
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => {
            // Heuristic: lines that look like function definitions or struct/class declarations
            trimmed.starts_with("struct ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("typedef ")
                || (trimmed.contains('(') && trimmed.ends_with('{'))
        }
        _ => false,
    }
}

fn extract_signatures_fallback(lines: &[&str], ext: &str) -> Vec<String> {
    let mut sigs = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if is_symbol_line(trimmed, ext) {
            sigs.push(format!("L{}: {}", i + 1, trimmed));
        }
    }
    sigs
}
