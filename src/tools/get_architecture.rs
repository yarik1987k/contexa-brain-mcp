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
                // Take first ~1000 chars as summary
                let summary: String = content.chars().take(1000).collect();
                writeln!(&mut output, "## From {}\n", doc_name)?;
                writeln!(&mut output, "{}", summary)?;
                if content.len() > 1000 {
                    writeln!(&mut output, "\n... [truncated, use get_file_context for full content]")?;
                }
                writeln!(&mut output)?;
                break;
            }
        }
    }

    // File statistics
    let stats = collect_file_stats(project_path)?;
    writeln!(&mut output, "## File Statistics\n")?;
    for (ext, count) in &stats {
        writeln!(&mut output, "- .{}: {} files", ext, count)?;
    }

    // Key directories
    writeln!(&mut output, "\n## Key Directories\n")?;
    let entries: Vec<_> = std::fs::read_dir(project_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            !crate::indexer::config::is_skip_dir(&name)
        })
        .collect();

    for entry in entries {
        writeln!(&mut output, "- {}/", entry.file_name().to_string_lossy())?;
    }

    // Detect tech stack from config files
    writeln!(&mut output, "\n## Detected Stack\n")?;
    let detections = detect_stack(project_path);
    for d in &detections {
        writeln!(&mut output, "- {}", d)?;
    }

    Ok(output)
}

fn collect_file_stats(dir: &Path) -> Result<Vec<(String, usize)>> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    count_files_recursive(dir, &mut counts)?;

    let mut stats: Vec<_> = counts.into_iter().collect();
    stats.sort_by(|a, b| b.1.cmp(&a.1));
    stats.truncate(15);
    Ok(stats)
}

fn count_files_recursive(dir: &Path, counts: &mut std::collections::HashMap<String, usize>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if crate::indexer::config::is_skip_dir(&name) {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            count_files_recursive(&path, counts)?;
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            *counts.entry(ext.to_string()).or_insert(0) += 1;
        }
    }
    Ok(())
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
