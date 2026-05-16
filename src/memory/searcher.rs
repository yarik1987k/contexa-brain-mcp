use std::fmt::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;

use crate::db::schema;
use crate::indexer::embedding_client;

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Recall memories using semantic search + keyword search + recency.
/// Prefers TurboQuant compressed embeddings for faster similarity (falls back to raw f32).
///
/// Side effect: every memory that ends up in the returned set has its
/// `last_accessed_at` bumped to now. This drives the access-recency half of
/// the scoring formula on future recalls — memories actually used rank higher
/// than untouched peers with identical content.
pub fn recall(
    project_path: &Path,
    query: &str,
    category: Option<&str>,
    max_results: u32,
) -> Result<String> {
    let db_path = schema::db_path(project_path);
    if !db_path.exists() {
        return Ok("No memories stored yet. Use save_memory to store context.".to_string());
    }

    let conn = schema::open_db(project_path)?;
    let mut output = String::new();

    // Generate query embedding for semantic search
    let query_embedding = embedding_client::try_embed_text(query);

    // Prepare TurboQuant query vector once (reused for all compressed comparisons)
    let tq = embedding_client::get_turboquant();
    let query_prepared = query_embedding.as_ref().map(|qe| {
        let (rotated, norm) = tq.prepare_query(qe);
        (qe, rotated, norm)
    });

    // Load memories
    let limit = crate::context::scoring::MAX_RECALL_CANDIDATES;
    let memories = if let Some(cat) = category {
        let sql = format!(
            "SELECT id, content, category, tags, created_at, embedding, embedding_compressed, last_accessed_at
             FROM memories WHERE category = ?1 ORDER BY created_at DESC LIMIT {}",
            limit
        );
        let mut s = conn.prepare(&sql)?;
        collect_memories(&mut s, Some(cat))?
    } else {
        let sql = format!(
            "SELECT id, content, category, tags, created_at, embedding, embedding_compressed, last_accessed_at
             FROM memories ORDER BY created_at DESC LIMIT {}",
            limit
        );
        let mut s = conn.prepare(&sql)?;
        collect_memories(&mut s, None)?
    };

    if memories.is_empty() {
        return Ok("No memories stored yet.".to_string());
    }

    // Score each memory
    let total = memories.len() as f32;
    let now_ts = now_epoch_seconds();
    let mut scored: Vec<(f32, &MemoryRow)> = memories
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut score: f32 = 0.0;

            use crate::context::scoring;

            // Semantic similarity — prefer compressed, fall back to raw
            if let Some((qe, ref rotated, norm)) = query_prepared {
                if let Some(ref blob) = row.embedding_compressed {
                    if let Some(qv) = schema::blob_to_quantized(blob) {
                        let sim = tq.fast_cosine_similarity(rotated, norm, &qv);
                        score += sim * scoring::MEMORY_SEMANTIC_WEIGHT;
                    }
                } else if let Some(ref me) = row.embedding {
                    let sim = embedding_client::cosine_similarity(qe, me);
                    score += sim * scoring::MEMORY_SEMANTIC_WEIGHT;
                }
            }

            // Keyword match
            let query_lower = query.to_lowercase();
            let content_lower = row.content.to_lowercase();
            if content_lower.contains(&query_lower) {
                score += scoring::MEMORY_KEYWORD_WEIGHT;
            } else {
                let query_words: Vec<&str> = query_lower.split_whitespace().collect();
                let matches = query_words.iter().filter(|w| content_lower.contains(*w)).count();
                if !query_words.is_empty() {
                    score += scoring::MEMORY_KEYWORD_WEIGHT * (matches as f32 / query_words.len() as f32);
                }
            }

            // Recency: split the MEMORY_RECENCY_WEIGHT budget between two signals:
            //  - half on "how new is this memory" (existing behaviour)
            //  - half on "how recently was it actually recalled"
            // A memory that hasn't been touched in months drops naturally below
            // the inclusion threshold (no hard deletion required).
            let create_recency = 1.0 - i as f32 / total.max(1.0);
            let access_recency = match row.last_accessed_at {
                Some(ts) => {
                    let days = ((now_ts - ts).max(0) as f32) / 86_400.0;
                    1.0 / (1.0 + days / 30.0) // ~half-life ≈ 30 days
                }
                None => 0.0, // never recalled → no bonus, but no penalty beyond zero
            };
            score += scoring::MEMORY_RECENCY_WEIGHT * 0.5 * create_recency;
            score += scoring::MEMORY_RECENCY_WEIGHT * 0.5 * access_recency;

            (score, row)
        })
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| crate::context::relevance_scorer::cmp_score_desc(a.0, b.0));
    scored.truncate(max_results as usize);

    // Filter out very low scores
    let scored: Vec<_> = scored.into_iter().filter(|(s, _)| *s > crate::context::scoring::MEMORY_MIN_SCORE).collect();

    if scored.is_empty() {
        writeln!(&mut output, "No relevant memories found for '{}'.", query)?;
        return Ok(output);
    }

    writeln!(&mut output, "Memories:\n")?;
    for (score, row) in &scored {
        let date = if row.created_at.len() >= 10 { &row.created_at[..10] } else { &row.created_at };
        let display_content = if row.content.len() > 300 {
            let cut = row.content[..300].rfind(' ').unwrap_or(300);
            format!("{}...", &row.content[..cut])
        } else {
            row.content.clone()
        };
        writeln!(
            &mut output,
            "- [{:.0}%] [{}] [{}] {}",
            score * 100.0,
            date,
            row.category,
            display_content
        )?;
        if !row.tags.is_empty() {
            writeln!(&mut output, "  Tags: {}", row.tags)?;
        }
    }

    // Mark the surfaced memories as accessed. Best-effort: a failed bump shouldn't
    // fail the recall itself.
    let accessed_ids: Vec<i64> = scored.iter().map(|(_, m)| m.id).collect();
    if !accessed_ids.is_empty() {
        let placeholders: String = (0..accessed_ids.len()).map(|i| format!("?{}", i + 2)).collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE memories SET last_accessed_at = ?1 WHERE id IN ({})",
            placeholders
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(accessed_ids.len() + 1);
        params.push(Box::new(now_ts));
        for id in &accessed_ids {
            params.push(Box::new(*id));
        }
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        if let Err(e) = conn.execute(&sql, param_refs.as_slice()) {
            tracing::debug!("last_accessed_at bump failed (non-fatal): {}", e);
        }
    }

    Ok(output)
}

struct MemoryRow {
    id: i64,
    content: String,
    category: String,
    tags: String,
    created_at: String,
    embedding: Option<Vec<f32>>,
    embedding_compressed: Option<Vec<u8>>,
    last_accessed_at: Option<i64>,
}

fn map_memory_row(row: &rusqlite::Row) -> rusqlite::Result<MemoryRow> {
    let embedding_blob: Option<Vec<u8>> = row.get(5)?;
    let compressed_blob: Option<Vec<u8>> = row.get(6)?;
    Ok(MemoryRow {
        id: row.get(0)?,
        content: row.get(1)?,
        category: row.get(2)?,
        tags: row.get(3)?,
        created_at: row.get(4)?,
        embedding: embedding_blob.map(|b| schema::blob_to_embedding(&b)),
        embedding_compressed: compressed_blob,
        last_accessed_at: row.get(7)?,
    })
}

fn collect_memories(
    stmt: &mut rusqlite::Statement,
    category: Option<&str>,
) -> Result<Vec<MemoryRow>> {
    let rows = if let Some(cat) = category {
        stmt.query_map(rusqlite::params![cat], map_memory_row)?
    } else {
        stmt.query_map([], map_memory_row)?
    };
    Ok(rows.filter_map(|r| r.ok()).collect())
}
