use std::path::PathBuf;
use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo, Implementation},
    schemars, tool, tool_handler, tool_router,
    ErrorData as McpError,
};
use serde::Deserialize;

use crate::tools;
use crate::memory;
use crate::indexer;
use crate::telemetry;

use std::time::Instant;

// ── Parameter structs ─────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListFilesParams {
    #[schemars(description = "Relative path from project root (default: root)")]
    pub path: Option<String>,
    #[schemars(description = "Max directory depth (default: 2)")]
    pub depth: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetFileContextParams {
    #[schemars(description = "Relative file path from project root")]
    pub path: String,
    #[schemars(description = "Mode: 'summary', 'smart', 'symbols', 'full' (default: summary)")]
    pub mode: Option<String>,
    #[schemars(description = "Max tokens (default: 1500)")]
    pub token_budget: Option<u32>,
    #[schemars(description = "Query for smart mode relevance filtering")]
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetSymbolParams {
    #[schemars(description = "Symbol name (function, class, struct, etc.)")]
    pub name: String,
    #[schemars(description = "File path hint to narrow search")]
    pub file: Option<String>,
    #[schemars(description = "Max code lines per symbol (default: 30). Pass 0 for full.")]
    pub max_lines: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchCodebaseParams {
    #[schemars(description = "Natural language query or keyword to search for")]
    pub query: String,
    #[schemars(description = "Max results (default: 5)")]
    pub max_results: Option<u32>,
    #[schemars(description = "Max tokens to return (default: 2500)")]
    pub token_budget: Option<u32>,
    #[schemars(description = "Detail level: 'pointers' (file+line+name only — minimal tokens), 'signatures' (+ type signatures), 'code' (+ function bodies). Default: 'pointers'")]
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveMemoryParams {
    #[schemars(description = "The content to remember")]
    pub content: String,
    #[schemars(description = "Category (default: general)")]
    pub category: Option<String>,
    #[schemars(description = "Tags, comma-separated")]
    pub tags: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecallMemoryParams {
    #[schemars(description = "Recall query")]
    pub query: String,
    #[schemars(description = "Filter by category")]
    pub category: Option<String>,
    #[schemars(description = "Max results (default: 5)")]
    pub max_results: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexProjectParams {
    #[schemars(description = "Force full re-index")]
    pub force: Option<bool>,
}

// ── Server struct ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ContextBrainServer {
    project_path: PathBuf,
    tool_router: ToolRouter<Self>,
    _watcher: Option<Arc<indexer::watch_manager::WatchManager>>,
}

impl ContextBrainServer {
    /// Record one tool call (best-effort, never blocks the reply).
    /// Heuristic empty-detection: search/recall print a "No ..." line on no
    /// matches; everything else just contributes latency + call count.
    fn record(&self, tool: &str, query: Option<&str>, output: &str, start: Instant) {
        let latency_ms = start.elapsed().as_millis() as i64;
        let trimmed = output.trim_start();
        let result_count = if trimmed.starts_with("No results")
            || trimmed.starts_with("No relevant memories")
            || trimmed.starts_with("No memories")
        {
            Some(0)
        } else {
            None
        };
        telemetry::record_tool_call(&self.project_path, tool, query, result_count, latency_ms);
    }
}

#[tool_router]
impl ContextBrainServer {
    pub fn new(project_path: PathBuf) -> Self {
        // Auto-index on startup if not already indexed
        if !indexer::pipeline::is_indexed(&project_path) {
            tracing::info!("Project not indexed. Auto-indexing...");
            match indexer::pipeline::index_project(&project_path) {
                Ok(stats) => tracing::info!("{}", stats),
                Err(e) => tracing::warn!("Auto-index failed (search will use live scan): {}", e),
            }
        }

        // Check embedding model health
        if !indexer::embedding_client::is_model_available() {
            tracing::error!("Embedding model failed to load. Semantic search will be unavailable — only keyword matching will work.");
        }

        // Start file watcher for incremental re-indexing
        let watcher = match indexer::watch_manager::WatchManager::start(project_path.clone()) {
            Ok(wm) => {
                tracing::info!("File watcher started");
                Some(Arc::new(wm))
            }
            Err(e) => {
                tracing::warn!("File watcher failed to start: {}", e);
                None
            }
        };

        Self {
            project_path,
            tool_router: Self::tool_router(),
            _watcher: watcher,
        }
    }

    #[tool(description = "List project files. Respects .gitignore.")]
    async fn list_files(
        &self,
        Parameters(params): Parameters<ListFilesParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let base = match &params.path {
                Some(p) => {
                    let joined = project_path.join(p);
                    let resolved = joined.canonicalize().map_err(|e| format!("Invalid path: {}", e))?;
                    if !resolved.starts_with(&project_path) {
                        return Err("Path escapes project directory".to_string());
                    }
                    resolved
                }
                None => project_path.clone(),
            };
            let max_depth = params.depth.unwrap_or(2);
            tools::list_files::build_file_tree(&base, max_depth)
                .map_err(|e| format!("Failed to list files: {}", e))
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(e, None))?;

        self.record("list_files", None, &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Read file context. Modes: 'summary' (signatures), 'smart' (query-aware: full code for relevant functions), 'symbols' (compact list), 'full'.")]
    async fn get_file_context(
        &self,
        Parameters(params): Parameters<GetFileContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let query_for_telemetry = params.query.clone();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            // Fix path duplication: if Claude passes "project-name/src/foo.js" but the project root is
            // already "/path/to/project-name", strip the leading directory component to avoid
            // resolving to "/path/to/project-name/project-name/src/foo.js".
            let relative = {
                let p = std::path::Path::new(&params.path);
                if let Some(project_dir_name) = project_path.file_name() {
                    if let Ok(stripped) = p.strip_prefix(project_dir_name) {
                        stripped.to_path_buf()
                    } else {
                        p.to_path_buf()
                    }
                } else {
                    p.to_path_buf()
                }
            };
            let file_path = project_path.join(&relative);
            let resolved = file_path.canonicalize().map_err(|e| format!("Invalid path: {}", e))?;
            if !resolved.starts_with(&project_path) {
                return Err("Path escapes project directory".to_string());
            }
            let mode = params.mode.unwrap_or_else(|| "summary".to_string());
            let budget = params.token_budget.unwrap_or(1500).min(100_000);
            tools::get_file_context::read_file_context(&resolved, &mode, budget, params.query.as_deref())
                .map_err(|e| format!("Failed to read file: {}", e))
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(e, None))?;

        self.record("get_file_context", query_for_telemetry.as_deref(), &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Get symbol code by name. Returns exact code block, truncated to max_lines (default 50).")]
    async fn get_symbol(
        &self,
        Parameters(params): Parameters<GetSymbolParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let name_for_telemetry = params.name.clone();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let max_lines = params.max_lines.unwrap_or(30);
            tools::get_symbol::get_symbol(&project_path, &params.name, params.file.as_deref(), max_lines)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to get symbol: {}", e), None))?;

        self.record("get_symbol", Some(&name_for_telemetry), &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Search codebase by semantic similarity + keywords. Use detail='pointers' (default, minimal), 'signatures', or 'code'.")]
    async fn search_codebase(
        &self,
        Parameters(params): Parameters<SearchCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let query_for_telemetry = params.query.clone();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let max = params.max_results.unwrap_or(5);
            let budget = params.token_budget.unwrap_or(2500);
            let detail = params.detail.unwrap_or_else(|| "pointers".to_string());
            tools::search_codebase::search(&project_path, &params.query, max, budget, &detail)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Search failed: {}", e), None))?;

        self.record("search_codebase", Some(&query_for_telemetry), &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Save context to persistent cross-session memory (searchable semantically). Near-duplicates merge automatically; cross-category duplicates link.")]
    async fn save_memory(
        &self,
        Parameters(params): Parameters<SaveMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let project_path = self.project_path.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let cat = params.category.unwrap_or_else(|| "general".to_string());
            let tags = params.tags.unwrap_or_default();
            memory::store::save(&project_path, &params.content, &cat, &tags)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to save memory: {}", e), None))?;

        let msg = match outcome {
            memory::store::SaveOutcome::Inserted(id) => format!("Memory saved (id {}).", id),
            memory::store::SaveOutcome::Merged(id) => format!("Near-duplicate detected — merged into existing memory id {}.", id),
            memory::store::SaveOutcome::Linked { new_id, peer_id } => format!(
                "Cross-category duplicate of memory id {} — saved as id {} with link.",
                peer_id, new_id
            ),
        };
        self.record("save_memory", None, &msg, start);
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "Recall from persistent memory using semantic search.")]
    async fn recall_memory(
        &self,
        Parameters(params): Parameters<RecallMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let query_for_telemetry = params.query.clone();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let max = params.max_results.unwrap_or(5);
            memory::searcher::recall(&project_path, &params.query, params.category.as_deref(), max)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to recall: {}", e), None))?;

        self.record("recall_memory", Some(&query_for_telemetry), &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Condensed project architecture: file stats, tech stack, README skeleton.")]
    async fn get_architecture(&self) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            tools::get_architecture::build_overview(&project_path)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to build overview: {}", e), None))?;

        self.record("get_architecture", None, &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Index codebase for fast semantic search. Run once, auto-updates on file changes.")]
    async fn index_project(
        &self,
        Parameters(params): Parameters<IndexProjectParams>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let force = params.force.unwrap_or(false);
            if !force && indexer::pipeline::is_indexed(&project_path) {
                return Ok("Project already indexed. Use force=true to re-index.".to_string());
            }
            if force {
                // Delete the DB entirely to avoid any corruption or FK constraint issues
                let db_path = crate::db::schema::db_path(&project_path);
                if db_path.exists() {
                    let _ = std::fs::remove_file(&db_path);
                    // Also remove WAL and SHM files if they exist
                    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
                    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
                }
            }
            let stats = indexer::pipeline::index_project(&project_path)
                .map_err(|e| format!("Indexing failed: {}", e))?;
            Ok(format!("Indexing complete! {}", stats))
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(e, None))?;

        self.record("index_project", None, &result, start);
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }
}

#[tool_handler]
impl ServerHandler for ContextBrainServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(
            Implementation::new("context-brain", env!("CARGO_PKG_VERSION"))
        )
        .with_instructions(
            "context-brain is a local-first MCP server that cuts your token bills. \
             Instead of reading full files, prefer these tools: \
             search_codebase (semantic + keyword search with ranked results), \
             get_file_context (mode='summary'/'smart' returns slices, not whole files), \
             get_symbol (extract a specific function/class by name), \
             save_memory and recall_memory (persistent cross-session memory for decisions and conventions), \
             get_architecture (~500-token project overview), list_files (directory tree), \
             index_project (build or refresh the search index). All processing is local — no outbound API calls.".to_string()
        )
    }
}
