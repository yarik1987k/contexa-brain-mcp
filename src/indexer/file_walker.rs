use std::path::{Path, PathBuf};
use anyhow::Result;
use ignore::WalkBuilder;

/// Information about a discovered file.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub absolute_path: PathBuf,
    pub relative_path: String,
    pub extension: String,
    pub size_bytes: u64,
}

/// Walk a project directory respecting .gitignore rules.
/// Returns only source code files (no binaries, lock files, etc.).
pub fn walk_project(project_path: &Path) -> Result<Vec<FileEntry>> {
    let mut files = Vec::new();

    let walker = WalkBuilder::new(project_path)
        .hidden(true)           // skip hidden files by default
        .git_ignore(true)       // respect .gitignore
        .git_global(true)       // respect global gitignore
        .git_exclude(true)      // respect .git/info/exclude
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !super::config::is_skip_dir(&name)
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Skip directories
        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(true) {
            continue;
        }

        let path = entry.path().to_path_buf();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        // Only include source code files
        if !super::config::is_source_file(&ext) {
            continue;
        }

        let relative = path
            .strip_prefix(project_path)
            .unwrap_or(&path)
            .display()
            .to_string();

        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

        files.push(FileEntry {
            absolute_path: path,
            relative_path: relative,
            extension: ext,
            size_bytes: size,
        });
    }

    // Sort by path for consistent output
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

