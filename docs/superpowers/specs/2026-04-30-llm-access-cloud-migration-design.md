# LLM Access Cloud Migration Design

## Background

The immediate production problem is not only application latency. The home
network is likely being classified as PCDN-like traffic by the ISP, causing
home upstream throttling. StaticFlow currently exposes public traffic through a
cloud ingress relay, but the real backend payload is still served by the local
machine through pb-mapper. LLM access traffic is the worst fit for this shape:
it is high-frequency, stream-oriented, and response-heavy, so every SSE stream
keeps consuming the throttled home upstream path until completion.

The migration should remove LLM access traffic from the home uplink quickly
without moving the rest of StaticFlow.

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

## Recommended Architecture

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
- LanceDB-backed content, comments, music, and legacy LLM source data during
  migration.
- pb-mapper server-side registration for non-LLM StaticFlow traffic.

## Route Split

The initial routing boundary should be path based. Caddy sends only LLM-related
paths to cloud `llm-access`; everything else keeps the current local path.

Candidate LLM paths:

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

Before implementation, verify the exact route list against backend routes. The
principle is simple: routes that issue upstream LLM requests, manage LLM keys,
manage Kiro/Codex accounts, write usage events, or handle LLM contribution
queues belong to `llm-access`.

## Storage Layout

The target cloud state root is mounted through JuiceFS:

```text
/mnt/llm-access
  /control/llm-access.sqlite3
  /analytics/usage.duckdb
  /auths/kiro
  /auths/codex
  /cdc
  /logs
  /backups
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

- Wide `usage_events` fact table.
- Usage details side table.
- Hourly and daily rollups.
- CDC audit history.

The first production deployment is single-writer. The service must not run two
active writers against the same SQLite/DuckDB files.

## JuiceFS Requirements

JuiceFS is used to decouple service state from the VM root disk. For this goal,
both data objects and metadata must survive VM replacement.

Recommended first version:

- Object storage in the same cloud and region as the VM.
- External metadata backend when possible, such as managed Redis, Postgres, or
  MySQL.
- If SQLite metadata is used temporarily, the metadata database itself must be
  backed up and not treated as disposable VM-local state.
- Systemd must gate `llm-access.service` on the JuiceFS mount being ready.

The service should fail closed if `/mnt/llm-access` is not mounted or required
state files are missing. It should not silently initialize against an empty
local directory.

## Migration Flow

1. Enable source-side LLM CDC outbox in local StaticFlow.
2. Export a full LLM snapshot from existing LanceDB-backed StaticFlow data.
3. Import the snapshot into cloud SQLite and DuckDB.
4. Replay source CDC rows from the snapshot high-water mark.
5. Start cloud `llm-access` with test-only keys.
6. Add Caddy canary routing for a narrow LLM path or test key.
7. Compare cloud `llm-access` usage output with local StaticFlow usage output.
8. Switch all LLM paths to cloud `llm-access`.
9. Make local StaticFlow LLM write paths read-only, redirected, or disabled so
   cloud `llm-access` becomes the source of truth.

## Rollback

Before final source-of-truth cutover, rollback is simple: point Caddy LLM paths
back to the existing local pb-mapper route.

After final source-of-truth cutover, rollback must be treated as a data
migration. The cloud `llm-access` state must either be replayed back into local
StaticFlow or local StaticFlow must continue to proxy LLM operations to cloud
`llm-access`.

## Operational Checks

For canary and cutover, track:

- NewAPI first SSE latency and stream finish latency.
- GCP VM CPU, memory, disk, and network egress.
- JuiceFS mount health and metadata backend latency.
- `llm-access` request error rate.
- Usage event write rate and rollup correctness.
- Home upstream traffic dropping after LLM path cutover.
- pb-mapper health for non-LLM paths.

## Implementation Order

1. Finish `llm-access` provider runtime for the existing Kiro/Codex/OpenAI and
   Claude-compatible request paths.
2. Implement snapshot export/import for current LLM state.
3. Extend CDC replay to all LLM entities.
4. Write usage events into DuckDB and control-plane state into SQLite.
5. Add deployment files for JuiceFS mount and `llm-access.service`.
6. Add Caddy path split for LLM paths.
7. Run test-key canary, then switch production LLM paths.

## Current Implementation Snapshot

Implemented in this repo:

- Source-side SQLite CDC outbox for StaticFlow LLM mutations.
- `llm-access-migrations`, `llm-access-store`, `llm-access-migrator`, and
  `llm-access` workspace crates.
- SQLite control-plane schema and DuckDB analytics schema files.
- Control-plane CDC replay for keys, runtime config, account groups, proxy
  config/bindings, public request queues, and sponsor requests.
- Snapshot manifest/export scaffold with CDC high-water marks and optional
  `staticflow-source` JSONL export using the existing StaticFlow store APIs.
- `llm-access` HTTP shell with route ownership tests and explicit 401 for
  unauthenticated provider entries.
- Deployment bundle templates:
  `deployment-examples/systemd/llm-access.service.template`,
  `deployment-examples/systemd/llm-access-juicefs.mount.template`, and
  `deployment-examples/caddy/llm-access-path-split.Caddyfile`.

Still pending before real production cutover:

- Move the actual Kiro/Codex provider runtime into `llm-access`.
- Import snapshot JSONL rows into SQLite/DuckDB, not only initialize targets.
- Write live usage events to DuckDB from the standalone service.
- Run a test-key canary before routing all LLM paths to the cloud service.

## Open Decisions

- Exact GCP storage backend for JuiceFS metadata and objects.
- Exact public route list after backend route verification.
- Whether admin LLM pages are served by local StaticFlow frontend while their
  APIs point to cloud `llm-access`, or whether those admin APIs are fully owned
  by cloud `llm-access` from the first cutover.
- Whether local StaticFlow should proxy legacy LLM paths to cloud `llm-access`
  after cutover for backward compatibility.
