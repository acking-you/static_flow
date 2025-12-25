# StaticFlow

[中文文档](./README.zh.md)

A local-first dynamic blog system. Write Markdown locally, auto-generate summaries and tags via Claude Code/Codex skills, store in LanceDB via CLI tools, full-stack Rust.

## Philosophy

> **"Don't build agents, build skills instead."**
> — [Anthropic Expert Talk](https://www.youtube.com/watch?v=CEvIs9y1uog)

This project does not build a standalone AI Agent.

The reality: the capability gap between mainstream agent tools (Claude Code, Codex, Cursor) and other solutions is enormous. Even now, when agent technology is rapidly evolving, the performance differences between products are staggering. Building a proprietary agent with Rig/LangChain—no matter how much effort you invest—will never catch up with the iteration speed of mainstream tools.

The right approach: **Build Skills, Not Agents**.

AI automation strategy:
- **Intelligence**: Delegate to Claude Code/Codex, describe workflows via skills
- **Tooling**: Build simple CLI tools for LanceDB read/write operations

Benefits:
1. Zero agent development/maintenance cost
2. Automatically benefit from mainstream agent upgrades
3. CLI stays simple, focused on data operations

## Architecture

```
static-flow/
├── frontend/     # Yew WASM frontend
├── backend/      # Axum backend (LanceDB query layer)
├── shared/       # Shared types
├── cli/          # LanceDB CLI tools (simple read/write)
└── content/      # Local Markdown and images
```

## Tech Stack

| Module | Technology |
|--------|------------|
| Frontend | Yew 0.21 + WASM + TailwindCSS v4 |
| Backend | Axum 0.7 + LanceDB |
| CLI | clap + LanceDB Rust SDK |
| Shared | serde + shared data models |

## Quick Start

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown
cargo install trunk

# Start backend
cd backend && cargo run

# Start frontend (another terminal)
cd frontend && trunk serve --open
```

Backend: `http://localhost:3000` | Frontend: `http://localhost:8080`

## CLI Tools

```bash
cd cli

# Initialize LanceDB
cargo run -- init --db-path ./data/lancedb

# Write article (with Claude Code generated metadata)
cargo run -- write-article \
  --file ../content/post.md \
  --summary "Article summary" \
  --tags "rust,wasm" \
  --category "Tech"

# Batch write images
cargo run -- write-images --dir ../content/images

# Query verification
cargo run -- query --table articles --limit 10
```

## Claude Code Workflow

Process new articles with Claude Code/Codex:

1. Read Markdown file content
2. AI generates summary (100-200 words), tags (3-5), category
3. Call CLI tool to write to LanceDB

Example prompt:
```
Read content/new-post.md, generate summary and tags, then call sf-cli write-article to write to database
```

## API

| Endpoint | Description |
|----------|-------------|
| `GET /api/articles` | Article list (supports tag/category filter) |
| `GET /api/articles/:id` | Article detail |
| `GET /api/search?q=` | Full-text search |
| `GET /api/semantic-search?q=` | Semantic search (vector) |
| `GET /api/image/:id` | Image service |
| `GET /api/tags` | Tag list |
| `GET /api/categories` | Category list |

## Roadmap

### Phase 1: CLI Development
- [ ] LanceDB schema design and initialization
- [ ] write-article command
- [ ] write-images command
- [ ] query command

### Phase 2: Backend LanceDB Integration
- [ ] Remove filesystem implementation
- [ ] Integrate LanceDB Rust SDK
- [ ] Refactor all API endpoints
- [ ] Implement vector search

### Phase 3: Feature Completion
- [ ] Semantic search UI
- [ ] Image-to-image search
- [ ] Related articles recommendation

## Development Commands

```bash
# Workspace commands
cargo build --workspace      # Build all crates
cargo test --workspace       # Run tests
cargo fmt --all              # Format code
cargo clippy --workspace     # Lint check

# Frontend
cd frontend && trunk serve   # Dev mode
cd frontend && trunk build --release  # Production build

# Backend
cd backend && cargo run      # Dev mode
cd backend && cargo run --release     # Production mode
```

## License

MIT
