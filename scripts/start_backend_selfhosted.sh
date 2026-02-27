#!/usr/bin/env bash
set -euo pipefail

# Start backend in self-hosted mode (serves SPA + API + SEO on same origin).
#
# Pairs with: scripts/build_frontend_selfhosted.sh
#
# Usage:
#   ./scripts/start_backend_selfhosted.sh
#   ./scripts/start_backend_selfhosted.sh --daemon
#   DB_ROOT=/mnt/e/static-flow-data PORT=39080 ./scripts/start_backend_selfhosted.sh

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
DB_ROOT="${DB_ROOT:-/mnt/e/static-flow-data}"
DB_PATH="${DB_PATH:-${LANCEDB_URI:-$DB_ROOT/lancedb}}"
COMMENTS_DB_PATH="${COMMENTS_DB_PATH:-${COMMENTS_LANCEDB_URI:-$DB_ROOT/lancedb-comments}}"
MUSIC_DB_PATH="${MUSIC_DB_PATH:-${MUSIC_LANCEDB_URI:-$DB_ROOT/lancedb-music}}"
HOST="${HOST:-${BIND_ADDR:-127.0.0.1}}"
PORT="${PORT:-39080}"
SITE_BASE_URL="${SITE_BASE_URL:-https://ackingliu.top}"
FRONTEND_DIST_DIR="${FRONTEND_DIST_DIR:-$ROOT_DIR/frontend/dist}"
DAEMON="false"
LOG_FILE="${LOG_FILE:-/tmp/staticflow-backend.log}"

log() { echo "[selfhosted] $*"; }
fail() { echo "[selfhosted][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/start_backend_selfhosted.sh [options]

Options:
  --daemon         Run in background (nohup), log to LOG_FILE
  --port <port>    Override PORT (default: 39080)
  --host <addr>    Override BIND_ADDR (default: 127.0.0.1)
  --build          Build release binary before starting
  --build-frontend Build frontend (selfhosted mode) before starting
  -h, --help       Show this help

Environment variables (all optional):
  DB_ROOT              Data root (default: /mnt/e/static-flow-data)
  DB_PATH              Content DB override
  COMMENTS_DB_PATH     Comments DB override
  MUSIC_DB_PATH        Music DB override
  SITE_BASE_URL        Public URL (default: https://ackingliu.top)
  FRONTEND_DIST_DIR    Frontend dist path (default: ./frontend/dist)
  LOG_FILE             Daemon log path (default: /tmp/staticflow-backend.log)
  ADMIN_TOKEN          If set, allows remote admin access with this token
  ADMIN_LOCAL_ONLY     Default true; set to false to disable IP check

Worker env vars (passed through if set):
  COMMENT_AI_*         Comment AI worker config
  MUSIC_WISH_*         Music wish worker config
  ARTICLE_REQUEST_*    Article request worker config
EOF
}

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
BUILD_BACKEND="false"
BUILD_FRONTEND="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon)    DAEMON="true"; shift ;;
    --port)      [[ $# -ge 2 ]] || fail "--port requires a value"; PORT="$2"; shift 2 ;;
    --host)      [[ $# -ge 2 ]] || fail "--host requires a value"; HOST="$2"; shift 2 ;;
    --build)     BUILD_BACKEND="true"; shift ;;
    --build-frontend) BUILD_FRONTEND="true"; shift ;;
    -h|--help)   usage; exit 0 ;;
    *)           fail "Unknown option: $1 (use --help)" ;;
  esac
done

# ---------------------------------------------------------------------------
# Resolve binary
# ---------------------------------------------------------------------------
resolve_backend_bin() {
  if [[ -n "${BACKEND_BIN:-}" && -x "$BACKEND_BIN" ]]; then
    echo "$BACKEND_BIN"; return
  fi
  if [[ -x "$ROOT_DIR/bin/static-flow-backend" ]]; then
    echo "$ROOT_DIR/bin/static-flow-backend"; return
  fi
  if [[ -x "$ROOT_DIR/target/release/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/release/static-flow-backend"; return
  fi
  if [[ -x "$ROOT_DIR/target/debug/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-backend"; return
  fi
  fail "Backend binary not found. Run with --build or: cargo build --release -p static-flow-backend"
}

# ---------------------------------------------------------------------------
# Optional builds
# ---------------------------------------------------------------------------
if [[ "$BUILD_FRONTEND" == "true" ]]; then
  log "Building frontend (selfhosted mode)..."
  "$ROOT_DIR/scripts/build_frontend_selfhosted.sh"
fi

if [[ "$BUILD_BACKEND" == "true" ]]; then
  log "Building backend (release)..."
  cargo build --release -p static-flow-backend
  # Copy to bin/ for consistency
  cp "$ROOT_DIR/target/release/static-flow-backend" "$ROOT_DIR/bin/static-flow-backend"
  log "Binary copied to bin/static-flow-backend"
fi

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
BACKEND_BIN_PATH="$(resolve_backend_bin)"
[[ -d "$DB_PATH" ]] || fail "Content DB not found: $DB_PATH"
mkdir -p "$COMMENTS_DB_PATH" "$MUSIC_DB_PATH"

if [[ ! -f "$FRONTEND_DIST_DIR/index.html" ]]; then
  log "Warning: $FRONTEND_DIST_DIR/index.html not found — SEO pages will use fallback HTML"
fi

# Check port
if ss -tlnp 2>/dev/null | grep -q ":${PORT} "; then
  fail "Port $PORT is already in use"
fi

# ---------------------------------------------------------------------------
# Worker defaults (only set if not already in env)
# ---------------------------------------------------------------------------
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

# ---------------------------------------------------------------------------
# Export and run
# ---------------------------------------------------------------------------
export BIND_ADDR="$HOST"
export PORT
export LANCEDB_URI="$DB_PATH"
export COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH"
export MUSIC_LANCEDB_URI="$MUSIC_DB_PATH"
export SITE_BASE_URL
export FRONTEND_DIST_DIR
export COMMENT_AI_CONTENT_API_BASE
export COMMENT_AI_CODEX_SANDBOX
export COMMENT_AI_CODEX_JSON_STREAM
export COMMENT_AI_CODEX_BYPASS
export COMMENT_AI_RESULT_DIR
export COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS

if [[ "$DAEMON" == "true" ]]; then
  nohup "$BACKEND_BIN_PATH" > "$LOG_FILE" 2>&1 &
  local_pid=$!
  log "Started in background (pid=$local_pid, log=$LOG_FILE)"
  # Wait briefly and verify it's still running
  sleep 2
  if kill -0 "$local_pid" 2>/dev/null; then
    log "Backend is running. Verify: curl http://${HOST}:${PORT}/api/articles"
  else
    fail "Backend exited immediately. Check $LOG_FILE"
  fi
else
  log "Starting in foreground (Ctrl+C to stop)..."
  exec "$BACKEND_BIN_PATH"
fi
