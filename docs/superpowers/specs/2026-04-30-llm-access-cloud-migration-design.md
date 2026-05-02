# LLM Access Cloud Migration Design

> Status update on 2026-05-02: production cutover is complete. GCP Caddy routes
> LLM paths directly to cloud `llm-access` on `127.0.0.1:19080`; non-LLM
> StaticFlow paths continue through pb-mapper to the local Pingora gateway.
> `llm-access` is now the production source of truth for gateway keys, account
> state, runtime config, public request queues, and usage analytics. State lives
> on a JuiceFS mount backed by Cloudflare R2 object storage and Valkey metadata,
> with the active mutable DuckDB segment on local GCP VM block storage.

## Background

The original production problem was not only application latency. The home
network is likely being classified as PCDN-like traffic by the ISP, causing
home upstream throttling. StaticFlow exposed public traffic through a cloud
ingress relay while the real backend payload was still served by the local
machine through pb-mapper. LLM access traffic was the worst fit for that shape:
it is high-frequency, stream-oriented, and response-heavy, so every SSE stream
kept consuming the throttled home upstream path until completion.

The migration removed LLM access traffic from the home uplink without moving
the rest of StaticFlow.

## Goals

- Move all LLM access request handling to a cloud VM.
- Keep non-LLM StaticFlow features running locally with the existing pb-mapper
  path.
- Preserve current public URLs where possible.
- Make the cloud VM disposable by storing service state on JuiceFS-backed
  storage rather than the VM root disk.
- Avoid LanceDB in the new `llm-access` service.
- Support a controlled migration with snapshot import, CDC replay, canary
  routing, and rollback.

## Non-Goals

- Do not move the full StaticFlow backend to the cloud.
- Do not redesign article, music, comment, local media, or frontend publishing
  storage.
- Do not build multi-writer or active-active `llm-access` in the first phase.
- Do not run SQLite or DuckDB from multiple active writer instances.

## Current Architecture

Public traffic continues to enter through the cloud Caddy instance.

```text
User / NewAPI
  -> GCP Caddy :443
      -> LLM paths: 127.0.0.1:19080 llm-access
      -> Other paths: 127.0.0.1:39080 pb-mapper client
          -> pb-mapper server
          -> local StaticFlow gateway 127.0.0.1:39180
```

The cloud VM runs:

- Caddy for TLS termination and path-based routing.
- `llm-access` on `127.0.0.1:19080`.
- pb-mapper client for non-LLM fallback to local StaticFlow.
- JuiceFS mount for `llm-access` state.

The local machine keeps:

- StaticFlow backend and Pingora gateway.
- LanceDB-backed content, comments, music, and legacy LLM source/migration
  tables.
- pb-mapper server-side registration for non-LLM StaticFlow traffic.
- pb-mapper client subscription to cloud `llm-access` for local external-mode
  backend/dev requests.

## Route Split

The routing boundary is path based. Caddy sends LLM-related paths to cloud
`llm-access`; everything else keeps the local StaticFlow path.

Current LLM paths:

```text
/v1/*
/cc/v1/*
/api/llm-gateway/*
/api/kiro-gateway/*
/api/codex-gateway/*
/api/llm-access/*
```

The deployment template uses a Caddy path matcher plus `handle @llm_access`.
It intentionally does not use `handle_path`, because `handle_path` strips the
matched prefix and would break provider routes such as `/v1/chat/completions`.

The principle is simple: routes that issue upstream LLM requests, manage LLM
keys, manage Kiro/Codex accounts, write usage events, or handle LLM
contribution queues belong to `llm-access`.

## Storage Layout

The production cloud state root is mounted through JuiceFS:

```text
/mnt/llm-access
  /control/llm-access.sqlite3
  /analytics/segments
  /analytics/catalog
  /auths/kiro
  /auths/codex
  /support/llm_access_support
  /cdc
  /logs
  /backups

/var/lib/staticflow/llm-access/analytics-active
  /usage-active-*.duckdb
```

SQLite is the runtime control plane:

- API keys and key state.
- Per-key route config.
- Runtime config.
- Account groups.
- Proxy configs and bindings.
- Key usage rollups.
- Token, account contribution, GPT2API account contribution, and sponsor
  request queues.
- CDC consumer offsets and recent apply state.

DuckDB is the append-heavy analytics plane:

- Active mutable `usage_events` facts in a local VM block-storage segment.
- Immutable archived usage segments under `/mnt/llm-access/analytics/segments`.
- A low-frequency segment catalog under `/mnt/llm-access/analytics/catalog`.
- Usage details side table.
- Hourly and daily rollups.
- CDC audit history.

The production deployment is single-writer. The service must not run two active
writers against the same SQLite/DuckDB/auth tree. Do not point a live writer at
`/mnt/llm-access/analytics/usage.duckdb` as a mutable all-history DuckDB file;
tiered mode keeps the current mutable file on local VM disk and archives
completed segments to JuiceFS/R2.

## JuiceFS Requirements

JuiceFS is used to decouple service state from the VM root disk. Both data
objects and metadata must survive VM replacement.

Current production backend:

- Object storage: Cloudflare R2 bucket for `llm-access`.
- Metadata backend: external Valkey, DB `11`, dedicated `juicefs` ACL user.
- GCP local cache: `/var/cache/juicefs/llm-access`, not inside
  `/mnt/llm-access`.
- Systemd gates `llm-access.service` on the JuiceFS mount and expected state
  files through `/usr/local/bin/staticflow-wait-llm-access-state`.

The service should fail closed if `/mnt/llm-access` is not mounted or required
state files are missing. It should not silently initialize against an empty
local directory.

## Completed Migration Flow

1. Enable source-side LLM CDC outbox in local StaticFlow.
2. Export a full LLM snapshot from existing LanceDB-backed StaticFlow data.
3. Import the snapshot into cloud SQLite and DuckDB.
4. Replay source CDC rows from the snapshot high-water mark.
5. Start cloud `llm-access` and validate it with narrow traffic.
6. Add Caddy LLM path split to `127.0.0.1:19080`.
7. Compare cloud `llm-access` usage output with previous StaticFlow usage
   output.
8. Switch all LLM paths to cloud `llm-access`.
9. Run local StaticFlow in external-LLM mode so legacy/local route families
   proxy to cloud `llm-access` instead of writing local LLM state.

## Rollback

Before source-of-truth cutover, rollback was simple: point Caddy LLM paths back
to the existing local pb-mapper route.

After source-of-truth cutover, rollback is a data migration. Cloud
`llm-access` has accepted live writes, so local StaticFlow must either continue
proxying LLM operations to cloud `llm-access`, or cloud state must be exported
and replayed back into a replacement writer.

## Operational Checks

For canary and cutover, track:

- NewAPI first SSE latency and stream finish latency.
- GCP VM CPU, memory, disk, and network egress.
- JuiceFS mount health and metadata backend latency.
- `llm-access` request error rate.
- Usage event write rate and rollup correctness.
- Home upstream traffic dropping after LLM path cutover.
- pb-mapper health for non-LLM paths.

## Implementation Order Record

1. Finish `llm-access` provider runtime for the existing Kiro/Codex/OpenAI and
   Claude-compatible request paths.
2. Implement snapshot export/import for current LLM state.
3. Extend CDC replay to all LLM entities.
4. Write usage events into DuckDB and control-plane state into SQLite.
5. Add deployment files for JuiceFS mount and `llm-access.service`.
6. Add Caddy path split for LLM paths.
7. Run test-key canary, then switch production LLM paths.
8. Add tiered DuckDB analytics so active writes stay on local VM block storage
   and completed segments archive to JuiceFS/R2.
9. Add bounded admin usage queries and memory guards for the 2c8g GCP VM.

## Current Implementation Snapshot

Implemented and live:

- Source-side SQLite CDC outbox for StaticFlow LLM mutations.
- `llm-access-migrations`, `llm-access-store`, `llm-access-migrator`, and
  `llm-access` workspace crates.
- SQLite control-plane schema and DuckDB analytics schema files.
- Control-plane CDC replay for keys, runtime config, account groups, proxy
  config/bindings, public request queues, and sponsor requests.
- Snapshot manifest/export scaffold with CDC high-water marks and optional
  `staticflow-source` JSONL export using the existing StaticFlow store APIs.
- `llm-access` standalone provider runtime for Codex/OpenAI-compatible and
  Kiro/Anthropic-compatible routes.
- Cloud path split for public LLM routes.
- External-mode local backend proxy for LLM route families.
- Tiered DuckDB usage analytics with local active segment, archived immutable
  segments, and bounded admin list queries.
- Deployment bundle templates:
  `deployment-examples/systemd/llm-access.service.template`,
  `deployment-examples/systemd/llm-access-juicefs.mount.template`, and
  `deployment-examples/caddy/llm-access-path-split.Caddyfile`.

Current production constraints:

- Keep only one active writer for the cloud SQLite/DuckDB/auth tree.
- Keep broad diagnostics out of in-process admin usage APIs; use external
  read-only DuckDB connections for large scans.
- Keep `/mnt/llm-access` as JuiceFS/R2 state and the active mutable DuckDB
  directory on local VM block storage.
- Treat the local LanceDB `llm_gateway_*` tables as legacy/source data, not the
  live production source of truth.

## Resolved Decisions

- JuiceFS object backend is Cloudflare R2; metadata backend is external Valkey
  DB `11`.
- Public LLM path list is `/v1/*`, `/cc/v1/*`, `/api/llm-gateway/*`,
  `/api/kiro-gateway/*`, `/api/codex-gateway/*`, and `/api/llm-access/*`.
- The frontend and non-LLM StaticFlow payloads remain local-first behind
  pb-mapper; LLM APIs are routed by GCP Caddy to cloud `llm-access`.
- Local StaticFlow uses external-LLM mode / pb-mapper subscription for
  compatibility instead of writing cloud JuiceFS state directly.
