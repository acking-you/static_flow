#!/usr/bin/env bash
set -euo pipefail

# Start a local-only backend dedicated to LLM gateway validation.
#
# This script intentionally uses an isolated /tmp data root instead of any
# production LanceDB directories. It is suitable for:
# - validating /api/llm-gateway/v1/*
# - creating a temporary test API key
# - letting Codex point at a disposable OpenAI-compatible base URL
#
# Usage:
#   ./scripts/start_backend_llm_gateway_dev.sh
#   ./scripts/start_backend_llm_gateway_dev.sh --daemon
#   ./scripts/start_backend_llm_gateway_dev.sh --fresh --build

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source "$ROOT_DIR/scripts/lib_local_media_proxy_env.sh"

STATE_DIR="${STATE_DIR:-/tmp/staticflow-llm-gateway-e2e}"
DB_ROOT="${DB_ROOT:-$STATE_DIR}"
DB_PATH="${DB_PATH:-${LANCEDB_URI:-$DB_ROOT/content}}"
COMMENTS_DB_PATH="${COMMENTS_DB_PATH:-${COMMENTS_LANCEDB_URI:-$DB_ROOT/comments}}"
MUSIC_DB_PATH="${MUSIC_DB_PATH:-${MUSIC_LANCEDB_URI:-$DB_ROOT/music}}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-3000}"
FRONTEND_DIST_DIR="${FRONTEND_DIST_DIR:-$ROOT_DIR/frontend/dist}"
LOG_FILE="${LOG_FILE:-$STATE_DIR/backend.log}"
KEY_FILE="${KEY_FILE:-$STATE_DIR/key-created-latest.json}"
KEY_NAME="${KEY_NAME:-llm-dev-public}"
KEY_LIMIT="${KEY_LIMIT:-100000000}"
KEY_PUBLIC_VISIBLE="${KEY_PUBLIC_VISIBLE:-true}"
RUST_LOG="${RUST_LOG:-warn,static_flow_backend=info,static_flow_backend::llm_gateway=debug}"
TABLE_COMPACT_ENABLED="${TABLE_COMPACT_ENABLED:-0}"
ADMIN_LOCAL_ONLY="${ADMIN_LOCAL_ONLY:-true}"
STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL="${STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL:-http://127.0.0.1:11111}"
DAEMON="false"
BUILD_BACKEND="false"
FRESH_STATE="false"
AUTO_CREATE_TEST_KEY="${AUTO_CREATE_TEST_KEY:-1}"

log() {
  echo "[llm-gateway-dev] $*"
}

fail() {
  echo "[llm-gateway-dev][ERROR] $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: ./scripts/start_backend_llm_gateway_dev.sh [options]

Options:
  --daemon         Run in background and write logs to LOG_FILE
  --build          Build debug backend before starting
  --fresh          Remove STATE_DIR first (safe only under /tmp)
  --host <addr>    Override HOST (default: 127.0.0.1)
  --port <port>    Override PORT (default: 3000)
  --no-test-key    Do not auto-create/reuse a test LLM gateway key
  -h, --help       Show this help

Environment variables:
  STATE_DIR        Root of isolated temp state (default: /tmp/staticflow-llm-gateway-e2e)
  DB_ROOT          Override temp DB root (default: $STATE_DIR)
  DB_PATH          Content DB override (default: $DB_ROOT/content)
  COMMENTS_DB_PATH Comments DB override (default: $DB_ROOT/comments)
  MUSIC_DB_PATH    Music DB override (default: $DB_ROOT/music)
  LOG_FILE         Backend log path (default: $STATE_DIR/backend.log)
  KEY_FILE         Where the created/reused test key JSON is written
  KEY_NAME         Test key display name (default: llm-dev-public)
  KEY_LIMIT        Test key quota_billable_limit (default: 100000000)
  KEY_PUBLIC_VISIBLE true/false (default: true)
  CODEX_AUTH_JSON_PATH Optional upstream Codex auth.json override
  STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL Optional upstream base override
  STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL Optional proxy override (default: http://127.0.0.1:11111)
  RUST_LOG         Backend log filter

Safety:
  By default this script refuses to use non-/tmp DB paths. Set ALLOW_NON_TMP_DB=1
  only if you intentionally want to bypass that guard.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon) DAEMON="true"; shift ;;
    --build) BUILD_BACKEND="true"; shift ;;
    --fresh) FRESH_STATE="true"; shift ;;
    --host) [[ $# -ge 2 ]] || fail "--host requires a value"; HOST="$2"; shift 2 ;;
    --port) [[ $# -ge 2 ]] || fail "--port requires a value"; PORT="$2"; shift 2 ;;
    --no-test-key) AUTO_CREATE_TEST_KEY="0"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1 (use --help)" ;;
  esac
done

sf_apply_local_media_proxy_defaults

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
  if [[ -x "$ROOT_DIR/bin/static-flow-backend" ]]; then
    echo "$ROOT_DIR/bin/static-flow-backend"
    return
  fi
  log "Backend binary not found, building debug binary..."
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
    if curl -fsS "http://${host}:${port}/api/llm-gateway/access" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

reuse_existing_key() {
  local host="$1"
  local port="$2"
  local tmp_json
  tmp_json="$(mktemp)"
  if ! curl -fsS "http://${host}:${port}/admin/llm-gateway/keys" >"$tmp_json"; then
    rm -f "$tmp_json"
    return 1
  fi
  if python3 - "$tmp_json" "$KEY_NAME" "$KEY_FILE" <<'PY'
import json, sys
src, wanted_name, out = sys.argv[1:4]
with open(src, 'r', encoding='utf-8') as fh:
    payload = json.load(fh)
for row in payload.get("keys", []):
    if row.get("name") == wanted_name and row.get("status") == "active":
        with open(out, 'w', encoding='utf-8') as wf:
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
  local host="$1"
  local port="$2"
  local public_literal="true"
  if [[ "$KEY_PUBLIC_VISIBLE" != "true" ]]; then
    public_literal="false"
  fi
  curl -fsS \
    -X POST "http://${host}:${port}/admin/llm-gateway/keys" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"${KEY_NAME}\",\"quota_billable_limit\":${KEY_LIMIT},\"public_visible\":${public_literal}}" \
    >"$KEY_FILE"
}

maybe_prepare_test_key() {
  local host="$1"
  local port="$2"
  [[ "$AUTO_CREATE_TEST_KEY" == "1" ]] || return 0
  mkdir -p "$(dirname "$KEY_FILE")"
  if reuse_existing_key "$host" "$port"; then
    log "Reused existing test key: $KEY_FILE"
  else
    create_test_key "$host" "$port"
    log "Created new test key: $KEY_FILE"
  fi
}

print_summary() {
  local host="$1"
  local port="$2"
  local base_url="http://${host}:${port}/api/llm-gateway/v1"

  echo
  log "Backend is ready."
  log "Listen:        http://${host}:${port}"
  log "Gateway base:  ${base_url}"
  log "Access page:   http://${host}:${port}/llm-access"
  log "Access API:    http://${host}:${port}/api/llm-gateway/access"
  log "Admin keys:    http://${host}:${port}/admin/llm-gateway/keys"
  log "Models:        ${base_url}/models"
  log "Responses:     ${base_url}/responses"
  log "Compact:       ${base_url}/responses/compact"
  if [[ "$AUTO_CREATE_TEST_KEY" == "1" ]]; then
    log "Key JSON:      $KEY_FILE"
  fi
  echo
  echo "[Codex config]"
  cat <<EOF
model_provider = "staticflow"

[model_providers.staticflow]
name = "OpenAI"
base_url = "${base_url}"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
EOF
  echo
  echo "[Quick checks]"
  echo "curl ${base_url}/models"
  echo "python3 scripts/test_llm_gateway_chat.py --base-url ${base_url%/v1} --api-key-file ${KEY_FILE} \"Reply with exactly OK.\""
  echo
}

assert_tmp_path "$STATE_DIR" "STATE_DIR"
assert_tmp_path "$DB_ROOT" "DB_ROOT"
assert_tmp_path "$DB_PATH" "DB_PATH"
assert_tmp_path "$COMMENTS_DB_PATH" "COMMENTS_DB_PATH"
assert_tmp_path "$MUSIC_DB_PATH" "MUSIC_DB_PATH"
assert_tmp_path "$LOG_FILE" "LOG_FILE"
assert_tmp_path "$KEY_FILE" "KEY_FILE"

if [[ "$FRESH_STATE" == "true" ]]; then
  log "Removing previous temp state under $STATE_DIR"
  rm -rf "$STATE_DIR"
fi

mkdir -p "$STATE_DIR" "$DB_PATH" "$COMMENTS_DB_PATH" "$MUSIC_DB_PATH" "$(dirname "$LOG_FILE")"

if [[ "$BUILD_BACKEND" == "true" ]]; then
  log "Building debug backend..."
  cargo build -p static-flow-backend
fi

BACKEND_BIN_PATH="$(resolve_backend_bin)"

if is_port_busy "$PORT"; then
  fail "Port $PORT is already in use. Refusing to replace the existing process."
fi

if [[ ! -f "$FRONTEND_DIST_DIR/index.html" ]]; then
  log "Warning: $FRONTEND_DIST_DIR/index.html not found — SPA fallback pages may be limited."
fi

log "Using isolated STATE_DIR=$STATE_DIR"
log "Using CONTENT_DB_PATH=$DB_PATH"
log "Using COMMENTS_DB_PATH=$COMMENTS_DB_PATH"
log "Using MUSIC_DB_PATH=$MUSIC_DB_PATH"
log "Using BACKEND_BIN=$BACKEND_BIN_PATH"
log "Using HOST=$HOST PORT=$PORT"
log "Using LOG_FILE=$LOG_FILE"
log "Using TABLE_COMPACT_ENABLED=$TABLE_COMPACT_ENABLED"
log "Using STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL=$STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL"
log "Using LOCAL_MEDIA_MODE=$LOCAL_MEDIA_MODE"
log "Using STATICFLOW_MEDIA_PROXY_BASE_URL=${STATICFLOW_MEDIA_PROXY_BASE_URL:-<unset>}"

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
  STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL="$STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL" \
  STATICFLOW_MEDIA_PROXY_BASE_URL="${STATICFLOW_MEDIA_PROXY_BASE_URL:-}" \
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
    STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL="$STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL" \
    STATICFLOW_MEDIA_PROXY_BASE_URL="${STATICFLOW_MEDIA_PROXY_BASE_URL:-}" \
    MEM_PROF_ENABLED="${MEM_PROF_ENABLED:-0}" \
    "$BACKEND_BIN_PATH" >"$LOG_FILE" 2>&1 < /dev/null &
  BACKEND_PID=$!
  log "Started in background (pid=$BACKEND_PID)"
  sleep 1
  if ! kill -0 "$BACKEND_PID" >/dev/null 2>&1; then
    fail "Backend exited immediately. Check $LOG_FILE"
  fi
  if ! wait_backend_ready "$HOST" "$PORT"; then
    fail "Backend failed to become ready. Check $LOG_FILE"
  fi
  maybe_prepare_test_key "$HOST" "$PORT"
  print_summary "$HOST" "$PORT"
  exit 0
fi

run_backend >"$LOG_FILE" 2>&1 &
BACKEND_PID=$!

cleanup() {
  if kill -0 "$BACKEND_PID" >/dev/null 2>&1; then
    log "Stopping backend (pid=$BACKEND_PID)..."
    kill "$BACKEND_PID" >/dev/null 2>&1 || true
    wait "$BACKEND_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

if ! wait_backend_ready "$HOST" "$PORT"; then
  fail "Backend failed to become ready. Check $LOG_FILE"
fi

maybe_prepare_test_key "$HOST" "$PORT"
print_summary "$HOST" "$PORT"
log "Foreground mode: tailing log file. Press Ctrl+C to stop."
tail -n +1 -f "$LOG_FILE" &
TAIL_PID=$!
wait "$BACKEND_PID"
kill "$TAIL_PID" >/dev/null 2>&1 || true
