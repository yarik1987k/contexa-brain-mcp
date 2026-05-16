//! Memory hygiene: dedupe-on-save, access-recency decay, contradiction review.
//!
//! These tests use the real FastEmbed pipeline so they skip on environments
//! where the model is unavailable (e.g. cold CI without the cache).

use std::path::PathBuf;

use context_brain::indexer::embedding_client::is_model_available;
use context_brain::memory;
use context_brain::memory::store::SaveOutcome;

fn fresh_project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path().to_path_buf();
    std::fs::write(p.join("placeholder.rs"), "pub fn _x() {}\n").unwrap();
    (tmp, p)
}

/// Saving a near-identical memory in the same category merges into the
/// existing row instead of creating a duplicate.
#[test]
fn dedupe_merges_same_category() {
    if !is_model_available() {
        eprintln!("skipped: embedding model unavailable");
        return;
    }

    let (_tmp, project) = fresh_project();

    let first = memory::store::save(&project, "We use JWT for authentication tokens", "decision", "auth")
        .expect("first save");
    let first_id = match first {
        SaveOutcome::Inserted(id) => id,
        other => panic!("expected Inserted on first save, got {:?}", other),
    };

    // Near-identical phrasing — should dedupe.
    let second = memory::store::save(
        &project,
        "We use JWT for authentication tokens.",
        "decision",
        "auth",
    )
    .expect("second save");
    match second {
        SaveOutcome::Merged(merged_into) => {
            assert_eq!(merged_into, first_id, "expected merge into the first row");
        }
        other => panic!("expected Merged outcome, got {:?}", other),
    }

    // DB should still hold exactly one row.
    let conn = rusqlite::Connection::open(project.join(".context-brain.db")).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "expected exactly one memory row after merge");
}

/// Same content saved under a *different* category creates a new row but with
/// a `linked_id` pointing back to the existing peer — for the review CLI.
#[test]
fn dedupe_links_cross_category() {
    if !is_model_available() {
        eprintln!("skipped: embedding model unavailable");
        return;
    }

    let (_tmp, project) = fresh_project();

    let first = memory::store::save(&project, "Pin tokio to 1.x for now", "decision", "deps")
        .expect("first save");
    let first_id = match first {
        SaveOutcome::Inserted(id) => id,
        other => panic!("expected Inserted, got {:?}", other),
    };

    // Same content, different category — should link rather than merge.
    let second = memory::store::save(
        &project,
        "Pin tokio to 1.x for now",
        "constraint",
        "deps,version",
    )
    .expect("second save");
    match second {
        SaveOutcome::Linked { new_id, peer_id } => {
            assert_ne!(new_id, first_id, "new row should have a new id");
            assert_eq!(peer_id, first_id, "linked back to the original");
            // Verify the link landed in the DB.
            let conn = rusqlite::Connection::open(project.join(".context-brain.db")).unwrap();
            let stored_peer: Option<i64> = conn
                .query_row(
                    "SELECT linked_id FROM memories WHERE id = ?1",
                    rusqlite::params![new_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stored_peer, Some(first_id), "linked_id column not populated");
        }
        other => panic!("expected Linked outcome, got {:?}", other),
    }
}

/// A memory that's been recalled bumps its `last_accessed_at`, so subsequent
/// recall ranks it above an untouched peer with equal content match.
#[test]
fn access_tracking_bumps_last_accessed_at() {
    if !is_model_available() {
        eprintln!("skipped: embedding model unavailable");
        return;
    }

    let (_tmp, project) = fresh_project();

    memory::store::save(&project, "Database migrations run on deploy", "ops", "deploy").unwrap();
    memory::store::save(&project, "We chose Postgres for primary storage", "decision", "db").unwrap();

    // First recall — both rows should have NULL last_accessed_at beforehand.
    let conn = rusqlite::Connection::open(project.join(".context-brain.db")).unwrap();
    let nulls_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories WHERE last_accessed_at IS NULL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(nulls_before, 2, "expected both rows unaccessed before first recall");

    let _ = memory::searcher::recall(&project, "deploy migration plan", None, 5).unwrap();

    // After the recall, at least one row should have last_accessed_at populated.
    let bumped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE last_accessed_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(bumped >= 1, "expected ≥1 row to have last_accessed_at bumped, got {}", bumped);
}

/// The contradiction review surfaces nothing when there's only one memory.
#[test]
fn contradiction_review_empty_below_two_memories() {
    let (_tmp, project) = fresh_project();
    // No memories at all.
    let pairs = memory::hygiene::find_contradictions(&project, 10).unwrap();
    assert!(pairs.is_empty());
    let rendered = memory::hygiene::render_review(&pairs);
    assert!(rendered.contains("No candidate contradictions"));
}
