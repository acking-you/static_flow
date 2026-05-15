# llm-access control-plane Neon cutover design

Date: 2026-05-16
Status: approved in brainstorming, pending implementation plan

## Summary

Replace the live `llm-access` control-plane backend from the current SQLite
database on JuiceFS with the Neon Postgres database that was already seeded
from production data on 2026-05-16. The cutover is a single-window hard switch:
both `llm-access.service` and `llm-access-usage-worker.service` move to Neon in
the same maintenance window, with no dual-write period and no attempt to
preserve recent SQLite-side drift after the imported Neon snapshot.

The existing SQLite control file remains on disk as rollback state and audit
material, but it stops being the live source of truth after cutover.

This design is intentionally scoped to the control plane. It does not implement
the future federated multi-node usage architecture. The only usage-side rule
that is part of this design is a hard operational invariant:

- `usage-journal` must remain on local disk and must not be placed on JuiceFS.

## Problem

The current production control plane is anchored to one SQLite file under:

- `/mnt/llm-access/control/llm-access.sqlite3`

That design is operationally simple in single-node mode, but it is not the
correct long-term foundation for:

- multi-node concurrent control writes;
- clean separation between durable control truth and disposable usage
  observability;
- service-level cutovers that do not depend on one shared mutable file over a
  network-backed filesystem.

At the same time, the user has already accepted two key simplifications:

1. the Neon snapshot that was imported on 2026-05-16 can be treated as the new
   control-plane starting truth;
2. recent SQLite-side quota drift after that import does not need to be
   preserved.

These two constraints make a hard cutover strictly better than a dual-write or
incremental replay migration.

## Goals

- replace live control-plane reads and writes with Neon Postgres;
- cut over both API and usage-worker in the same maintenance window;
- keep the old SQLite control DB intact on disk for rollback only;
- eliminate live control dependence on shared SQLite state after cutover;
- keep runtime configuration configuration-driven via shared mounted files;
- explicitly preserve `usage-journal` on local disk only.

## Non-goals

- no dual-write period between SQLite and Postgres;
- no backfill or replay of SQLite writes after the imported Neon snapshot;
- no attempt to make SQLite and Postgres stay in sync after cutover;
- no deletion of the SQLite control file;
- no implementation of the future federated usage architecture in this task;
- no migration of usage analytics storage off its current tiered shape beyond
  restating the local-journal invariant.

## Scope boundary

In scope:

- live control-store backend replacement;
- runtime and config changes required to open Postgres instead of SQLite;
- both `llm-access.service` and `llm-access-usage-worker.service`;
- rollout and rollback procedures;
- verification for live GCP cutover.

Out of scope:

- frontend feature work;
- new usage-query federation logic;
- changes to detail packs, archive namespaces, or node-aware usage views;
- deletion of existing SQLite data.

## Current state

### Control backend

The current code uses `SqliteControlRepository` as the concrete implementation
for the full control-plane store surface.

It is directly opened by:

- API runtime store bootstrap
- usage-worker bootstrap

This is not a single-trait or single-endpoint swap. The concrete repository
currently implements essentially the whole control surface, including:

- `ControlStore`
- `AdminConfigStore`
- `AdminKeyStore`
- `AdminAccountGroupStore`
- `AdminProxyStore`
- `AdminCodexAccountStore`
- `AdminKiroAccountStore`
- `ProviderRouteStore`
- `PublicAccessStore`
- `PublicCommunityStore`
- `PublicUsageStore`
- `PublicSubmissionStore`
- `AdminReviewQueueStore`
- `PublicStatusStore`

### Service wiring

The current systemd units pass SQLite paths directly on startup:

- API unit passes `--sqlite-control ${LLM_ACCESS_SQLITE_CONTROL}`
- usage-worker unit passes `--sqlite-control /mnt/llm-access/control/llm-access.sqlite3`

The usage worker currently also reads control-plane runtime config from the
same repository instance that serves the API.

### Usage-journal invariant

`usage-journal` is already intended to live on local disk:

- `/var/lib/staticflow/llm-access/usage-journal`

However, historical JuiceFS paths and old drop-ins have existed before. This
design treats local-disk-only `usage-journal` as a hard requirement and keeps
that invariant explicit during the Neon cutover.

## Chosen approach

### 1. Full control-store backend replacement

Introduce a Postgres-backed repository implementing the same control-plane
traits that are currently implemented by `SqliteControlRepository`.

This is not a partial split where some tables stay in SQLite. The control plane
has one live source of truth after cutover: Neon Postgres.

### 2. Hard cutover from imported Neon snapshot

Use the already imported Neon dataset as the starting live control state.

There is no dual-write phase and no replay window. At cutover time:

- API switches from SQLite to Postgres;
- usage-worker switches from SQLite to Postgres;
- SQLite stops receiving live control writes.

### 3. Shared mounted config for connection data

Store the live Postgres connection configuration under the shared control mount:

- `/mnt/llm-access/config/neon.env`

This file becomes the persistent operational source for the control-plane
database URL used by both services.

### 4. Preserve local `usage-journal`

Even though this task is control-only, the rollout must also ensure:

- `LLM_ACCESS_USAGE_JOURNAL_DIR=/var/lib/staticflow/llm-access/usage-journal`

No live unit participating in the cutover may point `usage-journal` at a
JuiceFS path.

## Why this approach

This design is preferred over the alternatives for four reasons:

1. the user has already accepted that the imported Neon snapshot is good enough
   to become the new truth, so dual-write complexity buys nothing useful;
2. both API and usage-worker depend on the same control repository surface, so
   splitting the cutover window would create avoidable mixed-truth behavior;
3. keeping SQLite on disk is enough for rollback, so live overlap is not
   needed for safety;
4. `usage-journal` local-disk-only is orthogonal to the control cutover and can
   be preserved as a stable invariant during the switch.

## Architecture

### 1. New control repository

Add a concrete `PostgresControlRepository` that owns the live control schema in
Neon and implements the same trait surface currently served by the SQLite
repository.

The new repository becomes the canonical implementation for:

- key lookup and authentication;
- quota rollup updates;
- admin config reads and writes;
- account management;
- proxy config management;
- public token/account contribution workflows;
- status cache persistence;
- review queue state;
- usage-related control metadata that belongs to the control plane.

### 2. Runtime selection

Runtime bootstrap must choose the control repository via explicit configuration
rather than by assuming one hard-coded SQLite path.

First-version recommendation:

- keep SQLite bootstrap support for rollback only;
- add explicit Postgres config support;
- fail startup if neither backend is configured correctly;
- prefer one unambiguous active control backend at runtime.

This task does not want a hidden heuristic that silently falls back from
Postgres to SQLite when Postgres is broken.

### 3. Shared configuration source

Create:

- `/mnt/llm-access/config/neon.env`

Recommended contents:

- `LLM_ACCESS_CONTROL_DATABASE_URL=postgresql://...`

Optional future keys may include:

- statement timeout settings;
- application name;
- SSL or pool sizing overrides.

Both services must consume the same shared connection config so they switch
truth sources together.

### 4. SQLite preservation

The existing SQLite control DB remains in place at:

- `/mnt/llm-access/control/llm-access.sqlite3`

After cutover it is:

- not deleted;
- not the live source of truth;
- not expected to stay in sync with Postgres;
- retained only for rollback and historical inspection.

## Data ownership

### Live truth after cutover

Neon Postgres owns all control-plane state that currently lives in SQLite.

That includes the schema currently represented by the imported control tables,
such as:

- keys;
- key route config;
- key usage rollups;
- runtime config;
- account groups;
- proxy configs and proxy bindings;
- Codex accounts and status cache;
- Kiro accounts and status cache;
- token requests;
- account contribution requests;
- sponsor requests;
- import jobs and import job items;
- schema migration bookkeeping.

### No split-brain ownership

There is no supported mode where:

- some control writes go to SQLite;
- others go to Postgres;
- and the system tries to pretend both are current.

After cutover, Postgres is authoritative. SQLite is passive.

## Configuration model

### Service env

The live service env should continue to be sourced from:

- `/etc/llm-access/llm-access.env`

But this file should reference the shared mounted Neon config so that:

- secrets and connection rotation remain centralized;
- rollout does not depend on editing binaries or hard-coding values into units.

### Local-journal requirement

The env and effective startup configuration must preserve:

- `LLM_ACCESS_USAGE_JOURNAL_DIR=/var/lib/staticflow/llm-access/usage-journal`

This rule must hold for:

- `llm-access.service`
- `llm-access-usage-worker.service`

Historical `/mnt/llm-access/usage-journal` paths become dead configuration and
must not be reused by this cutover.

## Cutover procedure

### Preconditions

- Neon Postgres already contains the imported control snapshot;
- the imported snapshot is accepted as the new control truth;
- a short maintenance window is acceptable;
- both API and usage-worker can be restarted together;
- the shared Neon config file is present under `/mnt/llm-access/config/neon.env`.

### Execution model

1. Freeze or minimize control-plane mutations during the maintenance window.
2. Confirm the live SQLite file still exists and is readable.
3. Install binaries and config that support Postgres-backed control.
4. Ensure both service units read the Neon config.
5. Ensure both service units still point `usage-journal` to local disk.
6. Restart usage-worker and API within the same window.
7. Verify live reads, writes, and control-plane workflows against Neon.
8. End the maintenance window.

### No replay step

There is deliberately no final SQLite delta replay into Neon before cutover.
That omission is intentional and accepted by product/operations for this task.

## Rollback

Rollback is configuration-driven, not data-migration-driven.

Rollback procedure:

1. stop or drain both services;
2. switch active control backend configuration back to SQLite;
3. restart usage-worker and API;
4. verify control workflows against SQLite;
5. leave Postgres data intact for later inspection.

Rollback does not require:

- deleting Neon data;
- merging Neon writes back into SQLite;
- making both stores consistent after the fact.

## Failure handling

### Postgres unavailable at startup

Startup should fail clearly rather than silently falling back to SQLite.

Reason:

- silent fallback would hide a partial cutover failure;
- it would make it unclear which control truth source is active.

### Postgres runtime failure after cutover

The service should surface explicit control-store failure. Operators may then
choose to execute the rollback procedure.

### usage-journal path regression

If the effective service env points `usage-journal` back to a JuiceFS path,
cutover is considered invalid and must not be treated as successful.

## Operational invariants

After cutover:

- Neon Postgres is the only live control source;
- both API and usage-worker use the same control source;
- SQLite control file still exists;
- `usage-journal` remains on local disk only;
- no live service depends on `/mnt/llm-access/usage-journal`;
- no hidden runtime fallback rewires control back to SQLite.

## Verification expectations

Before declaring the cutover complete, implementation must prove:

- API control reads come from Postgres;
- API control writes land in Postgres;
- usage-worker control reads come from Postgres;
- runtime config updates propagate correctly through Postgres-backed control;
- public access/key lookup still works;
- contribution/review/admin flows still work;
- status caches still persist correctly;
- the SQLite file still exists untouched on JuiceFS;
- effective `usage-journal` path is local disk on both services.

## Testing expectations

Implementation should include:

- unit coverage for the Postgres repository behavior where practical;
- parity tests against existing control-store contracts;
- config parsing tests for the new backend selection rules;
- local end-to-end verification for both API and worker bootstrap against
  Postgres;
- live GCP smoke verification during the maintenance window.

## Explicitly rejected alternatives

### Dual write between SQLite and Postgres

Rejected because:

- the user explicitly accepts the imported Neon snapshot as the new truth;
- dual write adds complexity and consistency risk with no meaningful benefit.

### API-first, worker-later cutover

Rejected because:

- both services read control state;
- split cutover would create a temporary mixed-truth system.

### Silent runtime fallback from Postgres to SQLite

Rejected because:

- it obscures real cutover failures;
- it makes control truth ambiguous;
- it defeats the purpose of the migration.

### Reusing JuiceFS-backed `usage-journal`

Rejected because:

- hot journal writes should not consume object-storage bandwidth;
- the local-disk invariant has already been established operationally and must
  stay true.

## Final design statement

The control-plane migration is a hard switch from shared SQLite to Neon
Postgres using the already imported dataset as the starting live truth. API and
usage-worker cut over together in one maintenance window. SQLite remains on
disk only as rollback state. `usage-journal` stays strictly on local disk and
is explicitly preserved as a non-negotiable operational invariant during and
after the cutover.
