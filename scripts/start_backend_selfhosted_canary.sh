#!/usr/bin/env bash
set -euo pipefail

# Start a canary backend in self-hosted mode on a separate port.
#
# Usage:
#   ./scripts/start_backend_selfhosted_canary.sh
#   ./scripts/start_backend_selfhosted_canary.sh --daemon --build
#   DB_ROOT=/mnt/wsl/data4tb/static-flow-data PORT=39081 ./scripts/start_backend_selfhosted_canary.sh

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source "$ROOT_DIR/scripts/lib_local_media_proxy_env.sh"

DB_ROOT="${DB_ROOT:-/mnt/wsl/data4tb/static-flow-data}"
DB_PATH="${DB_PATH:-${LANCEDB_URI:-$DB_ROOT/lancedb}}"
COMMENTS_DB_PATH="${COMMENTS_DB_PATH:-${COMMENTS_LANCEDB_URI:-$DB_ROOT/lancedb-comments}}"
MUSIC_DB_PATH="${MUSIC_DB_PATH:-${MUSIC_LANCEDB_URI:-$DB_ROOT/lancedb-music}}"
HOST="${HOST:-${BIND_ADDR:-127.0.0.1}}"
PORT="${PORT:-39081}"
FRONTEND_DIST_DIR="${FRONTEND_DIST_DIR:-$ROOT_DIR/frontend/dist}"
DAEMON="false"
BUILD_BACKEND="false"
BUILD_FRONTEND="false"
LOCAL_MEDIA_MODE="${LOCAL_MEDIA_MODE:-enabled}"
STATICFLOW_MEDIA_PROXY_BASE_URL="${STATICFLOW_MEDIA_PROXY_BASE_URL:-}"
BACKEND_DEFAULT_FEATURES="${BACKEND_DEFAULT_FEATURES:-}"
BACKEND_FEATURES="${BACKEND_FEATURES:-}"
FRONTEND_DEFAULT_FEATURES="${FRONTEND_DEFAULT_FEATURES:-}"
FRONTEND_FEATURES="${FRONTEND_FEATURES:-}"
LOG_FILE="${LOG_FILE:-}"
CANARY_BIN_PATH="${CANARY_BIN_PATH:-$ROOT_DIR/bin/static-flow-backend-canary}"
PID_FILE="${PID_FILE:-}"
STATICFLOW_LOG_DIR="${STATICFLOW_LOG_DIR:-$ROOT_DIR/tmp/runtime-logs}"

log() { echo "[canary] $*"; }
fail() { echo "[canary][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/start_backend_selfhosted_canary.sh [options]

Options:
  --daemon         Run in background (nohup), backend logs roll under STATICFLOW_LOG_DIR
  --port <port>    Override PORT (default: 39081)
  --host <addr>    Override BIND_ADDR (default: 127.0.0.1)
  --build          Build release binary before starting
  --build-frontend Build frontend (selfhosted mode) before starting
  -h, --help       Show this help

Environment variables (all optional):
  DB_ROOT              Data root (default: /mnt/wsl/data4tb/static-flow-data)
  DB_PATH              Content DB override
  COMMENTS_DB_PATH     Comments DB override
  MUSIC_DB_PATH        Music DB override
  SITE_BASE_URL        Public URL override (default: http://127.0.0.1:$PORT)
  FRONTEND_DIST_DIR    Frontend dist path (default: ./frontend/dist)
  LOG_FILE             Legacy wrapper log path (backend runtime logs now roll under STATICFLOW_LOG_DIR)
  CANARY_BIN_PATH      Output binary path (default: ./bin/static-flow-backend-canary)
  PID_FILE             Daemon pid file (default: ./tmp/staticflow-backend-canary.pid)
  STATICFLOW_LOG_DIR   Runtime log root (default: ./tmp/runtime-logs)
  STATICFLOW_LOG_SERVICE Service log folder name (default: backend-canary-$PORT)
  STATICFLOW_LOG_STDOUT Mirror backend logs to stdout; script overrides to 0 in daemon mode
  LOCAL_MEDIA_MODE     enabled|disabled (default: enabled)
  STATICFLOW_MEDIA_PROXY_BASE_URL Upstream media service base URL (default when enabled: http://127.0.0.1:39085)
  STATICFLOW_MEDIA_PROXY_HOST Default proxy host when base URL is unset
  STATICFLOW_MEDIA_PROXY_PORT Default proxy port when base URL is unset
  BACKEND_DEFAULT_FEATURES 0 disables default backend features for no-media build
  BACKEND_FEATURES     Extra cargo backend features
  FRONTEND_DEFAULT_FEATURES 0 disables default frontend features for no-media build
  FRONTEND_FEATURES    Extra trunk frontend features
  ADMIN_TOKEN          If set, allows remote admin access with this token
  ADMIN_LOCAL_ONLY     Default true; set to false to disable IP check

Worker env vars (passed through if set):
  COMMENT_AI_*         Comment AI worker config
  MUSIC_WISH_*         Music wish worker config
  ARTICLE_REQUEST_*    Article request worker config
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon) DAEMON="true"; shift ;;
    --port) [[ $# -ge 2 ]] || fail "--port requires a value"; PORT="$2"; shift 2 ;;
    --host) [[ $# -ge 2 ]] || fail "--host requires a value"; HOST="$2"; shift 2 ;;
    --build) BUILD_BACKEND="true"; shift ;;
    --build-frontend) BUILD_FRONTEND="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1 (use --help)" ;;
  esac
done

if [[ "$LOCAL_MEDIA_MODE" == "disabled" ]]; then
  if [[ -z "$BACKEND_DEFAULT_FEATURES" ]]; then
    BACKEND_DEFAULT_FEATURES="0"
  fi
  if [[ -z "$FRONTEND_DEFAULT_FEATURES" ]]; then
    FRONTEND_DEFAULT_FEATURES="0"
  fi
  if [[ "${CANARY_BIN_PATH}" == "$ROOT_DIR/bin/static-flow-backend-canary" ]]; then
    CANARY_BIN_PATH="$ROOT_DIR/bin/static-flow-backend-canary-no-media"
  fi
else
  if [[ -z "$BACKEND_DEFAULT_FEATURES" ]]; then
    BACKEND_DEFAULT_FEATURES="1"
  fi
  if [[ -z "$FRONTEND_DEFAULT_FEATURES" ]]; then
    FRONTEND_DEFAULT_FEATURES="1"
  fi
fi

sf_apply_local_media_proxy_defaults

SITE_BASE_URL="${SITE_BASE_URL:-http://127.0.0.1:${PORT}}"
LOG_FILE="${LOG_FILE:-$ROOT_DIR/tmp/staticflow-backend-canary-${PORT}.log}"
PID_FILE="${PID_FILE:-$ROOT_DIR/tmp/staticflow-backend-canary-${PORT}.pid}"
STATICFLOW_LOG_SERVICE="${STATICFLOW_LOG_SERVICE:-backend-canary-${PORT}}"

mkdir -p "$ROOT_DIR/tmp" "$(dirname "$LOG_FILE")" "$(dirname "$CANARY_BIN_PATH")" "$(dirname "$PID_FILE")"
mkdir -p "$STATICFLOW_LOG_DIR"

resolve_backend_bin() {
  if [[ -n "${BACKEND_BIN:-}" && -x "$BACKEND_BIN" ]]; then
    echo "$BACKEND_BIN"; return
  fi
  if [[ -x "$CANARY_BIN_PATH" ]]; then
    echo "$CANARY_BIN_PATH"; return
  fi
  if [[ -x "$ROOT_DIR/target/release-backend/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/release-backend/static-flow-backend"; return
  fi
  if [[ -x "$ROOT_DIR/target/release/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/release/static-flow-backend"; return
  fi
  if [[ -x "$ROOT_DIR/target/debug/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-backend"; return
  fi
  fail "Backend binary not found. Run with --build or: cargo build --profile release-backend -p static-flow-backend"
}

build_backend_bin() {
  log "Building backend for canary via make bin-backend ..."
  BACKEND_DEFAULT_FEATURES="$BACKEND_DEFAULT_FEATURES" \
  BACKEND_FEATURES="$BACKEND_FEATURES" \
  BACKEND_BIN_NAME="$(basename "$CANARY_BIN_PATH")" \
    make bin-backend >/dev/null
  chmod +x "$CANARY_BIN_PATH"
  log "Binary copied to ${CANARY_BIN_PATH#$ROOT_DIR/}"
}

if [[ "$BUILD_FRONTEND" == "true" ]]; then
  log "Building frontend (selfhosted mode)..."
  FRONTEND_DEFAULT_FEATURES="$FRONTEND_DEFAULT_FEATURES" \
  FRONTEND_FEATURES="$FRONTEND_FEATURES" \
    "$ROOT_DIR/scripts/build_frontend_selfhosted.sh"
fi

if [[ "$BUILD_BACKEND" == "true" ]]; then
  build_backend_bin
fi

BACKEND_BIN_PATH="$(resolve_backend_bin)"
[[ -d "$DB_PATH" ]] || fail "Content DB not found: $DB_PATH"
mkdir -p "$COMMENTS_DB_PATH" "$MUSIC_DB_PATH" "$ROOT_DIR/tmp" "$(dirname "$LOG_FILE")"

if [[ ! -f "$FRONTEND_DIST_DIR/index.html" ]]; then
  log "Warning: $FRONTEND_DIST_DIR/index.html not found — SEO pages will use fallback HTML"
fi

if ss -tlnp 2>/dev/null | grep -q ":${PORT} "; then
  fail "Port $PORT is already in use"
fi

: "${COMMENT_AI_CONTENT_API_BASE:=http://${HOST}:${PORT}/api}"
: "${COMMENT_AI_CODEX_SANDBOX:=danger-full-access}"
: "${COMMENT_AI_CODEX_JSON_STREAM:=1}"
: "${COMMENT_AI_CODEX_BYPASS:=0}"
: "${COMMENT_AI_RESULT_DIR:=/tmp/staticflow-comment-results}"
: "${COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS:=1}"

log "Binary:   $BACKEND_BIN_PATH"
log "DB root:  $DB_ROOT"
log "Listen:   $HOST:$PORT"
log "Site URL: $SITE_BASE_URL"
log "Frontend: $FRONTEND_DIST_DIR"
log "Local media mode: $LOCAL_MEDIA_MODE"
log "Media proxy base URL: ${STATICFLOW_MEDIA_PROXY_BASE_URL:-<disabled>}"
log "Build features: BACKEND_DEFAULT_FEATURES=$BACKEND_DEFAULT_FEATURES BACKEND_FEATURES=${BACKEND_FEATURES:-<none>} FRONTEND_DEFAULT_FEATURES=$FRONTEND_DEFAULT_FEATURES FRONTEND_FEATURES=${FRONTEND_FEATURES:-<none>}"

export BIND_ADDR="$HOST"
export PORT
export LANCEDB_URI="$DB_PATH"
export COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH"
export MUSIC_LANCEDB_URI="$MUSIC_DB_PATH"
export SITE_BASE_URL
export FRONTEND_DIST_DIR
export COMMENT_AI_CONTENT_API_BASE
export STATICFLOW_LOG_DIR
export STATICFLOW_LOG_SERVICE
export MEM_PROF_ENABLED="${MEM_PROF_ENABLED:-0}"
export COMMENT_AI_CODEX_SANDBOX
export COMMENT_AI_CODEX_JSON_STREAM
export COMMENT_AI_CODEX_BYPASS
export COMMENT_AI_RESULT_DIR
export COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS
if [[ "$LOCAL_MEDIA_MODE" == "enabled" ]]; then
  export STATICFLOW_MEDIA_PROXY_BASE_URL
else
  unset STATICFLOW_MEDIA_PROXY_BASE_URL
fi

if [[ "$DAEMON" == "true" ]]; then
  export STATICFLOW_LOG_STDOUT=0
  rm -f "$PID_FILE"
  setsid "$BACKEND_BIN_PATH" < /dev/null >/dev/null 2>&1 &
  local_pid=$!
  echo "$local_pid" > "$PID_FILE"
  log "Started in background (pid=$local_pid, runtime_logs=$STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE)"
  sleep 2
  if kill -0 "$local_pid" 2>/dev/null; then
    log "Backend is running. Verify: curl http://${HOST}:${PORT}/api/articles"
  else
    fail "Backend exited immediately. Check $STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE"
  fi
else
  export STATICFLOW_LOG_STDOUT="${STATICFLOW_LOG_STDOUT:-1}"
  log "Starting in foreground (Ctrl+C to stop, runtime_logs=$STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE)..."
  exec "$BACKEND_BIN_PATH"
fi
