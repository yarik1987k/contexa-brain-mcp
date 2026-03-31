use anyhow::Result;
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use std::sync::Mutex;

/// Global embedding model — initialized once, reused across calls.
static MODEL: std::sync::LazyLock<Result<Mutex<TextEmbedding>, String>> =
    std::sync::LazyLock::new(|| {
        eprintln!("[context-brain] Initializing embedding model (multilingual-e5-small, first run may download ~90MB)...");
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_show_download_progress(true),
        )
        .map_err(|e| format!("Failed to init embedding model: {}", e))?;
        eprintln!("[context-brain] Embedding model ready");
        Ok(Mutex::new(model))
    });

/// Get reference to the global model.
fn get_model() -> Result<&'static Mutex<TextEmbedding>> {
    MODEL
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Generate an embedding vector for a single text.
/// Recovers from mutex poison (prior panic) by clearing the poison.
pub fn embed_text(text: &str) -> Result<Vec<f32>> {
    // Cap input length to prevent OOM in the model
    let text = if text.len() > 8192 {
        &text[..8192]
    } else {
        text
    };

    let mutex = get_model()?;
    // Recover from poison: if a prior call panicked, the mutex is poisoned
    // but the inner data is still valid (TextEmbedding doesn't have invariants
    // that a panic could violate). Use into_inner() on the poison error.
    let model = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[context-brain] WARNING: Embedding mutex was poisoned, recovering...");
            poisoned.into_inner()
        }
    };
    let embeddings = model.embed(vec![text], None)?;
    embeddings
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
}

/// Generate embeddings for a batch of texts.
/// More efficient than calling embed_text in a loop.
pub fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    // Cap each input
    let capped: Vec<&str> = texts.iter().map(|t| {
        if t.len() > 8192 { &t[..8192] } else { t }
    }).collect();

    let mutex = get_model()?;
    let model = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let embeddings = model.embed(capped, None)?;
    Ok(embeddings)
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

/// Embedding dimension for the current model (MultilingualE5Small = 384).
pub const EMBEDDING_DIM: usize = 384;

// ── TurboQuant integration ──────────────────────────────────────────

use crate::turboquant::{TurboQuant, QuantMode, QuantizedVector};

/// Global TurboQuant engine — initialized once for the model dimension.
static TURBOQUANT: std::sync::LazyLock<TurboQuant> =
    std::sync::LazyLock::new(|| TurboQuant::new(EMBEDDING_DIM));

/// Get a reference to the global TurboQuant engine.
pub fn get_turboquant() -> &'static TurboQuant {
    &TURBOQUANT
}

/// Quantize an embedding using TurboQuant (2-bit, fast mode).
pub fn quantize_embedding(embedding: &[f32]) -> QuantizedVector {
    TURBOQUANT.quantize(embedding, 2, QuantMode::Fast)
}
