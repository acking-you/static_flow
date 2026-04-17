# StaticFlow Pingora Gateway and Seamless Backend Upgrade Design

Date: 2026-04-17

Status: finalized after discussion.

## Goal

Add a local Pingora-based gateway in front of `static-flow-backend` so the
backend can be upgraded with no user-visible port change and no connection
refused window. The gateway must live in this repository as a new binary, use
Pingora crates from `deps/pingora`, and keep backend upgrades independent from
gateway process lifetime.

At the same time, backend and gateway runtime logs must be unified under one
logging model:

- logs rotate automatically instead of growing forever
- retention is capped at 24 hours
- application logs and access logs are separated
- the implementation uses existing logging libraries rather than custom
  rotation logic

## Current Problem

Today local self-hosted backend startup scripts append stdout and stderr into a
single stable file path:

- `scripts/start_backend_selfhosted.sh`
- `scripts/start_backend_selfhosted_canary.sh`

This creates two real operational problems:

1. the public entry port is tied directly to a backend instance, so backend
   replacement still depends on process restart choreography
2. local logs can grow into multi-GB single files because no built-in rotation
   or retention exists in the current script-driven startup path

## User Requirements

1. The gateway is a new binary in this repository, not a modification of
   `deps/pingora`.
2. The gateway uses Pingora crates from the local submodule as dependencies.
3. The gateway runs from the project root and proxies the entire backend as one
   service.
4. The backend may be up or down independently of the gateway. If the backend
   is down, the gateway should return upstream failure. When the backend comes
   back, requests should succeed again without restarting the gateway.
5. Backend upgrades should be seamless by switching the gateway's active
   upstream from one backend slot to another.
6. Gateway configuration should be file-driven and support graceful reload.
7. Logging must be detailed by default.
8. Log retention must not exceed 24 hours.
9. Log rotation and retention should rely on existing libraries instead of
   custom hand-written rotation code.

## Non-Goals

The first version intentionally does not attempt to solve the following:

- automatic runtime failover between backends during ordinary outages
- weighted load balancing
- path-based routing policies
- config file auto-watch and auto-reload
- log compression
- log total-size enforcement in MB or GB
- cluster orchestration or multi-host deployment

This design is for a stable local front door plus controlled backend cutover,
not a general-purpose HA control plane.

## Rejected Approaches

### Continue using script-level file redirection plus `logrotate`

Rejected because it keeps backend and gateway logging behavior fragmented across
foreground mode, daemon mode, and systemd mode. It also leaves rotation policy
outside the application runtime where request correlation and access logging are
actually defined.

### Depend entirely on systemd journald

Rejected because the current local workflow relies heavily on project scripts
and direct process launches. The gateway and backend must behave consistently
even when not started as systemd units.

### Build failover directly into the gateway

Rejected for v1 because it would blur the line between controlled upgrade logic
and autonomous traffic management. That adds state complexity and weakens the
operator's mental model. During normal runtime the gateway should proxy only to
the configured active backend slot.

## Chosen Approach

Introduce a dedicated gateway binary inside this repository:

- crate directory: `gateway/`
- package/binary name: `staticflow-pingora-gateway`

The gateway uses Pingora path dependencies from `deps/pingora`, listens on a
stable front-door port, and forwards all requests to the configured active
backend upstream. Two backend slots, `blue` and `green`, are defined in the
gateway config. Backend upgrades happen by starting a new backend on the
inactive slot, validating it, switching `active_upstream`, and gracefully
reloading the gateway.

Backend and gateway logging are unified through a shared native logging helper
that builds on `tracing`, `tracing-subscriber`, and `tracing-appender`.

## Process Boundaries

The system is split into three independent parts.

### 1. Gateway process

`staticflow-pingora-gateway` is a long-running standalone process that:

- owns the stable public local port
- reads gateway configuration from a YAML file
- proxies all incoming backend traffic to the active backend slot
- logs application events and access events
- supports Pingora graceful reload using the official upgrade socket mechanism

### 2. Backend process

`static-flow-backend` remains a separate process that:

- can run on different local ports such as `39080`, `39081`, or `39082`
- does not know about Pingora
- exposes the real API and frontend payloads
- may have an old instance and a candidate instance alive at the same time

### 3. Script orchestration layer

Shell scripts are only responsible for orchestration:

- starting and stopping the gateway
- validating config before reload
- starting a new backend candidate
- checking health
- switching active upstream
- triggering graceful reload
- rolling back if cutover fails

Scripts do not own request processing and do not own rotating business logs.

## Configuration Model

One YAML file is used for both Pingora's native runtime configuration and
StaticFlow-specific gateway settings.

Pingora already ignores unknown config keys, so a custom `staticflow:` block is
safe to include in the same file.

File location:

- `conf/pingora/staticflow-gateway.yaml`

Config shape:

```yaml
version: 1
daemon: false
threads: 2
pid_file: /home/ts_user/rust_pro/static_flow/tmp/staticflow-gateway.pid
error_log: /home/ts_user/rust_pro/static_flow/tmp/runtime-logs/gateway/pingora-error/current.log
upgrade_sock: /home/ts_user/rust_pro/static_flow/tmp/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:39180
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true

  upstreams:
    blue: 127.0.0.1:39080
    green: 127.0.0.1:39081

  active_upstream: blue

  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
```

### Configuration rules

- `active_upstream` must be one of the named upstream entries
- first version supports exactly two logical slots: `blue` and `green`
- connect timeout is short because upstream unavailability should fail fast
- read and write timeouts are idle timeouts, not total request-duration limits
- no hard total request timeout is configured in v1
- retry count defaults to zero because requests may be stateful or streaming

## Logging Model

### Logging stack

The runtime logging implementation uses:

- `tracing`
- `tracing-subscriber`
- `tracing-appender`

This keeps the project on its current tracing stack and adds rotation through
existing library support instead of custom file-rotation code.

### Shared logging helper

A shared native-only helper is introduced in the `shared/` crate so backend and
gateway reuse the same logging bootstrap logic rather than duplicating writer,
rotation, and retention setup.

Responsibilities:

- configure `EnvFilter`
- create rolling file appenders
- set hourly rotation
- cap retained files at 24 per log stream
- optionally duplicate logs to stdout in foreground mode
- keep `WorkerGuard` values alive for non-blocking writers

### Log directory layout

```text
tmp/runtime-logs/
  backend/
    app/
    access/
  gateway/
    app/
    access/
  ops/
```

### Retention policy

Retention is intentionally strict:

- rotation granularity: hourly
- retention limit: 24 files per stream
- effective retention window: at most 24 hours

This design does not add separate total-MB cleanup logic in v1. That trade-off
is deliberate: it avoids custom cleanup code and stays within the capabilities
needed from `tracing-appender`.

### Log stream separation

Each service writes at least two streams:

- `app log`
  - lifecycle events
  - reload events
  - backend selection
  - warnings and errors
- `access log`
  - one structured line per request

### Required fields

`app log` entries should include:

- timestamp
- level
- service name (`backend` or `gateway`)
- pid
- request_id when available
- trace_id when available
- message
- structured fields relevant to the event such as `upstream`,
  `old_upstream`, `new_upstream`, `reload_generation`, or `error`

`access log` entries should include:

- timestamp
- service name
- request_id
- trace_id
- remote_addr
- method
- host
- path
- status
- elapsed_ms
- bytes_in
- bytes_out

Gateway access logs additionally include:

- active_upstream
- upstream_addr
- upstream_status when known

Backend access logs additionally include:

- handler or route identifier where practical

### Logging controls

Logging behavior is configured through a shared set of environment variables
recognized by backend and gateway, for example:

- `STATICFLOW_LOG_DIR`
- `STATICFLOW_LOG_SERVICE`
- `STATICFLOW_LOG_ROTATION`
- `STATICFLOW_LOG_MAX_FILES`
- `STATICFLOW_ACCESS_LOG_ENABLED`
- `STATICFLOW_LOG_STDOUT`

The gateway's routing behavior stays in YAML. Logging stays env-driven so it
fits existing backend startup patterns.

## Request Correlation

The gateway participates in request correlation instead of leaving it entirely
to the backend.

Rules:

1. If the incoming request already carries `x-request-id` or `x-trace-id`, the
   gateway preserves them.
2. If they are missing, the gateway generates them at ingress.
3. The gateway forwards both headers to the backend.
4. The backend keeps using and returning the same values.

To maximize reuse, request-id generation and header-name constants are shared
through `shared/src/request_ids.rs` instead of maintaining two unrelated
implementations.

## Backend Health Endpoint

The backend gains a minimal health endpoint:

- `GET /api/healthz`

Its purpose is cutover validation, not deep dependency diagnosis.

Response:

```json
{
  "status": "ok",
  "pid": 12345,
  "port": 39081,
  "started_at": 1760000000000,
  "version": "git-sha-or-build-id"
}
```

Rules:

- it reports process readiness only
- it should not perform expensive DB scans
- it should not fail because an external dependency is slow or absent

## Gateway Binary Behavior

The gateway binary uses Pingora's built-in CLI handling by constructing
the server with `Opt::parse_args()`. This preserves official Pingora command
line behavior such as:

- `-c` / `--conf` for config file path
- `-d` / `--daemon`
- `-u` / `--upgrade`

The gateway does not introduce a second incompatible CLI grammar when Pingora
already provides the needed runtime entry points.

The gateway implementation:

- create `Server::new(Some(Opt::parse_args()))`
- bootstrap Pingora normally
- create a single HTTP proxy service
- listen on `staticflow.listen_addr`
- implement request, upstream-request, response, and logging callbacks as
  needed for header propagation and access logging

## Script Interface

Two orchestration scripts define the operator workflow.

### `scripts/pingora_gateway.sh`

This script manages only the gateway process.

Supported commands:

- `run`
- `start`
- `check`
- `reload`
- `status`
- `stop`
- `switch <blue|green>`

Behavior:

- always sets the working directory to the repository root
- always operates on the configured gateway YAML file
- `check` validates config and prints the effective listen address, active
  upstream, timeout settings, and log locations
- `reload` performs Pingora graceful upgrade rather than a kill-and-restart
- `switch` updates `active_upstream`, runs `check`, then runs `reload`

### `scripts/backend_gateway_upgrade.sh`

This script manages a controlled backend cutover.

Default flow:

1. read current gateway config
2. determine current active slot and inactive candidate slot
3. start a new backend on the inactive slot's port
4. wait for `/api/healthz` on the candidate backend
5. update `active_upstream` in gateway config
6. run gateway graceful reload
7. validate through the gateway front door that traffic now reaches the
   candidate backend
8. stop the old backend
9. if any cutover-stage validation fails, roll back to the old slot

## Graceful Reload Model

Gateway reload must follow Pingora's documented graceful upgrade flow:

1. start the new gateway instance with `--upgrade`
2. let the new instance acquire listening sockets from the old instance via the
   configured `upgrade_sock`
3. send `SIGQUIT` to the old instance
4. old instance drains in-flight requests and exits
5. new instance accepts new requests without closing the public listening port

This is a zero-downtime socket handoff, not a kill-and-restart.

## Failure Handling

The gateway is deliberately conservative.

### Runtime outage policy

During ordinary runtime:

- the gateway proxies only to the configured active upstream
- if that backend is down, the gateway returns upstream failure
- the gateway does not automatically fail over to the standby slot

### Upgrade-time failure policy

1. Candidate backend start failure
   - abort upgrade
   - leave gateway and old backend untouched

2. Candidate backend health check failure
   - abort upgrade
   - leave gateway and old backend untouched

3. Gateway config write failure or config validation failure
   - abort before reload

4. Gateway graceful reload failure
   - restore old config
   - keep old gateway instance serving

5. Reload succeeds but gateway-front-door validation still fails against the
   new backend
   - switch config back to the old slot
   - reload again
   - revalidate old path
   - keep candidate backend alive for debugging

6. Old backend stop failure after successful cutover
   - mark upgrade as successful with warning
   - require manual cleanup later

## Operational Logs

Upgrade orchestration emits its own log stream under:

- `tmp/runtime-logs/ops/`

This log is separate from service app/access logs. It records:

- old active slot
- candidate slot
- candidate port
- backend start command
- health check attempts
- config change summary
- reload start and completion
- rollback events
- final success or failure outcome

## Codebase Impact

Planned new or modified areas:

### New

- `gateway/Cargo.toml`
- `gateway/src/main.rs`
- `gateway/src/config.rs`
- `gateway/src/proxy.rs`
- `gateway/src/access_log.rs`
- `conf/pingora/staticflow-gateway.yaml`
- `scripts/pingora_gateway.sh`
- `scripts/backend_gateway_upgrade.sh`

### Shared runtime reuse

- `shared/src/runtime_logging.rs`
- `shared/src/request_ids.rs`

### Backend changes

- `backend/src/main.rs` for shared logging bootstrap
- `backend/src/request_context.rs` to reuse shared request-id helpers
- `backend/src/routes.rs`
- `backend/src/health.rs` for `/api/healthz`
- self-hosted backend startup scripts to stop owning business-log rotation

### Workspace metadata

- root `Cargo.toml` to add the new `gateway` member and required dependency
  wiring

## Verification Strategy

Verification is split into three layers.

### 1. Unit tests

Cover:

- gateway YAML parsing and validation
- `active_upstream` validation
- shared logging config defaults
- request-id propagation helpers
- backend `/api/healthz` response shape

### 2. Integration tests

Cover:

- gateway to single backend proxy success
- backend down yields `502/503`
- backend comes back without gateway restart and requests recover
- switching `active_upstream` plus graceful reload sends new requests to the
  new backend
- reload failure leaves the old serving instance available

### 3. Script-level smoke tests

Run the real scripts:

- `scripts/pingora_gateway.sh start`
- `scripts/pingora_gateway.sh switch green`
- `scripts/backend_gateway_upgrade.sh`

Validate:

- gateway front-door port stays stable
- `/api/healthz` behind the gateway reflects the expected backend
- request IDs can be traced across gateway and backend logs
- hourly rolling files are created under the expected runtime log directories

## Security and Compatibility Notes

- existing backend APIs remain compatible
- the gateway is local-first and intended to front the existing backend, not to
  replace backend auth logic
- admin-only behavior remains in the backend
- no compatibility shim is added for automatic failover; controlled cutover is
  the intended design

## Open Implementation Constraint

The first implementation uses the simplest correct mechanism:

- one stable gateway
- two named upstream slots
- one minimal health endpoint
- explicit reload commands
- library-backed hourly rotation with 24 retained files

If future needs require auto-watch, weighted routing, compression, or size
limits, those should be separate follow-up designs rather than quietly folding
new state machines into this implementation.
