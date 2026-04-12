#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-39085}"
DAEMON="false"
BUILD_MEDIA="false"
LOG_FILE="${LOG_FILE:-}"
CANARY_BIN_PATH="${CANARY_BIN_PATH:-$ROOT_DIR/bin/static-flow-media-canary}"
PID_FILE="${PID_FILE:-}"
STATICFLOW_LOCAL_MEDIA_ROOT="${STATICFLOW_LOCAL_MEDIA_ROOT:-/mnt/e/videos/static/未归类}"
STATICFLOW_LOCAL_MEDIA_CACHE_DIR="${STATICFLOW_LOCAL_MEDIA_CACHE_DIR:-$ROOT_DIR/tmp/local-media-cache-canary}"
STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG="${STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG:-1}"

log() { echo "[media-canary] $*"; }
fail() { echo "[media-canary][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/start_media_service_canary.sh [options]

Options:
  --daemon         Run in background (nohup), log to LOG_FILE
  --port <port>    Override PORT (default: 39085)
  --host <addr>    Override HOST (default: 127.0.0.1)
  --build          Build release binary before starting
  -h, --help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon) DAEMON="true"; shift ;;
    --port) [[ $# -ge 2 ]] || fail "--port requires a value"; PORT="$2"; shift 2 ;;
    --host) [[ $# -ge 2 ]] || fail "--host requires a value"; HOST="$2"; shift 2 ;;
    --build) BUILD_MEDIA="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1 (use --help)" ;;
  esac
done

LOG_FILE="${LOG_FILE:-$ROOT_DIR/tmp/staticflow-media-canary-${PORT}.log}"
PID_FILE="${PID_FILE:-$ROOT_DIR/tmp/staticflow-media-canary-${PORT}.pid}"

mkdir -p "$ROOT_DIR/tmp" "$(dirname "$LOG_FILE")" "$(dirname "$CANARY_BIN_PATH")" "$(dirname "$PID_FILE")"

resolve_media_bin() {
  if [[ -n "${MEDIA_BIN:-}" && -x "$MEDIA_BIN" ]]; then
    echo "$MEDIA_BIN"; return
  fi
  if [[ -x "$CANARY_BIN_PATH" ]]; then
    echo "$CANARY_BIN_PATH"; return
  fi
  if [[ -x "$ROOT_DIR/target/release-backend/static-flow-media" ]]; then
    echo "$ROOT_DIR/target/release-backend/static-flow-media"; return
  fi
  if [[ -x "$ROOT_DIR/target/release/static-flow-media" ]]; then
    echo "$ROOT_DIR/target/release/static-flow-media"; return
  fi
  if [[ -x "$ROOT_DIR/target/debug/static-flow-media" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-media"; return
  fi
  fail "Media binary not found. Run with --build or: cargo build --profile release-backend -p static-flow-media"
}

build_media_bin() {
  log "Building media service canary via make bin-media ..."
  MEDIA_BIN_NAME="$(basename "$CANARY_BIN_PATH")" make bin-media >/dev/null
  chmod +x "$CANARY_BIN_PATH"
  log "Binary copied to ${CANARY_BIN_PATH#$ROOT_DIR/}"
}

if [[ "$BUILD_MEDIA" == "true" ]]; then
  build_media_bin
fi

MEDIA_BIN_PATH="$(resolve_media_bin)"
log "Binary:   $MEDIA_BIN_PATH"
log "Listen:   $HOST:$PORT"
log "Media root: $STATICFLOW_LOCAL_MEDIA_ROOT"
log "Media cache: $STATICFLOW_LOCAL_MEDIA_CACHE_DIR"
log "Auto download ffmpeg: $STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG"

export HOST
export PORT
export STATICFLOW_LOCAL_MEDIA_ROOT
export STATICFLOW_LOCAL_MEDIA_CACHE_DIR
export STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG

if [[ "$DAEMON" == "true" ]]; then
  : > "$LOG_FILE"
  rm -f "$PID_FILE"
  setsid "$MEDIA_BIN_PATH" < /dev/null >> "$LOG_FILE" 2>&1 &
  local_pid=$!
  echo "$local_pid" > "$PID_FILE"
  log "Started in background (pid=$local_pid, log=$LOG_FILE)"
  sleep 2
  if kill -0 "$local_pid" 2>/dev/null; then
    log "Media service is running. Verify: curl http://${HOST}:${PORT}/internal/local-media/list?limit=2"
  else
    fail "Media service exited immediately. Check $LOG_FILE"
  fi
else
  log "Starting in foreground (Ctrl+C to stop, log=$LOG_FILE)..."
  exec "$MEDIA_BIN_PATH"
fi
