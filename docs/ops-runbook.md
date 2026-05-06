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
- Cloud `llm-access` systemd unit: `llm-access.service`, serving
  `127.0.0.1:19080`.
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

## Cloud llm-access Deployment Shape

- `llm-access` must be a single-writer service for its SQLite, DuckDB, and auth
  JSON files. Do not run local and cloud `llm-access` processes that both write
  the same JuiceFS-mounted state tree.
- The production JuiceFS mount point is `/mnt/llm-access`; llm-access should
  use:
  - state root: `/mnt/llm-access`
  - SQLite control DB: `/mnt/llm-access/control/llm-access.sqlite3`
  - local active DuckDB dir: `/var/lib/staticflow/llm-access/analytics-active`
  - archived immutable DuckDB segments:
    `/mnt/llm-access/analytics/segments`
  - DuckDB segment catalog: `/mnt/llm-access/analytics/catalog`
  - current cloud bind address: `127.0.0.1:19080`
- Live GCP `llm-access.service` currently runs as the non-root `ts_user`
  service user. The checked-in template still uses a dedicated `llm-access`
  user for fresh deployments; either is acceptable if file ownership, FUSE
  permissions, and readiness checks are consistent.
- Current GCP systemd units verified on 2026-05-02:
  - `juicefs-llm-access.service`: mounts `/mnt/llm-access`
  - `llm-access.service`: serves `127.0.0.1:19080`
  - `pb-mapper-server-cli@llm-access.service`: registers cloud
    `127.0.0.1:19080` as pb-mapper key `llm-access`
- Current GCP llm-access logs:
  - systemd journal: `sudo journalctl -u llm-access.service -f`
  - runtime app logs:
    `/var/log/staticflow-runtime/llm-access/app/current.*.log`
  - runtime access logs:
    `/var/log/staticflow-runtime/llm-access/access/current.*.log`
  - JuiceFS mount logs:
    `sudo journalctl -u juicefs-llm-access.service -f`; the configured
    `/var/log/juicefs/llm-access.log` may not receive every active-run line
    because the mount process is systemd-supervised.
  Runtime logs rotate hourly and retain the latest 4 files per stream.
- Current GCP JuiceFS local cache uses `/var/cache/juicefs/llm-access`. Keep
  the cache on the VM local ext4 disk, not inside `/mnt/llm-access`. The
  systemd mount template sets `cache-size=40960` (MiB) so hot reads do not
  constantly round-trip to R2.

## GCP Memory Guard (2c8g VM)

- `/swapfile` and `/swapfile-llm-extra` are each 2 GiB emergency swap files,
  enabled through `/etc/fstab`; host swap total is 4 GiB.
- `/etc/sysctl.d/99-staticflow-memory-guard.conf` sets `vm.swappiness=10`.
- `llm-access.service` has `MemoryHigh=2200M`, `MemoryMax=3072M`,
  `MemorySwapMax=1024M`, `TasksMax=256`, and `OOMPolicy=kill`.
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
- The completed migration model is:
  1. Production `llm-access` state lives under `/mnt/llm-access` and the local
     VM active DuckDB directory described above.
  2. Only cloud `llm-access.service` writes that state.
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

- Do not use the in-process admin usage APIs for broad diagnostics such as
  large `limit` pages or ad-hoc scans over the full DuckDB usage table. Those
  queries execute inside the production `llm-access` process, so DuckDB scan
  buffers are charged to the service RSS. For production analysis, prefer an
  external read-only DuckDB connection or a deliberately narrow SQL/API query.
  Online usage list endpoints must stay server-bounded and lightweight: max
  `limit` is 20, max `offset` is 200, and list responses must not read/return
  heavy diagnostic fields such as message content or routing diagnostics. The
  response `total` should still be the exact count for the active filter
  condition, including key/provider/time filters. Use the per-event detail
  endpoint by `event_id` when heavy fields are needed.

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
