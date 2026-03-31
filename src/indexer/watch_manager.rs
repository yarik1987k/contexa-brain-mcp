//! File watcher for incremental re-indexing.
//!
//! Uses the `notify` crate to watch the project directory for changes and
//! marks modified files as needing re-indexing in the SQLite database.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, warn};

/// Start watching `project_path` for file changes. When a file is created or
/// modified, its path is written into the `needs_reindex` table in the SQLite
/// database at `db_path`.
///
/// This function spawns a background thread and returns immediately. The
/// watcher keeps running until the returned `WatchHandle` is dropped.
pub fn start_watching(project_path: PathBuf, db_path: PathBuf) -> Result<WatchHandle> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default()
        .with_poll_interval(Duration::from_secs(2)))?;

    watcher.watch(&project_path, RecursiveMode::Recursive)?;

    info!("[watch_manager] Watching {} for changes", project_path.display());

    let thread = std::thread::spawn(move || {
        // Open (or create) the database and ensure the table exists.
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("[watch_manager] Could not open DB at {}: {}", db_path.display(), e);
                return;
            }
        };

        if let Err(e) = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS needs_reindex (
                 path TEXT PRIMARY KEY,
                 flagged_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
             );",
        ) {
            warn!("[watch_manager] Could not create needs_reindex table: {}", e);
            return;
        }

        for result in rx {
            match result {
                Ok(event) => {
                    if should_reindex(&event.kind) {
                        for path in &event.paths {
                            if is_source_file(path) {
                                let path_str = path.to_string_lossy();
                                debug!("[watch_manager] Flagging for re-index: {}", path_str);
                                if let Err(e) = conn.execute(
                                    "INSERT OR REPLACE INTO needs_reindex (path) VALUES (?1)",
                                    rusqlite::params![path_str.as_ref()],
                                ) {
                                    warn!("[watch_manager] DB insert failed: {}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[watch_manager] Watch error: {}", e);
                }
            }
        }

        info!("[watch_manager] Watcher channel closed, shutting down.");
    });

    Ok(WatchHandle { _watcher: watcher, _thread: thread })
}

/// Returns true for event kinds that mean a file's content has changed.
fn should_reindex(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_)
    )
}

/// Returns true if the path looks like a source file we care about.
fn is_source_file(path: &Path) -> bool {
    // Skip hidden files / directories and common non-source paths
    for component in path.components() {
        let s = component.as_os_str().to_string_lossy();
        if s.starts_with('.') || super::config::is_skip_dir(&s) {
            return false;
        }
    }

    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| super::config::is_source_file(ext))
        .unwrap_or(false)
}

/// Opaque handle that keeps the watcher alive. Drop to stop watching.
pub struct WatchHandle {
    _watcher: RecommendedWatcher,
    _thread: std::thread::JoinHandle<()>,
}
