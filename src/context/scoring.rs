/// Scoring constants used across search, relevance scoring, and memory recall.
/// Centralized here so thresholds are documented and easy to tune.

// ── Search scoring (search_codebase) ─────────────────────────────────

/// Bonus for exact symbol name match in search results.
pub const SEARCH_EXACT_NAME_BONUS: f32 = 5.0;
/// Bonus for substring symbol name match.
pub const SEARCH_SUBSTRING_NAME_BONUS: f32 = 3.0;
/// Minimum cosine similarity for symbol embedding to count.
pub const SEARCH_SYMBOL_SIM_THRESHOLD: f32 = 0.3;
/// Multiplier applied to symbol embedding similarity score.
pub const SEARCH_SYMBOL_SIM_WEIGHT: f32 = 5.0;
/// Minimum cosine similarity for file embedding to count.
pub const SEARCH_FILE_SIM_THRESHOLD: f32 = 0.35;
/// Multiplier applied to file embedding similarity score.
pub const SEARCH_FILE_SIM_WEIGHT: f32 = 3.0;
/// Bonus for file path matching the query.
pub const SEARCH_PATH_MATCH_BONUS: f32 = 4.0;
/// Maximum boost from import-centrality ranking.
pub const SEARCH_CENTRALITY_MAX_BOOST: f32 = 2.0;

// ── Relevance scoring (file_summarizer) ──────────────────────────────

/// Threshold above which a symbol gets full code body included.
pub const RELEVANCE_HIGH_THRESHOLD: f32 = 0.25;
/// Threshold above which a symbol gets signature included (below high).
pub const RELEVANCE_MEDIUM_THRESHOLD: f32 = 0.05;
/// Minimum similarity for embedding to contribute to relevance score.
pub const RELEVANCE_SIM_THRESHOLD: f32 = 0.3;

// ── Memory recall ────────────────────────────────────────────────────

/// Minimum score for a memory to be included in recall results.
pub const MEMORY_MIN_SCORE: f32 = 0.15;
