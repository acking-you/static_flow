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
- Local compact work directory on VM block storage:
  `/var/lib/staticflow/llm-access/analytics-active/compacting`
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

The catalog is a SQLite database under the catalog directory. It records segment
metadata and lookup aids with this explicit contract:

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

Recommended initial rollover threshold: `512MiB`. Start smaller than `2GiB` so
crash recovery, local compaction, and cold-query candidate files stay cheap.

## Asynchronous Archive Path

A background sealer consumes `pending_seal` segments:

1. Open the pending segment read-only in a controlled background task.
2. Run a final checkpoint before sealing if the segment has not already been
   checkpointed.
3. Create a fresh DuckDB file in the local compact work directory.
4. Initialize the fresh file with the current `llm-access` DuckDB schema.
5. Attach the pending segment read-only and copy table contents into the fresh
   file with DuckDB SQL. Rust must not materialize rows for this copy.
6. Run `CHECKPOINT` on the compacted file.
7. Verify integrity metadata on the compacted file: row count, min/max
   timestamp, and event-id count must match the pending segment.
8. Copy the compacted file to a temporary archive path on JuiceFS `segments`.
9. Atomically rename the temporary archive path to its final immutable segment
   path.
10. Build or update catalog entries for the final archived segment.
11. Atomically publish the segment as `archived`.
12. Delete the local pending and compact work files after publication succeeds.

Failure is retried with backoff. Archive lag is surfaced in admin/runtime
status, but it does not fail user requests.

The sealer must never copy a DuckDB file that is still being written. Async
archive means archive publication is asynchronous; it does not mean copying a
live mutable database file.

The sealer must also never archive the original pending file directly. DuckDB
files can contain large reusable free-block regions after appends, checkpoints,
failed writes, and schema/index churn. Direct file copy preserves that physical
slack and makes object storage permanently inherit the hot file's bloat. The
archive object should be a fresh compacted DuckDB database whose physical bytes
come from a logical rewrite of the intended schema.

## Compact-Then-Archive Contract

Compaction is a local background operation, not a request-path operation:

```text
active write file
  -> pending closed file on local disk
  -> compacted temporary DuckDB on local disk
  -> temporary JuiceFS archive object
  -> immutable JuiceFS archive object
  -> catalog publication
```

The compacted file must preserve the logical contract of `usage_events` and
related usage tables, but it must not preserve obsolete physical layout,
free-block regions, or legacy explicit indexes. The destination database is
created from the current migration SQL and populated from the pending source.
This keeps archive files aligned with the current storage contract even if an
older pending segment was produced before index cleanup.

Catalog publication is the commit point. Before catalog publication, any
failure leaves the pending file retryable. After catalog publication, the
archive file is immutable and discoverable by normal query paths. If the
archive copy succeeds but catalog publication fails, retry may reuse or replace
the same final archive file only after validating that its row count and time
range match the pending segment.

Temporary names are required:

```text
/var/lib/staticflow/llm-access/analytics-active/compacting/<segment>.tmp.duckdb
/mnt/llm-access/analytics/segments/<segment>.uploading.duckdb
/mnt/llm-access/analytics/segments/<segment>.duckdb
```

Startup recovery should remove stale local compact temporary files and retry
any pending segment files. It should ignore `.uploading.duckdb` archive files
unless an operator explicitly chooses to inspect or remove them.

The sealer concurrency is one. Multiple concurrent compactions can multiply
DuckDB memory use and JuiceFS cache pressure on the 2c8g cloud host. Rollover
may enqueue many pending segments, but only one sealer should be actively
compacting or uploading at a time.

The compact connection should use a bounded DuckDB memory setting. The exact
SQL should be verified against the linked DuckDB crate during implementation;
the intended operational default is to keep compaction below the service
memory high watermark rather than relying only on systemd to kill the process.

The catalog should keep `size_bytes` as the final archived file size. Additive
columns may record `source_size_bytes` and `compacted_size_bytes` for
observability, but query correctness must not depend on those columns.

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
