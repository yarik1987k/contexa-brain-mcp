use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

/// Build a condensed project architecture overview.
pub fn build_overview(project_path: &Path) -> Result<String> {
    let mut output = String::new();
    let project_name = project_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    writeln!(&mut output, "# Project: {}\n", project_name)?;

    // Try to read ARCHITECTURE.md or README.md
    for doc_name in &["ARCHITECTURE.md", "README.md", "readme.md"] {
        let doc_path = project_path.join(doc_name);
        if doc_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&doc_path) {
                writeln!(&mut output, "## From {}\n", doc_name)?;
                let summary = extract_doc_skeleton(&content, 2000);
                writeln!(&mut output, "{}", summary)?;
                writeln!(&mut output)?;
                break;
            }
        }
    }

    // File statistics (compact single line)
    let stats = collect_file_stats(project_path)?;
    let files_str: Vec<String> = stats.iter().map(|(ext, count)| format!(".{}({})", ext, count)).collect();
    writeln!(&mut output, "Files: {}", files_str.join(" "))?;

    // Key directories (compact)
    let entries: Vec<_> = std::fs::read_dir(project_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            !crate::indexer::config::is_skip_dir(&name)
        })
        .collect();

    let dirs_str: Vec<String> = entries.iter().map(|e| format!("{}/", e.file_name().to_string_lossy())).collect();
    writeln!(&mut output, "Dirs: {}", dirs_str.join(", "))?;

    // Detect tech stack (compact)
    let detections = detect_stack(project_path);
    if !detections.is_empty() {
        writeln!(&mut output, "Stack: {}", detections.join(", "))?;
    }

    Ok(output)
}

/// Extract a skeleton from a markdown document: all headers + content under each section.
/// Skips code blocks but preserves prose, lists, and tables. Captures the full document
/// structure within a character budget instead of blindly truncating.
fn extract_doc_skeleton(content: &str, char_budget: usize) -> String {
    let mut result = String::new();
    let mut in_code_block = false;
    let mut chars_used: usize = 0;
    let mut lines_since_header: usize = 0;

    for line in content.lines() {
        // Track code blocks — skip their content entirely
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        let is_header = line.starts_with('#');
        let is_blank = line.trim().is_empty();

        if is_header {
            // Always include headers — they're the skeleton
            if chars_used + line.len() + 2 > char_budget {
                result.push_str("\n... [remaining sections omitted — use get_file_context for full content]\n");
                break;
            }
            if chars_used > 0 { result.push('\n'); chars_used += 1; }
            result.push_str(line);
            result.push('\n');
            chars_used += line.len() + 1;
            lines_since_header = 0;
        } else if is_blank {
            if lines_since_header > 0 && lines_since_header < 6 {
                result.push('\n');
                chars_used += 1;
                lines_since_header += 1;
            }
        } else if lines_since_header < 6 {
            // Include up to 6 content lines per section
            if chars_used + line.len() + 2 > char_budget {
                result.push_str("\n... [truncated — use get_file_context for full content]\n");
                break;
            }
            result.push_str(line);
            result.push('\n');
            chars_used += line.len() + 1;
            lines_since_header += 1;
        }
    }

    result
}

fn collect_file_stats(dir: &Path) -> Result<Vec<(String, usize)>> {
    use crate::indexer::file_walker;

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Use the gitignore-aware file walker to avoid counting node_modules, venv, etc.
    if let Ok(files) = file_walker::walk_project(dir) {
        for file in &files {
            *counts.entry(file.extension.clone()).or_insert(0) += 1;
        }
    }

    let mut stats: Vec<_> = counts.into_iter().collect();
    stats.sort_by(|a, b| b.1.cmp(&a.1));
    stats.truncate(8);
    Ok(stats)
}

fn detect_stack(project_path: &Path) -> Vec<String> {
    let mut stack = Vec::new();

    let checks = [
        ("package.json", "Node.js"),
        ("Cargo.toml", "Rust"),
        ("go.mod", "Go"),
        ("requirements.txt", "Python"),
        ("pyproject.toml", "Python"),
        ("Gemfile", "Ruby"),
        ("pom.xml", "Java (Maven)"),
        ("build.gradle", "Java (Gradle)"),
        ("tsconfig.json", "TypeScript"),
        ("next.config.js", "Next.js"),
        ("next.config.mjs", "Next.js"),
        ("vite.config.ts", "Vite"),
        ("vite.config.js", "Vite"),
        ("tailwind.config.ts", "Tailwind CSS"),
        ("tailwind.config.js", "Tailwind CSS"),
        ("docker-compose.yml", "Docker"),
        ("Dockerfile", "Docker"),
        (".env", "Environment variables"),
        ("ecosystem.config.js", "PM2"),
    ];

    for (file, tech) in &checks {
        if project_path.join(file).exists() {
            stack.push(tech.to_string());
        }
    }

    stack
}
