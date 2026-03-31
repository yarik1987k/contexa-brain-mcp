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
    #[schemars(description = "Reading mode: 'full', 'summary', 'smart', or 'symbols' (default: summary). 'smart' mode uses the query to include full code for relevant functions and only signatures for the rest.")]
    pub mode: Option<String>,
    #[schemars(description = "Max tokens to return (default: 3000)")]
    pub token_budget: Option<u32>,
    #[schemars(description = "Optional query/context for smart mode — helps select which functions to show in full")]
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetSymbolParams {
    #[schemars(description = "Symbol name to look up (function, class, struct, etc.)")]
    pub name: String,
    #[schemars(description = "Optional file path hint to narrow the search")]
    pub file: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchCodebaseParams {
    #[schemars(description = "Natural language query or keyword to search for")]
    pub query: String,
    #[schemars(description = "Max results to return (default: 10)")]
    pub max_results: Option<u32>,
    #[schemars(description = "Max tokens to return (default: 4000)")]
    pub token_budget: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveMemoryParams {
    #[schemars(description = "The content to remember")]
    pub content: String,
    #[schemars(description = "Category: 'decision', 'architecture', 'task', 'bug', 'todo' (default: general)")]
    pub category: Option<String>,
    #[schemars(description = "Optional tags for retrieval (comma-separated)")]
    pub tags: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecallMemoryParams {
    #[schemars(description = "What to recall, e.g. 'what did we decide about the database?'")]
    pub query: String,
    #[schemars(description = "Filter by category: 'decision', 'architecture', 'task', 'bug', 'todo'")]
    pub category: Option<String>,
    #[schemars(description = "Max memories to return (default: 5)")]
    pub max_results: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexProjectParams {
    #[schemars(description = "Force re-index even if already indexed (default: false)")]
    pub force: Option<bool>,
}

// ── Server struct ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ContextBrainServer {
    project_path: PathBuf,
    tool_router: ToolRouter<Self>,
    _watcher: Option<Arc<indexer::watch_manager::WatchManager>>,
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

    #[tool(description = "List files in the project directory tree with metadata. Respects .gitignore.")]
    async fn list_files(
        &self,
        Parameters(params): Parameters<ListFilesParams>,
    ) -> Result<CallToolResult, McpError> {
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

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Read a file with smart token optimization. Modes: 'full' (entire file), 'summary' (imports + AST signatures), 'smart' (query-aware: full code for relevant functions, signatures for rest — best for targeted work), 'symbols' (compact symbol list). Use 'smart' with a query for maximum token savings.")]
    async fn get_file_context(
        &self,
        Parameters(params): Parameters<GetFileContextParams>,
    ) -> Result<CallToolResult, McpError> {
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
            let budget = params.token_budget.unwrap_or(3000).min(100_000);
            tools::get_file_context::read_file_context(&resolved, &mode, budget, params.query.as_deref())
                .map_err(|e| format!("Failed to read file: {}", e))
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(e, None))?;

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Get a specific function, class, or symbol by name. Returns the exact code block — the most token-efficient way to get specific code.")]
    async fn get_symbol(
        &self,
        Parameters(params): Parameters<GetSymbolParams>,
    ) -> Result<CallToolResult, McpError> {
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            tools::get_symbol::get_symbol(&project_path, &params.name, params.file.as_deref())
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to get symbol: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Search the codebase using semantic similarity + keyword matching. Returns relevant code snippets ranked by relevance within a token budget.")]
    async fn search_codebase(
        &self,
        Parameters(params): Parameters<SearchCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let max = params.max_results.unwrap_or(10);
            let budget = params.token_budget.unwrap_or(4000);
            tools::search_codebase::search(&project_path, &params.query, max, budget)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Search failed: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Save a decision, architectural insight, or task context to persistent memory. This memory persists across Cursor sessions and is searchable semantically.")]
    async fn save_memory(
        &self,
        Parameters(params): Parameters<SaveMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let project_path = self.project_path.clone();
        tokio::task::spawn_blocking(move || {
            let cat = params.category.unwrap_or_else(|| "general".to_string());
            let tags = params.tags.unwrap_or_default();
            memory::store::save(&project_path, &params.content, &cat, &tags)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to save memory: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text("Memory saved successfully.")]))
    }

    #[tool(description = "Recall past decisions, context, and insights from persistent memory. Uses semantic similarity to find relevant memories even with different wording.")]
    async fn recall_memory(
        &self,
        Parameters(params): Parameters<RecallMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let max = params.max_results.unwrap_or(5);
            memory::searcher::recall(&project_path, &params.query, params.category.as_deref(), max)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to recall: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Get a condensed project architecture overview (~500 tokens). Reads README/ARCHITECTURE.md plus dynamic file stats and tech stack detection.")]
    async fn get_architecture(&self) -> Result<CallToolResult, McpError> {
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            tools::get_architecture::build_overview(&project_path)
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(format!("Failed to build overview: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(description = "Index the project codebase: extract symbols via AST, generate embeddings for semantic search, and store in local database. Run this once for fast future searches.")]
    async fn index_project(
        &self,
        Parameters(params): Parameters<IndexProjectParams>,
    ) -> Result<CallToolResult, McpError> {
        let project_path = self.project_path.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let force = params.force.unwrap_or(false);
            if !force && indexer::pipeline::is_indexed(&project_path) {
                return Ok("Project already indexed. Use force=true to re-index.".to_string());
            }
            let stats = indexer::pipeline::index_project(&project_path)
                .map_err(|e| format!("Indexing failed: {}", e))?;
            Ok(format!("Indexing complete! {}", stats))
        }).await.map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
          .map_err(|e| McpError::internal_error(e, None))?;

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
            "Context Brain is an intelligent context manager that saves tokens and remembers context. \
             Tools: search_codebase (semantic + keyword search), get_file_context (smart file reading), \
             get_symbol (extract specific function/class by name), save_memory/recall_memory (persistent cross-session memory), \
             get_architecture (project overview), list_files (directory tree), index_project (build search index).".to_string()
        )
    }
}
