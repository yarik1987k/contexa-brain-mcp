use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};

use crate::context::scoring;
use crate::db::schema;
use crate::indexer::embedding_client;

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Result of saving a memory — useful for tests + a future `memory stats` command.
#[derive(Debug)]
pub enum SaveOutcome {
    /// New memory inserted (no near-duplicate found). Returns the row id.
    Inserted(i64),
    /// Found a near-duplicate in the same category; merged into the existing
    /// row (appended content if new, bumped `updated_at`). Returns the row id
    /// of the absorbed-into entry.
    Merged(i64),
    /// Found a near-duplicate in a different category; inserted a new row and
    /// linked it via `linked_id` to the existing peer. Returns the new row id.
    Linked { new_id: i64, peer_id: i64 },
}

/// Save a memory entry with embedding, applying hygiene rules from scoring.rs:
/// - Reject when over `MAX_MEMORY_SIZE` or `MAX_MEMORIES`.
/// - If a near-duplicate exists (cosine ≥ `DEDUPE_THRESHOLD`), merge into it
///   (same category) or link to it (different category) instead of creating a
///   pure duplicate row.
pub fn save(project_path: &Path, content: &str, category: &str, tags: &str) -> Result<SaveOutcome> {
    if content.is_empty() {
        bail!("Memory content cannot be empty");
    }
    if content.len() > scoring::MAX_MEMORY_SIZE {
        bail!(
            "Memory content too large ({:.1}KB). Max is {}KB.",
            content.len() as f64 / 1024.0,
            scoring::MAX_MEMORY_SIZE / 1024
        );
    }
    if category.len() > 100 {
        bail!("Category too long (max 100 chars)");
    }
    if tags.len() > 500 {
        bail!("Tags too long (max 500 chars)");
    }

    let conn = schema::open_db(project_path)?;

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
    if count >= scoring::MAX_MEMORIES {
        bail!(
            "Memory limit reached ({}/{}). Delete old memories before adding new ones.",
            count,
            scoring::MAX_MEMORIES
        );
    }

    let embedding = embedding_client::try_embed_text(content);
    let embedding_compressed = embedding.as_ref().map(|e| {
        let qv = embedding_client::quantize_embedding(e);
        schema::quantized_to_blob(&qv)
    });

    // Dedupe: only meaningful when we have an embedding to compare against.
    if let Some(ref new_embedding) = embedding {
        if let Some(dup) = find_duplicate(&conn, new_embedding, content)? {
            if dup.category == category {
                // Same-category dup: merge content + bump updated_at.
                merge_into(&conn, dup.id, &dup.content, content)?;
                return Ok(SaveOutcome::Merged(dup.id));
            } else {
                // Cross-category dup: keep both, but link so review can spot
                // related decisions filed differently.
                let new_id = insert_with_link(
                    &conn,
                    content,
                    category,
                    tags,
                    embedding.as_deref(),
                    embedding_compressed.as_deref(),
                    Some(dup.id),
                )?;
                return Ok(SaveOutcome::Linked { new_id, peer_id: dup.id });
            }
        }
    }

    // Fresh insert (no near-duplicate, or no embedding to compare with).
    let new_id = insert_with_link(
        &conn,
        content,
        category,
        tags,
        embedding.as_deref(),
        embedding_compressed.as_deref(),
        None,
    )?;
    Ok(SaveOutcome::Inserted(new_id))
}

// ── Helpers ─────────────────────────────────────────────────────────────

struct DuplicateMemory {
    id: i64,
    category: String,
    content: String,
}

fn find_duplicate(
    conn: &rusqlite::Connection,
    new_embedding: &[f32],
    new_content: &str,
) -> Result<Option<DuplicateMemory>> {
    // Dedupe runs once per save — accuracy matters more than speed. Use the
    // raw f32 embedding for true cosine; 2-bit TurboQuant similarity is too
    // lossy to clear the 0.92 threshold reliably, even for near-identical text.
    //
    // Cosine alone over-merges templated content (e.g. "Decision 1: pick option 1"
    // vs "Decision 2: pick option 2" — same shape, different facts). We require
    // BOTH a high cosine AND a high lexical Jaccard overlap. The token check is
    // cheap (lowercase + split + set intersect) and rules out structurally-similar
    // but factually-distinct memories.
    let new_tokens = tokenize(new_content);

    let mut stmt = conn.prepare(
        "SELECT id, category, content, embedding
         FROM memories
         WHERE embedding IS NOT NULL",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<Vec<u8>>>(3)?,
        ))
    })?;

    let mut best: Option<(f32, DuplicateMemory)> = None;
    for row in rows.filter_map(|r| r.ok()) {
        let (id, cat, content, raw) = row;
        let Some(blob) = raw else { continue };
        let other = schema::blob_to_embedding(&blob);
        if other.is_empty() {
            continue;
        }
        let sim = embedding_client::cosine_similarity(new_embedding, &other);
        if sim < scoring::DEDUPE_THRESHOLD {
            continue;
        }
        // Second gate: lexical Jaccard. Templated-distinct memories fail here
        // even when their embeddings are very close.
        let other_tokens = tokenize(&content);
        if jaccard(&new_tokens, &other_tokens) < scoring::DEDUPE_TOKEN_OVERLAP_MIN {
            continue;
        }
        let dup = DuplicateMemory { id, category: cat, content };
        match &best {
            Some((prev, _)) if *prev >= sim => {}
            _ => best = Some((sim, dup)),
        }
    }
    Ok(best.map(|(_, d)| d))
}

/// Lowercase + split on whitespace and punctuation. We KEEP single-char
/// tokens — they're often the meaningful difference (numbers, version IDs,
/// option labels) — and instead drop a small set of stop-words so they don't
/// dominate the Jaccard for short texts.
fn tokenize(s: &str) -> std::collections::BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "and", "or", "of", "to", "in", "on", "at", "is",
        "it", "we", "i", "for", "as", "by", "be", "are", "was", "with",
    ];
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !STOP_WORDS.contains(w))
        .map(|w| w.to_string())
        .collect()
}

fn jaccard(a: &std::collections::BTreeSet<String>, b: &std::collections::BTreeSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 { 0.0 } else { inter / union }
}

fn merge_into(
    conn: &rusqlite::Connection,
    existing_id: i64,
    existing_content: &str,
    new_content: &str,
) -> Result<()> {
    // Only append if the new content adds information (not a substring of the old).
    let appended = if existing_content.contains(new_content) {
        existing_content.to_string()
    } else {
        format!("{}\n— {}", existing_content, new_content)
    };
    conn.execute(
        "UPDATE memories SET content = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![appended, now_epoch_seconds(), existing_id],
    )?;
    Ok(())
}

fn insert_with_link(
    conn: &rusqlite::Connection,
    content: &str,
    category: &str,
    tags: &str,
    embedding: Option<&[f32]>,
    embedding_compressed: Option<&[u8]>,
    linked_id: Option<i64>,
) -> Result<i64> {
    let embedding_blob = schema::embedding_to_blob(embedding);
    conn.execute(
        "INSERT INTO memories (content, category, tags, embedding, embedding_compressed, linked_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![content, category, tags, embedding_blob, embedding_compressed, linked_id],
    )?;
    Ok(conn.last_insert_rowid())
}
