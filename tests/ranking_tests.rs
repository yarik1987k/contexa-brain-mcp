//! Tests that guard the search and memory ranking constants in
//! `src/context/scoring.rs`. The tuning there is hand-pulled from real use
//! ("file embeddings are too noisy", "path match is the strongest signal",
//! "hub files shouldn't outscore leaf implementations"). These tests lock
//! that behaviour so future scoring changes are intentional, not accidental.
//!
//! Integration tests that index a real fixture and run search depend on the
//! FastEmbed model being available. They auto-skip if the model failed to load
//! (e.g. offline CI, first-run download timing out). Pure constant + function
//! tests run unconditionally.

mod common;

use context_brain::context::{relevance_scorer::has_word_match, scoring};
use context_brain::indexer::embedding_client::is_model_available;
use context_brain::memory;

/// Test 1 — Exact-name match outranks substring match.
///
/// Validates `SEARCH_EXACT_NAME_BONUS` (5.0) > `SEARCH_SUBSTRING_NAME_BONUS` (3.0)
/// produces the expected ordering end-to-end through indexing + search.
#[test]
fn exact_name_beats_substring() {
    if !is_model_available() {
        eprintln!("skipped: embedding model unavailable");
        return;
    }

    let (_tmp, project) = common::setup_indexed_project(&[
        (
            "src/profile.js",
            "function getUser(id) {\n  return db.fetch(id);\n}\n",
        ),
        (
            "src/avatar.js",
            "function getUserAvatar(id) {\n  return cdn.url(id);\n}\n",
        ),
    ]);

    let (out, ranked) = common::search(&project, "getUser");

    let profile_rank = common::rank_of(&ranked, "src/profile.js")
        .unwrap_or_else(|| panic!("profile.js missing from results:\n{}", out));
    let avatar_rank = common::rank_of(&ranked, "src/avatar.js")
        .unwrap_or_else(|| panic!("avatar.js missing from results:\n{}", out));

    assert!(
        profile_rank < avatar_rank,
        "expected exact match (profile.js) above substring match (avatar.js), got:\n{}",
        out
    );
}

/// Test 2 — File whose path matches the query outranks a file whose content
/// merely mentions the term.
///
/// Validates the scoring.rs:21 note that "path match is the strongest signal
/// for finding implementations".
#[test]
fn path_match_beats_content_mention() {
    if !is_model_available() {
        eprintln!("skipped: embedding model unavailable");
        return;
    }

    let (_tmp, project) = common::setup_indexed_project(&[
        // Path contains "auth" + has real definitions.
        (
            "src/auth/middleware.rs",
            "pub fn check_token(t: &str) -> bool { !t.is_empty() }\n\
             pub fn enforce(req: &str) -> bool { check_token(req) }\n",
        ),
        // Path does NOT contain "auth", body only mentions it in a comment.
        (
            "src/handlers.rs",
            "// handles auth-related routing concerns elsewhere\n\
             pub fn dispatch(route: &str) -> &str { route }\n",
        ),
    ]);

    let (out, ranked) = common::search(&project, "auth");

    let middleware = common::rank_of(&ranked, "src/auth/middleware.rs")
        .unwrap_or_else(|| panic!("middleware.rs missing:\n{}", out));
    let handlers = common::rank_of(&ranked, "src/handlers.rs");

    if let Some(handlers_rank) = handlers {
        assert!(
            middleware < handlers_rank,
            "expected path-match (auth/middleware.rs) above content-mention (handlers.rs), got:\n{}",
            out
        );
    }
    // If handlers.rs doesn't appear at all, the path-match dominance is even
    // stronger — that's fine.
}

/// Test 3 — Centrality boost is bounded so a hub file with no name match
/// cannot outrank a leaf file with an exact name match.
///
/// Validates scoring.rs:24 note about capping centrality so hub files
/// (routes etc.) don't outscore leaf implementations.
#[test]
fn centrality_boost_cannot_override_exact_match() {
    // The arithmetic guarantee: max possible centrality boost = 0.5
    // and exact name bonus alone is 5.0, plus the indexed.rs "is_definition"
    // multiplier of 1.5 brings it to 7.5. Both are pre-CTI multipliers.
    // No realistic combination of FILE_SIM and CENTRALITY can close that gap
    // for a file that has zero symbol name match.
    let max_no_match_score =
        scoring::SEARCH_FILE_SIM_WEIGHT * 1.0 + scoring::SEARCH_CENTRALITY_MAX_BOOST;
    let min_exact_match_score = scoring::SEARCH_EXACT_NAME_BONUS;

    assert!(
        max_no_match_score < min_exact_match_score,
        "centrality + file-sim ({} max) must stay below exact-name ({}), otherwise hub files \
         could outrank leaf implementations",
        max_no_match_score,
        min_exact_match_score,
    );
}

/// Test 4 — Word-boundary matching: "get" matches getUser (camelCase) and
/// get_data (underscore), but NOT target, budget, or forget.
///
/// Boundaries recognized by `has_word_match`: start/end of string, non-alphanumeric,
/// underscore, and an uppercase char following the needle (camelCase). Lowercase
/// concatenations like "getuser" do NOT match — this is intentional and tested.
#[test]
fn word_boundary_matching_is_correct() {
    // True positives — needle at a word boundary.
    assert!(has_word_match("fn get_data() {}", "get"), "get_data should match (underscore boundary)");
    assert!(has_word_match("call get and go", "get"), "standalone word should match (whitespace boundary)");
    assert!(has_word_match("getUser", "get"), "getUser should match (camelCase boundary)");
    assert!(has_word_match("get()", "get"), "get( should match (paren boundary)");

    // False positives we must NOT report.
    assert!(!has_word_match("getuser(id)", "get"), "lowercase 'getuser' should NOT match (no boundary)");
    assert!(!has_word_match("target = 0", "get"), "target should NOT match get");
    assert!(!has_word_match("budget = 0", "get"), "budget should NOT match get");
    assert!(!has_word_match("forget()", "get"), "forget should NOT match get");
}

/// Test 5 — Scoring constants are sane. Numerical guarantees the rest of the
/// search ranking depends on. Cheap unit test, catches accidental tuning bugs.
#[test]
fn scoring_constants_are_sane() {
    // Exact-name must beat substring.
    assert!(
        scoring::SEARCH_EXACT_NAME_BONUS > scoring::SEARCH_SUBSTRING_NAME_BONUS,
        "exact-name must outrank substring"
    );

    // Thresholds must be valid cosine values.
    for &t in &[
        scoring::SEARCH_SYMBOL_SIM_THRESHOLD,
        scoring::SEARCH_FILE_SIM_THRESHOLD,
        scoring::RELEVANCE_HIGH_THRESHOLD,
        scoring::RELEVANCE_MEDIUM_THRESHOLD,
        scoring::RELEVANCE_SIM_THRESHOLD,
    ] {
        assert!(t >= 0.0 && t <= 1.0, "threshold {} not in [0,1]", t);
    }

    // Relevance bands must be ordered.
    assert!(
        scoring::RELEVANCE_HIGH_THRESHOLD > scoring::RELEVANCE_MEDIUM_THRESHOLD,
        "high relevance must exceed medium"
    );

    // Memory recall weights sum to 1.0 (within float epsilon).
    let total = scoring::MEMORY_SEMANTIC_WEIGHT
        + scoring::MEMORY_KEYWORD_WEIGHT
        + scoring::MEMORY_RECENCY_WEIGHT;
    assert!(
        (total - 1.0).abs() < 1e-5,
        "memory weights must sum to 1.0, got {}",
        total
    );

    // Caps must be positive.
    assert!(scoring::MAX_MEMORIES > 0);
    assert!(scoring::MAX_MEMORY_SIZE > 0);
    assert!(scoring::MAX_SEARCH_MATCHES > 0);
    assert!(scoring::MAX_RECALL_CANDIDATES > 0);
}

/// Test 6 — Memory recall ranks the keyword-matching entry highest among
/// peers with no shared terms. End-to-end through save → recall.
#[test]
fn memory_recall_ranks_match_above_unrelated() {
    if !is_model_available() {
        eprintln!("skipped: embedding model unavailable");
        return;
    }

    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().to_path_buf();
    // Need the DB present, but no source files needed for memory recall.
    std::fs::write(project.join("dummy.rs"), "pub fn _x() {}\n").unwrap();

    memory::store::save(&project, "We chose JWT for authentication tokens", "decision", "auth,security").unwrap();
    memory::store::save(&project, "Use Redis for session caching", "decision", "cache").unwrap();
    memory::store::save(&project, "Migrations run on deploy", "ops", "deploy").unwrap();

    let out = memory::searcher::recall(&project, "authentication token strategy", None, 5).unwrap();

    let jwt_pos = out.find("JWT");
    let redis_pos = out.find("Redis");

    assert!(
        jwt_pos.is_some(),
        "expected JWT memory to surface in recall, got:\n{}",
        out
    );
    if let (Some(jp), Some(rp)) = (jwt_pos, redis_pos) {
        assert!(
            jp < rp,
            "expected JWT memory to outrank Redis memory for auth-token query, got:\n{}",
            out
        );
    }
}
