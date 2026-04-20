---
name: selfhosted-gateway-seamless-upgrade
description: Use when upgrading the local StaticFlow backend behind the Pingora gateway with no frontend port change, especially when blue-green backend slots, health-checked cutover, and rollback-safe config switching are required.
---

# Selfhosted Gateway Seamless Upgrade

Use this skill when StaticFlow traffic should stay on one stable gateway port
while backend instances are upgraded behind it.

## Core Rule

The gateway process stays up. Backend versions move by changing the active
upstream in the gateway config and reloading that config in-process.

Do not use Pingora process-level `--upgrade` for backend rollout.

## Preflight

Before touching any slot, determine the real traffic path and the supervisor
mode.

1. Confirm the gateway is the actual stable frontend:
   - `curl -fsS http://127.0.0.1:39180/api/healthz`
   - `env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY curl -fsS https://ackingliu.top/api/healthz`
2. Confirm the returned JSON `port` matches the same active slot from both
   checks.
3. Run `./scripts/pingora_gateway.sh status`.
4. Classify the deployment:
   - If gateway and backend slot units are registered, it is `systemd`-managed.
   - If `status` shows healthy gateway traffic but units are `not-found`, the
     gateway is externally supervised, usually by `tmux`.

If public traffic is still mapped directly to `39080` instead of the gateway
port `39180`, switching `active_upstream` will not change the real production
path. Do not claim the rollout is seamless until that is verified.

## When To Use

- Local self-hosted StaticFlow already has, or should have, a Pingora gateway
  in front of the backend.
- The user wants no frontend port change during backend rollout.
- The rollout should keep the old backend serving until the new backend passes
  `/api/healthz`.
- The rollback path should be "switch traffic back to the old slot", not
  "replace one binary in place".

## When Not To Use

- If there is no gateway in front of the backend yet, bootstrap the gateway
  first.
- If the deployment still runs as one standalone backend process on `39080`
  with direct traffic and no blue-green slots, use
  `selfhosted-seamless-upgrade` instead.

## Default Layout

- Gateway config: `conf/pingora/staticflow-gateway.yaml`
- Gateway script: `scripts/pingora_gateway.sh`
- Upgrade script: `scripts/backend_gateway_upgrade.sh`
- Default gateway listen addr: `127.0.0.1:39180`
- Default backend slots:
  - `blue -> 127.0.0.1:39080`
  - `green -> 127.0.0.1:39081`

The gateway port is the stable frontend. The backend port is the thing that
changes.

## Supervisor Modes

### `systemd`-managed

Use the existing scripts directly:

- `./scripts/pingora_gateway.sh switch <blue|green>`
- `./scripts/backend_gateway_upgrade.sh`

These paths assume gateway and slot units are registered. The script currently
hard-requires registered units before reload and switch.

### External supervisor, usually `tmux`

Do not call `./scripts/pingora_gateway.sh switch` or rely on
`./scripts/backend_gateway_upgrade.sh` as-is. Those paths currently route
through `systemctl reload` and will fail even if the live gateway is healthy.

For this mode:

- start the inactive backend slot manually, usually with `tmux`
- edit `active_upstream` in `conf/pingora/staticflow-gateway.yaml`
- send `SIGHUP` directly to the running gateway pid
- verify local and public health
- rollback by restoring the previous config and sending `SIGHUP` again

## Bootstrap Workflow

Run this once when the gateway is not already up:

1. Make sure the currently active backend slot is already serving traffic.
2. Check the gateway config:
   - `./scripts/pingora_gateway.sh check`
3. Start the gateway:
   - `./scripts/pingora_gateway.sh start`
4. Verify through the gateway, not the backend directly:
   - `curl -fsS http://127.0.0.1:39180/api/healthz`
5. Confirm the returned JSON `port` matches the active backend slot.

## Upgrade Workflow

### Common build rule

Build the candidate backend artifact first, but do not overwrite
`bin/static-flow-backend` before the cutover:

- `cargo build --profile release-backend -p static-flow-backend`

Use the fresh candidate via `BACKEND_BIN=target/release-backend/static-flow-backend`
when starting the inactive slot. This avoids in-place overwrite failures while
the old binary is still serving traffic.

Run normal quality gates before deployment:

- format only touched Rust files with `rustfmt`
- `cargo check --workspace`
- `cargo clippy -p static-flow-backend -p static-flow-frontend -- -D warnings`

If gateway code or gateway scripts changed, refresh the gateway binary first:

- `FORCE_BUILD_GATEWAY=1 ./scripts/pingora_gateway.sh check`

### `systemd` workflow

1. Run the rollout:
   - `./scripts/backend_gateway_upgrade.sh`
2. The rollout script is expected to:
   - build the candidate backend into `target/release-backend/static-flow-backend`
   - start the inactive slot
   - wait for `http://127.0.0.1:<candidate-port>/api/healthz`
   - switch the gateway to the new slot
   - verify `http://127.0.0.1:39180/api/healthz` now reports the new port
   - stop the old backend pid only after gateway verification succeeds

### External-supervisor workflow, usually `tmux`

1. Determine the active slot from `./scripts/pingora_gateway.sh status` and
   choose the other slot as the candidate.
2. Confirm the inactive slot port is free:
   - `ss -ltnp | rg '39080|39081'`
3. Start the candidate slot under `tmux`, passing the freshly built artifact by
   `BACKEND_BIN`, for example:
   - `tmux new-session -d -s staticflow-backend-blue "cd /path/to/static_flow && export BACKEND_BIN=/path/to/static_flow/target/release-backend/static-flow-backend && exec ./scripts/start_backend_selfhosted_slot.sh blue"`
4. Verify the candidate slot directly before any switch:
   - `curl -fsS http://127.0.0.1:<candidate-port>/api/healthz`
   - `curl -fsS 'http://127.0.0.1:<candidate-port>/api/articles?limit=1'`
   - `curl -fsS 'http://127.0.0.1:<candidate-port>/api/llm-gateway/status'`
5. Back up the gateway config and change `active_upstream` to the candidate
   slot.
6. Reload the running gateway in-process:
   - `kill -HUP <gateway-pid>`
7. Verify the stable gateway path now points to the candidate:
   - `curl -fsS http://127.0.0.1:39180/api/healthz`
   - `env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY curl -fsS https://ackingliu.top/api/healthz`
8. Only after both checks report the new slot port, stop the old slot
   supervisor.
9. After a successful cutover, sync `bin/static-flow-backend` to the new
   artifact using a new inode, not an in-place overwrite.

## Gateway Operations

Use `scripts/pingora_gateway.sh` for all gateway lifecycle actions:

- `check`
  - parse and validate gateway YAML
- `start`
  - start one long-lived gateway process under shell supervision
- `reload`
  - send `SIGHUP` so the gateway reloads config in-process
- `switch <blue|green>`
  - update `active_upstream`, reload config, verify the gateway now serves the
    expected backend port
- `status`
  - show current pid, listen addr, and active slot
- `stop`
  - stop the gateway process

Do not edit the pid file manually. Do not run multiple gateway operations in
parallel.

If the gateway is externally supervised, reuse the script for `check`,
`status`, and health inspection, but do not assume `reload` and `switch` will
work.

## Manual Reload and Rollback

For an externally supervised gateway, the safe manual fallback is:

1. `cp conf/pingora/staticflow-gateway.yaml tmp/<timestamp>.gateway.yaml.bak`
2. edit `active_upstream` to the candidate slot
3. `./scripts/pingora_gateway.sh check`
4. `kill -HUP <gateway-pid>`
5. wait for `http://127.0.0.1:39180/api/healthz` to report the candidate port

If step 5 fails:

1. restore the backup config
2. `kill -HUP <gateway-pid>` again
3. verify `http://127.0.0.1:39180/api/healthz` reports the old slot port
4. leave the old slot serving and stop there

The rollback path is "restore config and reload", not "restart everything and
hope".

## Environment Overrides

- `CONF_FILE`
  - alternate gateway YAML, useful for isolated test ports
- `GATEWAY_URL`
  - alternate gateway base URL for post-switch verification
- `FORCE_BUILD_GATEWAY=1`
  - rebuild the gateway binary even if one already exists
- `STATICFLOW_LOG_DIR`
  - override runtime log root

## Verification

Always verify from the stable gateway port after switching:

1. `./scripts/pingora_gateway.sh status`
2. `curl -fsS http://127.0.0.1:39180/api/healthz`
3. `curl -fsS 'http://127.0.0.1:39180/api/llm-gateway/status'`
4. If this is public production, also verify:
   - `env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY curl -fsS https://ackingliu.top/api/healthz`
5. Confirm:
   - `status == "ok"`
   - `port` matches the new backend slot
   - the old backend port is no longer the active route

Useful log roots:

- `tmp/runtime-logs/gateway/`
- `tmp/runtime-logs/backend-blue-39080/`
- `tmp/runtime-logs/backend-canary-39081/`

For `tmux`-managed slots, also inspect:

- `tmux capture-pane -pt staticflow-backend-blue:0 | tail -n 120`
- `tmux capture-pane -pt staticflow-gateway:0 | tail -n 120`

## Binary Path Pitfalls

- `scripts/start_backend_selfhosted.sh` prefers `BACKEND_BIN`, then
  `bin/static-flow-backend`, then `target/release-backend/static-flow-backend`.
- During blue-green rollout, do not overwrite `bin/static-flow-backend` before
  the candidate slot is up and healthy. Start the inactive slot from
  `target/release-backend/static-flow-backend` through `BACKEND_BIN`.
- If `cp target/release-backend/static-flow-backend bin/static-flow-backend`
  fails with `Text file busy`, do not force it. The usual causes are:
  - the old process is still executing `bin/static-flow-backend`
  - `bin/static-flow-backend` and `target/release-backend/static-flow-backend`
    are hard-linked to the same inode
- Safe update pattern after cutover:
  - `install -m 755 target/release-backend/static-flow-backend bin/static-flow-backend.new`
  - `mv -f bin/static-flow-backend.new bin/static-flow-backend`

## Cleanup Rules

- After a successful switch, stop the old slot and confirm its port no longer
  listens.
- Treat `tmux ls` plus `ss -ltnp` as the source of truth.
- A surviving `tmux` server process may keep the original `new-session` argv in
  `ps`, which can still mention `green` even after the `green` session is gone.
  Do not mistake that for a live backend slot.
- Do not kill unrelated development backends on other ports such as `39102` or
  `39121` unless they are actually occupying the blue/green slot ports.

## Stop Conditions

- If gateway config validation fails, stop before switching traffic.
- If the candidate backend never becomes healthy, leave the old slot serving.
- If gateway post-switch verification fails, switch back to the old slot before
  declaring success.
- If the gateway is not the active frontend yet, do not claim the rollout is
  seamless.
