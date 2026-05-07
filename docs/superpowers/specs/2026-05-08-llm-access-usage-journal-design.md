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
- Admin usage queries continue to read settled DuckDB analytics and may report
  ingestion lag separately.

## Architecture

Introduce a new workspace library crate:

```text
llm-usage-journal/
```

The crate owns the journal wire format, writer, reader, consumer claim logic,
retention logic, and CLI-friendly inspection primitives. Both the API producer
and analytics consumer depend on this library.

Use one independent analytics process:

```text
llm-access API process
  -> SQLite rollup sink
  -> local usage journal writer

llm-access usage worker process
  -> claim sealed journal file
  -> read compressed blocks in batches
  -> append rows to DuckDB
  -> delete journal file after successful DuckDB commit
```

Implement the worker as a separate `llm-access-usage-worker` binary target and
run it as a separate systemd unit from the API service. The important boundary
is process isolation: DuckDB memory growth or stalls must not consume the API
process memory budget.

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
6. Insert rows into DuckDB in batches.
7. Commit DuckDB writes.
8. Delete the claimed journal file.

If validation fails, the worker moves the file to a configured bad-file
directory or deletes it only when `journal_delete_bad_files = true`. The default
is to quarantine bad files so an operator can inspect them with the CLI.

If DuckDB import fails before commit, the worker keeps the claimed file for
retry. If the process crashes after DuckDB commit but before file delete, startup
may retry the file. Therefore imports must be idempotent by `event_id`.

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
- worker last successful import timestamp
- worker last error

The frontend only visualizes this status and backlog. It does not browse raw
journal events.

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
```

The API process requires `usage_journal_enabled = true` before DuckDB is removed
from the hot process. Migration should support a short shadow period where the
existing DuckDB analytics writer and the journal writer both receive events, but
only for verification. The final production state must not keep DuckDB writes in
the API process.

## Migration Plan

1. Add the `llm-usage-journal` crate and unit tests for writer, reader,
   rollover, retention, CRC validation, and corrupted-tail handling.
2. Add the CLI and verify it can list, inspect, dump, and grep generated
   journals.
3. Add a journal-backed `UsageEventSink` and wire it into `UsageAccounting`
   while keeping SQLite rollups in process.
4. Add the independent analytics worker and consumer state database.
5. Add idempotent DuckDB import tests that retry after a simulated post-commit
   crash.
6. Add admin status APIs and a small frontend panel for backlog/status only.
7. Run a shadow deployment with both DuckDB and journal writes enabled.
8. Cut over production so the API process no longer opens DuckDB analytics for
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
- CLI `inspect` validates metadata without dumping full payloads.
- Admin status reports backlog and dropped-file counters.

Required operational verification:

- API process can serve traffic with the worker stopped.
- Worker memory growth does not affect API process RSS.
- A large diagnostic payload workload compresses into bounded journal files.
- Retention pressure produces visible counters and logs.
- Restarting the API process recovers the active sequence without consuming
  partial files.

## Open Decisions Fixed By This Spec

- Use a custom binary block journal, not NDJSON or Parquet.
- Use `postcard + zstd + crc32c`, not `rkyv`.
- Use block-level CRC only.
- Consume whole sealed files and delete them after successful import.
- Allow deletion of old unconsumed sealed files under retention pressure.
- Keep raw journal inspection in CLI, not in the usage events frontend.
- Show only journal backlog/status in the admin frontend.
