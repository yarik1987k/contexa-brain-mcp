use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

use crate::indexer::file_walker;

/// Build a directory tree string using the proper file walker (respects .gitignore).
pub fn build_file_tree(base: &Path, max_depth: u32) -> Result<String> {
    let mut output = String::new();
    let dir_name = base.file_name().unwrap_or_default().to_string_lossy();
    writeln!(&mut output, "{}/\n", dir_name)?;

    // Get all files via the gitignore-aware walker
    let files = file_walker::walk_project(base)?;

    // Group files by directory for tree display
    let mut current_dir = String::new();
    for file in &files {
        // Check depth
        let depth = file.relative_path.matches('/').count() as u32;
        if depth >= max_depth {
            continue;
        }

        // Print directory headers
        let dir = if let Some(pos) = file.relative_path.rfind('/') {
            &file.relative_path[..pos]
        } else {
            ""
        };

        if dir != current_dir {
            if !dir.is_empty() {
                writeln!(&mut output, "  {}/", dir)?;
            }
            current_dir = dir.to_string();
        }

        // Print file with size
        let name = if let Some(pos) = file.relative_path.rfind('/') {
            &file.relative_path[pos + 1..]
        } else {
            &file.relative_path
        };

        let size = format_size(file.size_bytes);
        let indent = if current_dir.is_empty() { "  " } else { "    " };
        writeln!(&mut output, "{}├── {} ({})", indent, name, size)?;
    }

    writeln!(&mut output, "\n{} files total", files.len())?;
    Ok(output)
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
