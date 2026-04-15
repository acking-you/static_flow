#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source "$ROOT_DIR/scripts/lib_media_service_common.sh"

HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-39085}"
DAEMON="false"
BUILD_MEDIA="false"
LOG_FILE="${LOG_FILE:-}"
CANARY_BIN_PATH="${CANARY_BIN_PATH:-$ROOT_DIR/bin/static-flow-media-canary}"
PID_FILE="${PID_FILE:-}"
STATICFLOW_LOCAL_MEDIA_ROOT="${STATICFLOW_LOCAL_MEDIA_ROOT:-}"
STATICFLOW_LOCAL_MEDIA_CACHE_DIR="${STATICFLOW_LOCAL_MEDIA_CACHE_DIR:-$ROOT_DIR/tmp/local-media-cache-canary}"
STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG="${STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG:-}"

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

Environment variables (all optional):
  HOST                               Bind address (default: 127.0.0.1)
  PORT                               Fixed canary port (default: 39085)
  LOG_FILE                           Daemon log path (default: ./tmp/staticflow-media-canary-$PORT.log)
  PID_FILE                           Daemon pid file (default: ./tmp/staticflow-media-canary-$PORT.pid)
  CANARY_BIN_PATH                    Output binary path (default: ./bin/static-flow-media-canary)
  MEDIA_BIN                          Explicit media binary path
  STATICFLOW_LOCAL_MEDIA_ROOT        Media root (default: /mnt/e/videos/static)
  STATICFLOW_LOCAL_MEDIA_CACHE_DIR   Media cache dir (default: ./tmp/local-media-cache-canary)
  STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG Optional; default 1

Behavior:
  - Uses a fixed canary port unless PORT is overridden.
  - Waits for the standalone media service readiness before returning.
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

sf_apply_media_service_defaults

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
log "Using MEDIA_BIN=$MEDIA_BIN_PATH"
log "Using HOST=$HOST PORT=$PORT"
log "Using STATICFLOW_LOCAL_MEDIA_ROOT=$STATICFLOW_LOCAL_MEDIA_ROOT"
log "Using STATICFLOW_LOCAL_MEDIA_CACHE_DIR=$STATICFLOW_LOCAL_MEDIA_CACHE_DIR"
log "Using STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG=$STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG"

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
  if ! sf_wait_media_service_ready "$HOST" "$PORT"; then
    if kill -0 "$local_pid" 2>/dev/null; then
      fail "Media service failed to become ready: $(sf_media_service_health_url "$HOST" "$PORT" 1)"
    fi
    fail "Media service exited immediately. Check $LOG_FILE"
  fi
  log "Media service is ready at $(sf_media_service_health_url "$HOST" "$PORT" 1)"
  log "Verification URL: $(sf_media_service_health_url "$HOST" "$PORT" 2)"
  log "Daemon pid=$local_pid log=$LOG_FILE pid_file=$PID_FILE"
  exit 0
else
  "$MEDIA_BIN_PATH" &
  local_pid=$!

  cleanup() {
    if kill -0 "$local_pid" >/dev/null 2>&1; then
      log "Stopping media service (pid=$local_pid)..."
      kill "$local_pid" >/dev/null 2>&1 || true
      wait "$local_pid" 2>/dev/null || true
    fi
  }
  trap cleanup EXIT INT TERM

  if ! sf_wait_media_service_ready "$HOST" "$PORT"; then
    fail "Media service failed to become ready: $(sf_media_service_health_url "$HOST" "$PORT" 1)"
  fi

  log "Media service is ready."
  log "Verification URL: $(sf_media_service_health_url "$HOST" "$PORT" 2)"
  wait "$local_pid"
fi
