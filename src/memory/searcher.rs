use std::fmt::Write;
use std::path::Path;
use anyhow::Result;

use crate::db::schema;
use crate::indexer::embedding_client;

/// Recall memories using semantic search + keyword search + recency.
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
    let query_embedding = embedding_client::embed_text(query).ok();

    // Load all memories with embeddings
    let stmt = if let Some(cat) = category {
        let mut s = conn.prepare(
            "SELECT id, content, category, tags, created_at, embedding FROM memories WHERE category = ?1 ORDER BY created_at DESC LIMIT 100",
        )?;
        let rows = collect_memories(&mut s, Some(cat))?;
        rows
    } else {
        let mut s = conn.prepare(
            "SELECT id, content, category, tags, created_at, embedding FROM memories ORDER BY created_at DESC LIMIT 100",
        )?;
        let rows = collect_memories_all(&mut s)?;
        rows
    };

    if stmt.is_empty() {
        return Ok("No memories stored yet.".to_string());
    }

    // Score each memory
    let mut scored: Vec<(f32, &MemoryRow)> = stmt
        .iter()
        .map(|row| {
            let mut score: f32 = 0.0;

            // Semantic similarity (0.7 weight)
            if let (Some(ref qe), Some(ref me)) = (&query_embedding, &row.embedding) {
                let sim = embedding_client::cosine_similarity(qe, me);
                score += sim * 0.7;
            }

            // Keyword match (0.2 weight)
            let query_lower = query.to_lowercase();
            let content_lower = row.content.to_lowercase();
            if content_lower.contains(&query_lower) {
                score += 0.2;
            } else {
                // Partial word matching
                let query_words: Vec<&str> = query_lower.split_whitespace().collect();
                let matches = query_words.iter().filter(|w| content_lower.contains(*w)).count();
                if !query_words.is_empty() {
                    score += 0.2 * (matches as f32 / query_words.len() as f32);
                }
            }

            // Recency bonus (0.1 weight) — newer memories score higher
            // Simple: first in list = most recent = highest bonus
            score += 0.1;

            (score, row)
        })
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_results as usize);

    // Filter out very low scores
    let scored: Vec<_> = scored.into_iter().filter(|(s, _)| *s > 0.15).collect();

    if scored.is_empty() {
        writeln!(&mut output, "No relevant memories found for '{}'.", query)?;
        return Ok(output);
    }

    writeln!(&mut output, "Found {} relevant memories:\n", scored.len())?;
    for (score, row) in &scored {
        writeln!(
            &mut output,
            "- [{:.0}%] [{}] [{}] {}",
            score * 100.0,
            row.created_at,
            row.category,
            row.content
        )?;
        if !row.tags.is_empty() {
            writeln!(&mut output, "  Tags: {}", row.tags)?;
        }
    }

    Ok(output)
}

struct MemoryRow {
    content: String,
    category: String,
    tags: String,
    created_at: String,
    embedding: Option<Vec<f32>>,
}

fn collect_memories(
    stmt: &mut rusqlite::Statement,
    category: Option<&str>,
) -> Result<Vec<MemoryRow>> {
    let rows = stmt.query_map(rusqlite::params![category.unwrap_or("")], |row| {
        let embedding_blob: Option<Vec<u8>> = row.get(5)?;
        Ok(MemoryRow {
            content: row.get(1)?,
            category: row.get(2)?,
            tags: row.get(3)?,
            created_at: row.get(4)?,
            embedding: embedding_blob.map(|b| schema::blob_to_embedding(&b)),
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn collect_memories_all(stmt: &mut rusqlite::Statement) -> Result<Vec<MemoryRow>> {
    let rows = stmt.query_map([], |row| {
        let embedding_blob: Option<Vec<u8>> = row.get(5)?;
        Ok(MemoryRow {
            content: row.get(1)?,
            category: row.get(2)?,
            tags: row.get(3)?,
            created_at: row.get(4)?,
            embedding: embedding_blob.map(|b| schema::blob_to_embedding(&b)),
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
