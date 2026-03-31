use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Extract raw import paths from source code based on language.
pub fn extract_imports(source: &str, extension: &str) -> Vec<String> {
    match extension {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => extract_js_imports(source),
        "py" | "pyi" => extract_python_imports(source),
        "rs" => extract_rust_imports(source),
        "go" => extract_go_imports(source),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => extract_c_imports(source),
        _ => Vec::new(),
    }
}

/// Build import counts: for each file, how many other files import it.
pub fn build_import_counts(
    files: &[(String, String, String)], // (relative_path, extension, content)
    known_paths: &HashSet<String>,
) -> HashMap<String, u32> {
    let mut counts: HashMap<String, u32> = HashMap::new();

    for (importing_path, extension, content) in files {
        let raw_imports = extract_imports(content, extension);
        for raw in raw_imports {
            if let Some(resolved) = resolve_import(importing_path, &raw, extension, known_paths) {
                *counts.entry(resolved).or_insert(0) += 1;
            }
        }
    }

    counts
}

/// Resolve a raw import string to a relative file path in the project.
fn resolve_import(
    importing_file: &str,
    raw_import: &str,
    extension: &str,
    known_paths: &HashSet<String>,
) -> Option<String> {
    match extension {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            resolve_js_import(importing_file, raw_import, known_paths)
        }
        "py" | "pyi" => resolve_python_import(raw_import, known_paths),
        "rs" => resolve_rust_import(importing_file, raw_import, known_paths),
        "go" => resolve_go_import(raw_import, known_paths),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => {
            resolve_c_import(importing_file, raw_import, known_paths)
        }
        _ => None,
    }
}

// ── JS/TS imports ────────────────────────────────────────────────────

fn extract_js_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        // import ... from 'path' or import ... from "path"
        if trimmed.starts_with("import ") {
            if let Some(path) = extract_quoted_after(trimmed, "from") {
                imports.push(path);
            }
        }
        // require('path') or require("path")
        if let Some(start) = trimmed.find("require(") {
            if let Some(after) = trimmed.get(start + 8..) {
                if let Some(path) = extract_first_quoted(after) {
                    imports.push(path);
                }
            }
        }
        // export ... from 'path'
        if trimmed.starts_with("export ") && trimmed.contains(" from ") {
            if let Some(path) = extract_quoted_after(trimmed, "from") {
                imports.push(path);
            }
        }
    }
    imports
}

fn resolve_js_import(importing_file: &str, raw: &str, known: &HashSet<String>) -> Option<String> {
    // Skip bare specifiers (npm packages)
    if !raw.starts_with('.') && !raw.starts_with('/') {
        return None;
    }

    let dir = Path::new(importing_file).parent().unwrap_or(Path::new(""));
    let resolved = dir.join(raw);
    let normalized = normalize_path(&resolved);

    // Try exact, then with extensions, then as directory index
    let candidates = vec![
        normalized.clone(),
        format!("{}.ts", normalized),
        format!("{}.tsx", normalized),
        format!("{}.js", normalized),
        format!("{}.jsx", normalized),
        format!("{}/index.ts", normalized),
        format!("{}/index.tsx", normalized),
        format!("{}/index.js", normalized),
        format!("{}/index.jsx", normalized),
    ];

    candidates.into_iter().find(|c| known.contains(c))
}

// ── Python imports ───────────────────────────────────────────────────

fn extract_python_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("from ") {
            // from foo.bar import baz
            if let Some(module) = trimmed.strip_prefix("from ").and_then(|s| s.split_whitespace().next()) {
                imports.push(module.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            // import foo.bar, baz.qux
            for part in rest.split(',') {
                let module = part.trim().split(" as ").next().unwrap_or("").trim();
                if !module.is_empty() {
                    imports.push(module.to_string());
                }
            }
        }
    }
    imports
}

fn resolve_python_import(raw: &str, known: &HashSet<String>) -> Option<String> {
    // Convert dots to path: foo.bar -> foo/bar
    let path = raw.replace('.', "/");

    let candidates = vec![
        format!("{}.py", path),
        format!("{}/__init__.py", path),
        format!("{}.pyi", path),
    ];

    candidates.into_iter().find(|c| known.contains(c))
}

// ── Rust imports ─────────────────────────────────────────────────────

fn extract_rust_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        // use crate::foo::bar
        if trimmed.starts_with("use crate::") {
            let path = trimmed.strip_prefix("use crate::").unwrap_or("");
            let module = path.split('{').next().unwrap_or(path)
                .split("::").next().unwrap_or("")
                .trim_end_matches(';');
            if !module.is_empty() {
                imports.push(module.to_string());
            }
        }
        // mod foo;
        if trimmed.starts_with("mod ") && trimmed.ends_with(';') {
            let module = trimmed.strip_prefix("mod ").unwrap_or("")
                .trim_end_matches(';').trim();
            if !module.is_empty() {
                imports.push(module.to_string());
            }
        }
    }
    imports
}

fn resolve_rust_import(importing_file: &str, raw: &str, known: &HashSet<String>) -> Option<String> {
    let dir = Path::new(importing_file).parent().unwrap_or(Path::new(""));

    let candidates = vec![
        dir.join(format!("{}.rs", raw)).display().to_string(),
        dir.join(raw).join("mod.rs").display().to_string(),
        format!("src/{}.rs", raw),
        format!("src/{}/mod.rs", raw),
    ];

    candidates.into_iter()
        .map(|c| normalize_path(Path::new(&c)))
        .find(|c| known.contains(c))
}

// ── Go imports ───────────────────────────────────────────────────────

fn extract_go_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut in_import_block = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed == "import (" {
            in_import_block = true;
            continue;
        }
        if in_import_block {
            if trimmed == ")" {
                in_import_block = false;
                continue;
            }
            if let Some(path) = extract_first_quoted(trimmed) {
                imports.push(path);
            }
        } else if trimmed.starts_with("import ") {
            if let Some(path) = extract_first_quoted(trimmed) {
                imports.push(path);
            }
        }
    }
    imports
}

/// Go imports are package paths. We try the last component as a directory
/// containing .go files. E.g. "myproject/pkg/auth" -> look for "pkg/auth/*.go".
fn resolve_go_import(raw: &str, known: &HashSet<String>) -> Option<String> {
    // Skip standard library and external packages (no dots in first component = local)
    // Go module imports typically start with a domain like "github.com/..."
    // Local relative imports are rare in Go but we try to match by suffix
    let parts: Vec<&str> = raw.split('/').collect();

    // Try matching known paths that end with this import path
    for known_path in known {
        if known_path.ends_with(".go") {
            // Check if the file is in a directory matching the import suffix
            let dir = Path::new(known_path).parent().map(|p| p.display().to_string()).unwrap_or_default();
            if dir.ends_with(raw) || (parts.len() == 1 && dir.ends_with(parts[0])) {
                return Some(known_path.clone());
            }
        }
    }
    None
}

// ── C/C++ imports ────────────────────────────────────────────────────

fn extract_c_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        // Only local includes: #include "header.h" (not <system.h>)
        if let Some(rest) = trimmed.strip_prefix("#include \"") {
            if let Some(end) = rest.find('"') {
                imports.push(rest[..end].to_string());
            }
        }
    }
    imports
}

fn resolve_c_import(importing_file: &str, raw: &str, known: &HashSet<String>) -> Option<String> {
    let dir = Path::new(importing_file).parent().unwrap_or(Path::new(""));
    let resolved = dir.join(raw);
    let normalized = normalize_path(&resolved);

    if known.contains(&normalized) {
        Some(normalized)
    } else if known.contains(raw) {
        Some(raw.to_string())
    } else {
        None
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn extract_quoted_after(line: &str, keyword: &str) -> Option<String> {
    let idx = line.find(keyword)?;
    let after = &line[idx + keyword.len()..];
    extract_first_quoted(after)
}

fn extract_first_quoted(s: &str) -> Option<String> {
    for quote in ['"', '\''] {
        if let Some(start) = s.find(quote) {
            if let Some(end) = s[start + 1..].find(quote) {
                return Some(s[start + 1..start + 1 + end].to_string());
            }
        }
    }
    None
}

/// Normalize a path: remove `.` and `..` components without filesystem access.
fn normalize_path(path: &Path) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(s) => {
                parts.push(s.to_str().unwrap_or(""));
            }
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir => {}
            _ => {}
        }
    }
    parts.join("/")
}
