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

1. Run the normal quality gates before deployment.
   - Format only touched Rust files with `rustfmt`
   - Run `cargo clippy` for affected crates with `-D warnings`
2. If gateway code or gateway scripts changed, refresh the gateway binary first:
   - `FORCE_BUILD_GATEWAY=1 ./scripts/pingora_gateway.sh check`
3. Run the rollout:
   - `./scripts/backend_gateway_upgrade.sh`
4. The rollout script is expected to:
   - build the candidate backend into `target/release-backend/static-flow-backend`
   - start the inactive slot
   - wait for `http://127.0.0.1:<candidate-port>/api/healthz`
   - switch the gateway to the new slot
   - verify `http://127.0.0.1:39180/api/healthz` now reports the new port
   - stop the old backend pid only after gateway verification succeeds

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
3. Confirm:
   - `status == "ok"`
   - `port` matches the new backend slot
   - the old backend port is no longer the active route

Useful log roots:

- `tmp/runtime-logs/gateway/`
- `tmp/runtime-logs/backend/`
- `tmp/runtime-logs/backend-canary-39081/`

## Stop Conditions

- If gateway config validation fails, stop before switching traffic.
- If the candidate backend never becomes healthy, leave the old slot serving.
- If gateway post-switch verification fails, switch back to the old slot before
  declaring success.
- If the gateway is not the active frontend yet, do not claim the rollout is
  seamless.
