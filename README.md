# Context Brain

Intelligent MCP context manager for AI coding assistants. Sits between your IDE and Claude/GPT to reduce token usage by 92% and remember context across sessions.

## What It Does

- **Smart file reading** — sends function signatures instead of full files (96% token savings)
- **Semantic search** — finds code by meaning, not just keywords
- **Persistent memory** — saves decisions across sessions, never re-explain your architecture
- **Works with** Cursor, Claude Code, and any MCP-compatible editor

## Quick Setup (2 minutes)

### 1. Build from source

```bash
# Requires Rust (https://rustup.rs)
git clone <repo-url> context-brain
cd context-brain
cargo build --release
```

The binary is at `./target/release/context-brain` (~40MB).

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

You should see `context-brain` in your MCP settings with 6 tools enabled.

## Tools

| Tool | What it does |
|---|---|
| `search_codebase` | Find code by meaning or keywords |
| `get_file_context` | Read files in full/summary/symbols mode |
| `save_memory` | Save a decision or insight for later |
| `recall_memory` | Recall past decisions semantically |
| `get_architecture` | Get project overview in ~500 tokens |
| `list_files` | Directory tree with file sizes |

## CLI Usage

You can also use Context Brain from the terminal:

```bash
# List project files
context-brain list --project /path/to/your/project

# Search for code
context-brain search --query "authentication" --project .

# Get file summary (96% fewer tokens than full file)
context-brain summary --file src/main.rs --mode summary

# Save a memory
context-brain remember --content "We use JWT for auth" --category decision --project .

# Recall memories
context-brain recall --query "authentication" --project .
```

## Run the Test Script

```bash
# Run the included test to verify everything works
chmod +x tests/test_context_brain.sh
./tests/test_context_brain.sh /path/to/your/project
```

## Benchmarks

Tested on a real 800-file Node.js/React codebase:

| Metric | Without CB | With CB | Savings |
|---|---|---|---|
| Tokens per task | 35,573 | 2,617 | 92% |
| Tokens over 100 sessions (memory) | 2,698,000 | 58,462 | 97% |
| Monthly cost (Opus, 20 tasks/day) | $213 | $16 | $197/mo |

## Requirements

- Rust 1.75+ (for building)
- macOS or Linux
- No other dependencies (embeddings run locally, SQLite bundled)
