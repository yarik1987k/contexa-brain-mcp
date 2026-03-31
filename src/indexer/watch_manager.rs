use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// File watcher that triggers incremental re-indexing on changes.
///
/// Uses the `notify` crate to watch the project directory. Events are
/// debounced (500ms) so rapid saves don't trigger multiple re-indexes.
pub struct WatchManager {
    _watcher: RecommendedWatcher,
    _task: tokio::task::JoinHandle<()>,
}

impl WatchManager {
    /// Start watching `project_path` for file changes.
    /// Spawns a background tokio task that debounces events and triggers re-indexing.
    pub fn start(project_path: PathBuf) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<Event>();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?;

        watcher.watch(&project_path, RecursiveMode::Recursive)?;

        let pp = Arc::new(project_path);
        let task = tokio::task::spawn(async move {
            event_loop(pp, rx).await;
        });

        Ok(Self {
            _watcher: watcher,
            _task: task,
        })
    }
}

/// Main event loop: collect file events, debounce, then re-index.
async fn event_loop(project_path: Arc<PathBuf>, rx: std::sync::mpsc::Receiver<Event>) {
    use std::collections::HashSet;

    loop {
        // Block until first event (via spawn_blocking since rx.recv() is blocking)
        let first_event = {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(e) => e,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    tracing::info!("File watcher channel closed, stopping");
                    return;
                }
            }
        };

        // Debounce: collect events for 500ms
        let mut changed_paths: HashSet<PathBuf> = HashSet::new();
        let mut deleted_paths: HashSet<PathBuf> = HashSet::new();

        classify_event(&first_event, &mut changed_paths, &mut deleted_paths);

        let debounce_until = std::time::Instant::now() + Duration::from_millis(500);
        while std::time::Instant::now() < debounce_until {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => classify_event(&event, &mut changed_paths, &mut deleted_paths),
                Err(_) => {}
            }
        }

        // Filter to source files within the project
        let pp = project_path.as_ref();
        let to_reindex: Vec<String> = changed_paths
            .iter()
            .filter_map(|p| to_relative_source(p, pp))
            .collect();

        let to_delete: Vec<String> = deleted_paths
            .iter()
            .filter_map(|p| to_relative_source(p, pp))
            .collect();

        // Process deletions
        if !to_delete.is_empty() {
            let pp_clone = project_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                for rel in &to_delete {
                    if let Err(e) = crate::indexer::pipeline::delete_file(&pp_clone, rel) {
                        tracing::warn!("Failed to remove {} from index: {}", rel, e);
                    }
                }
            }).await;
        }

        // Process re-indexing
        if !to_reindex.is_empty() {
            let pp_clone = project_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                match crate::indexer::pipeline::index_files(&pp_clone, &to_reindex) {
                    Ok(n) if n > 0 => tracing::info!("Watcher re-indexed {} files", n),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("Watcher re-index failed: {}", e),
                }
            }).await;
        }
    }
}

/// Classify a notify event into changed or deleted paths.
fn classify_event(event: &Event, changed: &mut std::collections::HashSet<PathBuf>, deleted: &mut std::collections::HashSet<PathBuf>) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in &event.paths {
                changed.insert(path.clone());
                deleted.remove(path);
            }
        }
        EventKind::Remove(_) => {
            for path in &event.paths {
                deleted.insert(path.clone());
                changed.remove(path);
            }
        }
        _ => {}
    }
}

/// Convert an absolute path to a relative source-file path, or None if it should be skipped.
fn to_relative_source(path: &Path, project_path: &Path) -> Option<String> {
    // Must be within project
    let relative = path.strip_prefix(project_path).ok()?;
    let rel_str = relative.display().to_string();

    // Skip hidden files, skip dirs, and the database itself
    for component in relative.components() {
        let name = component.as_os_str().to_string_lossy();
        if name.starts_with('.') || super::config::is_skip_dir(&name) {
            return None;
        }
    }

    // Must be a source file
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !super::config::is_source_file(ext) {
        return None;
    }

    Some(rel_str)
}
