# llm-access Tiered Usage DuckDB Storage Design

## Problem

The standalone `llm-access` service currently treats one DuckDB file as both the
hot write database and the full historical usage store. That shape is fragile
when the file lives on a JuiceFS/R2-backed mount: DuckDB writes, checkpoints,
and large string-column reads all target a mutable database file over FUSE and
object-storage semantics.

The goal is to keep object storage for capacity while moving mutable DuckDB
work back to local block storage. Usage events are operational diagnostics, not
the source of truth for request success, so archive lag or archive failure must
not block API traffic.

## Principles

- Keep the request path independent from JuiceFS/R2.
- Keep exactly one mutable DuckDB file open for writes.
- Treat archived DuckDB files as immutable read-only segments.
- Prefer exact active data and eventually visible archive data over blocking
  business traffic.
- Do not scan broad historical data inside the production request process when a
  manifest or rollup can answer the query.

## Storage Layout

Use three distinct areas:

- Local active directory on VM block storage:
  `/var/lib/staticflow/llm-access/analytics-active`
- JuiceFS archive directory:
  `/mnt/llm-access/analytics/segments`
- JuiceFS catalog directory:
  `/mnt/llm-access/analytics/catalog`

The active writer uses a file like:

```text
/var/lib/staticflow/llm-access/analytics-active/usage-active-000123.duckdb
```

Sealed archive files use stable names:

```text
/mnt/llm-access/analytics/segments/usage-20260502-000123.duckdb
```

Catalog data records segment metadata and lookup aids. The catalog may start as
SQLite or JSONL, but its contract must be explicit:

- `segment_id`
- `archive_path`
- `state`
- `start_ms`
- `end_ms`
- `row_count`
- `size_bytes`
- `sealed_at_ms`
- per-key/provider/time rollups needed by list, total, and chart views
- event-id locator entries, mapping `event_id` to `segment_id`

## Segment State Machine

Segments move through these states:

```text
active -> pending_seal -> sealing -> archived
                         -> seal_failed
```

`active` is the only writable state. `archived` files are immutable. A
`seal_failed` segment remains locally available for retry if disk pressure
allows it; otherwise it can be dropped only with an explicit operator decision.

## Hot Write Path

The online write path must stay short:

1. Request handling emits `UsageEvent` into the existing bounded in-memory queue.
2. The usage writer appends batches to the current active DuckDB file.
3. When the active file crosses the configured threshold, the writer finishes
   the current batch, closes that active file, marks it `pending_seal`, and
   immediately opens the next active DuckDB file.
4. New usage events write to the new active file.

The request path must not wait for JuiceFS copy, R2 upload, archive checkpoint,
or cold catalog publication.

Recommended initial rollover threshold: `512MiB` or `1GiB`. Start smaller than
`2GiB` so crash recovery and cold-query candidate files stay cheap.

## Asynchronous Archive Path

A background sealer consumes `pending_seal` segments:

1. Open the pending segment in a controlled background task.
2. Run a final checkpoint if needed.
3. Verify minimal integrity metadata: row count, min/max timestamp, size.
4. Copy or move the closed file to JuiceFS `segments`.
5. Build or update catalog entries for the segment.
6. Atomically publish the segment as `archived`.
7. Delete the local pending file after publication succeeds.

Failure is retried with backoff. Archive lag is surfaced in admin/runtime
status, but it does not fail user requests.

The sealer must never copy a DuckDB file that is still being written. Async
archive means archive publication is asynchronous; it does not mean copying a
live mutable database file.

## Query Model

The API should expose one logical usage surface while allowing the frontend to
choose a source:

- `hot`: active DuckDB only.
- `archive`: immutable archived segments only.
- `all`: active plus archived data.

Default admin usage views should query `hot` or a recent time window. Historical
views can query `archive` or `all`.

List queries remain capped and return lightweight rows only. Detail queries use
`event_id`:

1. Check active DuckDB.
2. Check catalog locator.
3. Open only the matching archived segment read-only.
4. Return heavy diagnostic fields from that one segment.

Counts and charts should use catalog rollups plus active DuckDB deltas. They
should not attach and scan every archived DuckDB segment.

## Frontend Behavior

The usage UI should make data source visible:

- Recent/Hot usage: default fast view.
- Historical archive: explicit mode.
- All usage: combines active and archived totals when needed.

If archive lag exists, show it as operational status, for example:

```text
Archive pending: 2 segments, 1.4 GiB
```

The page can temporarily miss events from `pending_seal` segments until they
are cataloged. That is acceptable because usage events are diagnostic data.

## Failure Handling

Business traffic behavior:

- Active DuckDB append failure is logged and retried by the usage buffer.
- Archive copy/catalog failure is logged and retried by the sealer.
- Archive failure must not affect Codex/Kiro request dispatch.

Operator behavior:

- Expose active segment size, pending segment count, pending bytes, last archive
  error, and archive lag.
- Keep manual recovery possible by inspecting local pending files and JuiceFS
  archived files.

## Implementation Boundaries

The first implementation should only change usage analytics storage. It should
not move SQLite control data, auth JSON, gateway routing, or provider request
logic.

The existing single-file `DuckDbUsageRepository` should become a tier-aware
store with:

- one local active writer
- one background sealer
- one catalog reader/writer
- read-only archived segment readers opened only for narrow queries

The current frontend/admin API shape should remain backward compatible. New
source filters can be additive.

## Verification

Minimum verification:

- Unit test active append and query behavior.
- Unit test rollover creates a new active file without waiting for archive.
- Unit test sealer publishes an immutable segment and catalog entry.
- Unit test detail lookup finds an archived event by `event_id`.
- Unit test archive failure does not break active appends.
- Integration smoke test with a small rollover threshold and a local directory
  standing in for JuiceFS.

Production rollout should start with a small threshold and verbose archive
status logging, then increase threshold after observing memory, rollover time,
and archive lag.
