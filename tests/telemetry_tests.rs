//! End-to-end verification of the local telemetry pipeline:
//! record_tool_call → SQLite → summarize → stats::render.
//!
//! Confirms the on-device data path works; the privacy guarantee (no network)
//! is structural — `telemetry` has no HTTP client and no outbound code path.

use std::path::PathBuf;

use context_brain::{telemetry, tools};

fn fresh_project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path().to_path_buf();
    // schema::open_db is called lazily by telemetry::record; we just need
    // a writable directory and any source file so the DB lives somewhere.
    std::fs::write(p.join("placeholder.rs"), "pub fn _x() {}\n").unwrap();
    (tmp, p)
}

#[test]
fn records_a_tool_call_and_summarizes() {
    let (_tmp, project) = fresh_project();

    // Each record_tool_call returns the background-thread handle so we can
    // wait for the write to land before reading.
    let h1 = telemetry::record_tool_call(&project, "search_codebase", Some("auth flow"), Some(3), 84);
    let h2 = telemetry::record_tool_call(&project, "search_codebase", Some("auth flow"), Some(0), 71);
    let h3 = telemetry::record_tool_call(&project, "get_file_context", None, None, 22);
    h1.join().unwrap();
    h2.join().unwrap();
    h3.join().unwrap();

    let summary = telemetry::summarize(&project, 0).unwrap();
    assert_eq!(summary.total_calls, 3, "expected 3 recorded calls");

    let search = summary
        .by_tool
        .iter()
        .find(|t| t.tool_name == "search_codebase")
        .expect("search_codebase row missing");
    assert_eq!(search.call_count, 2);
    assert_eq!(search.empty_result_count, 1, "one of the two search calls returned 0 results");

    // The empty-query hash should appear in the top list.
    assert!(
        !summary.top_empty_query_hashes.is_empty(),
        "expected at least one empty-result query hash"
    );

    let rendered = tools::stats::render(&project, None).unwrap();
    assert!(rendered.contains("search_codebase"), "rendered report missing search_codebase:\n{}", rendered);
    assert!(rendered.contains("Total tool calls: 3"), "rendered total wrong:\n{}", rendered);
}

#[test]
fn empty_project_renders_an_empty_report() {
    let (_tmp, project) = fresh_project();
    let rendered = tools::stats::render(&project, Some(7)).unwrap();
    assert!(
        rendered.contains("No tool calls recorded yet"),
        "expected empty-state message, got:\n{}",
        rendered
    );
}
