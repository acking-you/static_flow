# StaticFlow Agent Guide

## Project Intent
StaticFlow is a local-first writing, knowledge-management, and media platform.
Full-stack Rust: Axum backend + Yew/WASM frontend + LanceDB storage.
Core capabilities: article publishing, metadata enrichment, image/music asset
ingestion, AI-powered comment review, music wish fulfillment, external article
repost ingestion, and searchable knowledge organization — all on a local machine.

## LanceDB Data Location
StaticFlow uses three LanceDB roots:
- Content DB — `/mnt/wsl/data4tb/static-flow-data/lancedb`
  tables: `articles`, `images`, `taxonomies`, `article_views`, `api_behavior_events`,
  `article_requests`, `article_request_ai_runs`, `article_request_ai_run_chunks`,
  `interactive_pages`, `interactive_page_locales`, `interactive_assets`,
  `llm_gateway_keys`, `llm_gateway_usage_events`, `llm_gateway_runtime_config`
- Comments DB — `/mnt/wsl/data4tb/static-flow-data/lancedb-comments`
  tables: `comment_tasks`, `comment_published`, `comment_audit_logs`,
  `comment_ai_runs`, `comment_ai_run_chunks`
- Music DB — `/mnt/wsl/data4tb/static-flow-data/lancedb-music`
  tables: `songs`, `music_plays`, `music_comments`,
  `music_wishes`, `music_wish_ai_runs`, `music_wish_ai_run_chunks`

Canonical root: `/mnt/wsl/data4tb/static-flow-data`

The local content DB `llm_gateway_*` tables are legacy/source-of-migration
state. Current production LLM access is owned by the cloud `llm-access`
service: SQLite control under `/mnt/llm-access/control` and tiered DuckDB
analytics with an active local VM segment plus archived JuiceFS/R2 segments.

Current storage invariants:
- All current production tables use stable row IDs.
- Blob v2 tables: content DB `images.data`, `interactive_assets.bytes`;
  music DB `songs.audio_data`.
- `images.thumbnail` remains regular `Binary`; only original payloads use blob v2.

When invoking `sf-cli`, default `--db-path` should point to the content DB
(`.../lancedb`) unless explicitly overridden.

For backend local startup via `scripts/start_backend_from_tmp.sh`, prefer one root:
- `DB_ROOT=/path/to/data-root` (auto-resolves content/comments/music DBs)
- Optional explicit overrides: `DB_PATH`, `COMMENTS_DB_PATH`, `MUSIC_DB_PATH`

## Runtime Log Paths
Default runtime log root: `./tmp/runtime-logs` (override: `STATICFLOW_LOG_DIR`)

- Backend: `./tmp/runtime-logs/backend/{app,access}/current.*.log`
- Canary: `./tmp/runtime-logs/backend-canary-<port>/{app,access}/current.*.log`
- Gateway: `./tmp/runtime-logs/gateway/{app,access}/current.*.log`
  plus `./tmp/runtime-logs/gateway/daemon-stderr.log`

Logs rotate hourly, retain up to 4 files per stream.

## Local Notes Source (Obsidian)
Primary local notes: `/mnt/e/note-by-obsidian/learning`

## Operating Preference
Prefer reproducible CLI workflows (`sf-cli`) over ad-hoc manual database edits.
Always verify published records after write operations.
Avoid degradation handling, fallbacks, heuristics, local stabilizations, or
post-processing bandages when fixing core algorithms or storage formats.
Prefer faithful upstream/mainline behavior plus explicit data migration over
runtime compatibility layers.
For `sf-cli`, rebuild the CLI when the active checkout is newer than the
existing binary, then use the rebuilt `target/release` or `target/debug`
artifact. Do not prefer legacy `./bin/sf-cli` snapshots for
storage-format-sensitive writes.

## Current Production Deployment Mode
Hybrid: GCP owns TLS ingress and standalone `llm-access`; local StaticFlow
serves content, comments, music, media, frontend, and Pingora blue/green slots.

Traffic path:
- `https://ackingliu.top` → GCP Caddy `:443` → route split
- LLM paths (`/v1/*`, `/cc/v1/*`, `/api/llm-gateway/*`, `/api/kiro-gateway/*`,
  `/api/codex-gateway/*`, `/api/llm-access/*`) → cloud `llm-access` `127.0.0.1:19080`
- Non-LLM paths → cloud pb-mapper `127.0.0.1:39080` → pb-mapper-server `:7666`
  → local Pingora `127.0.0.1:39180` → active backend slot
- Local `pbmapper-llm-access` on `127.0.0.1:19182` subscribes cloud `llm-access`
  back for local dev/testing

Key rules:
- Do not restart the local Pingora gateway (`39180`) during routine hot updates.
- For production frontend builds, **only** use `scripts/build_frontend_selfhosted.sh`
  (compiles `STATICFLOW_API_BASE=/api`). Bare `trunk build --release` falls back
  to `localhost:3000/api` and breaks public users.
- Agents may inherit local proxy env vars; unset them for direct-public checks.
- `/_caddy_health` only proves Caddy is alive, not the full pb-mapper data path.

For full GCP/Valkey/JuiceFS/systemd details and emergency recovery, see
`docs/ops-runbook.md`.

Local tmux-supervised runtime (verified 2026-04-29):

| tmux session | Role | Address |
|---|---|---|
| `sf-gateway` | Pingora ingress (do not stop) | `127.0.0.1:39180` |
| `sf-backend-green` | Active backend slot | `127.0.0.1:39081` |
| `gpt2api-rs` | GPT2API image gateway | `127.0.0.1:18787` |
| `pbmapper-sf-backend` | Registers gateway with cloud relay | → `ackingliu.top:7666` |
| `pbmapper-llm-access` | Subscribes cloud llm-access locally | `127.0.0.1:19182` |
| `pbmapper-home-ubuntu` | Registers local SSH with cloud relay | → `ackingliu.top:7666` |

## Mandatory Quality Gates (Hard Rule)
- Run `cargo clippy` for affected crates and fix all warnings to zero before
  considering any coding task done.
- Before any commit, run `rustfmt` on changed files.
- **Only one local Rust build/check may run at a time.** Concurrent builds can
  OOM the machine and kill the live backend. Before starting, check with
  `pgrep -af 'cargo|rustc|trunk|ld|lld|mold'`.
- **All Cargo build artifacts must live on the large mount:**
  `CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow`.
  Confirm mount with `df -h /mnt/wsl/data4tb`. Do not grow
  `/home/ts_user/rust_pro/static_flow/target` for routine work.
- Treat `static_flow`, `deps/lance`, `deps/lancedb`, `deps/gpt2api_rs` as one
  shared build budget. No parallel builds across them.
- When memory is comfortable, use `--jobs 4` to `--jobs 8`. Drop below 4 only
  under memory pressure.
- **NEVER run `cargo fmt --all` or `cargo fmt` at workspace root.** `deps/lance`
  and `deps/lancedb` have their own formatting. Only format files you changed:
  `rustfmt path/to/file.rs` or `cargo fmt -p <crate>`.
- **NEVER run `cargo fmt` inside `deps/lance` or `deps/lancedb`.**

## Testing
```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow

# Run tests for specific crates
cargo test -p static-flow-shared --jobs 8
cargo test -p static-flow-backend --jobs 8
cargo test -p sf-cli --jobs 8

# Clippy for specific crates
cargo clippy -p static-flow-shared -p static-flow-backend --jobs 8 -- -D warnings

# Format only changed files
rustfmt path/to/changed_file.rs

# CLI E2E tests
./scripts/test_cli_e2e.sh
```

## Frontend Build
```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow

# Production self-hosted build (STATICFLOW_API_BASE=/api)
bash scripts/build_frontend_selfhosted.sh

# Local dev with hot-reload (trunk proxies /api → localhost:39080)
bash scripts/start_frontend_with_api.sh --open
```

Do not run bare `trunk build --release` for the public deployment.

## Skill Routing (Soft Rule)
Use the following skill by default according to task type:

- Publishing/syncing Markdown or images into LanceDB, or table/API verification:
  `staticflow-cli-publisher`
- Ingesting external blog posts (HTML or Markdown sources, with optional translation):
  `external-blog-repost-publisher`
- AI-powered comment review and response generation:
  `comment-review-ai-responder`
- Managing Hugging Face dataset Git/Xet repositories:
  `huggingface-git-xet-dataset-publisher`
- Translating one Chinese article into full English and rewriting bilingual summaries:
  `article-bilingual-translation-publisher`
- Regenerating or improving `detailed_summary.zh/en` only:
  `article-summary-architect`
- Writing technical implementation documentation/specs:
  `tech-impl-deep-dive-writer`
- Ingesting music files (Netease search/download, NCM decrypt, local mp3/flac):
  `music-ingestion-publisher`
- Optimizing (compact + prune) LanceDB tables:
  `lancedb-optimize`
- Setting up Caddy HTTPS reverse proxy:
  `caddy-https-reverse-proxy`
- Upgrading the local backend behind Pingora with blue-green cutover:
  `selfhosted-gateway-seamless-upgrade`
- Ingesting JS-heavy external pages as standalone interactive mirrors:
  `interactive-page-repost-publisher`
- Operating the gpt2api-rs image gateway (lifecycle, admin, StaticFlow integration):
  `gpt2api-rs-admin`
- Checking daily Kiro usage credits and account breakdowns:
  `kiro-usage-day-report`
- Recalibrating Kiro cache-estimation coefficients from usage samples:
  `kiro-kmodel-calibrator`
- Searching local Codex session history:
  `codex-session-history`
- Drafting GitHub PR titles, bodies, and maintainer comments:
  `github-pr-message-writer`

If multiple skills apply, use the smallest set that fully covers the task.

## Author Field Convention (Soft Rule)
When writing/updating article records:
- Preferred author values: `ackingliu` or `LB7666`
- If user explicitly specifies one, follow the user input
- If not specified:
  - default to `ackingliu` for engineering/deep-dive/system notes
  - use `LB7666` for content explicitly marked as personal/brand output

## Worker Architecture
Three background AI workers run as Codex agents, spawned by the backend via
`mpsc` channels and shell runner scripts:

| Worker | Runner Script | Skill | DB |
|---|---|---|---|
| Comment AI | `scripts/comment_ai_worker_runner.sh` | `comment-review-ai-responder` | comments DB |
| Music Wish | `scripts/music_wish_worker_runner.sh` | `music-ingestion-publisher` | music DB |
| Article Request | `scripts/article_request_worker_runner.sh` | `external-blog-repost-publisher` | content DB |

Key conventions:
- DB path propagation: `main.rs` → `AppState` → `WorkerConfig` → payload JSON field + env var
- Worker workdir: configurable via `*_WORKDIR` env var, defaults to backend process cwd
- Context discovery: prompt instructs agent to check for and read `AGENTS.md`,
  `CLAUDE.md`, `README.md`, `CONTRIBUTING.md` in workdir (agent-driven, not injected)
- Result files: written to `/tmp/staticflow-*-results/` as JSON
- All data processing in workers happens under `/tmp/`, not the project root

## Local Lance / LanceDB / Pingora Forks
All three are **git submodules** under `deps/`, user forks on `feat/static-flow`.

| Submodule | Path | Fork |
|---|---|---|
| lance | `deps/lance` | `acking-you/lance` |
| lancedb | `deps/lancedb` | `acking-you/lancedb` |
| pingora | `deps/pingora` | `acking-you/pingora` |

Key points:
- Root `Cargo.toml` uses path deps, not crates.io. Root workspace has
  `exclude = ["deps/lance", "deps/lancedb"]`.
- After cloning: `git submodule update --init --recursive`
- Do not run `cargo fmt` in `deps/lance` or `deps/lancedb`.
- When modifying submodule source, commit inside the submodule first, then
  update the submodule ref in the parent repo.

## Codebase Structure
```
# Workspace crates (14)
shared/              Rust library — LanceDB stores, data types (#[cfg(not(wasm32))] gated)
backend/             Axum HTTP server — handlers, routes, state, workers, email
frontend/            Yew/WASM SPA — pages, components, api, router, i18n
cli/                 sf-cli binary — LanceDB operations (write/query/embed/optimize)
gateway/             Pingora-based local ingress gateway (blue/green upstream switching)
runtime/             Shared runtime utilities (logging, tracing, signal handling)
media-service/       Media processing service (image/audio pipelines)
media-types/         Shared media type definitions
llm-access/          Standalone LLM access service binary (cloud deployment)
llm-access-core/     Core LLM access logic (routing, quota, proxy resolution)
llm-access-codex/    Codex/OpenAI-compatible gateway implementation
llm-access-kiro/     Kiro/Anthropic-compatible gateway implementation
llm-access-migrations/ Schema migration tooling for llm-access stores
llm-access-store/    Storage layer for llm-access (SQLite control + DuckDB analytics)

# Non-crate directories
skills/              Codex/Claude agent skill definitions (SKILL.md + references)
scripts/             Shell scripts — worker runners, backend/frontend launchers, e2e tests
docs/                Technical documentation, implementation deep-dives, ops runbook
content/             Article Markdown source files and images
conf/                Configuration files (Pingora gateway YAML, systemd templates)
tools/               Third-party utilities (ncmdump-rs, pb-mapper)
bin/                 Pre-built backend binary
deployment-examples/ Legacy Nginx reverse proxy configs (superseded by Caddy)
patches/             Vendored crate patches (object_store)
deps/                Git submodules — lance/lancedb/pingora forks
```
