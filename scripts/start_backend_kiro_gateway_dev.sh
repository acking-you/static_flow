#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

STATE_DIR="${STATE_DIR:-/tmp/staticflow-kiro-gateway-e2e}"
DB_ROOT="${DB_ROOT:-$STATE_DIR}"
DB_PATH="${DB_PATH:-${LANCEDB_URI:-$DB_ROOT/content}}"
COMMENTS_DB_PATH="${COMMENTS_DB_PATH:-${COMMENTS_LANCEDB_URI:-$DB_ROOT/comments}}"
MUSIC_DB_PATH="${MUSIC_DB_PATH:-${MUSIC_LANCEDB_URI:-$DB_ROOT/music}}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-3010}"
FRONTEND_DIST_DIR="${FRONTEND_DIST_DIR:-$ROOT_DIR/frontend/dist}"
LOG_FILE="${LOG_FILE:-$STATE_DIR/backend.log}"
KEY_FILE="${KEY_FILE:-$STATE_DIR/kiro-key-created-latest.json}"
KEY_NAME="${KEY_NAME:-kiro-dev-public}"
KEY_LIMIT="${KEY_LIMIT:-100000000}"
KEY_PUBLIC_VISIBLE="${KEY_PUBLIC_VISIBLE:-true}"
KIRO_SQLITE_PATH="${KIRO_SQLITE_PATH:-$HOME/.local/share/kiro-cli/data.sqlite3}"
STATICFLOW_KIRO_AUTHS_DIR="${STATICFLOW_KIRO_AUTHS_DIR:-$STATE_DIR/kiro-auths}"
RUST_LOG="${RUST_LOG:-warn,static_flow_backend=info,static_flow_backend::kiro_gateway=debug}"
TABLE_COMPACT_ENABLED="${TABLE_COMPACT_ENABLED:-0}"
ADMIN_LOCAL_ONLY="${ADMIN_LOCAL_ONLY:-true}"
STATICFLOW_KIRO_UPSTREAM_PROXY_URL="${STATICFLOW_KIRO_UPSTREAM_PROXY_URL:-${STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL:-http://127.0.0.1:11111}}"
AUTO_IMPORT_LOCAL_AUTH="${AUTO_IMPORT_LOCAL_AUTH:-1}"
AUTO_CREATE_TEST_KEY="${AUTO_CREATE_TEST_KEY:-1}"
FORCE_REFRESH_ON_START="${FORCE_REFRESH_ON_START:-1}"
DAEMON="false"
BUILD_BACKEND="true"
FRESH_STATE="false"

log() {
  echo "[kiro-gateway-dev] $*"
}

fail() {
  echo "[kiro-gateway-dev][ERROR] $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: ./scripts/start_backend_kiro_gateway_dev.sh [options]

Options:
  --daemon         Run in background
  --build          Build debug backend before starting
  --fresh          Remove STATE_DIR first (must stay under /tmp by default)
  --host <addr>    Override HOST
  --port <port>    Override PORT
  --no-import      Skip auto-importing local Kiro CLI auth
  --no-test-key    Skip auto-creating/reusing a Kiro key
  -h, --help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon) DAEMON="true"; shift ;;
    --build) BUILD_BACKEND="true"; shift ;;
    --fresh) FRESH_STATE="true"; shift ;;
    --host) [[ $# -ge 2 ]] || fail "--host requires a value"; HOST="$2"; shift 2 ;;
    --port) [[ $# -ge 2 ]] || fail "--port requires a value"; PORT="$2"; shift 2 ;;
    --no-import) AUTO_IMPORT_LOCAL_AUTH="0"; shift ;;
    --no-test-key) AUTO_CREATE_TEST_KEY="0"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1" ;;
  esac
done

realish_path() {
  python3 - "$1" <<'PY'
import os, sys
print(os.path.realpath(sys.argv[1]))
PY
}

assert_tmp_path() {
  local path_value="$1"
  local label="$2"
  if [[ "${ALLOW_NON_TMP_DB:-0}" == "1" ]]; then
    return
  fi
  local resolved
  resolved="$(realish_path "$path_value")"
  if [[ "$resolved" != /tmp/* ]]; then
    fail "$label must stay under /tmp for this dev script: $resolved (set ALLOW_NON_TMP_DB=1 to bypass)"
  fi
}

is_port_busy() {
  local port="$1"
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
}

resolve_backend_bin() {
  if [[ -n "${BACKEND_BIN:-}" && -x "$BACKEND_BIN" ]]; then
    echo "$BACKEND_BIN"
    return
  fi
  if [[ -x "$ROOT_DIR/target/debug/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-backend"
    return
  fi
  if [[ -x "$ROOT_DIR/target/release-backend/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/release-backend/static-flow-backend"
    return
  fi
  cargo build -p static-flow-backend >/dev/null
  if [[ -x "$ROOT_DIR/target/debug/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-backend"
    return
  fi
  fail "Failed to build/find static-flow-backend binary."
}

wait_backend_ready() {
  local host="$1"
  local port="$2"
  for _ in $(seq 1 120); do
    if curl -fsS "http://${host}:${port}/api/kiro-gateway/access" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

import_local_auth() {
  [[ "$AUTO_IMPORT_LOCAL_AUTH" == "1" ]] || return 0
  [[ -f "$KIRO_SQLITE_PATH" ]] || fail "Kiro sqlite not found: $KIRO_SQLITE_PATH"
  log "Importing local Kiro auth from $KIRO_SQLITE_PATH"
  curl -fsS \
    -X POST "http://${HOST}:${PORT}/admin/kiro-gateway/accounts/import-local" \
    -H "Content-Type: application/json" \
    -d "{\"sqlite_path\":\"${KIRO_SQLITE_PATH}\"}" >/dev/null
}

force_refresh_path() {
  [[ "$FORCE_REFRESH_ON_START" == "1" ]] || return 0
  local auth_file="$STATICFLOW_KIRO_AUTHS_DIR/default.json"
  [[ -f "$auth_file" ]] || return 0
  log "Clearing cached access token to force refresh on first Kiro request"
  python3 - "$auth_file" <<'PY'
import json, sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as fh:
    payload = json.load(fh)
payload["accessToken"] = ""
payload["expiresAt"] = None
with open(path, "w", encoding="utf-8") as fh:
    json.dump(payload, fh, ensure_ascii=False, indent=2)
PY
}

reuse_existing_key() {
  local tmp_json
  tmp_json="$(mktemp)"
  if ! curl -fsS "http://${HOST}:${PORT}/admin/kiro-gateway/keys" >"$tmp_json"; then
    rm -f "$tmp_json"
    return 1
  fi
  if python3 - "$tmp_json" "$KEY_NAME" "$KEY_FILE" <<'PY'
import json, sys
src, wanted_name, out = sys.argv[1:4]
with open(src, "r", encoding="utf-8") as fh:
    payload = json.load(fh)
for row in payload.get("keys", []):
    if row.get("name") == wanted_name and row.get("status") == "active":
        with open(out, "w", encoding="utf-8") as wf:
            json.dump(row, wf, ensure_ascii=False, indent=2)
        raise SystemExit(0)
raise SystemExit(1)
PY
  then
    rm -f "$tmp_json"
    return 0
  fi
  rm -f "$tmp_json"
  return 1
}

create_test_key() {
  local public_literal="true"
  if [[ "$KEY_PUBLIC_VISIBLE" != "true" ]]; then
    public_literal="false"
  fi
  curl -fsS \
    -X POST "http://${HOST}:${PORT}/admin/kiro-gateway/keys" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"${KEY_NAME}\",\"quota_billable_limit\":${KEY_LIMIT},\"public_visible\":${public_literal}}" \
    >"$KEY_FILE"
}

maybe_prepare_test_key() {
  [[ "$AUTO_CREATE_TEST_KEY" == "1" ]] || return 0
  mkdir -p "$(dirname "$KEY_FILE")"
  if reuse_existing_key; then
    log "Reused existing Kiro key: $KEY_FILE"
  else
    create_test_key
    log "Created new Kiro key: $KEY_FILE"
  fi
}

print_summary() {
  local base_root="http://${HOST}:${PORT}/api/kiro-gateway"
  echo
  log "Backend is ready."
  log "Listen:          http://${HOST}:${PORT}"
  log "Kiro access:     http://${HOST}:${PORT}/kiro-access"
  log "Kiro admin:      http://${HOST}:${PORT}/admin/kiro-gateway"
  log "Gateway root:    ${base_root}"
  log "Models:          ${base_root}/v1/models"
  log "Messages:        ${base_root}/v1/messages"
  log "Claude Code:     ${base_root}/cc/v1/messages"
  log "Auths dir:       ${STATICFLOW_KIRO_AUTHS_DIR}"
  if [[ "$AUTO_CREATE_TEST_KEY" == "1" ]]; then
    log "Key JSON:        $KEY_FILE"
  fi
  echo
  echo "[Claude Code / Anthropic env]"
  cat <<EOF
export ANTHROPIC_BASE_URL="${base_root}"
export ANTHROPIC_API_KEY="$(python3 - "$KEY_FILE" <<'PY'
import json, sys
try:
    with open(sys.argv[1], "r", encoding="utf-8") as fh:
        print(json.load(fh).get("secret", ""))
except Exception:
    print("")
PY
)"
EOF
  echo
}

assert_tmp_path "$STATE_DIR" "STATE_DIR"
assert_tmp_path "$DB_ROOT" "DB_ROOT"
assert_tmp_path "$DB_PATH" "DB_PATH"
assert_tmp_path "$COMMENTS_DB_PATH" "COMMENTS_DB_PATH"
assert_tmp_path "$MUSIC_DB_PATH" "MUSIC_DB_PATH"
assert_tmp_path "$LOG_FILE" "LOG_FILE"
assert_tmp_path "$KEY_FILE" "KEY_FILE"
assert_tmp_path "$STATICFLOW_KIRO_AUTHS_DIR" "STATICFLOW_KIRO_AUTHS_DIR"

if [[ "$FRESH_STATE" == "true" ]]; then
  rm -rf "$STATE_DIR"
fi

mkdir -p \
  "$STATE_DIR" \
  "$DB_PATH" \
  "$COMMENTS_DB_PATH" \
  "$MUSIC_DB_PATH" \
  "$STATICFLOW_KIRO_AUTHS_DIR" \
  "$(dirname "$LOG_FILE")"

if [[ "$BUILD_BACKEND" == "true" ]]; then
  cargo build -p static-flow-backend
fi

BACKEND_BIN_PATH="$(resolve_backend_bin)"

if is_port_busy "$PORT"; then
  fail "Port $PORT is already in use."
fi

run_backend() {
  RUST_ENV=development \
  RUST_LOG="$RUST_LOG" \
  BIND_ADDR="$HOST" \
  PORT="$PORT" \
  LANCEDB_URI="$DB_PATH" \
  COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH" \
  MUSIC_LANCEDB_URI="$MUSIC_DB_PATH" \
  FRONTEND_DIST_DIR="$FRONTEND_DIST_DIR" \
  ADMIN_LOCAL_ONLY="$ADMIN_LOCAL_ONLY" \
  TABLE_COMPACT_ENABLED="$TABLE_COMPACT_ENABLED" \
  STATICFLOW_KIRO_AUTHS_DIR="$STATICFLOW_KIRO_AUTHS_DIR" \
  STATICFLOW_KIRO_UPSTREAM_PROXY_URL="$STATICFLOW_KIRO_UPSTREAM_PROXY_URL" \
  MEM_PROF_ENABLED="${MEM_PROF_ENABLED:-0}" \
  "${BACKEND_BIN_PATH}"
}

if [[ "$DAEMON" == "true" ]]; then
  nohup env \
    RUST_ENV=development \
    RUST_LOG="$RUST_LOG" \
    BIND_ADDR="$HOST" \
    PORT="$PORT" \
    LANCEDB_URI="$DB_PATH" \
    COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH" \
    MUSIC_LANCEDB_URI="$MUSIC_DB_PATH" \
    FRONTEND_DIST_DIR="$FRONTEND_DIST_DIR" \
    ADMIN_LOCAL_ONLY="$ADMIN_LOCAL_ONLY" \
    TABLE_COMPACT_ENABLED="$TABLE_COMPACT_ENABLED" \
    STATICFLOW_KIRO_AUTHS_DIR="$STATICFLOW_KIRO_AUTHS_DIR" \
    STATICFLOW_KIRO_UPSTREAM_PROXY_URL="$STATICFLOW_KIRO_UPSTREAM_PROXY_URL" \
    MEM_PROF_ENABLED="${MEM_PROF_ENABLED:-0}" \
    "$BACKEND_BIN_PATH" >"$LOG_FILE" 2>&1 < /dev/null &
  BACKEND_PID=$!
  sleep 1
  kill -0 "$BACKEND_PID" >/dev/null 2>&1 || fail "Backend exited immediately. Check $LOG_FILE"
  wait_backend_ready "$HOST" "$PORT" || fail "Backend failed to become ready. Check $LOG_FILE"
  import_local_auth
  force_refresh_path
  maybe_prepare_test_key
  print_summary
  exit 0
fi

run_backend >"$LOG_FILE" 2>&1 &
BACKEND_PID=$!

cleanup() {
  if kill -0 "$BACKEND_PID" >/dev/null 2>&1; then
    kill "$BACKEND_PID" >/dev/null 2>&1 || true
    wait "$BACKEND_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

wait_backend_ready "$HOST" "$PORT" || fail "Backend failed to become ready. Check $LOG_FILE"
import_local_auth
force_refresh_path
maybe_prepare_test_key
print_summary
log "Foreground mode: tailing log file. Press Ctrl+C to stop."
tail -n +1 -f "$LOG_FILE" &
TAIL_PID=$!
wait "$BACKEND_PID"
kill "$TAIL_PID" >/dev/null 2>&1 || true
