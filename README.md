# Context Brain

Intelligent MCP context manager for AI coding assistants. Sits between your IDE and Claude/GPT to reduce token usage and remember context across sessions.

## What It Does

- **Smart file reading** — sends function signatures instead of full files, query-aware mode includes full code only for relevant functions
- **Semantic search** — finds code by meaning using local embeddings, combined with keyword matching and import-centrality ranking
- **Persistent memory** — saves decisions across sessions with semantic recall, never re-explain your architecture
- **File watching** — automatically re-indexes changed files in the background, keeping search results fresh
- **Works with** Cursor, Claude Code, and any MCP-compatible editor

## Quick Setup (2 minutes)

### 1. Build from source

```bash
# Requires Rust 1.75+ (https://rustup.rs)
git clone <repo-url> context-brain
cd context-brain
cargo build --release
```

The binary is at `./target/release/context-brain`.

> **First run note:** The embedding model (~90MB, multilingual-e5-small) downloads automatically on first use. Subsequent runs are instant.

### 2. Add to your editor

**For Cursor** — create `.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "context-brain": {
      "command": "/path/to/context-brain",
      "args": ["serve", "--project", "."]
    }
  }
}
```

**For Claude Code** — create `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "context-brain": {
      "command": "/path/to/context-brain",
      "args": ["serve", "--project", "."]
    }
  }
}
```

### 3. Restart your editor

You should see `context-brain` in your MCP settings with 8 tools enabled.

## Tools

| Tool | What it does |
|---|---|
| `search_codebase` | Semantic + keyword search with import-centrality ranking |
| `get_file_context` | Read files in full/summary/smart/symbols mode |
| `get_symbol` | Extract a specific function/class by name |
| `save_memory` | Save a decision or insight for later |
| `recall_memory` | Recall past decisions semantically |
| `get_architecture` | Get project overview in ~500 tokens |
| `list_files` | Directory tree with file sizes |
| `index_project` | Build/rebuild the search index |

### Smart file reading modes

| Mode | When to use | Token savings |
|---|---|---|
| `full` | Need entire file | None |
| `summary` | Exploring structure | ~60-80% |
| `smart` | Targeted work (pass a query) | ~70-90% |
| `symbols` | Quick symbol list | ~90%+ |

## CLI Usage

```bash
# Start MCP server
context-brain serve --project /path/to/project

# Build search index
context-brain index --project .

# List project files
context-brain list --project .

# Search for code
context-brain search --query "authentication" --project .

# Get file summary
context-brain summary --file src/main.rs --mode summary

# Save a memory
context-brain remember --content "We use JWT for auth" --category decision --project .

# Recall memories
context-brain recall --query "authentication" --project .
```

## How It Works

### Indexing pipeline

1. Walk project files (respects .gitignore)
2. Extract symbols via tree-sitter AST parsing (JS/TS, Python, Rust, Go, C/C++)
3. Generate embeddings using FastEmbed (local, no API calls)
4. Compress embeddings with TurboQuant (2-bit quantization, 90% size reduction)
5. Build import graph for centrality ranking
6. Store in SQLite with FTS5 full-text search

### File watching

The server watches the project directory for changes and automatically re-indexes modified files. No manual re-indexing needed during development.

### Search scoring

Results are ranked by combining multiple signals:

| Signal | Max score | Description |
|---|---|---|
| Exact name match | 5.0 | Symbol name equals query |
| Substring match | 3.0 | Query is part of symbol name |
| Embedding similarity | 5.0 | Semantic similarity via cosine distance |
| Path match | 4.0 | Query found in file path |
| Import centrality | 2.0 | Files imported by many others rank higher |

### Language support

| Feature | Languages |
|---|---|
| AST symbol extraction | JavaScript, TypeScript, Python, Rust, Go, C, C++ |
| Import graph analysis | JS/TS (import/require), Python, Rust, Go, C/C++ |
| Keyword search | All text files |

## Architecture

```
src/
  server.rs           # MCP server + file watcher integration
  tools/              # MCP tool implementations
    search_codebase/   # Semantic + keyword search (indexed & live)
    get_file_context.rs
    get_symbol.rs
    get_architecture.rs
    list_files.rs
  indexer/             # Code indexing pipeline
    pipeline.rs        # Full + incremental indexing
    symbol_extractor.rs # Tree-sitter AST parsing
    import_extractor.rs # Import graph extraction
    embedding_client.rs # FastEmbed + TurboQuant
    watch_manager.rs   # File system watcher
    file_walker.rs     # .gitignore-aware file discovery
  context/             # Context optimization
    file_summarizer.rs # Smart query-aware summarization
    relevance_scorer.rs # Symbol relevance scoring
    token_estimator.rs # Token counting heuristics
    scoring.rs         # Centralized scoring constants
  memory/              # Persistent memory
    store.rs           # Save with embeddings
    searcher.rs        # Semantic recall
  db/schema.rs         # SQLite schema + migrations
  turboquant/          # Embedding compression (2-bit quantization)
```

## Running Tests

```bash
cargo test
```

27 tests covering symbol extraction (5 languages), import extraction, bit-packing round-trips, TurboQuant quantization, and scoring.

## Requirements

- Rust 1.75+ (for building)
- macOS or Linux
- No external dependencies at runtime (embeddings run locally, SQLite bundled)

## License

MIT
