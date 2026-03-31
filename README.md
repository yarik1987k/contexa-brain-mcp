# Context Brain

Intelligent MCP context manager for AI coding assistants. Sits between your IDE and Claude/GPT to reduce token usage and remember context across sessions.

**~5,200 lines of Rust** | **34 tests** | **0 unsafe code** | **0 compiler warnings**

## What It Does

- **Smart file reading** — AST-based symbol extraction sends signatures instead of full files; query-aware mode includes full code only for relevant functions
- **Semantic search** — local embeddings (multilingual-e5-small, supports Hebrew and 100+ languages) combined with keyword matching, word-boundary awareness, and import-centrality ranking
- **Persistent memory** — saves decisions across sessions with semantic recall (cosine similarity + keyword + recency scoring)
- **File watching** — debounced filesystem watcher automatically re-indexes changed files in the background via incremental indexing
- **Import-centrality ranking** — files imported by many others are boosted in search results
- **Works with** Cursor, Claude Code, and any MCP-compatible editor

## Quick Setup

### 1. Build from source

```bash
# Requires Rust 1.75+ (https://rustup.rs)
git clone <repo-url> context-brain
cd context-brain
cargo build --release
```

The binary is at `./target/release/context-brain`.

> **First run:** The embedding model (~90MB, multilingual-e5-small) downloads automatically on first use. Subsequent runs are instant. If the model fails to load, the server still works with keyword-only search and displays a warning in results.

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
# Start MCP server (also starts file watcher)
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

1. Walk project files (respects .gitignore via `ignore` crate)
2. Content-hash diffing — skip unchanged files
3. Extract symbols via tree-sitter AST parsing (JS/TS, Python, Rust, Go, C/C++)
4. Batch-generate embeddings using FastEmbed (local, no API calls, 60s timeout)
5. Compress embeddings with TurboQuant (2-bit quantization, ~90% size reduction)
6. Build import graph for centrality ranking
7. Store in SQLite with FTS5 full-text search

### File watching

The server watches the project directory using the `notify` crate. Events are debounced (500ms via `tokio::time::sleep`) to handle rapid editor saves. Changed files are incrementally re-indexed without rebuilding the full index.

### Search scoring

Results are ranked by combining multiple signals (constants defined in `scoring.rs`):

| Signal | Max score | Description |
|---|---|---|
| Exact name match | 5.0 | Symbol name equals query |
| Substring match | 3.0 | Query is part of symbol name (word-boundary aware) |
| Embedding similarity | 5.0 | Cosine similarity via TurboQuant compressed vectors |
| Path match | 4.0 | Query found in file path |
| Import centrality | 2.0 | Logarithmic boost for hub files (imported by many others) |

Word-boundary matching prevents false positives — "get" matches `getUser` and `get_data` but not `target` or `budget`.

### Memory recall scoring

| Signal | Weight | Description |
|---|---|---|
| Semantic similarity | 0.7 | Embedding cosine distance (TurboQuant fast path) |
| Keyword match | 0.2 | Full-text or partial word matching |
| Recency | 0.1 | Newer memories ranked higher |

### Language support

| Feature | Languages |
|---|---|
| AST symbol extraction | JavaScript, TypeScript, Python, Rust, Go, C, C++ |
| Import graph analysis | JS/TS (import/require/export), Python (import/from), Rust (use/mod), Go (import), C/C++ (#include) |
| Keyword search | All text files |

### Embedding model

Uses **multilingual-e5-small** (384 dimensions, ~90MB) via FastEmbed. Runs locally with zero API calls. Supports 100+ languages including Hebrew, making it suitable for codebases with non-English comments and documentation.

If the model fails to load, the server degrades gracefully to keyword-only search and displays a warning in results.

### TurboQuant compression

Custom 2-bit vector quantization that reduces embedding storage by ~90%:

1. Random rotation via Fast Walsh-Hadamard Transform
2. Lloyd-Max optimal Gaussian codebooks for quantization
3. Packed bit storage (96 bytes per 384-dim vector vs 1,536 bytes raw)

Search uses `fast_cosine_similarity` directly on compressed vectors — no decompression needed.

## Architecture

```
src/
  server.rs              # MCP server, auto-indexing, file watcher startup
  lib.rs                 # Library exports for testing
  main.rs                # CLI entry point (serve, index, search, etc.)
  tools/                 # MCP tool implementations
    search_codebase/     # Split into indexed.rs, live.rs, types.rs
    get_file_context.rs  # Smart file reading (full/summary/smart/symbols)
    get_symbol.rs        # Extract specific function/class by name
    get_architecture.rs  # Project overview generation
    list_files.rs        # Directory tree with metadata
  indexer/               # Code indexing pipeline
    pipeline.rs          # Full + incremental indexing (shared embed_and_store)
    symbol_extractor.rs  # Tree-sitter AST parsing (data-driven rules)
    import_extractor.rs  # Import graph extraction (comment-aware)
    embedding_client.rs  # FastEmbed provider trait + TurboQuant integration
    watch_manager.rs     # Async file watcher with debouncing
    file_walker.rs       # .gitignore-aware file discovery
    config.rs            # Centralized extension lists and skip dirs
  context/               # Context optimization
    file_summarizer.rs   # Query-aware AST summarization
    relevance_scorer.rs  # Symbol scoring + word boundary matching
    token_estimator.rs   # Token counting heuristics
    scoring.rs           # All scoring constants in one place
  memory/                # Persistent cross-session memory
    store.rs             # Save with embeddings + TurboQuant compression
    searcher.rs          # Semantic + keyword + recency recall
  db/schema.rs           # SQLite schema, migrations, blob serialization
  turboquant/            # Embedding compression engine
    mod.rs               # Quantize, fast similarity, prepare query
    codebooks.rs         # Lloyd-Max Gaussian codebooks + bit packing
    rotation.rs          # FWHT random rotation
    qjl.rs               # Quantized Johnson-Lindenstrauss projection
```

### Key design decisions

- **All blocking I/O in `spawn_blocking`** — MCP tool handlers are async but all file reads, DB queries, and embedding calls run in the blocking thread pool
- **Embedding timeout** — 60s timeout via thread-spawn-with-channel pattern (MutexGuard can't cross thread boundaries, so the spawned thread acquires the lock itself)
- **EmbeddingProvider trait** — abstracts the embedding model for testability; production uses `FastEmbedProvider`, tests can mock
- **Data-driven symbol extraction** — language rules defined as const tables, reducing boilerplate across 6 language extractors
- **Single source of truth for extensions** — `config.rs` defines `AST_EXTENSIONS`, `SOURCE_EXTENSIONS`, `SKIP_DIRS`; no hardcoded lists elsewhere

## Running Tests

```bash
cargo test        # 34 tests
cargo check       # 0 warnings
cargo clippy      # style warnings only, no correctness issues
```

### Test coverage

| Area | Tests | Notes |
|---|---|---|
| Symbol extraction | 7 | JS, TS, Python, Rust, Go + edge cases |
| Import extraction | 5 | JS, Python, Rust, C + comment skipping |
| Import resolution | 2 | Path resolution + count accumulation |
| Bit packing | 4 | Round-trip for 2/3/4-bit + invalid input |
| TurboQuant | 4 | Quantize/dequantize, similarity, compression, zero vector |
| Scoring | 4 | Constants sanity, exact match, substring match, word boundaries |
| Word boundaries | 2 | False positive prevention, camelCase/snake_case |
| Token estimation | 4 | Empty, single word, code line, budget check |
| Misc | 2 | Unicode safety, model availability |

### Known gaps

- No end-to-end tests (index a project, search it, verify results)
- No tests that touch the database or MCP server
- Import extraction is string-based (doesn't handle imports inside string literals or multi-line imports)

## Requirements

- Rust 1.75+ (for building)
- macOS or Linux
- ~90MB disk for embedding model (downloaded on first run)
- No external API calls at runtime — everything runs locally

## License

MIT
