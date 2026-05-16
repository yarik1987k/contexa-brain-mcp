//! Passive hygiene: surface candidate-contradiction pairs for human review.
//!
//! No automatic deletion — this is intentionally a *review* tool. The user
//! looks at the flagged pair and decides whether one supersedes the other.
//!
//! Heuristic for flagging a pair:
//!   1. Cosine similarity ≥ `CONTRADICTION_SIM_THRESHOLD` (very similar topic).
//!   2. created_at delta > 7 days (likely different occasions, not a typo fix).
//!   3. Content differs by ≥ 30% Levenshtein ratio (actual divergence, not paraphrase).

use std::fmt::Write;
use std::path::Path;

use anyhow::Result;

use crate::db::schema;
use crate::indexer::embedding_client;

const CONTRADICTION_SIM_THRESHOLD: f32 = 0.85;
const MIN_AGE_DELTA_DAYS: i64 = 7;
const MIN_DIVERGENCE_RATIO: f32 = 0.30;

#[derive(Debug, Clone)]
pub struct ContradictionPair {
    pub older_id: i64,
    pub older_created: String,
    pub older_content: String,
    pub newer_id: i64,
    pub newer_created: String,
    pub newer_content: String,
    pub similarity: f32,
    pub divergence: f32,
}

/// Find candidate contradiction pairs in the project's memory store.
/// Returns at most `limit` pairs, sorted by similarity descending.
pub fn find_contradictions(project_path: &Path, limit: usize) -> Result<Vec<ContradictionPair>> {
    let conn = schema::open_db(project_path)?;

    // Pull memories with embeddings. We only consider embedded memories — without
    // embeddings, similarity is meaningless.
    let mut stmt = conn.prepare(
        "SELECT id, content, created_at, embedding, embedding_compressed
         FROM memories
         WHERE embedding_compressed IS NOT NULL OR embedding IS NOT NULL
         ORDER BY created_at",
    )?;
    let rows: Vec<(i64, String, String, Option<Vec<u8>>, Option<Vec<u8>>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.len() < 2 {
        return Ok(Vec::new());
    }

    // Decompress embeddings once.
    let tq = embedding_client::get_turboquant();
    let memories: Vec<MemoryView> = rows
        .into_iter()
        .filter_map(|(id, content, created_at, raw, compressed)| {
            let embedding = if let Some(blob) = compressed {
                let qv = schema::blob_to_quantized(&blob)?;
                // Dequantize to f32 for cosine similarity in O(n^2) — keep it
                // simple. Could swap for fast_cosine_similarity later if perf
                // matters; n is bounded by MAX_MEMORIES = 1000.
                Some(tq.dequantize(&qv))
            } else if let Some(blob) = raw {
                let v = schema::blob_to_embedding(&blob);
                if v.is_empty() { None } else { Some(v) }
            } else {
                None
            }?;
            Some(MemoryView { id, content, created_at, embedding })
        })
        .collect();

    let mut pairs: Vec<ContradictionPair> = Vec::new();
    for i in 0..memories.len() {
        for j in (i + 1)..memories.len() {
            let a = &memories[i];
            let b = &memories[j];
            let sim = embedding_client::cosine_similarity(&a.embedding, &b.embedding);
            if sim < CONTRADICTION_SIM_THRESHOLD {
                continue;
            }
            if !age_delta_meets(&a.created_at, &b.created_at, MIN_AGE_DELTA_DAYS) {
                continue;
            }
            let div = divergence_ratio(&a.content, &b.content);
            if div < MIN_DIVERGENCE_RATIO {
                continue;
            }
            // Older first by created_at; SQL ORDER BY created_at means a is older.
            pairs.push(ContradictionPair {
                older_id: a.id,
                older_created: a.created_at.clone(),
                older_content: a.content.clone(),
                newer_id: b.id,
                newer_created: b.created_at.clone(),
                newer_content: b.content.clone(),
                similarity: sim,
                divergence: div,
            });
        }
    }

    pairs.sort_by(|x, y| y.similarity.partial_cmp(&x.similarity).unwrap_or(std::cmp::Ordering::Equal));
    pairs.truncate(limit);
    Ok(pairs)
}

/// Render a human-readable review report.
pub fn render_review(pairs: &[ContradictionPair]) -> String {
    if pairs.is_empty() {
        return "No candidate contradictions found.\n\nA pair is flagged when memories share an embedding similarity ≥ 0.85 but were saved more than 7 days apart and differ in actual content. Save more memories or check back later.\n".to_string();
    }

    let mut out = String::new();
    let _ = writeln!(out, "Candidate contradictions ({}):\n", pairs.len());
    for (i, p) in pairs.iter().enumerate() {
        let _ = writeln!(
            out,
            "[{}] sim {:.2}  divergence {:.0}%",
            i + 1,
            p.similarity,
            p.divergence * 100.0
        );
        let _ = writeln!(out, "  [{}] id {} — {}", trim_date(&p.older_created), p.older_id, oneline(&p.older_content));
        let _ = writeln!(out, "  [{}] id {} — {}", trim_date(&p.newer_created), p.newer_id, oneline(&p.newer_content));
        let _ = writeln!(out, "    → review these manually; this tool never deletes anything.");
        let _ = writeln!(out);
    }
    out
}

// ── Internals ───────────────────────────────────────────────────────────

struct MemoryView {
    id: i64,
    content: String,
    created_at: String,
    embedding: Vec<f32>,
}

fn oneline(s: &str) -> String {
    let cleaned: String = s.replace('\n', " ");
    if cleaned.chars().count() > 200 {
        let mut out: String = cleaned.chars().take(200).collect();
        out.push_str("...");
        out
    } else {
        cleaned
    }
}

fn trim_date(ts: &str) -> &str {
    if ts.len() >= 10 { &ts[..10] } else { ts }
}

/// Levenshtein-style divergence as fraction of the longer string.
/// Cheap implementation: count chars that don't appear in the same position.
/// Not a proper edit distance — that would be O(n*m) — but a good-enough
/// "are these obviously different?" filter for the contradiction screen.
fn divergence_ratio(a: &str, b: &str) -> f32 {
    let aa: Vec<char> = a.chars().collect();
    let bb: Vec<char> = b.chars().collect();
    let len = aa.len().max(bb.len());
    if len == 0 {
        return 0.0;
    }
    let mismatched = aa
        .iter()
        .zip(bb.iter())
        .filter(|(x, y)| x.to_ascii_lowercase() != y.to_ascii_lowercase())
        .count()
        + aa.len().abs_diff(bb.len());
    mismatched as f32 / len as f32
}

/// Crude date-string comparison: parse the first 10 chars as YYYY-MM-DD if possible.
/// Returns true when the gap is at least `min_days`.
fn age_delta_meets(a: &str, b: &str, min_days: i64) -> bool {
    let da = parse_ymd_days(a);
    let db = parse_ymd_days(b);
    match (da, db) {
        (Some(x), Some(y)) => (x - y).abs() >= min_days,
        _ => true, // If we can't parse, don't suppress — let the similarity + divergence filters decide.
    }
}

/// Parse a "YYYY-MM-DD..." string to a day-count since a fixed epoch.
/// Approximate but monotonic, good enough for delta comparisons.
fn parse_ymd_days(s: &str) -> Option<i64> {
    if s.len() < 10 {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: i64 = s[5..7].parse().ok()?;
    let d: i64 = s[8..10].parse().ok()?;
    Some(y * 365 + m * 31 + d)
}
