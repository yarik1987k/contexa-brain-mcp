mod server;
mod tools;
mod indexer;
mod memory;
mod context;
mod turboquant;
mod db;

use anyhow::Result;
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

use crate::server::ContextBrainServer;

/// Context Brain — Intelligent MCP context manager
/// Pay less, get more from AI coding assistants
#[derive(Parser, Debug)]
#[command(name = "context-brain", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Start the MCP server (stdio transport for Cursor/Claude Code)
    Serve {
        /// Path to the project to manage context for
        #[arg(short, long, default_value = ".")]
        project: String,
    },
    /// Build the codebase index for a project
    Index {
        /// Path to the project to index
        #[arg(short, long, default_value = ".")]
        project: String,
    },
    /// Show file context (summary, symbols, or full) — same as get_file_context MCP tool
    Summary {
        /// File to summarize
        #[arg(short, long)]
        file: String,
        /// Mode: 'full', 'summary', 'symbols'
        #[arg(short, long, default_value = "summary")]
        mode: String,
    },
    /// Search the codebase — same as search_codebase MCP tool
    Search {
        /// Query to search for
        #[arg(short, long)]
        query: String,
        /// Path to the project root
        #[arg(short, long, default_value = ".")]
        project: String,
        /// Max results
        #[arg(short, long, default_value_t = 10)]
        max_results: u32,
    },
    /// Save a memory
    Remember {
        /// Content to remember
        #[arg(short, long)]
        content: String,
        /// Category
        #[arg(long, default_value = "general")]
        category: String,
        /// Project path
        #[arg(short, long, default_value = ".")]
        project: String,
    },
    /// Recall memories
    Recall {
        /// Query to recall
        #[arg(short, long)]
        query: String,
        /// Project path
        #[arg(short, long, default_value = ".")]
        project: String,
    },
    /// Print a directory tree (same logic as the list_files MCP tool)
    List {
        /// Path to the project root
        #[arg(short, long, default_value = ".")]
        project: String,
        /// Relative subdirectory under the project (default: project root)
        #[arg(long)]
        path: Option<String>,
        /// Max directory depth
        #[arg(short, long, default_value_t = 2)]
        depth: u32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { project } => {
            // Logs MUST go to stderr — stdout is the MCP transport
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::from_default_env()
                        .add_directive(tracing::Level::INFO.into()),
                )
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .init();

            let project_path = std::fs::canonicalize(&project)?;
            tracing::info!("Starting Context Brain MCP server for: {}", project_path.display());

            let server = ContextBrainServer::new(project_path);
            let service = server.serve(stdio()).await?;
            service.waiting().await?;
        }
        Commands::Index { project } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::from_default_env()
                        .add_directive(tracing::Level::INFO.into()),
                )
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .init();

            let project_path = std::fs::canonicalize(&project)?;
            tracing::info!("Indexing project: {}", project_path.display());
            let stats = indexer::pipeline::index_project(&project_path)?;
            tracing::info!("{}", stats);
        }
        Commands::Summary { file, mode } => {
            let path = std::fs::canonicalize(&file)?;
            let result = tools::get_file_context::read_file_context(&path, &mode, 3000)?;
            print!("{}", result);
        }
        Commands::Search { query, project, max_results } => {
            let project_path = std::fs::canonicalize(&project)?;
            let result = tools::search_codebase::search(&project_path, &query, max_results, 4000)?;
            print!("{}", result);
        }
        Commands::Remember { content, category, project } => {
            let project_path = std::fs::canonicalize(&project)?;
            memory::store::save(&project_path, &content, &category, "")?;
            println!("Memory saved.");
        }
        Commands::Recall { query, project } => {
            let project_path = std::fs::canonicalize(&project)?;
            let result = memory::searcher::recall(&project_path, &query, None, 5)?;
            print!("{}", result);
        }
        Commands::List {
            project,
            path,
            depth,
        } => {
            let project_path = std::fs::canonicalize(&project)?;
            let base = path
                .map(|p| project_path.join(p))
                .unwrap_or(project_path);
            let tree = tools::list_files::build_file_tree(&base, depth)?;
            print!("{}", tree);
        }
    }

    Ok(())
}
