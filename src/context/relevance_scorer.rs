use crate::indexer::embedding_client;
use crate::indexer::symbol_extractor::Symbol;

/// Compare two f32 scores for descending sort (highest first).
/// Handles NaN by treating it as equal.
pub fn cmp_score_desc(a: f32, b: f32) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

/// Score multiple symbols' relevance to a query in one batch.
/// Much faster than scoring individually — generates all embeddings in one model call.
pub fn score_symbols_batch(
    symbols: &[Symbol],
    query: &str,
    query_embedding: Option<&[f32]>,
) -> Vec<f32> {
    let query_lower = query.to_lowercase();

    // Batch-generate symbol embeddings if we have a query embedding
    let sym_embeddings: Option<Vec<Vec<f32>>> = query_embedding.and_then(|_| {
        let texts: Vec<String> = symbols
            .iter()
            .map(|sym| format!("{} {}", sym.name, sym.signature))
            .collect();
        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        embedding_client::embed_batch(&refs).ok()
    });

    symbols
        .iter()
        .enumerate()
        .map(|(i, sym)| {
            let mut score: f32 = 0.0;
            let name_lower = sym.name.to_lowercase();
            let sig_lower = sym.signature.to_lowercase();

            // Exact name match — highest signal
            if name_lower == query_lower {
                score += 0.5;
            } else if name_lower.contains(&query_lower) || query_lower.contains(&name_lower) {
                score += 0.3;
            }

            // Keyword match in signature (word-boundary aware)
            for word in query_lower.split_whitespace() {
                if word.len() < 3 {
                    continue;
                }
                if has_word_match(&sig_lower, word) {
                    score += 0.1;
                }
            }

            // Semantic similarity via pre-computed batch embeddings
            if let (Some(qe), Some(ref embeds)) = (query_embedding, &sym_embeddings) {
                if let Some(se) = embeds.get(i) {
                    let sim = embedding_client::cosine_similarity(qe, se);
                    if sim > super::scoring::RELEVANCE_SIM_THRESHOLD {
                        score += sim * 0.4;
                    }
                }
            }

            // Boost larger symbols (likely more important)
            let lines = sym.end_line.saturating_sub(sym.start_line) + 1;
            if lines > 10 {
                score += 0.05;
            }

            score.min(1.0)
        })
        .collect()
}

/// Check if `needle` appears in `haystack` at a word boundary.
/// A word boundary is: start/end of string, underscore, non-alphanumeric char,
/// or a camelCase transition (lowercase followed by uppercase).
pub fn has_word_match(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let mut search_from = 0;
    while let Some(pos) = haystack[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        let end_pos = abs_pos + needle.len();

        let before_ok = abs_pos == 0
            || !h[abs_pos - 1].is_ascii_alphanumeric()
            || h[abs_pos - 1] == b'_';

        let after_ok = end_pos >= h.len()
            || !h[end_pos].is_ascii_alphanumeric()
            || h[end_pos] == b'_'
            || h[end_pos].is_ascii_uppercase();

        if before_ok && after_ok {
            return true;
        }
        search_from = abs_pos + 1;
    }
    false
}
