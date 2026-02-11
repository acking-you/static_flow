# StaticFlow

[中文文档](./README.zh.md)

[CLI Guide (ZH)](./docs/cli-user-guide.zh.md)

A local-first dynamic blog system. Run backend locally, expose secure API via local Nginx + pb-mapper, and write Markdown notes plus images into LanceDB through CLI.

## Philosophy

> **"Don't build agents, build skills instead."**
> — [Anthropic Expert Talk](https://www.youtube.com/watch?v=CEvIs9y1uog)

This project does not build a standalone AI Agent.

AI automation strategy:
- **Intelligence**: Delegate to Claude Code/Codex and describe workflows via skills
- **Tooling**: Keep CLI simple, only for LanceDB read/write

## Architecture

```text
static-flow/
├── frontend/     # Yew WASM frontend
├── backend/      # Axum backend (LanceDB query layer)
├── shared/       # Shared types
├── cli/          # LanceDB CLI tools
└── content/      # Sample local markdown + images
```

## Deployment Topology (Recommended)

1. Run `backend` on local machine (`127.0.0.1:3000`).
2. Put local Nginx in front of backend for local HTTPS (`127.0.0.1:3443`).
3. Use `pb-mapper` to map local `127.0.0.1:3443` to a cloud endpoint (for example `https://<cloud-host>:8888`).
4. Frontend (already loaded in browser) directly calls that cloud HTTPS endpoint as API.
5. Optional: add cloud Nginx on `443` for domain/cert management and reverse-proxy to pb-mapper local port.

Main request chain (frontend fetch perspective):

```text
Frontend(fetch/XHR)
  -> https://<cloud-host>:8888/api
  -> pb-mapper tunnel
  -> Local Nginx https://127.0.0.1:3443
  -> Local backend http://127.0.0.1:3000
```

Reference configs:
- Local Nginx HTTPS: `deployment-examples/nginx-staticflow-api.conf`
- Optional cloud Nginx HTTPS proxy: `deployment-examples/nginx-staticflow-cloud-proxy.conf`

## Quick Start

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown
cargo install trunk

# Build binaries
make bin-all

# Initialize LanceDB tables
cd cli
../target/release/sf-cli init --db-path ../data/lancedb

# Start backend
cd ../backend
LANCEDB_URI=../data/lancedb ../target/release/static-flow-backend

# Start frontend (another terminal)
cd ../frontend
trunk serve --open
```

Backend: `http://localhost:3000` | Frontend: `http://localhost:8080`

## CLI Tools

```bash
cd cli

# Build CLI binary
make bin-cli

# Run full CLI E2E checks (docs + images + CRUD + API)
cd ..
./scripts/test_cli_e2e.sh
# or: BUILD_PROFILE=release ./scripts/test_cli_e2e.sh
cd cli

# Initialize LanceDB
../target/release/sf-cli init --db-path ../data/lancedb

# Manually ensure all expected indexes (useful after bulk imports)
# - articles.content (FTS)
# - articles.vector_en / articles.vector_zh (vector)
# - images.vector (vector)
# - taxonomies table stores category/tag metadata (no vector index)
../target/release/sf-cli ensure-indexes --db-path ../data/lancedb

# Write single article
../target/release/sf-cli write-article \
  --db-path ../data/lancedb \
  --file ../content/post-001.md \
  --summary "Article summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes about Rust + WASM"

# Optional in markdown frontmatter for sync/write
# category_description: "Engineering notes about Rust + WASM"

# Batch write images
../target/release/sf-cli write-images \
  --db-path ../data/lancedb \
  --dir ../content/images \
  --recursive \
  --generate-thumbnail

# Thumbnail implementation details
# - Generated only with --generate-thumbnail; size controlled by --thumbnail-size (default 256)
# - Stored as PNG bytes in images.thumbnail
# - GET /api/images/:id-or-filename?thumb=true prefers thumbnail and falls back to original data

# Sync a local notes folder (markdown + image files)
# - Auto imports referenced local images into `images` table
# - Rewrites markdown image links to `images/<sha256_id>`
# - Upserts article records into `articles` table
# - Upserts category/tag metadata into `taxonomies` table
../target/release/sf-cli sync-notes \
  --db-path ../data/lancedb \
  --dir ../content \
  --recursive \
  --generate-thumbnail

# Query verification
../target/release/sf-cli query --db-path ../data/lancedb --table articles --limit 10
../target/release/sf-cli query --db-path ../data/lancedb --table articles --limit 1 --format vertical

# Database-style management (CRUD + index)
../target/release/sf-cli db --db-path ../data/lancedb list-tables
../target/release/sf-cli db --db-path ../data/lancedb describe-table articles
../target/release/sf-cli db --db-path ../data/lancedb query-rows articles --where "category='Tech'" --columns id,title,date --limit 5
../target/release/sf-cli db --db-path ../data/lancedb query-rows articles --limit 1 --format vertical
../target/release/sf-cli db --db-path ../data/lancedb count-rows articles --where "vector_en IS NOT NULL"
../target/release/sf-cli db --db-path ../data/lancedb update-rows articles --set "category='Notes'" --where "id='post-001'"
../target/release/sf-cli db --db-path ../data/lancedb delete-rows articles --where "id='draft-001'"
../target/release/sf-cli db --db-path ../data/lancedb list-indexes articles --with-stats
../target/release/sf-cli db --db-path ../data/lancedb ensure-indexes
../target/release/sf-cli db --db-path ../data/lancedb optimize articles

# Managed tables
# - articles: article body/metadata + vectors
# - images: binary image data + vectors
# - taxonomies: category/tag metadata (`kind`, `key`, `name`, `description`)

# Backend-like API debug commands
../target/release/sf-cli api --db-path ../data/lancedb list-articles --category "Tech"
../target/release/sf-cli api --db-path ../data/lancedb get-article frontend-architecture
../target/release/sf-cli api --db-path ../data/lancedb search --q "staticflow"
../target/release/sf-cli api --db-path ../data/lancedb semantic-search --q "前端 架构"
../target/release/sf-cli api --db-path ../data/lancedb related-articles frontend-architecture
../target/release/sf-cli api --db-path ../data/lancedb list-tags
../target/release/sf-cli api --db-path ../data/lancedb list-categories
../target/release/sf-cli api --db-path ../data/lancedb list-images
../target/release/sf-cli api --db-path ../data/lancedb search-images --id <image_id>
../target/release/sf-cli api --db-path ../data/lancedb get-image <image_id_or_filename> --thumb --out ./tmp-thumb.bin
```

## API

| Endpoint | Description |
|----------|-------------|
| `GET /api/articles` | Article list (supports tag/category filter) |
| `GET /api/articles/:id` | Article detail |
| `GET /api/articles/:id/related` | Related articles (vector similarity) |
| `GET /api/search?q=` | Full-text search |
| `GET /api/semantic-search?q=` | Semantic search (vector) |
| `GET /api/images` | Image catalog |
| `GET /api/images/:id-or-filename` | Read image binary from LanceDB (`?thumb=true`, fallback to original if thumbnail missing) |
| `GET /api/image-search?id=` | Similar images |
| `GET /api/tags` | Tag list |
| `GET /api/categories` | Category list |

## Key Env Vars

Backend (`backend/.env`):
- `LANCEDB_URI` (default `../data/lancedb`)
- `PORT` (default `3000`)
- `BIND_ADDR` (dev: `0.0.0.0`, production: `127.0.0.1`)
- `RUST_ENV` (`development` or `production`)
- `ALLOWED_ORIGINS` (optional comma-separated CORS list in production)

Frontend build-time:
- `STATICFLOW_API_BASE` (direct pb-mapper endpoint, e.g. `https://<cloud-host>:8888/api`)
- If using cloud Nginx proxy, set it to your domain (e.g. `https://api.yourdomain.com/api`)

## Development Commands

```bash
# Workspace commands
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings

# Frontend
cd frontend && trunk serve
cd frontend && trunk build --release

# Backend
make bin-backend
cd backend && ../target/release/static-flow-backend
cd backend && RUST_ENV=production BIND_ADDR=127.0.0.1 ../target/release/static-flow-backend
```

## License

MIT
