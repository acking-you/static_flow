# llm-access usage worker JuiceFS cache redesign

Date: 2026-05-16
Status: approved in brainstorming, pending implementation plan

## Summary

Replace the current worker-owned usage persistence path that mixes local active
DuckDB, JuiceFS archived segments, and direct R2 detail uploads with a single
worker-owned JuiceFS usage mount that uses read cache and writeback. Keep
`control` durability and API request accounting unchanged. Reset all existing
usage analytics data and rebuild from empty state after rollout.

This redesign is intentionally scoped to the usage worker side. The API service
continues to write SQLite rollups and hot local usage-journal files. Legacy
public/admin usage routes remain proxied to the worker without API behavior
changes.

## Problem

The current usage analytics pipeline is too expensive for a cost-sensitive
deployment:

- detail payloads are uploaded directly to R2, creating unnecessary bandwidth
  pressure for data that is operationally disposable;
- worker cleanup is not aggressive enough, so stale usage artifacts can linger
  even after retention windows change;
- any cleanup strategy that reads JuiceFS `.duckdb` contents to decide what to
  delete would spend the same bandwidth it is supposed to save.

At the same time, usage details must still exist for troubleshooting, and quota
accounting must remain correct. The boundary is that SQLite rollups remain the
source of truth for quota/account usage, while worker-owned analytics data is
allowed to be lossy.

## Goals

- remove direct R2 detail uploads from the worker path;
- keep usage details available for recent troubleshooting windows;
- make retention aggressively delete old and meaningless usage data;
- ensure cleanup only reads low-cost metadata, never JuiceFS `.duckdb` content
  or detail blob content;
- keep the change worker-only from a service ownership perspective;
- make the final rollout note explicit that only the worker needs updating.

## Non-goals

- no API behavior rewrite;
- no SQLite quota/accounting changes;
- no migration or compatibility for old usage analytics objects;
- no attempt to preserve historical usage analytics data during the switch.

## Scope boundary

This work is worker-only.

Affected runtime ownership:

- `llm-access-usage-worker.service`
- worker-owned usage mount/unit and its environment/config
- worker-owned usage cleanup, retention, and detail persistence logic

Not changed in behavior:

- `llm-access.service` API request handling
- SQLite control DB semantics
- API-to-worker usage query proxy contract

The API service may continue to use the existing `/mnt/llm-access` control
mount and local `/var/lib/staticflow/llm-access/usage-journal` hot path. The
worker rollout may require systemd/mount changes, but those changes remain
worker-scoped operationally and do not require shipping a new API binary.

## Chosen approach

Use a dedicated JuiceFS mount for worker-owned usage persistence, separate from
the existing control mount.

Recommended mount split:

- control mount: `/mnt/llm-access`
  - SQLite control DB
  - auth snapshots
  - stable config
- usage mount: `/mnt/llm-access-usage`
  - archived usage segments
  - segment catalog
  - usage detail blobs
  - worker-owned disposable usage metadata

The dedicated usage mount enables read cache and writeback only for usage
artifacts. This avoids applying aggressive cache semantics to `control`.

## Storage layout

Keep hot mutable producer state local:

- usage journal: `/var/lib/staticflow/llm-access/usage-journal`
- active mutable DuckDB: `/var/lib/staticflow/llm-access/analytics-active`

Persist worker-owned historical artifacts on the dedicated usage mount:

- segments:
  `/mnt/llm-access-usage/analytics/segments/YYYY-MM-DD/<segment>.duckdb`
- catalog:
  `/mnt/llm-access-usage/analytics/catalog/...`
- details:
  `/mnt/llm-access-usage/details/YYYY-MM-DD/<event-id>.json.zst`

Time-bucket directories are mandatory for `segments` and `details`. Cleanup is
directory-oriented, not content-oriented.

## Cleanup I/O contract

Worker cleanup must obey these rules:

Allowed reads:

- catalog files
- directory listings
- filenames
- file mtimes/sizes
- local worker state files

Forbidden reads:

- JuiceFS `.duckdb` file contents
- JuiceFS detail blob contents
- any content scan whose only purpose is deciding deletion

Deletion decisions must be driven by metadata only.

## Retention and deletion rules

`retention_days` stays configurable. The default may remain `7`, but runtime
config is the source of truth.

Cleanup rules:

1. Delete expired `details/YYYY-MM-DD/` buckets by directory date only.
2. Delete expired `segments/YYYY-MM-DD/` buckets by directory date, after
   removing matching catalog entries.
3. Delete orphan segment files by filename and catalog membership only. Do not
   open the segment file.
4. Delete orphan detail files by bucket/date policy only. No detail payload
   reads.
5. Delete stale `tmp`, `uploading`, `partial`, `bad`, and abandoned worker
   scratch files by path convention and age only.
6. If a bucket or worker-owned usage metadata is inconsistent, damaged, or too
   expensive to validate, delete it instead of repairing it.

The worker should run cleanup frequently enough that disk usage converges
quickly after retention changes, but the cleanup loop must remain metadata-only
for remote paths.

## One-time reset during rollout

Existing usage analytics data is intentionally disposable and should be deleted
before or during rollout. The worker starts fresh from empty historical usage
state.

Delete on GCP:

- old archived DuckDB segments;
- old catalog contents;
- old local active DuckDB files;
- old usage journal backlog that is not needed for the fresh start;
- old worker scratch/tmp/bad/orphan directories;
- obsolete direct-R2 detail configuration for the worker.

No migration is required. If a usage query asks for data that no longer exists,
the worker returns an empty result set.

## Worker configuration changes

The worker should stop depending on `LLM_ACCESS_USAGE_DETAILS_OBJECT_STORE_URL`
for normal detail persistence.

Worker runtime should instead use:

- a dedicated usage mount path such as `/mnt/llm-access-usage`;
- cached catalog path under that mount;
- cached segment archive path under that mount;
- cached detail root under that mount;
- existing local journal path;
- existing local active DuckDB path.

Systemd for the worker should require the dedicated usage mount and keep its
cache directory on local VM storage, not inside the mount itself.

## Operational guidance

Rollout and rollback should be worker-centered:

- stop worker;
- clear old worker-owned usage data;
- update worker binary/config/systemd/mount;
- start worker on empty usage state;
- verify worker status and new usage writes;
- leave API running unchanged.

The final operator-facing release note must state clearly:

> only the worker needs updating; the API service does not require a matching
> binary rollout for this change.

## Verification expectations

Implementation should prove:

- worker writes details to the usage mount instead of direct R2;
- worker cleanup deletes expired buckets without opening JuiceFS segment files;
- retention changes converge quickly;
- empty-history rollout still serves usage queries without API breakage;
- GCP disk usage improves after removing old worker-owned usage artifacts.

## Risks

- recent usage details may be lost if the VM or JuiceFS usage mount fails before
  writeback flushes;
- resetting old usage analytics means historical admin views start from the new
  rollout point;
- path/date naming becomes part of the deletion contract and must stay stable.

These risks are acceptable because SQLite rollups remain the source of truth for
quota/account usage and worker-owned usage analytics are intentionally
disposable.
