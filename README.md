# StaticFlow

[中文文档](./README.zh.md)

[CLI Guide (ZH)](./docs/cli-user-guide.zh.md)

A local-first dynamic blog system. Run backend locally behind the Pingora gateway, expose public HTTPS through cloud Caddy + pb-mapper, and write Markdown notes plus images into LanceDB through CLI.

StaticFlow also includes a public LLM access layer on top of the content system: an OpenAI-compatible Codex gateway, an Anthropic-compatible Kiro gateway, provider-scoped upstream proxy routing, quota-managed gateway keys, and usage accounting. For the current implementation details, see [docs/llm-access-and-kiro-gateway-implementation.md](./docs/llm-access-and-kiro-gateway-implementation.md).

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

## Data Repository (Hugging Face)

`static-flow` website: <https://acking-you.github.io/>

This project keeps runtime data in **two** Hugging Face dataset repos plus one
local-only music DB:

Canonical local data root:
- `/mnt/wsl/data4tb/static-flow-data`

- Content DB (content + request + interactive mirror + llm gateway tables)
  - HF dataset repo: <https://huggingface.co/datasets/LB7666/my_lancedb_data>
  - Remote: `git@hf.co:datasets/LB7666/my_lancedb_data`
  - Local path: `/mnt/wsl/data4tb/static-flow-data/lancedb`
  - Tables: `articles`, `images`, `taxonomies`, `article_views`, `api_behavior_events`, `article_requests`, `article_request_ai_runs`, `article_request_ai_run_chunks`, `interactive_pages`, `interactive_page_locales`, `interactive_assets`, `llm_gateway_keys`, `llm_gateway_usage_events`, `llm_gateway_runtime_config`, `llm_gateway_proxy_configs`, `llm_gateway_proxy_bindings`, `llm_gateway_token_requests`, `llm_gateway_account_contribution_requests`, `llm_gateway_sponsor_requests`
- Comments DB (comment moderation + AI run traces)
  - HF dataset repo: <https://huggingface.co/datasets/LB7666/static-flow-comments>
  - Remote: `git@hf.co:datasets/LB7666/static-flow-comments`
  - Local path: `/mnt/wsl/data4tb/static-flow-data/lancedb-comments`
  - Tables: `comment_tasks`, `comment_published`, `comment_audit_logs`, `comment_ai_runs`, `comment_ai_run_chunks`
- Music DB (local-first media store; not mirrored to HF by default)
  - Local path: `/mnt/wsl/data4tb/static-flow-data/lancedb-music`
  - Tables: `songs`, `music_plays`, `music_comments`, `music_wishes`, `music_wish_ai_runs`, `music_wish_ai_run_chunks`

Recommended workflow:

1. Write/update data via `sf-cli` (for schema/index consistency).
2. Sync dataset changes with Git commits.
3. Push to Hugging Face dataset remote.

Quick setup (SSH + Git Xet):

```bash
# 1) Prepare SSH key and add it to https://huggingface.co/settings/keys
ls ~/.ssh/id_ed25519.pub || ssh-keygen -t ed25519 -C "LB7666@hf"
eval "$(ssh-agent -s)"
ssh-add ~/.ssh/id_ed25519
mkdir -p ~/.ssh
ssh-keyscan -H hf.co >> ~/.ssh/known_hosts
chmod 600 ~/.ssh/known_hosts
ssh -T git@hf.co

# 2) Bind local CONTENT DB to HF dataset remote
cd /mnt/wsl/data4tb/static-flow-data/lancedb
git init -b main
git remote remove origin 2>/dev/null || true
git remote add origin git@hf.co:datasets/LB7666/my_lancedb_data
git fetch origin main
git checkout -B main origin/main

# 3) Bind local COMMENTS DB to HF dataset remote
cd /mnt/wsl/data4tb/static-flow-data/lancedb-comments
git init -b main
git remote remove origin 2>/dev/null || true
git remote add origin git@hf.co:datasets/LB7666/static-flow-comments
git fetch origin main
git checkout -B main origin/main

# 4) Install and enable Git Xet
bash <(curl -fsSL https://raw.githubusercontent.com/huggingface/xet-core/main/git_xet/install.sh)
export PATH="$HOME/.local/bin:$PATH"
git xet install

# 5) Configure tracking rules in BOTH repos
cd /mnt/wsl/data4tb/static-flow-data/lancedb
git xet track "*.lance" "*.txn" "*.manifest" "*.idx"
cd /mnt/wsl/data4tb/static-flow-data/lancedb-comments
git xet track "*.lance" "*.txn" "*.manifest" "*.idx"

# 6) Daily sync (content DB)
cd /mnt/wsl/data4tb/static-flow-data/lancedb
git add -A
git commit -m "data(content): sync $(date '+%F %T')" || echo "no content changes"
git push origin main

# 7) Daily sync (comments DB)
cd /mnt/wsl/data4tb/static-flow-data/lancedb-comments
git add -A
git commit -m "data(comments): sync $(date '+%F %T')" || echo "no comments changes"
git push origin main
```

Note: after `git xet track`, `.gitattributes` may still show `filter=lfs`; this is expected
on Hugging Face's Xet-integrated transfer path.

## Deployment Modes

### Mode A: Self-Hosted (Recommended)

Backend serves both API and frontend static files. In current production,
public HTTPS enters through cloud Caddy and pb-mapper, then lands on the local
Pingora gateway.

```text
Browser -> https://ackingliu.top
        -> cloud Caddy (:443)
        -> cloud pb-mapper-client (127.0.0.1:39080)
        -> cloud pb-mapper-server (:7666)
        -> local tmux `pbmapper-sf-backend`
        -> local Pingora gateway (127.0.0.1:39180)
        -> active backend slot (currently green, 127.0.0.1:39081)
           ├── /api/*        → API handlers
           ├── /posts/:id    → SEO-injected page
           ├── /sitemap.xml  → Dynamic sitemap
           └── /*            → Frontend static (SPA fallback)
```

Current local production binaries supervised by tmux:

| tmux session | Binary | Role | Listen / target |
| --- | --- | --- | --- |
| `sf-gateway` | `target/release-backend/staticflow-pingora-gateway --conf conf/pingora/staticflow-gateway.yaml` | Stable local ingress. Do not stop it during routine backend hot updates. | Listens on `127.0.0.1:39180` |
| `sf-backend-green` | `scripts/start_backend_selfhosted.sh --port 39081` with `BACKEND_BIN=target/release-backend/static-flow-backend` | Active backend slot selected by Pingora. | Listens on `127.0.0.1:39081`; uses `DB_ROOT=/mnt/wsl/data4tb/static-flow-data` |
| `gpt2api-rs` | `deps/gpt2api_rs/target/release/gpt2api-rs serve` | GPT2API image gateway used by StaticFlow routes/admin. | Listens on `127.0.0.1:18787` |
| `pbmapper-sf-backend` | `~/.local/pbmapper/current/pb-mapper-server-cli ... tcp-server --key sf-backend --addr 127.0.0.1:39180` | Registers the local Pingora gateway with the cloud relay. | Connects to `ackingliu.top:7666` |
| `pbmapper-home-ubuntu` | `~/.local/pbmapper/current/pb-mapper-server-cli ... tcp-server --key home-ubuntu --addr 127.0.0.1:22` | Registers local SSH access with the cloud relay. | Connects to `ackingliu.top:7666` |

Notes:
- `conf/pingora/staticflow-gateway.yaml` is the local gateway source of truth.
  At the time of writing, `active_upstream: green` means `39180 -> 39081`.
- The local Pingora listener has `downstream_h2c: true`: Caddy/pb-mapper may
  use cleartext HTTP/2 prior-knowledge to `127.0.0.1:39180`, while ordinary
  HTTP/1.1 clients still work through protocol fallback.
- Cloud-side Caddy and pb-mapper remain systemd services on `ubuntu@ackingliu.top`
  (`caddy`, `pb-mapper-server.service`, and
  `pb-mapper-client-cli@sf-backend.service`).
- Deployment secrets such as `MSG_HEADER_KEY` and the GPT2API admin token are
  intentionally not documented here. Inspect live tmux/process environment only
  when operating the deployment.

Useful runtime checks:

```bash
tmux list-panes -a -F '#{session_name}|#{pane_pid}|#{pane_current_command}|#{pane_start_command}'
ss -tlnp '( sport = :39180 or sport = :39080 or sport = :39081 or sport = :18787 )'
readlink -f /proc/<pid>/exe
curl --http2-prior-knowledge -o /dev/null -sS -w 'h2c=%{http_version} code=%{http_code}\n' http://127.0.0.1:39180/api/healthz
```

Cloud Caddy should prefer h2c toward the local relay endpoint and keep HTTP/1.1
as fallback:

```caddy
reverse_proxy 127.0.0.1:39080 {
    transport http {
        versions h2c 1.1
    }
}
```

For one-off manual self-hosted starts, the scripts are:

```bash
# 1. Build frontend (API_BASE=/api, same-origin)
bash scripts/build_frontend_selfhosted.sh

# 2. Start backend (serves frontend static files)
bash scripts/start_backend_selfhosted.sh --daemon

# View logs
tail -f /tmp/staticflow-backend.log

# After frontend code changes: rebuild + restart backend (index.html is cached at startup)
bash scripts/build_frontend_selfhosted.sh
bash scripts/start_backend_selfhosted.sh --daemon
```

Self-hosted pitfall: do not run bare `trunk build --release` inside
`frontend/` for the public `ackingliu.top` deployment. `STATICFLOW_API_BASE` is
compiled into the WASM; without the self-hosted script it falls back to
`http://localhost:3000/api`, so public users' browsers try to call their own
localhost and report `Network error: JsError(... Failed to fetch ...)`.
Recover by rebuilding with `bash scripts/build_frontend_selfhosted.sh` and
confirming the served WASM contains `/api/...`, not `localhost:3000`.

### Mode B: Local Development (trunk hot-reload)

Frontend served by trunk dev server with hot-reload; trunk proxies `/api` to backend.

```bash
# 1. Start backend
bash scripts/start_backend_selfhosted.sh

# 2. Start frontend (trunk serve, proxies /api -> localhost:39080)
bash scripts/start_frontend_with_api.sh --open
```

Backend: `http://127.0.0.1:39080` | Frontend: `http://127.0.0.1:38080`

### Mode C: GitHub Pages (Frontend-only)

Frontend deployed to GitHub Pages; API accessed via pb-mapper tunnel to local backend.
CI builds automatically; `STATICFLOW_API_BASE` configured via GitHub repo variables.

```text
Browser -> https://acking-you.github.io (GitHub Pages static files)
        -> fetch(STATICFLOW_API_BASE/api/...) -> pb-mapper -> Local backend
```

Reference configs:
- Self-hosted Caddy: cloud `/etc/caddy/Caddyfile`
- GitHub Pages CI: `.github/workflows/deploy.yml`
- Legacy Nginx configs: `deployment-examples/`

## LLM Access / Kiro Access

Self-hosted mode exposes two public read-only access pages:

- `/llm-access`: Codex access, backed by the OpenAI-compatible gateway at `/api/llm-gateway/v1`
- `/kiro-access`: Kiro access, backed by the Anthropic-compatible gateway at `/api/kiro-gateway`

Shared characteristics:

- both gateways sit behind the StaticFlow backend instead of exposing upstream accounts directly
- provider-level proxy routing is resolved through one shared registry
- keys, runtime config, proxy configs, proxy bindings, and usage events are persisted in the same LLM gateway store
- the detailed runtime architecture is documented in [docs/llm-access-and-kiro-gateway-implementation.md](./docs/llm-access-and-kiro-gateway-implementation.md)

The Codex page additionally publishes:

- an OpenAI-compatible Base URL that already includes `/v1`
- public API keys that are explicitly marked as externally visible
- a ready-to-paste Codex provider snippet
- fallback `auth.json` content and plain chat examples (`curl` / Python SDK)

Key behavior:

- public access is read-only; key creation, quota changes, visibility, and TTL tuning
  stay under `/admin/llm-gateway` and `/admin/kiro-gateway`
- admin paths are blocked at the edge and not forwarded publicly
- upstream inference uses backend-managed real accounts rather than exposing upstream credentials
- Codex `/fast` requests are billed at `2x` billable tokens in StaticFlow quota accounting

Recommended Codex config:

```toml
model_provider = "staticflow"

[model_providers.staticflow]
name = "OpenAI"
base_url = "https://your-host/api/llm-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
```

This provider shape keeps Codex on the remote `/responses` and `/responses/compact`
path while avoiding the initial websocket fallback delay.

## Quick Start

On this host, the current long-running production setup is tmux-supervised on
the local machine, with Pingora selecting the active blue/green backend slot.
The cloud ingress side still uses systemd-managed Caddy and pb-mapper services.
The systemd quick-start remains useful as a reference deployment shape:

- [docs/self-hosted-systemd-quick-start.zh.md](docs/self-hosted-systemd-quick-start.zh.md)

下面这组命令仍然适合本地开发或最小化前台验证，不是新的推荐生产部署路径。

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown
cargo install trunk

# Build binaries
make bin-all

# Initialize LanceDB tables
cd cli
../target/release/sf-cli init --db-path ../data/lancedb

# --- Minimal self-hosted run ---
cd ..
bash scripts/build_frontend_selfhosted.sh
bash scripts/start_backend_selfhosted.sh --daemon

# --- Local dev mode ---
cd ..
bash scripts/start_backend_selfhosted.sh            # foreground
bash scripts/start_frontend_with_api.sh --open       # another terminal, trunk hot-reload
```

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
# - article_views scalar indexes are also managed here
# - runtime/request/interactive tables are handled by `sf-cli db ensure-indexes`
../target/release/sf-cli ensure-indexes --db-path ../data/lancedb

# By default, write-article / write-images / sync-notes auto-run index-only optimize
# to refresh index coverage for newly written rows.
# Disable per command with: --no-auto-optimize

# Write single article
../target/release/sf-cli write-article \
  --db-path ../data/lancedb \
  --file ../content/post-001.md \
  --date "2026-02-12" \
  --summary "Article summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes about Rust + WASM" \
  --content-en-file ../tmp/content_en.md \
  --summary-zh-file ../tmp/detailed_summary_zh.md \
  --summary-en-file ../tmp/detailed_summary_en.md

# Optional in markdown frontmatter for sync/write
# category_description: "Engineering notes about Rust + WASM"
# date: "2026-02-12"
# content_en: |
# detailed_summary:
#   zh: |
#   en: |
# If both are present, CLI --date overrides frontmatter date.
# `--summary-zh-file` and `--summary-en-file` must be provided together.

# Batch write images
../target/release/sf-cli write-images \
  --db-path ../data/lancedb \
  --dir ../content/images \
  --recursive \
  --generate-thumbnail

# Thumbnail implementation details
# - Generated only with --generate-thumbnail; size controlled by --thumbnail-size (default 256)
# - Original image bytes live in blob v2 sidecars via images.data
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
../target/release/sf-cli db --db-path ../data/lancedb optimize images
# One-time schema/storage migration for legacy image tables
../target/release/sf-cli db --db-path ../data/lancedb migrate-images-blob-v2
# One-command orphan cleanup (prune-only, no full rewrite)
../target/release/sf-cli db --db-path ../data/lancedb cleanup-orphans --table images
# Run orphan cleanup across all cleanup target tables (includes article_views; skips if missing)
../target/release/sf-cli db --db-path ../data/lancedb cleanup-orphans

# Content DB table groups
# - articles: article body/metadata + vectors + bilingual/repost fields
# - images: blob v2 original payloads + binary thumbnails + vectors
# - taxonomies: category/tag metadata (`kind`, `key`, `name`, `description`)
# - article_views / api_behavior_events: backend runtime analytics
# - article_requests / article_request_ai_*: article request worker runtime tables
# - interactive_pages / interactive_page_locales / interactive_assets: standalone interactive mirror pages and localized assets
# - llm_gateway_keys / llm_gateway_usage_events / llm_gateway_runtime_config: public gateway auth, usage ledger, and runtime cache config

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
| `GET /api/articles/:id/raw/:lang` | Raw markdown content by language (`lang=zh|en`) |
| `POST /api/articles/:id/view` | Track article view (default 60s dedupe per article+client) |
| `GET /api/articles/:id/view-trend` | Article view trend (day/hour buckets, Asia/Shanghai) |
| `GET /api/articles/:id/related` | Related articles (vector similarity) |
| `POST /api/comments/submit` | Submit a comment task (selection/footer entry, rate-limited) |
| `GET /api/comments/list` | List public comments for one article (user comments first, `ai_reply_markdown` may be null) |
| `GET /api/comments/stats` | Get public comment count for one article |
| `GET /api/search?q=` | Full-text search |
| `GET /api/semantic-search?q=` | Semantic search (vector, with cross-language fallback and semantic snippet highlight) |
| `GET /api/images` | Image catalog |
| `GET /api/images/:id-or-filename` | Read image binary from LanceDB (`?thumb=true`, fallback to original if thumbnail missing) |
| `GET /api/image-search?id=` | Similar images |
| `GET /api/tags` | Tag list |
| `GET /api/categories` | Category list |

> Observability: every backend response includes `x-request-id` and `x-trace-id`. The same IDs appear in backend/shared logs for request-level correlation.

> Query-path observability: logs include `query/path/fastest_path/is_fastest/reason/rows/elapsed_ms` to show whether index paths are used or fallbacks are triggered.

> Semantic highlight mode: `/api/semantic-search` defaults to fast highlight; append `&enhanced_highlight=true` for higher-precision snippet reranking (slower).

> View analytics: `/api/articles/:id/view` is intended to be called on article-detail entry; backend uses a 60-second dedupe window by default (configurable via local admin endpoint `/admin/view-analytics-config`) and stores trend buckets in `Asia/Shanghai`.

> Comment moderation: public users only hit `/api/comments/*`; moderation and runtime controls are under local `/admin/*` endpoints (do not expose publicly).
> Local admin endpoints include grouped task view (`/admin/comments/tasks/grouped`), approve-only + approve-and-run split, published comment patch/delete, and audit logs (`/admin/comments/audit-logs`).
> GeoIP status and diagnostics are available via local admin endpoint `/admin/geoip/status`.
> `ip_region` prefers province/state-level detail (`country/region[/city]`); if only country-level info is available, backend returns `Unknown`.
> AI worker completion is file-first: Codex must write final markdown to a task-specific file under `COMMENT_AI_RESULT_DIR`; stdout/stderr are kept for trace/audit chunks.

## Key Env Vars

Backend (set automatically by `scripts/start_backend_selfhosted.sh`):
- `DB_ROOT` (default `/mnt/wsl/data4tb/static-flow-data`, auto-resolves content/comments/music DBs)
- `PORT` (default `39080`)
- `HOST` (default `127.0.0.1`)
- `SITE_BASE_URL` (default `https://ackingliu.top`, used for SEO injection)
- `FRONTEND_DIST_DIR` (default `../frontend/dist`, static file directory for self-hosted mode)
- `RUST_ENV` (`development` or `production`)
- `ALLOWED_ORIGINS` (optional comma-separated CORS list in production)
- `ADMIN_LOCAL_ONLY` (default `true`, guard `/admin/*` to local/private sources)
- `ADMIN_TOKEN` (optional, checked from request header `x-admin-token`)
- `COMMENT_RATE_LIMIT_SECONDS` / `COMMENT_LIST_DEFAULT_LIMIT` / `COMMENT_CLEANUP_RETENTION_DAYS`
- `COMMENT_AI_CONTENT_API_BASE` (optional, default `http://127.0.0.1:$PORT/api`)
- `COMMENT_AI_CODEX_SANDBOX` (default `danger-full-access`)
- `COMMENT_AI_CODEX_JSON_STREAM` (default `1`, streams Codex events into run chunks)
- `COMMENT_AI_CODEX_BYPASS` (default `0`, set `1` to use `--dangerously-bypass-approvals-and-sandbox`)
- `COMMENT_AI_RESULT_DIR` (default `/tmp/staticflow-comment-results`, stores per-task markdown result files)
- `COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS` (default `1`, delete result file after successful publish)
- `ENABLE_GEOIP_AUTO_DOWNLOAD` (default `true`, auto-download mmdb when missing)
- `GEOIP_DB_PATH` / `GEOIP_DB_URL` (optional local DB path/source)
- `ENABLE_GEOIP_FALLBACK_API` / `GEOIP_FALLBACK_API_URL` (fallback API when local db lacks region detail)
- `GEOIP_REQUIRE_REGION_DETAIL` (default `true`, reject country-only labels)
- `GEOIP_PROXY_URL` (optional proxy, e.g. `http://127.0.0.1:7890`)

Frontend build-time:
- `STATICFLOW_API_BASE`
  - Self-hosted: `/api` (set by `build_frontend_selfhosted.sh`)
  - GitHub Pages: absolute URL (set by CI workflow repo variables)
  - Local dev: `http://127.0.0.1:39080/api` (set by `start_frontend_with_api.sh`)

Never rely on the fallback `http://localhost:3000/api` for any public build.
It is only a development fallback in source code; a self-hosted production
build must come from `scripts/build_frontend_selfhosted.sh`.

## Development Commands

```bash
# Workspace
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings

# Frontend (self-hosted build)
bash scripts/build_frontend_selfhosted.sh
# Do not replace this with: cd frontend && trunk build --release

# Frontend (trunk hot-reload dev)
bash scripts/start_frontend_with_api.sh --open

# Backend
make bin-backend
bash scripts/start_backend_selfhosted.sh            # foreground
bash scripts/start_backend_selfhosted.sh --daemon    # background (log: /tmp/staticflow-backend.log)
```

## License

MIT
