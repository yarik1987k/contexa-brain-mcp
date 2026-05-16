//! Local-only tool-call telemetry.
//!
//! Every tool invocation writes one row to the `tool_calls` table in the
//! per-project SQLite database. Raw queries are never stored — only a 16-char
//! FNV-1a hash (so equivalent queries group together but the original text
//! never lives in the DB). No network traffic, no aggregation, no external
//! reporting. The `stats` CLI reads from the same local table.
//!
//! Failures are silently swallowed: telemetry must never break a tool call.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::params;

use crate::db::schema;

/// Process-wide write lock. Serializes telemetry inserts so concurrent tool
/// calls don't race on SQLite WAL initialization. Reads (summarize) don't take
/// this lock — WAL handles concurrent readers fine.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Hash a query to a 16-char FNV-1a hex digest. Matches the algorithm used by
/// `pipeline::hash_content` so we stay stable across Rust versions and free of
/// extra crypto dependencies.
fn fnv1a_16(s: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001B3;
    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Record one tool call. Best-effort: any DB error is logged but never propagated.
///
/// Returns the background-thread `JoinHandle` so callers (notably tests) can
/// wait for the write to land before reading. Production callers simply drop
/// the handle — the thread runs to completion regardless.
pub fn record_tool_call(
    project_path: &Path,
    tool_name: &str,
    query: Option<&str>,
    result_count: Option<i64>,
    latency_ms: i64,
) -> std::thread::JoinHandle<()> {
    let project_path = project_path.to_path_buf();
    let tool_name = tool_name.to_string();
    let query_hash = query.map(fnv1a_16);
    let query_length = query.map(|q| q.chars().count() as i64);
    std::thread::spawn(move || {
        if let Err(e) = insert(&project_path, &tool_name, query_hash, query_length, result_count, latency_ms) {
            tracing::debug!("telemetry insert failed (non-fatal): {}", e);
        }
    })
}

fn insert(
    project_path: &Path,
    tool_name: &str,
    query_hash: Option<String>,
    query_length: Option<i64>,
    result_count: Option<i64>,
    latency_ms: i64,
) -> Result<()> {
    // Serialize writers — eliminates SQLite WAL-init races when many tool calls
    // fire in parallel (e.g. an LLM batching tool_use blocks).
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let conn = schema::open_db(project_path)?;
    conn.execute(
        "INSERT INTO tool_calls (tool_name, query_hash, query_length, result_count, latency_ms, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![tool_name, query_hash, query_length, result_count, latency_ms, now_epoch_seconds()],
    )?;
    Ok(())
}

// ── Aggregation for the `stats` CLI ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolStats {
    pub tool_name: String,
    pub call_count: i64,
    pub avg_latency_ms: f64,
    pub empty_result_count: i64,
}

#[derive(Debug, Clone)]
pub struct StatsSummary {
    pub project_path: PathBuf,
    pub total_calls: i64,
    pub by_tool: Vec<ToolStats>,
    pub top_empty_query_hashes: Vec<(String, i64)>, // (hash, count)
}

/// Summarize tool-call telemetry for the given project since `since_ts` (unix seconds).
/// Pass `since_ts = 0` to include everything.
pub fn summarize(project_path: &Path, since_ts: i64) -> Result<StatsSummary> {
    let conn = schema::open_db(project_path)?;

    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_calls WHERE ts >= ?1",
        params![since_ts],
        |row| row.get(0),
    ).unwrap_or(0);

    let mut by_tool_stmt = conn.prepare(
        "SELECT tool_name, COUNT(*), AVG(latency_ms),
                SUM(CASE WHEN result_count = 0 THEN 1 ELSE 0 END)
         FROM tool_calls
         WHERE ts >= ?1
         GROUP BY tool_name
         ORDER BY COUNT(*) DESC"
    )?;
    let by_tool: Vec<ToolStats> = by_tool_stmt
        .query_map(params![since_ts], |row| {
            Ok(ToolStats {
                tool_name: row.get(0)?,
                call_count: row.get(1)?,
                avg_latency_ms: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                empty_result_count: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut empty_stmt = conn.prepare(
        "SELECT query_hash, COUNT(*) AS c
         FROM tool_calls
         WHERE ts >= ?1 AND result_count = 0 AND query_hash IS NOT NULL
         GROUP BY query_hash
         ORDER BY c DESC
         LIMIT 5"
    )?;
    let top_empty_query_hashes: Vec<(String, i64)> = empty_stmt
        .query_map(params![since_ts], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(StatsSummary {
        project_path: project_path.to_path_buf(),
        total_calls: total,
        by_tool,
        top_empty_query_hashes,
    })
}
