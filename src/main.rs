mod server;
mod tools;
mod indexer;
mod memory;
mod context;
mod db;
mod turboquant;
mod telemetry;

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
        /// Detail level: pointers, signatures, code
        #[arg(short, long, default_value = "signatures")]
        detail: String,
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
    /// Show local telemetry: tool-call counts, latencies, empty-result rates.
    /// All data is stored on-device in the per-project SQLite DB.
    Stats {
        /// Project path
        #[arg(short, long, default_value = ".")]
        project: String,
        /// Window in days (omit for all-time)
        #[arg(short, long)]
        days: Option<u32>,
    },
    /// Memory hygiene tools (review near-duplicate / contradicting memories).
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
}

#[derive(clap::Subcommand, Debug)]
enum MemoryAction {
    /// Surface candidate-contradiction pairs for manual review.
    Review {
        /// Project path
        #[arg(short, long, default_value = ".")]
        project: String,
        /// Max pairs to display
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
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
            let result = tools::get_file_context::read_file_context(&path, &mode, 3000, None)?;
            print!("{}", result);
        }
        Commands::Search { query, project, max_results, detail } => {
            let project_path = std::fs::canonicalize(&project)?;
            let result = tools::search_codebase::search(&project_path, &query, max_results, 4000, &detail)?;
            print!("{}", result);
        }
        Commands::Remember { content, category, project } => {
            let project_path = std::fs::canonicalize(&project)?;
            let outcome = memory::store::save(&project_path, &content, &category, "")?;
            match outcome {
                memory::store::SaveOutcome::Inserted(id) => println!("Memory saved (id {}).", id),
                memory::store::SaveOutcome::Merged(id) => {
                    println!("Near-duplicate found — merged into existing memory id {}.", id);
                }
                memory::store::SaveOutcome::Linked { new_id, peer_id } => {
                    println!(
                        "Cross-category duplicate of memory id {} — saved as id {} with link.",
                        peer_id, new_id
                    );
                }
            }
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
            let base = match path {
                Some(p) => {
                    let joined = project_path.join(&p);
                    let resolved = std::fs::canonicalize(&joined)
                        .unwrap_or(joined);
                    if !resolved.starts_with(&project_path) {
                        anyhow::bail!("Path escapes project directory");
                    }
                    resolved
                }
                None => project_path,
            };
            let tree = tools::list_files::build_file_tree(&base, depth)?;
            print!("{}", tree);
        }
        Commands::Stats { project, days } => {
            let project_path = std::fs::canonicalize(&project)?;
            let report = tools::stats::render(&project_path, days)?;
            print!("{}", report);
        }
        Commands::Memory { action } => match action {
            MemoryAction::Review { project, limit } => {
                let project_path = std::fs::canonicalize(&project)?;
                let pairs = memory::hygiene::find_contradictions(&project_path, limit)?;
                let report = memory::hygiene::render_review(&pairs);
                print!("{}", report);
            }
        },
    }

    Ok(())
}
