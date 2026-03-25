---
name: selfhosted-seamless-upgrade
description: >-
  Rebuild StaticFlow self-hosted frontend and backend, then hot-swap
  `bin/static-flow-backend` with minimal downtime: keep the old process online
  until new artifacts are ready, redirect stdout/stderr to one log file, and
  verify the new server after restart. Use when the user asks for a local
  seamless upgrade, hot deploy, no/low-downtime restart, or rebuild-and-restart
  of the self-hosted backend.
---

# Selfhosted Seamless Upgrade

Use this skill when the user wants the local self-hosted StaticFlow deployment
updated to the latest workspace code with minimal interruption.

## Scope
1. Re-run quality gates before deployment.
2. Build `frontend/dist` for self-hosted mode.
3. Build the latest `release-backend` binary.
4. Keep the current backend online until the new artifacts are ready.
5. Swap the backend binary and restart with one merged log file.
6. Verify the new process on localhost after restart.

## Mandatory Workflow
1. Format only changed Rust files with `rustfmt`.
2. Run:
   - `cargo check --workspace`
   - `cargo clippy --workspace -- -D warnings`
3. Build self-hosted frontend with:
   - `env -u NO_COLOR ./scripts/build_frontend_selfhosted.sh`
   - local `trunk 0.21.x` may reject `NO_COLOR=1`; explicitly unset it
4. Build backend with:
   - `make bin-backend`
5. Do not stop the currently running backend before step 4 completes.

## Binary Swap Rule
- Desired runtime path is `./bin/static-flow-backend`.
- `scripts/start_backend_selfhosted.sh` resolves binaries in this order:
  - `BACKEND_BIN`
  - `./bin/static-flow-backend`
  - `./target/release-backend/static-flow-backend`
  - `./target/release/static-flow-backend`
  - `./target/debug/static-flow-backend`
- To guarantee the restart uses the newest backend, update
  `./bin/static-flow-backend` before starting the new process.
- If `make bin-backend` or manual copy fails with `Text file busy`, that means
  the old process is executing `./bin/static-flow-backend`. This is expected:
  1. finish the build first so `./target/release-backend/static-flow-backend` is ready
  2. stop the old backend
  3. copy `./target/release-backend/static-flow-backend` to `./bin/static-flow-backend`
  4. start the new backend immediately

## Restart Policy
- Prefer one merged log file under `/tmp/`, for example:
  - `/tmp/staticflow-backend-$(date +%Y%m%d-%H%M%S)-pty.log`
- In normal shells you may use the self-hosted script’s `--daemon` path.
- In Codex exec sessions, background `nohup` children may be reaped when the
  command session ends. In that environment, keep the backend alive with a
  persistent PTY-backed foreground process while redirecting stdout/stderr to
  the log file:
  - export the same env vars as `scripts/start_backend_selfhosted.sh`
  - `exec ./bin/static-flow-backend >>"$LOG_FILE" 2>&1`

## Environment Defaults
- `BIND_ADDR=127.0.0.1`
- `PORT=39080`
- `LANCEDB_URI=/mnt/wsl/data4tb/static-flow-data/lancedb`
- `COMMENTS_LANCEDB_URI=/mnt/wsl/data4tb/static-flow-data/lancedb-comments`
- `MUSIC_LANCEDB_URI=/mnt/wsl/data4tb/static-flow-data/lancedb-music`
- `FRONTEND_DIST_DIR=./frontend/dist`
- `SITE_BASE_URL=https://ackingliu.top`
- `COMMENT_AI_CONTENT_API_BASE=http://127.0.0.1:39080/api`

## Verification
1. Confirm the backend process exists after restart.
2. Confirm port `39080` is listening.
3. Verify at least:
   - `curl -fsS 'http://127.0.0.1:39080/api/articles?limit=1'`
   - `curl -fsS 'http://127.0.0.1:39080/api/llm-gateway/status'`
4. Tail the log file and confirm the new process reached `Listening on`.
5. Report:
   - log file path
   - current backend PID
   - whether build/check/clippy succeeded
   - verification endpoints used

## Stop Conditions
- If `cargo check` or `cargo clippy` fails, stop before deployment.
- If frontend build fails, do not restart the backend.
- If backend build fails, leave the old backend running.
- If restart fails, keep or restore the last known good binary/process before
  declaring success.
