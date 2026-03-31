use crate::indexer::embedding_client;
use crate::indexer::symbol_extractor::Symbol;

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

            // Keyword match in signature
            for word in query_lower.split_whitespace() {
                if word.len() < 3 {
                    continue;
                }
                if sig_lower.contains(word) {
                    score += 0.1;
                }
            }

            // Semantic similarity via pre-computed batch embeddings
            if let (Some(qe), Some(ref embeds)) = (query_embedding, &sym_embeddings) {
                if let Some(se) = embeds.get(i) {
                    let sim = embedding_client::cosine_similarity(qe, se);
                    if sim > 0.3 {
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

/// Score a single symbol's relevance to a query (for cases where batch isn't needed).
pub fn score_symbol(symbol: &Symbol, query: &str, query_embedding: Option<&[f32]>) -> f32 {
    score_symbols_batch(std::slice::from_ref(symbol), query, query_embedding)[0]
}
