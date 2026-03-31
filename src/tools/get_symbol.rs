use std::fmt::Write;
use std::path::Path;
use anyhow::{Result, bail};

use crate::db::schema;

/// Get a specific symbol (function/class/struct) by name.
/// Returns the exact code block from the source file.
pub fn get_symbol(project_path: &Path, name: &str, file_hint: Option<&str>) -> Result<String> {
    let db_path = schema::db_path(project_path);

    // Try indexed database first
    if db_path.exists() {
        if let Ok(result) = get_from_index(project_path, name, file_hint) {
            if !result.is_empty() {
                return Ok(result);
            }
        }
    }

    // Fallback: grep-like search through files
    get_from_files(project_path, name, file_hint)
}

fn get_from_index(project_path: &Path, name: &str, file_hint: Option<&str>) -> Result<String> {
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
             LIMIT 10",
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

        writeln!(&mut output, "No exact match for '{}'. Similar symbols:\n", name)?;
        for (sname, kind, start, end, sig, path) in &fuzzy {
            writeln!(&mut output, "- [{}] {} in {} (L{}-L{}): {}", kind, sname, path, start, end, sig)?;
        }
        return Ok(output);
    }

    // For each match, read the actual code from the file
    for (sname, kind, start_line, end_line, _sig, rel_path) in &results {
        let file_path = project_path.join(rel_path);
        let resolved = file_path.canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot resolve path {}: {}", rel_path, e))?;
        let canonical_project = project_path.canonicalize().unwrap_or_else(|_| project_path.to_path_buf());
        if !resolved.starts_with(&canonical_project) {
            bail!("Path escapes project directory: {}", rel_path);
        }
        let file_path = resolved;
        writeln!(&mut output, "## [{}] {} in {} (L{}-L{})\n", kind, sname, rel_path, start_line, end_line)?;

        if let Ok(content) = std::fs::read_to_string(&file_path) {
            let lines: Vec<&str> = content.lines().collect();
            let start = ((*start_line).max(0) as usize).saturating_sub(1);
            let end = ((*end_line).max(0) as usize).min(lines.len());

            // Include 2 lines of context before
            let ctx_start = start.saturating_sub(2);
            for i in ctx_start..end {
                let prefix = if i < start { "  // " } else { "" };
                writeln!(&mut output, "{}L{}: {}", prefix, i + 1, lines.get(i).unwrap_or(&""))?;
            }
        }
        writeln!(&mut output)?;
    }

    Ok(output)
}

fn get_from_files(project_path: &Path, name: &str, file_hint: Option<&str>) -> Result<String> {
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
                        "## [{}] {} in {} (L{}-L{})\n",
                        sym.kind, sym.name, file.relative_path, sym.start_line, sym.end_line
                    )?;

                    let lines: Vec<&str> = content.lines().collect();
                    let start = sym.start_line.saturating_sub(1);
                    let end = sym.end_line.min(lines.len());
                    let ctx_start = start.saturating_sub(2);

                    for i in ctx_start..end {
                        let prefix = if i < start { "  // " } else { "" };
                        writeln!(&mut output, "{}L{}: {}", prefix, i + 1, lines.get(i).unwrap_or(&""))?;
                    }
                    writeln!(&mut output)?;
                }
            }
        }
    }

    if output.is_empty() {
        writeln!(&mut output, "Symbol '{}' not found.", name)?;
    }

    Ok(output)
}
