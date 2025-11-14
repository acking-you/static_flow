# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**StaticFlow** is a local-first, automation-driven blog system built entirely in Rust. It combines a Yew WebAssembly frontend, Axum backend API, Meilisearch full-text search, and a CLI tool for automated content synchronization from local Markdown files (Obsidian/Typora).

**Tech Stack:**
- Frontend: Yew 0.21 + WebAssembly (compiled via Trunk)
- Backend: Axum 0.7 + SQLite + Meilisearch 1.5+
- CLI: Rust + notify (file watching) + reqwest
- Shared: Common data models between frontend/backend

**Current Status:** Project is in planning phase (Week 0). No code exists yet, only architectural design in README.md.

## Architecture

### Multi-Crate Workspace Structure
The project is designed as a Cargo workspace with 4 main crates:

1. **frontend/** - Yew WASM application
   - Components-based architecture (components/, pages/)
   - Client-side routing via yew-router
   - API calls via gloo-net
   - Markdown rendering via pulldown-cmark
   - TailwindCSS for styling

2. **backend/** - Axum REST API server
   - API routes: articles, search, resources (images)
   - Services: Meilisearch integration, Markdown processing
   - Middleware: auth (signature-based), rate limiting
   - Data: SQLite for metadata, Meilisearch for search

3. **cli-tool/** - Local content management CLI
   - File watcher for Markdown changes
   - Image path mapping (filename → full path)
   - Content processor (frontmatter extraction, AI metadata generation)
   - Sync logic to backend API + Meilisearch

4. **shared/** - Shared types and utilities
   - Data models used by both frontend and backend
   - Ensures type safety across the stack

### Key Design Principles

**Local-First Content Creation:**
- Users write in Obsidian/Typora (local Markdown files)
- CLI tool watches directory, auto-syncs changes
- No online editor needed

**AI-Driven Automation:**
- Auto-generate article summaries via LLM
- Auto-extract tags and categories
- Configurable AI provider (OpenAI or local Ollama)

**Image Path Mapping:**
- Local images referenced in Markdown (e.g., `![](images/foo.png)`)
- CLI builds filename → absolute path mapping
- Backend serves images via `/api/image/:base64_filename`
- Frontend transforms Markdown links to API endpoints

**Dynamic Serving:**
- Backend renders Markdown to HTML on-demand (not pre-built)
- Meilisearch provides instant full-text search
- API-driven architecture allows flexible frontend updates

## Development Commands

### Prerequisites Setup
```bash
# Install Rust toolchain
rustup install stable

# Add WASM target for frontend
rustup target add wasm32-unknown-unknown

# Install Trunk (WASM bundler)
cargo install trunk

# Install Meilisearch (macOS example)
brew install meilisearch

# Or use Docker
docker run -d --name meilisearch -p 7700:7700 -v $(pwd)/data/meili_data:/meili_data getmeili/meilisearch:v1.5
```

### Backend Development
```bash
cd backend

# Copy environment config
cp .env.example .env

# Initialize database schema
cargo run --bin init-db

# Run development server
cargo run

# Run with release optimizations
cargo run --release
```

Backend runs on `http://localhost:3000`

### Frontend Development
```bash
cd frontend

# Install TailwindCSS (if needed)
npm install -D tailwindcss

# Start dev server with hot reload
trunk serve

# Open browser automatically
trunk serve --open

# Custom port
trunk serve --port 8888

# Production build
trunk build --release
```

Frontend dev server runs on `http://localhost:8080`

### CLI Tool
```bash
cd cli-tool

# Build CLI
cargo build --release

# Initialize configuration
./target/release/static-flow-cli init

# One-time sync of Markdown directory
./target/release/static-flow-cli sync ~/my-blog-posts

# Watch directory for real-time sync
./target/release/static-flow-cli watch ~/my-blog-posts
```

### Workspace Commands
```bash
# Build all crates
cargo build --workspace

# Run tests across all crates
cargo test --workspace

# Check without building
cargo check --workspace

# Format all code
cargo fmt --all

# Lint all code
cargo clippy --workspace -- -D warnings
```

### WASM Optimization
For production frontend builds, optimize WASM size:
```bash
# Already configured in profile.release:
# opt-level = "z", lto = true, codegen-units = 1, panic = "abort"

# Further optimize with wasm-opt (install via binaryen)
wasm-opt -Oz -o dist/optimized.wasm dist/output.wasm
```

### Meilisearch Debugging
```bash
# List all indexes
curl http://localhost:7700/indexes

# Check articles index stats
curl http://localhost:7700/indexes/articles/stats

# Manual search test
curl "http://localhost:7700/indexes/articles/search?q=rust"

# Set master key (if needed)
export MEILI_MASTER_KEY=your_master_key_here
```

## API Design

### RESTful Endpoints
```
GET  /api/articles              # List articles (pagination, filters)
GET  /api/articles/:id          # Get article detail (rendered HTML)
GET  /api/search?q=keyword      # Full-text search via Meilisearch
GET  /api/tags                  # List all tags
GET  /api/categories            # List all categories
GET  /api/image/:base64_filename # Serve image by filename
```

### Data Models (Shared)
Key types to define in `shared/src/models.rs`:
- `Article` - id, title, summary, content_path, tags, category, timestamps
- `ArticleListItem` - lightweight version for list views
- `Tag` - name, count
- `Category` - name, count
- `SearchResult` - article hit with highlighted snippets

## Development Workflow

### Phase 1: MVP Frontend (Week 1)
1. Replicate old blog UI using Yew components
2. Use mock data for all content
3. Implement responsive design (desktop + mobile)
4. Dark/light theme toggle
5. Markdown rendering with syntax highlighting

### Phase 2: Backend + CLI Integration (Week 2)
1. Implement Axum API endpoints
2. SQLite schema + migrations
3. Meilisearch index configuration
4. Frontend API integration (replace mocks)
5. CLI file watcher + basic sync

### Phase 3: AI Automation (Week 3+)
1. LLM integration (OpenAI or Ollama)
2. Auto-generate summaries, tags, categories
3. Batch processing for existing articles
4. Configurable AI prompts

### Phase 4: Security & Deployment (Future)
1. Request signature authentication
2. Rate limiting (tower-governor)
3. Docker Compose setup
4. Rathole tunnel for local deployment
5. Nginx reverse proxy

## Important Notes

### Image Handling Strategy
The image path mapping is a core feature:
1. CLI scans image files, builds `filename → full_path` map
2. Map stored in backend DB (`images` table)
3. Backend serves images via base64-encoded filename route
4. Frontend Markdown renderer transforms relative paths to API URLs

Example transformation:
```markdown
<!-- In local Markdown -->
![screenshot](images/screenshot.png)

<!-- Rendered in frontend -->
<img src="http://localhost:3000/api/image/aW1hZ2VzL3NjcmVlbnNob3QucG5n">
```

### Workspace Dependencies
When adding dependencies, use workspace-level version management in root `Cargo.toml`:
```toml
[workspace]
members = ["frontend", "backend", "cli-tool", "shared"]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "1.35", features = ["full"] }
```

Then reference in crate `Cargo.toml`:
```toml
[dependencies]
serde = { workspace = true }
```

### Testing Strategy
- Unit tests for `shared` models and utilities
- Integration tests for backend API routes
- Mock API responses for frontend component tests
- E2E test: local file → CLI sync → backend → frontend display

### Configuration Files
- Backend: `.env` (Meilisearch URL, SQLite path, content directories)
- CLI: `config.toml` (watch paths, backend API URL, AI settings)
- Frontend: `Trunk.toml` (build settings, asset copying)

## Migration from Old Blog
The `old/` directory is reserved for static assets from the previous blog system. During Week 1 frontend development:
1. Extract CSS styles from `old/css/`
2. Identify UI components to replicate
3. Migrate theme colors and design tokens
4. Preserve responsive breakpoints

## Debugging Tips

**Frontend (WASM):**
- Use browser DevTools console for panics
- Enable `console_error_panic_hook` for better stack traces
- Check Network tab for API call failures

**Backend:**
- Enable `RUST_LOG=debug` for verbose logging
- Use `tokio-console` for async task inspection
- Check Meilisearch logs separately

**CLI:**
- Test file watcher with: `touch test.md` in watched directory
- Verify API connectivity: `curl http://localhost:3000/api/articles`
- Check sync status in backend SQLite DB

## Performance Considerations

**WASM Bundle Size:**
- Target <500KB gzipped for initial load
- Use `wasm-opt` in CI/CD pipeline
- Consider code splitting for large apps

**Meilisearch Indexing:**
- Batch updates when syncing multiple files
- Use async indexing to avoid blocking CLI
- Configure searchable/filterable attributes carefully

**Backend:**
- Use SQLite connection pool (sqlx)
- Cache rendered Markdown (optional, measure first)
- Implement ETag headers for static resources

## Future Extensions (Post-MVP)
- RSS feed generation
- GitHub-based commenting system
- Music player component (mentioned in README)
- Article view counters
- Reading time estimation
- Related articles recommendation
- Multi-language support
