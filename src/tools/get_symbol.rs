use std::fmt::Write;
use std::path::Path;
use anyhow::{Result, bail};

use crate::db::schema;

fn truncate_sig(sig: &str, max_chars: usize) -> String {
    if sig.len() <= max_chars { return sig.to_string(); }
    let cut = sig[..max_chars].rfind(|c: char| c == ',' || c == ')').unwrap_or(max_chars);
    format!("{}...", &sig[..cut])
}

/// Get a specific symbol (function/class/struct) by name.
/// Returns the exact code block from the source file.
pub fn get_symbol(project_path: &Path, name: &str, file_hint: Option<&str>, max_lines: usize) -> Result<String> {
    let db_path = schema::db_path(project_path);

    // Try indexed database first
    if db_path.exists() {
        if let Ok(result) = get_from_index(project_path, name, file_hint, max_lines) {
            if !result.is_empty() && !result.starts_with("No exact match") {
                return Ok(result);
            }
        }
    }

    get_from_files(project_path, name, file_hint, max_lines)
}

fn get_from_index(project_path: &Path, name: &str, file_hint: Option<&str>, max_lines: usize) -> Result<String> {
    let conn = schema::open_db(project_path)?;
    let mut output = String::new();

    let name_lower = name.to_lowercase();

    // Query symbols by name
    let sql = if file_hint.is_some() {
        "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, f.relative_path
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE LOWER(s.name) = ?1 AND f.relative_path LIKE ?2
         ORDER BY s.start_line"
    } else {
        "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, f.relative_path
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE LOWER(s.name) = ?1
         ORDER BY f.relative_path, s.start_line"
    };

    let mut stmt = conn.prepare(sql)?;

    let results: Vec<(String, String, i64, i64, String, String)> = if let Some(hint) = file_hint {
        let pattern = format!("%{}%", hint);
        stmt.query_map(rusqlite::params![&name_lower, &pattern], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?
        .filter_map(|r| r.ok())
        .collect()
    } else {
        stmt.query_map(rusqlite::params![&name_lower], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?
        .filter_map(|r| r.ok())
        .collect()
    };

    if results.is_empty() {
        // Try fuzzy match
        let mut fuzzy_stmt = conn.prepare(
            "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, f.relative_path
             FROM symbols s JOIN files f ON s.file_id = f.id
             WHERE LOWER(s.name) LIKE ?1
             ORDER BY f.relative_path, s.start_line
             LIMIT 5",
        )?;
        let pattern = format!("%{}%", name_lower);
        let fuzzy: Vec<(String, String, i64, i64, String, String)> = fuzzy_stmt
            .query_map(rusqlite::params![&pattern], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        if fuzzy.is_empty() {
            return Ok(String::new());
        }

        writeln!(&mut output, "No exact match for '{}'. Similar:\n", name)?;
        for (sname, kind, start, end, sig, path) in &fuzzy {
            let short_sig = truncate_sig(sig, 80);
            writeln!(&mut output, "- [{}] {} in {} L{}-{}: {}",
                crate::indexer::symbol_extractor::abbreviate_kind(kind),
                sname, path, start, end, short_sig)?;
        }
        return Ok(output);
    }

    // Resolve Export symbols with small spans — look for the actual Function definition
    let mut resolved_results: Vec<(String, String, i64, i64, String, String)> = Vec::new();
    for (sname, kind, start_line, end_line, sig, rel_path) in results {
        if kind == "Export" && (end_line - start_line) < 5 {
            // Try to find a Function/AsyncFunction with the same name in the same file
            let mut resolve_stmt = conn.prepare(
                "SELECT s.name, s.kind, s.start_line, s.end_line, s.signature, f.relative_path
                 FROM symbols s JOIN files f ON s.file_id = f.id
                 WHERE LOWER(s.name) = ?1 AND f.relative_path = ?2
                   AND s.kind IN ('Function', 'AsyncFunction')
                 ORDER BY (s.end_line - s.start_line) DESC
                 LIMIT 1"
            )?;
            let func_match: Option<(String, String, i64, i64, String, String)> = resolve_stmt
                .query_map(rusqlite::params![&sname.to_lowercase(), &rel_path], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
                })?
                .filter_map(|r| r.ok())
                .next();

            if let Some(func) = func_match {
                resolved_results.push(func);
            } else {
                resolved_results.push((sname, kind, start_line, end_line, sig, rel_path));
            }
        } else {
            resolved_results.push((sname, kind, start_line, end_line, sig, rel_path));
        }
    }

    // Deduplicate by (file, start_line)
    {
        let mut seen = std::collections::HashSet::new();
        resolved_results.retain(|r| seen.insert((r.5.clone(), r.2)));
    }

    // For each match, read the actual code from the file
    for (sname, kind, start_line, end_line, _sig, rel_path) in &resolved_results {
        let file_path = project_path.join(rel_path);
        let resolved = file_path.canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot resolve path {}: {}", rel_path, e))?;
        let canonical_project = project_path.canonicalize().unwrap_or_else(|_| project_path.to_path_buf());
        if !resolved.starts_with(&canonical_project) {
            bail!("Path escapes project directory: {}", rel_path);
        }
        let file_path = resolved;
        writeln!(&mut output, "[{}] {} in {} L{}-{}",
            crate::indexer::symbol_extractor::abbreviate_kind(kind),
            sname, rel_path, start_line, end_line)?;

        if let Ok(content) = std::fs::read_to_string(&file_path) {
            let lines: Vec<&str> = content.lines().collect();
            let start = ((*start_line).max(0) as usize).saturating_sub(1);
            let end = ((*end_line).max(0) as usize).min(lines.len());
            let total_lines = end - start;

            if max_lines > 0 && total_lines > max_lines {
                for i in start..(start + max_lines) {
                    writeln!(&mut output, "{}", lines.get(i).unwrap_or(&""))?;
                }
                writeln!(&mut output, "// ... +{} more lines (pass max_lines=0 for full)", total_lines - max_lines)?;
            } else {
                for i in start..end {
                    writeln!(&mut output, "{}", lines.get(i).unwrap_or(&""))?;
                }
            }
        }
        writeln!(&mut output)?;
    }

    Ok(output)
}

fn get_from_files(project_path: &Path, name: &str, file_hint: Option<&str>, max_lines: usize) -> Result<String> {
    use crate::indexer::{file_walker, symbol_extractor};

    let files = file_walker::walk_project(project_path)?;
    let mut output = String::new();
    let name_lower = name.to_lowercase();

    for file in &files {
        // If file hint provided, filter
        if let Some(hint) = file_hint {
            if !file.relative_path.to_lowercase().contains(&hint.to_lowercase()) {
                continue;
            }
        }

        let has_ast = crate::indexer::config::has_ast_support(&file.extension);

        if !has_ast {
            continue;
        }

        let content = match std::fs::read_to_string(&file.absolute_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Ok(symbols) = symbol_extractor::extract_symbols(&content, &file.extension) {
            for sym in &symbols {
                if sym.name.to_lowercase() == name_lower
                    || sym.name.to_lowercase().contains(&name_lower)
                {
                    writeln!(
                        &mut output,
                        "[{}] {} in {} L{}-{}",
                        sym.kind.short(), sym.name, file.relative_path, sym.start_line, sym.end_line
                    )?;

                    let lines: Vec<&str> = content.lines().collect();
                    let start = sym.start_line.saturating_sub(1);
                    let end = sym.end_line.min(lines.len());
                    let total = end - start;

                    if max_lines > 0 && total > max_lines {
                        for i in start..(start + max_lines) {
                            writeln!(&mut output, "{}", lines.get(i).unwrap_or(&""))?;
                        }
                        writeln!(&mut output, "// ... +{} more lines (pass max_lines=0 for full)", total - max_lines)?;
                    } else {
                        for i in start..end {
                            writeln!(&mut output, "{}", lines.get(i).unwrap_or(&""))?;
                        }
                    }
                    writeln!(&mut output)?;
                }
            }
        }
    }

    // If AST extraction found nothing, try raw text search as last resort
    if output.is_empty() {
        for file in &files {
            let content = match std::fs::read_to_string(&file.absolute_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Find lines containing the symbol name
            let mut match_lines: Vec<(usize, &str)> = Vec::new();
            for (i, line) in content.lines().enumerate() {
                if line.contains(name) {
                    match_lines.push((i, line));
                }
            }

            if match_lines.is_empty() {
                continue;
            }

            // Find the best match: prefer definition-like lines
            let def_line = match_lines.iter().find(|(_, line)| {
                let trimmed = line.trim();
                // Definition patterns: function/const/exports assignment (not imports)
                trimmed.contains(name) && !trimmed.starts_with("import ")
                    && !trimmed.starts_with("const {") // destructured import
                    && (trimmed.contains("function") || trimmed.contains("=>")
                        || trimmed.contains(&format!("exports.{}", name))
                        || trimmed.contains(&format!("{} =", name))
                        || trimmed.starts_with(&format!("const {}", name))
                        || trimmed.starts_with(&format!("let {}", name)))
            });

            if let Some(&(def_idx, _)) = def_line {
                let lines: Vec<&str> = content.lines().collect();
                // Find the end of this function by counting braces
                let mut brace_count = 0i32;
                let mut func_end = def_idx;
                let mut found_open = false;
                for j in def_idx..lines.len() {
                    for ch in lines[j].chars() {
                        if ch == '{' { brace_count += 1; found_open = true; }
                        if ch == '}' { brace_count -= 1; }
                    }
                    func_end = j;
                    if found_open && brace_count <= 0 {
                        break;
                    }
                    // Safety: don't go past 500 lines
                    if j - def_idx > 500 { break; }
                }

                writeln!(
                    &mut output,
                    "[match] {} in {} L{}-{}",
                    name, file.relative_path, def_idx + 1, func_end + 1
                )?;

                let total = func_end - def_idx + 1;
                if max_lines > 0 && total > max_lines {
                    for i in def_idx..(def_idx + max_lines).min(lines.len()) {
                        writeln!(&mut output, "{}", lines[i])?;
                    }
                    writeln!(&mut output, "// ... +{} more lines (pass max_lines=0 for full)", total - max_lines)?;
                } else {
                    for i in def_idx..=func_end.min(lines.len() - 1) {
                        writeln!(&mut output, "{}", lines[i])?;
                    }
                }
                writeln!(&mut output)?;
                break; // Found it
            }
        }
    }

    if output.is_empty() {
        writeln!(&mut output, "Symbol '{}' not found.", name)?;
    }

    Ok(output)
}
