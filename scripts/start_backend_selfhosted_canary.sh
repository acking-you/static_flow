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
LOG_FILE="${LOG_FILE:-$ROOT_DIR/tmp/staticflow-backend-canary.log}"
CANARY_BIN_PATH="${CANARY_BIN_PATH:-$ROOT_DIR/bin/static-flow-backend-canary}"
PID_FILE="${PID_FILE:-$ROOT_DIR/tmp/staticflow-backend-canary.pid}"

log() { echo "[canary] $*"; }
fail() { echo "[canary][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/start_backend_selfhosted_canary.sh [options]

Options:
  --daemon         Run in background (nohup), log to LOG_FILE
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
  LOG_FILE             Runtime log path (default: ./tmp/staticflow-backend-canary.log)
  CANARY_BIN_PATH      Output binary path (default: ./bin/static-flow-backend-canary)
  PID_FILE             Daemon pid file (default: ./tmp/staticflow-backend-canary.pid)
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

SITE_BASE_URL="${SITE_BASE_URL:-http://127.0.0.1:${PORT}}"

mkdir -p "$ROOT_DIR/tmp" "$(dirname "$LOG_FILE")" "$(dirname "$CANARY_BIN_PATH")" "$(dirname "$PID_FILE")"
if [[ "$DAEMON" != "true" ]]; then
  : > "$LOG_FILE"
  exec > >(tee -a "$LOG_FILE") 2>&1
fi

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

if [[ "$BUILD_FRONTEND" == "true" ]]; then
  log "Building frontend (selfhosted mode)..."
  "$ROOT_DIR/scripts/build_frontend_selfhosted.sh"
fi

if [[ "$BUILD_BACKEND" == "true" ]]; then
  log "Building backend (release-backend profile) for canary..."
  cargo build --profile release-backend -p static-flow-backend
  cp "$ROOT_DIR/target/release-backend/static-flow-backend" "$CANARY_BIN_PATH"
  chmod +x "$CANARY_BIN_PATH"
  log "Binary copied to ${CANARY_BIN_PATH#$ROOT_DIR/}"
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

export BIND_ADDR="$HOST"
export PORT
export LANCEDB_URI="$DB_PATH"
export COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH"
export MUSIC_LANCEDB_URI="$MUSIC_DB_PATH"
export SITE_BASE_URL
export FRONTEND_DIST_DIR
export COMMENT_AI_CONTENT_API_BASE
export MEM_PROF_ENABLED="${MEM_PROF_ENABLED:-0}"
export COMMENT_AI_CODEX_SANDBOX
export COMMENT_AI_CODEX_JSON_STREAM
export COMMENT_AI_CODEX_BYPASS
export COMMENT_AI_RESULT_DIR
export COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS

if [[ "$DAEMON" == "true" ]]; then
  : > "$LOG_FILE"
  rm -f "$PID_FILE"
  setsid "$BACKEND_BIN_PATH" < /dev/null >> "$LOG_FILE" 2>&1 &
  local_pid=$!
  echo "$local_pid" > "$PID_FILE"
  log "Started in background (pid=$local_pid, log=$LOG_FILE)"
  sleep 2
  if kill -0 "$local_pid" 2>/dev/null; then
    log "Backend is running. Verify: curl http://${HOST}:${PORT}/api/articles"
  else
    fail "Backend exited immediately. Check $LOG_FILE"
  fi
else
  log "Starting in foreground (Ctrl+C to stop, log=$LOG_FILE)..."
  exec "$BACKEND_BIN_PATH"
fi
