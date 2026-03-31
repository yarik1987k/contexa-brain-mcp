use anyhow::Result;
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use std::sync::Mutex;

/// Trait for embedding operations. Enables mocking in tests.
pub trait EmbeddingProvider: Send + Sync {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>>;
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Production embedding provider using FastEmbed.
pub struct FastEmbedProvider;

/// Global embedding model — initialized once, reused across calls.
static MODEL: std::sync::LazyLock<Result<Mutex<TextEmbedding>, String>> =
    std::sync::LazyLock::new(|| {
        tracing::info!("Initializing embedding model (multilingual-e5-small, first run may download ~90MB)...");
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_show_download_progress(true),
        )
        .map_err(|e| format!("Failed to init embedding model: {}", e))?;
        tracing::info!("Embedding model ready");
        Ok(Mutex::new(model))
    });

/// Get reference to the global model.
fn get_model() -> Result<&'static Mutex<TextEmbedding>> {
    MODEL
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Acquire the model lock, recovering from mutex poison if necessary.
/// Safety: TextEmbedding is a stateless inference engine — a prior panic
/// during `embed()` cannot corrupt its internal state because the model
/// weights are read-only and the only mutable state (output buffers) is
/// allocated fresh each call.
fn acquire_model() -> Result<std::sync::MutexGuard<'static, TextEmbedding>> {
    let mutex = get_model()?;
    match mutex.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            tracing::warn!("Embedding mutex was poisoned, recovering (TextEmbedding is stateless)");
            Ok(poisoned.into_inner())
        }
    }
}

/// Truncate a string to at most `max_bytes`, respecting UTF-8 char boundaries.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last char boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

const MAX_EMBED_INPUT: usize = 8192;

/// Timeout for a single embedding call (covers model download, inference, hangs).
const EMBED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Run an embedding operation with a timeout. The closure runs in a spawned thread
/// that acquires the model lock itself — avoiding the MutexGuard-not-Send problem.
fn embed_with_timeout<F, T>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(EMBED_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(anyhow::anyhow!("Embedding timed out after {}s", EMBED_TIMEOUT.as_secs()))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow::anyhow!("Embedding thread panicked"))
        }
    }
}

impl EmbeddingProvider for FastEmbedProvider {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let text = truncate_utf8(text, MAX_EMBED_INPUT).to_string();
        embed_with_timeout(move || {
            let model = acquire_model()?;
            let embeddings = model.embed(vec![text.as_str()], None)?;
            embeddings
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
        })
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let capped: Vec<String> = texts.iter()
            .map(|t| truncate_utf8(t, MAX_EMBED_INPUT).to_string())
            .collect();
        embed_with_timeout(move || {
            let model = acquire_model()?;
            let refs: Vec<&str> = capped.iter().map(|s| s.as_str()).collect();
            Ok(model.embed(refs, None)?)
        })
    }
}

// ── Free functions delegating to global FastEmbedProvider ──────────────
// These preserve the existing API so callers don't need to change.

/// Generate an embedding vector for a single text.
pub fn embed_text(text: &str) -> Result<Vec<f32>> {
    FastEmbedProvider.embed_text(text)
}

/// Generate embeddings for a batch of texts.
pub fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    FastEmbedProvider.embed_batch(texts)
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

/// Try to generate an embedding, logging a warning on failure.
/// Returns None instead of propagating the error — use this when embeddings
/// are optional (search, scoring) vs. `embed_text` when they're required.
pub fn try_embed_text(text: &str) -> Option<Vec<f32>> {
    match embed_text(text) {
        Ok(e) => Some(e),
        Err(err) => {
            tracing::warn!("Embedding generation failed (non-fatal): {}", err);
            None
        }
    }
}

/// Try to generate batch embeddings, logging a warning on failure.
pub fn try_embed_batch(texts: &[&str]) -> Vec<Vec<f32>> {
    match embed_batch(texts) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("Batch embedding generation failed (non-fatal): {}", err);
            Vec::new()
        }
    }
}

/// Returns true if the embedding model loaded successfully.
/// Call this to check whether semantic search is available.
pub fn is_model_available() -> bool {
    MODEL.as_ref().is_ok()
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
    // bits=2 is always valid (1-4 range), so unwrap is safe here
    TURBOQUANT.quantize(embedding, 2, QuantMode::Fast)
        .expect("bits=2 is always valid")
}
