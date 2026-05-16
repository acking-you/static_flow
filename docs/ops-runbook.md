# StaticFlow Operations Runbook

This document contains production deployment details, infrastructure configuration,
and emergency recovery procedures extracted from the agent guide. For day-to-day
development and coding guidance, see `CLAUDE.md` (or `AGENTS.md`).

## GCP Production Topology

Current source-of-truth production facts verified on 2026-05-02:
- Caddy config path: `/etc/caddy/Caddyfile`.
- Active Caddy site block proxies LLM paths to `127.0.0.1:19080` and all
  other paths to `127.0.0.1:39080`.
- Server-side pb-mapper client systemd unit:
  `pb-mapper-client-cli@sf-backend.service`.
- Server-side pb-mapper client env file:
  `/etc/pb-mapper/client-cli/sf-backend.env`.
- Current pb-mapper client settings are read from the private server-side env:
  - `PB_SERVER=<configured relay address>`
  - `SERVICE_KEY=sf-backend`
  - `LOCAL_ADDR=127.0.0.1:39080`
- Server-side pb-mapper server systemd unit: `pb-mapper-server.service`.
- Cloud `llm-access` API systemd unit: `llm-access.service`, serving
  provider/admin compatibility traffic on `127.0.0.1:19080`.
- Target cloud `llm-access` usage worker systemd unit:
  `llm-access-usage-worker.service`, serving DuckDB-backed usage queries on
  `127.0.0.1:19081`.
- Cloud JuiceFS mount service: `juicefs-llm-access.service`, mounting
  `/mnt/llm-access`.
- Cloud-to-local back-link unit:
  `pb-mapper-server-cli@llm-access.service`, registering `127.0.0.1:19080` as
  pb-mapper key `llm-access`.

## Standard Tier Ingress Trial (Rejected)

Standard Tier ingress trial on 2026-05-03:
- A temporary Standard Tier front door was tested as a TCP pass-through for
  `80`, `443`, and the configured pb-mapper relay port.
- The trial was rejected because the Standard Tier route from the local network
  showed much higher latency and packet loss than the existing Premium Tier
  path.
- The temporary VM and static IP were deleted on 2026-05-03. Do not use the
  trial endpoint for production or rollback; historical endpoint values belong
  only in private notes, not this repository.

## GCP / Valkey / JuiceFS Configuration

Current GCP / Valkey / JuiceFS facts verified on 2026-05-02. Concrete host,
user, key, bucket, account, and metadata endpoint values are stored in ignored
private env files, not in tracked docs.

- Local private cloud-release config is:
  `.local/llm-access-cloud-release.env`. Copy
  `conf/llm-access-cloud-release.env.example` when bootstrapping a new checkout.
- GCP SSH login from this workstation uses variables from that file:
  ```bash
  set -a
  source .local/llm-access-cloud-release.env
  set +a
  ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST"
  ```
  If `GCP_DEST` is unset, build it from `GCP_USER@$GCP_HOST`.
- The GCP machine has FUSE available (`/dev/fuse`) and `fusermount3`; the
  JuiceFS binary path is deployment-specific and should be read from the
  private host configuration.
- R2 bucket/account/endpoint values for llm-access JuiceFS storage live in the
  private JuiceFS env below. Do not commit R2 access keys or endpoint/account
  identifiers.
- Local private JuiceFS config is:
  `.local/common/juicefs/llm-access.env`. This file is intentionally ignored by
  git through the `.local/` rule, must stay mode `0600`, and contains all
  JuiceFS/R2/Valkey variables needed for one-command sourcing:
  ```bash
  set -a
  source .local/common/juicefs/llm-access.env
  set +a
  ```
- The same config was copied to the GCP host at
  `~/.config/staticflow/llm-access-juicefs.env`, also mode `0600`.

## Valkey Metadata

- Valkey metadata host/login are deployment secrets; load them from the private
  JuiceFS/cloud env before connecting:
  ```bash
  set -a
  source .local/common/juicefs/llm-access.env
  set +a
  ssh "$VALKEY_SSH_TARGET"
  ```
  Valkey runs as `valkey-16379.service`, listens on port `16379`, and uses:
  - config: `/etc/valkey/valkey-16379.conf`
  - ACL file: `/etc/valkey/valkey-16379.acl`
  - CLI: `/opt/valkey/current/bin/valkey-cli`
  - data dir: `/var/lib/valkey`
  - AOF: enabled, `appendfsync everysec`
  - max memory policy: `noeviction`
- A dedicated Valkey ACL user `juicefs` exists for JuiceFS metadata. Do not
  print or commit the ACL password; use the ignored env file above.
- Reserved Valkey DBs for JuiceFS metadata:
  - production DB: configured in the ignored JuiceFS env
  - temporary validation DB: configured in the ignored JuiceFS env when needed
- GCP validation already proved:
  - R2 object put/get/head/delete through JuiceFS `objbench` works from GCP.
  - JuiceFS format/mount/write/read/sha256/umount works on the temporary
    validation DB.
  - Valkey metadata ping from GCP was about 1-2 ms.
- Cloudflare R2 is usable for normal JuiceFS mount read/write, but its S3
  `ListObjects` behavior is not fully ordered. Avoid relying on JuiceFS
  maintenance commands that need complete ordered object listing, especially
  `gc`, `fsck`, `sync`, and `destroy`, unless separately tested for this R2
  bucket and accepted for the operation.

## llm-access Request-Path Valkey Cache

- The request-path Valkey cache is separate from JuiceFS metadata. It only
  caches hot Postgres control-plane reads used during bearer auth, route
  snapshot loading, runtime-config reads, Codex status snapshot reads,
  account-view selection, and selected-account auth hydration.
- Local private request-cache config lives in `.local/common/valkey/lb7666.env`
  and must stay ignored by git with mode `0600`. Source it explicitly when
  testing from this workstation:
  ```bash
  set -a
  source .local/common/valkey/lb7666.env
  set +a
  ```
- The shared env var name for the Valkey URL is
  `LLM_ACCESS_REQUEST_CACHE_URL`. The live GCP `/etc/llm-access/llm-access.env`
  should export that variable for both `llm-access.service` and
  `llm-access-usage-worker.service`.
- When the cloud `llm-access` API or usage worker uses Postgres control-plane
  state, start them with:
  - `--request-cache-url-env LLM_ACCESS_REQUEST_CACHE_URL`
  - `--request-cache-key-prefix llma`
- The request-cache key prefix must stay stable across all nodes that should
  share the same cache namespace. Changing the prefix intentionally cold-starts
  the cache.
- Postgres remains the only durable source of truth. Valkey cache loss, TTL
  expiry, or entry invalidation must never lose quota/account state.
- Cache freshness is maintained by explicit invalidation and generation bumps on
  key, group, proxy, runtime-config, auth-refresh, and account-state writes.
  TTL only bounds stale leftovers and memory growth; it is not the freshness
  mechanism.
- The request path is intentionally resilient to cache failures. If Valkey is
  unavailable or an entry decode fails, `llm-access` falls back to the current
  Postgres query path and logs a warning instead of rejecting user traffic.
- Current TTL policy uses deterministic jitter around multi-hour base TTLs to
  avoid synchronized expiry bursts. Do not replace that with identical fixed
  TTL values for all keys.

## Cloud llm-access Deployment Shape

- `llm-access` must keep a single active writer for its auth JSON files, local
  journal files, and mutable DuckDB state. Do not run local and cloud
  `llm-access` processes that both write the same JuiceFS-mounted state tree or
  the same VM-local usage state.
- Production `llm-access` now uses Neon Postgres as the live control plane.
  The shared connection file lives at `/mnt/llm-access/config/neon.env`. The
  older `/mnt/llm-access/control/llm-access.sqlite3` file is retained only as a
  rollback snapshot and must not be treated as the live source of truth.
- The production deployment uses two JuiceFS mount points:
  - control/state mount: `/mnt/llm-access`
  - usage analytics mount: `/mnt/llm-access-usage`
- llm-access should use:
  - API state root: `/mnt/llm-access`
  - shared Neon config: `/mnt/llm-access/config/neon.env`
  - rollback SQLite snapshot: `/mnt/llm-access/control/llm-access.sqlite3`
  - hot local usage journal dir: `/var/lib/staticflow/llm-access/usage-journal`
  - local active DuckDB dir: `/var/lib/staticflow/llm-access/analytics-active`
  - archived immutable DuckDB segments:
    `/mnt/llm-access-usage/analytics/segments`
  - DuckDB segment catalog: `/mnt/llm-access-usage/analytics/catalog`
  - packed per-event heavy usage details:
    `/mnt/llm-access-usage/details/packs/<provider>/<yyyy>/<mm>/<dd>/...`
  - email credentials: `/mnt/llm-access/config/email_accounts.json`
  - API bind address: `127.0.0.1:19080`
  - usage worker bind address: `127.0.0.1:19081`
- `usage-journal` is VM-local only. The active path is
  `/var/lib/staticflow/llm-access/usage-journal`. Do not recreate
  `/mnt/llm-access/usage-journal`; if that directory appears again, treat it as
  stale dead data and remove it.
- Gmail notification credentials must live on JuiceFS, not VM-local `/etc`.
  `llm-access.service` should set
  `EMAIL_ACCOUNTS_FILE=/mnt/llm-access/config/email_accounts.json`; keep the
  file mode `0600` and do not log credential contents.
- Service ownership after the Neon cutover:
  - `llm-access.service`: provider traffic, Neon control reads/writes, account
    status refreshers, and compact local usage journal production.
  - `llm-access-usage-worker.service`: runtime-config reads from Neon, journal
    consumption, tiered DuckDB summary writes, packed usage-detail writes,
    worker progress state, and legacy admin/public usage query routes on the
    worker port.
- Live GCP `llm-access.service` and `llm-access-usage-worker.service` currently
  run as the non-root `ts_user` service user. The critical requirement is not
  the username; it is that the same service user can read the shared JuiceFS
  config, the local journal directory, and both FUSE mounts consistently.
- Current GCP systemd units verified on 2026-05-16:
  - `juicefs-llm-access.service`: mounts `/mnt/llm-access`
  - `juicefs-llm-access-usage.service`: mounts `/mnt/llm-access-usage`
  - `llm-access.service`: serves `127.0.0.1:19080`, sources
    `/mnt/llm-access/config/neon.env`, and starts with
    `--postgres-control-database-url-env LLM_ACCESS_CONTROL_DATABASE_URL`
  - `llm-access-usage-worker.service`: serves `127.0.0.1:19081`, sources the
    same Neon env, consumes the local journal root, and writes usage artifacts
    under `/mnt/llm-access-usage`
  - `pb-mapper-server-cli@llm-access.service`: registers cloud
    `127.0.0.1:19080` as pb-mapper key `llm-access`
- Current GCP llm-access logs:
  - systemd journal: `sudo journalctl -u llm-access.service -f`
  - usage worker journal:
    `sudo journalctl -u llm-access-usage-worker.service -f`
  - runtime app logs:
    `/var/log/staticflow-runtime/llm-access/app/current.*.log`
  - runtime access logs:
    `/var/log/staticflow-runtime/llm-access/access/current.*.log`
  - JuiceFS mount logs:
    `sudo journalctl -u juicefs-llm-access.service -f`
    and `sudo journalctl -u juicefs-llm-access-usage.service -f`
  Runtime logs rotate hourly and retain the latest 4 files per stream.
- Background provider status refresh should stay enabled on
  `llm-access.service` during normal production operation. Do not pin
  `LLM_ACCESS_BACKGROUND_STATUS_REFRESH_ENABLED=0` in the service unit unless
  you are intentionally pausing periodic Codex/Kiro account-status refresh for
  incident mitigation.
- The service-level background refresher is separate from the per-account
  Codex "auto refresh" toggle in admin. The per-account toggle only controls
  whether that account may use its `refresh_token` to renew auth when needed;
  it does not replace the global periodic refresher.
- Current GCP JuiceFS local cache uses:
  - control mount cache: `/var/cache/juicefs/llm-access`
  - usage mount cache: `/var/cache/juicefs/llm-access-usage`
  Keep both caches on VM-local ext4 storage, not inside the FUSE mount.

## GCP Memory Guard (2c8g VM)

- `/swapfile` and `/swapfile-llm-extra` are each 2 GiB emergency swap files,
  enabled through `/etc/fstab`; host swap total is 4 GiB.
- `/etc/sysctl.d/99-staticflow-memory-guard.conf` sets `vm.swappiness=10`.
- `llm-access.service` has `MemoryHigh=3584M`, `MemoryMax=4096M`,
  `MemorySwapMax=1024M`, `TasksMax=256`, and `OOMPolicy=kill`.
- `llm-access-usage-worker.service` should carry the DuckDB memory budget after
  the split. Start with `MemoryHigh=2200M`, `MemoryMax=3072M`,
  `MemorySwapMax=1024M`, `TasksMax=128`, and `OOMPolicy=kill`; keep API and
  worker limits independent so a DuckDB scan cannot kill provider traffic.
- `juicefs-llm-access.service` has `MemoryHigh=1800M`,
  `MemoryMax=2560M`, `MemorySwapMax=0`, `TasksMax=256`, and
  `OOMPolicy=kill`.
These limits are meant to kill/restart the offending service before the whole
VM becomes unreachable. Do not raise `MemoryMax` casually on 8 GiB RAM; keep
extra swap as an emergency buffer, not as normal working memory.

## llm-access Usage Analytics

- Current llm-access usage analytics should run in tiered DuckDB mode: only the
  active mutable DuckDB file lives on local VM block storage; JuiceFS/R2 stores
  immutable archived segment files plus the low-frequency segment catalog. Do
  not point a live writer at `/mnt/llm-access/analytics/usage.duckdb` as a
  mutable all-history DuckDB file.
- Heavy per-event detail payloads are not part of the hot DuckDB write path
  anymore. The worker writes summary facts into tiered DuckDB, but writes
  detail payloads as packed files under `/mnt/llm-access-usage/details`. This
  keeps checkpoint/rollover memory bounded by summary analytics instead of full
  request bodies, without maintaining a second direct-R2 upload path in the
  application.
- The API process no longer writes usage events directly into DuckDB. It first
  commits control-plane rollups through the live control repository, then
  appends compact diagnostic usage events to
  `/var/lib/staticflow/llm-access/usage-journal`. The separate usage worker
  consumes sealed journal files in batches, imports them into tiered DuckDB,
  records worker progress in `consumer-state.sqlite3`, and deletes consumed
  journal files.
- Journal file rollover is controlled by both size and age. Retention is
  intentionally lossy for old unconsumed diagnostics: control-plane rollups in
  the live repository remain the source of truth for quota/account accounting,
  while journal events are for detailed troubleshooting.
- Legacy usage query paths remain compatible. The public/API service keeps the
  old `/admin/llm-gateway/usage*` and `/admin/kiro-gateway/usage*` routes for
  auth and compatibility, but proxies those queries to
  `usage_query_base_url` (`http://127.0.0.1:19081` by default).
- The completed migration model is:
  1. Production `llm-access` state lives under `/mnt/llm-access` and the local
     VM active DuckDB directory described above.
  2. Only cloud `llm-access.service` writes live control rows and local journal
     files; only `llm-access-usage-worker.service` writes tiered DuckDB.
  3. Cloud Caddy owns the public LLM route split and sends LLM paths directly
     to `127.0.0.1:19080`.
  4. Local StaticFlow reaches cloud `llm-access` through the local
     `127.0.0.1:19182` pb-mapper subscription or through the public same-origin
     path; it must not mount/write the JuiceFS state directly.

## llm-access Startup and Sandboxing Constraints

- Startup must be gated on the JuiceFS mount and expected state files. If
  `llm-access` starts before `/mnt/llm-access` is really mounted, it can
  initialize an empty local directory and make production state appear missing.
  The GCP service should install `/usr/local/bin/staticflow-wait-llm-access-state`
  and run it as `ExecStartPre` for `llm-access.service`; if JuiceFS is managed
  as a plain `.service`, add an `ExecStartPost` mountpoint gate there too so
  systemd does not mark the mount ready before FUSE has actually attached.
- Do not use systemd path sandboxing directives such as `ProtectSystem=`,
  `ReadWritePaths=`, or `PrivateTmp=` on the GCP `llm-access.service` while
  `/mnt/llm-access` is a JuiceFS FUSE mount with `default_permissions`. This
  combination can fail before the service starts with
  `status=226/NAMESPACE` and `Failed to set up mount namespacing:
  /mnt/llm-access: Permission denied`. Rely on the non-root service user,
  directory ownership, and the readiness script instead.

## llm-access Admin API Constraints

- Admin usage APIs are compatibility proxies to the usage worker. Broad
  diagnostics should still avoid large pages or full-table scans: DuckDB scan
  buffers now affect `llm-access-usage-worker.service` rather than provider
  traffic, but the worker is still a production process. Online usage list
  endpoints must stay server-bounded and lightweight: max `limit` is 20, max
  `offset` is 200, and list responses must not read/return heavy diagnostic
  fields such as message content or routing diagnostics. The response `total`
  should still be the exact count for the active filter condition, including
  key/provider/time filters. Use the per-event detail endpoint by `event_id`
  when heavy fields are needed.

## Current Runtime Verification Snapshot

- Verified on GCP at `2026-05-16T22:13:00Z`.
- Effective live API unit:
  - service user: `ts_user`
  - bind: `127.0.0.1:19080`
  - current `ExecStart`: API process with
    `--postgres-control-database-url-env LLM_ACCESS_CONTROL_DATABASE_URL`
    and `--usage-journal-dir /var/lib/staticflow/llm-access/usage-journal`
  - `/proc/<api-pid>/environ` contains
    `LLM_ACCESS_CONTROL_DATABASE_URL=<redacted>`
- Effective live usage-worker unit:
  - service user: `ts_user`
  - bind: `127.0.0.1:19081`
  - current `ExecStart`: worker process with the same Postgres control env,
    local journal root, local active DuckDB dir, dedicated JuiceFS usage
    archive/catalog dirs, and JuiceFS-packed usage details
  - cgroup guard observed live:
    `MemoryHigh=2200M`, `MemoryMax=3072M`, `MemorySwapMax=1024M`
- Live health checks that should all pass:
  ```bash
  curl -fsS http://127.0.0.1:19080/healthz
  curl -fsS https://ackingliu.top/api/llm-gateway/status
  curl -fsS http://127.0.0.1:19081/admin/llm-access/usage-worker/status
  findmnt -T /mnt/llm-access
  findmnt -T /mnt/llm-access-usage
  systemctl cat llm-access.service llm-access-usage-worker.service
  ps -o pid,args= -C llm-access -C llm-access-usage-worker
  tr '\0' '\n' </proc/$(systemctl show -p MainPID --value llm-access.service)/environ | grep '^LLM_ACCESS_CONTROL_DATABASE_URL='
  tr '\0' '\n' </proc/$(systemctl show -p MainPID --value llm-access-usage-worker.service)/environ | grep '^LLM_ACCESS_CONTROL_DATABASE_URL='
  ```
- Healthy interpretation:
  - API args must include `--postgres-control-database-url-env`.
  - worker args must include both `--postgres-control-database-url-env` and
    `--usage-journal-dir /var/lib/staticflow/llm-access/usage-journal`.
  - `worker.state == idle` with `last_error == null` and a local journal root
    means the worker is healthy even if there are no sealed files pending.
  - `llm_access_owner` should appear in Neon `pg_stat_activity` when the API
    and worker are live.

## Known Residuals After Neon Control Cutover

- `/mnt/llm-access/control/llm-access.sqlite3` is retained only as a rollback
  snapshot. It is no longer a live truth source.
- `/mnt/llm-access/usage-journal` was removed on 2026-05-16. If it reappears,
  treat it as stale wrong-path data and delete it after confirming no process
  has it open.
- `/mnt/llm-access/analytics` is legacy. Current worker archive/catalog/detail
  writes belong under `/mnt/llm-access-usage`, not the old control mount.

## Cloud Release and Post-Release Verification

- Keep a local ignored copy of the shared Neon control config at
  `.local/llm-access-neon.env`. It must define
  `LLM_ACCESS_CONTROL_DATABASE_URL=postgresql://...`.
- `prepare_llm_access_cloud_release.sh` now treats that local file as the
  release source of truth, uploads it into the staged bundle, and
  `activate_llm_access_cloud_release.sh` installs it back onto the live JuiceFS
  path `/mnt/llm-access/config/neon.env` before restarting API/worker.
- Local release preparation from this checkout:
  ```bash
  export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
  ./scripts/prepare_llm_access_cloud_release.sh
  ```
- Remote activation:
  ```bash
  set -a
  source .local/llm-access-cloud-release.env
  set +a
  ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST" \
    '/home/ts_user/staticflow-llm-access-release/activate_llm_access_cloud_release.sh'
  ```
- Required post-release checks:
  ```bash
  curl -fsS http://127.0.0.1:19080/healthz
  curl -fsS https://ackingliu.top/api/llm-gateway/status
  curl -fsS http://127.0.0.1:19081/admin/llm-access/usage-worker/status
  findmnt -T /mnt/llm-access
  findmnt -T /mnt/llm-access-usage
  systemctl cat llm-access.service llm-access-usage-worker.service
  ps -o pid,args= -C llm-access -C llm-access-usage-worker
  tr '\0' '\n' </proc/$(systemctl show -p MainPID --value llm-access.service)/environ | grep '^LLM_ACCESS_CONTROL_DATABASE_URL='
  tr '\0' '\n' </proc/$(systemctl show -p MainPID --value llm-access-usage-worker.service)/environ | grep '^LLM_ACCESS_CONTROL_DATABASE_URL='
  sudo journalctl -u llm-access.service -n 80 --no-pager -l
  sudo journalctl -u llm-access-usage-worker.service -n 80 --no-pager -l
  ```
- If API args still show `--sqlite-control`, or the Neon env is missing from
  `/proc/<pid>/environ`, inspect stale drop-ins and activation drift first:
  - `/etc/systemd/system/llm-access.service.d/readiness.conf`
  - `/etc/systemd/system/llm-access.service.d/tiered-duckdb.conf`
  - `/etc/systemd/system/llm-access.service.d/zz-usage-journal-split.conf`
- If the worker returns permission errors for `.../usage-journal/sealed`, check
  owner/group of `/var/lib/staticflow/llm-access/usage-journal` and confirm the
  local path still exists on ext4 rather than JuiceFS.
- If the worker starts but usage queries return empty unexpectedly, verify the
  worker is mounted on `/mnt/llm-access-usage`, and confirm its effective
  `LLM_ACCESS_STATE_ROOT`, `LLM_ACCESS_DUCKDB_ARCHIVE_DIR`,
  `LLM_ACCESS_DUCKDB_CATALOG_DIR`, and `LLM_ACCESS_USAGE_DETAILS_DIR`.

## Historical Incident Lesson: Usage Mount Restart Can Break API SQLite Handles

- During the early 2026-05-16 worker-only usage-storage rollout, before the
  Neon control cutover, a broken `juicefs-llm-access-usage.service` revision
  effectively controlled the main `/mnt/llm-access` mount instead of the
  dedicated `/mnt/llm-access-usage` mount. Restarting that unit transiently
  disrupted the control/state JuiceFS mount used by `llm-access.service`.
- The outage symptom was subtle: `llm-access.service` stayed `active/running`,
  `GET /healthz` and `GET /version` still worked, but endpoints that touched
  the SQLite control store such as `/api/llm-gateway/access` and
  `/api/llm-gateway/status` hung or timed out.
- The old usage analytics tree deletion under `/mnt/llm-access/analytics` was
  not the cause. The real failure mode was stale open SQLite file descriptors
  after the control mount bounced. On the affected process, `lsof -p <pid>`
  showed `/mnt/llm-access/control/llm-access.sqlite3` and WAL files as
  `Transport endpoint is not connected`.
- Fast triage sequence:
  ```bash
  findmnt -T /mnt/llm-access
  findmnt -T /mnt/llm-access-usage
  systemctl status juicefs-llm-access.service juicefs-llm-access-usage.service --no-pager
  curl -fsS http://127.0.0.1:19080/healthz
  curl -m 10 -fsS http://127.0.0.1:19080/api/llm-gateway/status
  sudo lsof -p "$(systemctl show -p MainPID --value llm-access.service)" | grep llm-access.sqlite3
  ```
- Recovery rule: if the mount has already recovered but `lsof` still shows
  stale SQLite descriptors, restart `llm-access.service`. Do not waste time
  debugging the API binary first; it needs to reopen SQLite on the restored
  FUSE mount.
- This exact stale-SQLite-fd symptom applied before the Neon cutover, but the
  mount-isolation prevention rules remain valid.

## Incident Lesson: Shared Neon Env Must Be Sourced Inside the Service Shell

- The shared control config now lives at `/mnt/llm-access/config/neon.env` on
  JuiceFS.
- Do not point a systemd `EnvironmentFile=` directly at that JuiceFS path and
  assume it behaves like a local ext4 file. With `default_permissions`,
  PID1/root-side file access and mount namespace behavior can differ from the
  `ts_user` service process that actually runs the binaries.
- Use `/usr/bin/bash -lc 'set -a; . /mnt/llm-access/config/neon.env; exec ...'`
  in both `ExecStartPre` and `ExecStart` so the same service user reads the
  same mounted file in the same namespace as the binary.
- Post-release verification must inspect both process args and
  `/proc/<pid>/environ`. A green `/healthz` alone does not prove the service is
  actually using Neon.

## Incident Lesson: Stale Drop-Ins Can Silently Keep API on SQLite

- Older `/etc/systemd/system/llm-access.service.d/*.conf` files can override a
  newly installed base unit `ExecStart` or `ExecStartPre`, even after a fresh
  release bundle is activated.
- The concrete stale files observed during the 2026-05-16 Neon cutover were:
  - `/etc/systemd/system/llm-access.service.d/readiness.conf`
  - `/etc/systemd/system/llm-access.service.d/tiered-duckdb.conf`
  - `/etc/systemd/system/llm-access.service.d/zz-usage-journal-split.conf`
- Activation must remove those stale drop-ins, run `systemctl daemon-reload`,
  and then verify `systemctl cat llm-access.service` plus
  `ps -o pid,args= -C llm-access`.
- A passing `/healthz` is not enough. The API can look healthy while still
  running the wrong storage backend if an old drop-in preserved the old
  `ExecStart`.

## Emergency Recovery for Sudden Public Outage

Common trigger pattern: home network flap, ISP reconnect, hotspot fallback,
or other local-to-cloud path churn causes pb-mapper data forwarding to wedge
while systemd units still look "active".

Another trigger pattern is downstream relay latency on non-LLM routes that is
much higher than StaticFlow's own route timings. Treat that as a likely stale
or wedged cloud-side Caddy/pb-mapper connection state before changing
StaticFlow backend code. For Codex/Kiro/NewAPI LLM latency, check cloud
`llm-access.service` and its DuckDB/JuiceFS state first; that path no longer
traverses the local `sf-backend` pb-mapper tunnel.

If cloud-side `pb-mapper-client` reports
`Not valid key:sf-backend,valid keys:[...]`, or cloud-side
`pb-mapper-server` reports
`subcribe server conn key not exist, key:sf-backend`, or
`127.0.0.1:39080` never comes back to `LISTEN`, then the home/local machine
likely has not re-registered the `sf-backend` service. In that case, prompt
the user to re-register/restart the local home-side pb-mapper service first,
rather than repeatedly restarting only cloud-side services.

First-line recovery for these cloud relay path issues is to restart the
cloud-side Caddy and pb-mapper services together. This drops stale HTTP
keepalive/HTTP2/tunnel connections and is often the fastest way to restore
normal downstream latency:

```bash
set -a
source .local/llm-access-cloud-release.env
set +a
ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST"
sudo systemctl restart caddy
sudo systemctl restart pb-mapper-server.service
sudo systemctl restart pb-mapper-client-cli@sf-backend.service
```

After restart, verify both the relay path and public path:

```bash
set -a
source .local/llm-access-cloud-release.env
set +a
ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST" \
  "curl -o /dev/null -sS -w 'code=%{http_code} size=%{size_download} start=%{time_starttransfer} total=%{time_total}\n' \
    -H 'Host: ackingliu.top' http://127.0.0.1:39080/"

env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl -o /dev/null -sS -w 'code=%{http_code} size=%{size_download} start=%{time_starttransfer} total=%{time_total}\n' \
  https://ackingliu.top/
```

If `sf-backend` was missing and has just been re-registered on the
home/local machine, restart cloud-side
`pb-mapper-client-cli@sf-backend.service` once more so it can re-subscribe to
the now-valid key.

If cloud-side restart does not restore service, continue debugging on the
home/local machine side of the tunnel before changing Caddy config.
