mod types;
mod indexed;
mod live;

use std::path::Path;
use anyhow::Result;

use crate::db::schema;

/// Search the codebase using the persistent index (fast) with fallback to live scan.
pub fn search(project_path: &Path, query: &str, max_results: u32, token_budget: u32) -> Result<String> {
    let max_results = max_results.min(50);
    let token_budget = token_budget.min(100_000);

    let db_path = schema::db_path(project_path);
    if db_path.exists() {
        match indexed::search_indexed(project_path, query, max_results, token_budget) {
            Ok(result) if !result.contains("No results found") => return Ok(result),
            _ => {}
        }
    }

    live::search_live(project_path, query, max_results, token_budget)
}
