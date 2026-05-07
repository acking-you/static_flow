# llm-access Usage Journal Design

## Problem

The standalone `llm-access` service currently keeps request handling, SQLite
rollups, and DuckDB analytics inside one process. The request path emits
`UsageEvent` values into an in-memory channel, then the background flusher first
persists authoritative rollups to SQLite and then writes analytics rows to
DuckDB. When DuckDB or its memory pressure becomes unhealthy, the service can
stay alive while request handling becomes extremely slow.

This design separates the hot API process from DuckDB analytics. The API process
must only do cheap accounting and append diagnostic usage events to a compact
local journal. A separate analytics worker consumes sealed journal files into
DuckDB in batches.

Usage events are operational diagnostics. SQLite key rollups remain the
authoritative accounting source. It is acceptable for old unconsumed journal
files to be dropped under configured retention pressure, as long as the drop is
visible in logs and metrics.

## Goals

- Keep DuckDB out of the hot `llm-access` API process.
- Preserve low-latency, bounded-memory usage event writes.
- Make journal writes batch-oriented and compression-friendly.
- Roll journal files by both size and age.
- Keep at most a configured number of journal files, deleting oldest sealed
  files when necessary, even if they have not been consumed.
- Let the analytics worker claim sealed files, import them into DuckDB, and
  delete each file after a successful import.
- Keep the DuckDB side compatible with the current tiered storage contract:
  write active DuckDB segments on local VM disk first, then seal/compact/archive
  them into JuiceFS-backed segment and catalog directories.
- Keep existing local StaticFlow and operator usage-detail workflows compatible
  by exposing settled usage queries on a new independent local port while
  preserving the old admin usage HTTP paths as proxy-compatible entrypoints.
- Let the frontend show the current worker consumption progress, including the
  currently claimed journal file, processed blocks/events/bytes, heartbeat, and
  last error.
- Provide a CLI for human or agent inspection of journal files.
- Surface journal backlog and loss counters in admin/runtime status without
  embedding a full journal browser in the existing usage page.

## Non-Goals

- The journal is not a second source of truth for billing or remaining quota.
- The existing usage events page will not read raw journal contents directly.
- The API process will not run DuckDB queries or DuckDB imports.
- The first implementation will not support multiple concurrent consumers.
- The format will not optimize for zero-copy reads inside Rust. Compression and
  schema evolution matter more than avoiding deserialization allocations.

## Current Code Boundary

The existing service opens the SQLite control repository, the DuckDB analytics
repository, and `UsageAccounting` together during runtime initialization. The
resulting store set exposes DuckDB as the active `UsageAnalyticsStore`.

The existing `UsageEvent` shape already includes large optional diagnostic
payloads such as `client_request_body_json`, `upstream_request_body_json`, and
`full_request_json`. Those fields are valuable for incident analysis, but they
make an in-memory retry buffer and direct DuckDB ingestion expensive under heavy
traffic.

The new design keeps the provider-facing `UsageEventSink` trait, but changes the
runtime composition:

- SQLite rollups continue in the API process.
- The API process appends normalized event batches to the journal.
- DuckDB analytics writes move to an independent worker process.
- Admin usage query paths keep their existing HTTP contract, but they no longer
  read DuckDB in the API process. They call the independent usage query service
  over its configured local URL and may report ingestion lag separately.

## Architecture

Introduce a new workspace library crate:

```text
llm-usage-journal/
```

The crate owns the journal wire format, writer, reader, consumer claim logic,
retention logic, and CLI-friendly inspection primitives. Both the API producer
and analytics consumer depend on this library.

Use one independent analytics process that has two responsibilities: consume
sealed journal files into DuckDB and serve read-only settled usage queries over
an independent loopback HTTP port.

```text
llm-access API process
  -> SQLite rollup sink
  -> local usage journal writer
  -> proxy legacy admin usage query paths to usage query port

llm-access usage worker process
  -> claim sealed journal file
  -> publish current consumption progress
  -> read compressed blocks in batches
  -> append rows to local active DuckDB segment
  -> seal/compact/archive DuckDB segments into JuiceFS
  -> serve read-only usage query API on a separate port
  -> delete journal file after successful DuckDB commit
```

Implement the worker as a separate `llm-access-usage-worker` binary target and
run it as a separate systemd unit from the API service. The important boundary
is process isolation: DuckDB memory growth or stalls must not consume the API
process memory budget.

Default ports:

- API service: `127.0.0.1:19080`
- Usage worker/query service: `127.0.0.1:19081`

The exact ports remain configurable, but production deployment must not expose
the usage worker directly to public traffic. Caddy and pb-mapper may route local
or private operator traffic to it.

## Remote Usage Query Compatibility

Existing clients use these settled usage query paths:

```text
/admin/llm-gateway/usage
/admin/llm-gateway/usage/:event_id
/admin/kiro-gateway/usage
/admin/kiro-gateway/usage/:event_id
```

Those paths remain valid on the main API service. After cutover, the main API
service handles them by forwarding the request to the configured usage query
base URL, then returning the same JSON response shape and HTTP status behavior
expected by the frontend and local StaticFlow tooling.

The independent usage query service also exposes the same paths on its own
port. This gives operators and local agents a direct detail-query path when they
are connected to the remote `llm-access` service through a separate private
relay. The intended deployment shape is:

```text
cloud llm-access API          127.0.0.1:19080
cloud llm-access usage query  127.0.0.1:19081

local mirror of API           127.0.0.1:19182
local mirror of usage query   127.0.0.1:19183
```

The local mirror ports are examples, not hard-coded values. The compatibility
contract is that existing admin pages can continue to call the old API paths,
while direct operational usage detail lookup can be configured to call the
usage query mirror port. If the usage query service is unavailable, the main API
service returns an explicit `503` for usage list/detail routes; model serving
and SQLite-backed accounting continue unaffected.

## DuckDB Storage Compatibility

The usage worker must reuse the current tiered DuckDB storage shape instead of
writing one mutable DuckDB file on JuiceFS.

The settled analytics path stays:

```text
local active directory:
  /var/lib/staticflow/llm-access/analytics-active

local compact work directory:
  /var/lib/staticflow/llm-access/analytics-active/compacting

JuiceFS archive directory:
  /mnt/llm-access/analytics/segments

JuiceFS catalog directory:
  /mnt/llm-access/analytics/catalog
```

The worker imports journal batches into the local active DuckDB segment through
the same `TieredDuckDbUsageConfig` contract used today. Rollover still closes
the local active segment, compacts it locally, publishes the immutable segment
to JuiceFS, and updates the JuiceFS-backed catalog. Journal consumption is only
the new upstream ingestion source; it does not replace the local-first DuckDB
active segment and archive pipeline.

Usage queries served by the worker read through the same tiered
`UsageAnalyticsStore`: current rows from the local active segment and historical
rows from archived JuiceFS segments selected by the catalog.

## Storage Layout

Use one configurable journal root:

```text
<journal_root>/
  active/
  sealed/
  consuming/
```

Active files are open for append by the API process:

```text
active/usage-000000000042.open
```

Sealed files are immutable and ready for consumption:

```text
sealed/usage-000000000041.journal
```

The worker claims a file by atomic rename:

```text
sealed/usage-000000000041.journal
  -> consuming/usage-000000000041.<worker_id>.journal
```

After a successful DuckDB import, the worker deletes the file from
`consuming/`. A crashed worker leaves a claimed file in `consuming/`; startup
recovery moves stale claimed files back to `sealed/` after the configured lease
age.

Retention applies to sealed and stale consuming files. The current active file
is never deleted by retention. If total journal file count exceeds
`journal_max_files`, the writer deletes oldest sealed files first, then oldest
stale consuming files. Deleting an unconsumed file is allowed and must increment
explicit drop counters.

## File Format

Use a custom binary block format:

```text
file header
block header + zstd(postcard(JournalUsageBatchV1)) + crc32c
block header + zstd(postcard(JournalUsageBatchV1)) + crc32c
...
file footer
```

Do not serialize `UsageEvent` directly. The journal crate defines stable,
versioned wire structs:

```rust
pub struct JournalUsageEventV1 { ... }
pub struct JournalUsageBatchV1 {
    pub events: Vec<JournalUsageEventV1>,
}
```

The conversion from `UsageEvent` to `JournalUsageEventV1` is explicit. New
fields are additive and must have defaults on read. Breaking changes require a
new schema version and a reader path for older versions still within retention.

### Header

The file header contains:

- magic bytes: `LLMUJNL1`
- format version
- schema version
- file sequence
- created timestamp
- writer id
- configured compression algorithm

### Block

Each block targets a bounded uncompressed payload size and event count. The
block header contains:

- block sequence
- event count
- minimum event timestamp
- maximum event timestamp
- uncompressed payload length
- compressed payload length
- crc32c

The CRC covers the block header bytes excluding the CRC field plus the
compressed payload. This catches torn writes, truncated files, and corrupt
payloads before the consumer imports or deletes the file. There is no per-event
CRC and no whole-file CRC in the first implementation.

### Footer

The footer is written only when a file is sealed. It contains:

- file sequence
- created timestamp
- sealed timestamp
- event count
- block count
- minimum event timestamp
- maximum event timestamp
- uncompressed bytes
- compressed bytes

If a file is found without a valid footer during startup, it is treated as an
unsealed active or partial file. The writer may recover valid blocks from it
only when it owns the active sequence; the worker never consumes files without a
valid footer.

## Encoding Choice

Use:

- `postcard` for compact serde-compatible binary encoding.
- `zstd` for high compression ratio on repeated JSON payloads and headers.
- `crc32c` for block-level corruption detection.

Do not use `rkyv` for this journal. Compression removes most practical
zero-copy benefits, while `rkyv` makes schema evolution, CLI inspection, and
format debugging harder. A serde-based versioned wire type is simpler and safer
for an operational log format.

## Producer Path

The API process producer uses one long-lived `JournalWriter`.

The hot path must be bounded:

1. Convert `UsageEvent` to `JournalUsageEventV1`.
2. Append the event to an in-memory block buffer.
3. Flush the block when event count or uncompressed byte target is reached.
4. Rotate the file when size or age threshold is reached.
5. Enforce retention after sealing a file.

The producer batches events before compression. It must not compress or fsync
one file per event. Initial defaults:

- `journal_block_target_uncompressed_bytes = 1MiB`
- `journal_block_max_events = 1024`
- `journal_zstd_level = 3`
- `journal_fsync_interval_ms = 250`

`journal_fsync_interval_ms = 0` means fsync every flushed block. A disabled
fsync mode can exist for local development, but production defaults must prefer
bounded crash-tail loss over synchronous latency spikes.

Journal write failure must not fail the user request after the SQLite rollup is
persisted. The producer records the failure with counters and logs, drops the
diagnostic event, and continues. This matches the data contract: diagnostics may
have gaps, accounting must not.

## Rollover And Retention

A journal file seals when either condition is met:

- `journal_max_file_bytes` is reached after a flushed block.
- `journal_max_file_age_ms` has elapsed since file creation.

Initial defaults:

- `journal_max_file_bytes = 64MiB`
- `journal_max_file_age_ms = 300000`
- `journal_max_files = 128`

Size rollover is evaluated after block flush, so a file may exceed the limit by
at most one compressed block plus footer. Age rollover is evaluated before each
append and after each block flush.

Retention count includes files in `sealed/` plus stale files in `consuming/`.
The current active file is excluded. When retention deletes an unconsumed file,
it increments:

- `usage_journal_dropped_files_total`
- `usage_journal_dropped_bytes_total`
- `usage_journal_dropped_unconsumed_files_total`

The writer logs the deleted path, sequence, bytes, and whether the file was
already claimed.

## Consumer Path

The analytics worker consumes whole files. A file is the commit unit.

1. Claim the oldest sealed file with atomic rename into `consuming/`.
2. Validate header, all block CRCs, and footer.
3. Read blocks one by one.
4. Decode each `JournalUsageBatchV1`.
5. Convert journal events into DuckDB `UsageEventRow` values.
6. Insert rows into the local active DuckDB segment in batches.
7. Commit DuckDB writes.
8. Delete the claimed journal file.

If validation fails, the worker moves the file to a configured bad-file
directory or deletes it only when `journal_delete_bad_files = true`. The default
is to quarantine bad files so an operator can inspect them with the CLI.

If DuckDB import fails before commit, the worker keeps the claimed file for
retry. If the process crashes after DuckDB commit but before file delete, startup
may retry the file. Therefore imports must be idempotent by `event_id`.

The journal file is deleted after the local active DuckDB commit succeeds. It
does not wait for the tiered DuckDB archive step to copy a sealed DuckDB segment
to JuiceFS. At that point the event is durably represented by the local DuckDB
active segment, and the existing tiered DuckDB sealer owns the later archive
publication.

The first implementation uses a local consumer state SQLite database under the
journal root:

```text
consumer-state.sqlite3
```

It records consumed file sequence, file digest, event count, and import
timestamp. The worker checks this state before importing a claimed file. DuckDB
insertion must also deduplicate `event_id` within the imported batch before
append. This avoids duplicate analytics rows without adding request-path cost.

## CLI

Add a standalone `llm-usage-journal` CLI binary around the journal crate. It
must not require the API service or the worker service to be running.

Required commands:

```text
llm-usage-journal list --dir <journal_root>
llm-usage-journal inspect <file>
llm-usage-journal stats --dir <journal_root>
llm-usage-journal dump <file> --limit 50
llm-usage-journal grep --dir <journal_root> --key-name <name> --since <duration>
llm-usage-journal grep --dir <journal_root> --event-id <event_id>
```

`list` reports active, sealed, consuming, and bad files with sequence, age,
bytes, footer validity, and event count when available. `inspect` validates CRCs
and footer metadata without printing full payloads by default. `dump` prints
JSON lines for selected events.

The CLI is the raw journal inspection path. The frontend should not duplicate
this functionality.

## Worker Progress Reporting

The worker exposes consumption progress as structured status, not raw journal
content. The status is updated when a file is claimed, after every successfully
committed block, when a file is deleted, and when an error or retry state is
entered.

The worker-only endpoint is:

```text
GET /admin/llm-access/usage-worker/status
```

The main API service exposes a combined frontend endpoint:

```text
GET /admin/llm-access/usage-journal/status
```

The combined endpoint returns API-process producer state plus worker state. The
API process owns producer fields such as active journal file, sealed backlog,
and writer drop counters. Worker state is fetched from `usage_query_base_url`.
If the worker is unavailable, the endpoint still returns producer state and a
worker state of `unreachable` with the proxy error message.

The worker progress snapshot contains:

- state: `idle`, `claiming`, `importing`, `deleting`, `retrying`, `unreachable`
- current file path, sequence, and claimed timestamp
- total blocks, events, and compressed bytes from the sealed file footer
- processed blocks, events, and compressed bytes
- successfully committed DuckDB rows for the current file
- progress percentage derived from processed events over total events
- current batch event count
- import rate in events per second over the current file
- last heartbeat timestamp
- last successful file sequence and import timestamp
- last error message and timestamp

The worker writes the same progress snapshot to `consumer-state.sqlite3` after
each block commit. This makes progress visible after a worker restart and avoids
requiring the frontend to infer progress by scanning files or querying DuckDB.

The frontend displays this in the journal status panel as a progress bar and a
small detail table. It must not render full event payloads or reuse the usage
events table for journal inspection.

## Admin Status

Add a small admin/runtime status surface, separate from the existing usage
events table. It reports:

- journal enabled
- journal root
- active file sequence
- active file bytes
- active file age
- sealed file count
- sealed bytes
- oldest sealed age
- consuming file count
- dropped file counters
- write failure counters
- last producer seal timestamp
- worker running status when available
- usage query base URL and last successful proxy check
- worker current state
- worker current file sequence and path
- worker current file processed/total blocks
- worker current file processed/total events
- worker current file processed/total compressed bytes
- worker current file progress percentage
- worker import rate and heartbeat age
- worker last successful import timestamp
- worker last error
- DuckDB active segment bytes and archive backlog from the tiered store

The frontend only visualizes this status, backlog, and worker progress. It does
not browse raw journal events.

## Configuration

Add runtime configuration fields with safe defaults:

```text
usage_journal_enabled
usage_journal_dir
usage_journal_max_file_bytes
usage_journal_max_file_age_ms
usage_journal_max_files
usage_journal_block_target_uncompressed_bytes
usage_journal_block_max_events
usage_journal_fsync_interval_ms
usage_journal_zstd_level
usage_journal_consumer_lease_ms
usage_journal_delete_bad_files
usage_query_bind_addr
usage_query_base_url
```

The API process requires `usage_journal_enabled = true` before DuckDB is removed
from the hot process. Migration should support a short shadow period where the
existing DuckDB analytics writer and the journal writer both receive events, but
only for verification. The final production state must not keep DuckDB writes in
the API process.

The usage worker requires the same tiered DuckDB settings currently used by the
API service:

```text
duckdb_active_dir
duckdb_archive_dir
duckdb_catalog_dir
duckdb_rollover_bytes
duckdb_usage_memory_limit_mib
duckdb_usage_checkpoint_threshold_mib
```

After cutover, those DuckDB settings belong to `llm-access-usage-worker`, not
the main API service. The API service only needs `usage_query_base_url` so it
can preserve the existing admin usage routes.

## Migration Plan

1. Add the `llm-usage-journal` crate and unit tests for writer, reader,
   rollover, retention, CRC validation, and corrupted-tail handling.
2. Add the CLI and verify it can list, inspect, dump, and grep generated
   journals.
3. Add a journal-backed `UsageEventSink` and wire it into `UsageAccounting`
   while keeping SQLite rollups in process.
4. Add the independent analytics worker, consumer state database, and tiered
   DuckDB repository wiring using the existing local-active-plus-JuiceFS storage
   contract.
5. Add the worker read-only usage query HTTP API on the independent port.
6. Add API-process proxy compatibility for existing admin usage routes.
7. Add worker progress reporting, the combined journal status endpoint, and the
   frontend status panel.
8. Add idempotent DuckDB import tests that retry after a simulated post-commit
   crash.
9. Run a shadow deployment with both DuckDB and journal writes enabled.
10. Cut over production so the API process no longer opens DuckDB analytics for
   writes.

## Testing

Required test coverage:

- Writer creates a valid journal with header, blocks, footer, and CRCs.
- Reader rejects a corrupted block.
- Reader ignores or reports an unsealed file without treating it as consumable.
- Rollover seals by size.
- Rollover seals by age.
- Retention deletes oldest sealed files and never deletes active file.
- Retention counters distinguish unconsumed deletes.
- Consumer claims files with atomic rename.
- Consumer deletes a file after successful import.
- Consumer retries a file after failed DuckDB import.
- Consumer does not duplicate rows after retrying a post-commit, pre-delete
  crash.
- Worker writes imported rows into the tiered DuckDB local active segment and
  preserves local rollover plus JuiceFS archive publication behavior.
- Usage query service returns the same JSON shape as the existing admin usage
  list/detail endpoints.
- API-process usage route proxy returns the same response as the usage query
  service and returns `503` when the usage query service is down.
- Worker progress status updates after claiming a file, after committing a
  block, after deleting a file, and after an import error.
- Combined journal status returns producer state even when the worker status
  proxy is unreachable.
- Frontend journal status panel renders backlog plus worker progress without
  rendering raw usage payloads.
- CLI `inspect` validates metadata without dumping full payloads.
- Admin status reports backlog and dropped-file counters.

Required operational verification:

- API process can serve traffic with the worker stopped.
- Worker memory growth does not affect API process RSS.
- Stopping the usage worker makes only usage list/detail routes unavailable; API
  model serving and SQLite accounting remain healthy.
- Local StaticFlow can query remote usage detail through the direct usage-query
  mirror port and through the legacy API mirror path.
- During a large import, the frontend progress bar advances as journal blocks
  are committed into DuckDB.
- A large diagnostic payload workload compresses into bounded journal files.
- Retention pressure produces visible counters and logs.
- Restarting the API process recovers the active sequence without consuming
  partial files.

## Open Decisions Fixed By This Spec

- Use a custom binary block journal, not NDJSON or Parquet.
- Use `postcard + zstd + crc32c`, not `rkyv`.
- Use block-level CRC only.
- Consume whole sealed files and delete them after successful import.
- Treat successful local active DuckDB commit as the journal delete point; DuckDB
  segment archival to JuiceFS remains the existing asynchronous tiered-store
  responsibility.
- Allow deletion of old unconsumed sealed files under retention pressure.
- Keep raw journal inspection in CLI, not in the usage events frontend.
- Show journal backlog/status and worker consumption progress in the admin
  frontend.
- Serve settled usage queries from the independent usage worker/query port and
  keep the old admin usage paths as compatibility proxies.
