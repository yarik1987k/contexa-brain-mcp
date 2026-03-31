use std::path::Path;
use anyhow::{Result, bail};

use crate::db::schema;
use crate::indexer::embedding_client;

use crate::context::scoring;

/// Save a memory entry with embedding to the persistent SQLite store.
pub fn save(project_path: &Path, content: &str, category: &str, tags: &str) -> Result<()> {
    // Validate input sizes
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

    // Check memory count limit
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
    if count >= scoring::MAX_MEMORIES {
        bail!(
            "Memory limit reached ({}/{}). Delete old memories before adding new ones.",
            count, scoring::MAX_MEMORIES
        );
    }

    // Generate embedding for semantic recall
    let embedding = embedding_client::try_embed_text(content);

    // Compress embedding with TurboQuant for faster recall
    let embedding_compressed = embedding.as_ref().map(|e| {
        let qv = embedding_client::quantize_embedding(e);
        schema::quantized_to_blob(&qv)
    });

    schema::insert_memory(
        &conn,
        content,
        category,
        tags,
        embedding.as_deref(),
        embedding_compressed.as_deref(),
    )?;

    Ok(())
}
