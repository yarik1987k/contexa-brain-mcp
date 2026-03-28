use std::path::Path;
use anyhow::Result;

use crate::db::schema;
use crate::indexer::embedding_client;

/// Save a memory entry with embedding to the persistent SQLite store.
pub fn save(project_path: &Path, content: &str, category: &str, tags: &str) -> Result<()> {
    let conn = schema::open_db(project_path)?;

    // Generate embedding for semantic recall
    let embedding = match embedding_client::embed_text(content) {
        Ok(e) => Some(e),
        Err(err) => {
            tracing::warn!("Failed to generate embedding for memory: {}", err);
            None
        }
    };

    schema::insert_memory(
        &conn,
        content,
        category,
        tags,
        embedding.as_deref(),
    )?;

    Ok(())
}
