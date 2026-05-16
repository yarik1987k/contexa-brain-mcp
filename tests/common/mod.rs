//! Shared test helpers for integration tests.
//!
//! Each test creates an isolated project in a tempdir, populates it with
//! a known file tree, then runs the real indexer + search pipeline against it.
//! No mocks, no fixtures-on-disk — everything is constructed in code so tests
//! are self-contained and reproducible.

#![allow(dead_code)] // helpers are shared across multiple test files

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Build a project at the given temp dir from a list of (relative_path, content) pairs.
/// Creates parent directories as needed.
pub fn write_project(root: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, content).expect("write file");
    }
}

/// Create a tempdir, write `files` into it, and run the full indexer.
/// Returns the tempdir guard (drop it to clean up) and the canonical project path.
pub fn setup_indexed_project(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create tempdir");
    let root = tmp.path().to_path_buf();
    write_project(&root, files);
    context_brain::indexer::pipeline::index_project(&root).expect("index project");
    (tmp, root)
}

/// Run the indexed search and return (raw_output, ranked_paths) where
/// `ranked_paths` is the paths in the order they appear in the output.
pub fn search(project: &Path, query: &str) -> (String, Vec<String>) {
    let out = context_brain::tools::search_codebase::search(project, query, 50, 100_000, "pointers")
        .expect("search");
    let paths = parse_ranked_paths(&out);
    (out, paths)
}

/// Extract paths from the formatted search output, preserving rank order.
/// The output format puts the path at the start of each result line (sometimes
/// indented by two spaces for the directory-grouping shortcut — we re-expand it).
pub fn parse_ranked_paths(output: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut last_dir = String::new();

    for line in output.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('(') || trimmed.starts_with("No ") || trimmed.starts_with("...") {
            continue;
        }
        // Two-space indent = same-dir shortcut: re-prepend the last directory.
        let path_part = if let Some(stripped) = trimmed.strip_prefix("  ") {
            format!("{}{}", last_dir, stripped.split_whitespace().next().unwrap_or(""))
        } else {
            trimmed.split_whitespace().next().unwrap_or("").to_string()
        };
        // Strip trailing ":" (signature format) and only keep entries that look like paths.
        let cleaned = path_part.trim_end_matches(':').to_string();
        if !cleaned.contains('/') && !cleaned.ends_with(".rs") && !cleaned.ends_with(".js")
            && !cleaned.ends_with(".ts") && !cleaned.ends_with(".py")
        {
            continue;
        }
        // Track last directory for the indent-shortcut path reconstruction.
        if let Some(slash) = cleaned.rfind('/') {
            last_dir = cleaned[..=slash].to_string();
        }
        // Deduplicate consecutive entries for the same path (a single hit can
        // emit multiple lines — one per symbol — but rank position is what we care about).
        if paths.last().map(|p| p.as_str()) != Some(cleaned.as_str()) {
            paths.push(cleaned);
        }
    }
    paths
}

/// Position of the first occurrence of `needle` in the ranked list, or None.
pub fn rank_of(paths: &[String], needle: &str) -> Option<usize> {
    paths.iter().position(|p| p == needle || p.ends_with(needle))
}
