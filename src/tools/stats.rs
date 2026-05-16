//! `stats` subcommand: print local telemetry summary for a project.
//! Reads from the same SQLite DB the rest of context-brain uses.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::telemetry::{summarize, StatsSummary};

/// Render a human-readable stats block for a project, scoped to the last
/// `days` days (None = all-time).
pub fn render(project: &Path, days: Option<u32>) -> Result<String> {
    let since_ts = match days {
        Some(d) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|s| s.as_secs() as i64)
                .unwrap_or(0);
            now - (d as i64) * 86_400
        }
        None => 0,
    };

    let summary = summarize(project, since_ts)?;
    Ok(format_summary(&summary, days))
}

fn format_summary(s: &StatsSummary, days: Option<u32>) -> String {
    let mut out = String::new();
    let window = match days {
        Some(d) => format!("Last {} day(s)", d),
        None => "All time".to_string(),
    };
    let _ = writeln!(out, "{} — {}", window, s.project_path.display());
    let _ = writeln!(out, "─────────────────────────────────");
    let _ = writeln!(out, "Total tool calls: {}", s.total_calls);
    let _ = writeln!(out);

    if s.by_tool.is_empty() {
        let _ = writeln!(out, "No tool calls recorded yet. Start using context-brain via MCP, then re-run `stats`.");
        return out;
    }

    for t in &s.by_tool {
        let empty_pct = if t.call_count > 0 {
            (t.empty_result_count as f64 / t.call_count as f64) * 100.0
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "  {:<20} {:>5}  avg {:>5.0}ms   {:>3.0}% empty results",
            t.tool_name, t.call_count, t.avg_latency_ms, empty_pct
        );
    }

    if !s.top_empty_query_hashes.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Top empty-result query hashes (consider tuning):");
        for (hash, count) in &s.top_empty_query_hashes {
            let _ = writeln!(out, "  {}    {} calls", &hash[..hash.len().min(8)], count);
        }
    }

    out
}
