#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST="${HOST:-127.0.0.1}"
PORT_BASE="${PORT_BASE:-39085}"
PORT_SCAN_LIMIT="${PORT_SCAN_LIMIT:-40}"
MEDIA_BIN_NAME="${MEDIA_BIN_NAME:-static-flow-media}"
LOG_FILE="${LOG_FILE:-$ROOT_DIR/tmp/staticflow-media.log}"
PID_FILE="${PID_FILE:-$ROOT_DIR/tmp/staticflow-media.pid}"
DAEMON="false"

log() {
  echo "[start-media] $*"
}

fail() {
  echo "[start-media][ERROR] $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: ./scripts/start_media_service_from_tmp.sh [--daemon]

Environment variables:
  HOST                               Bind address (default: 127.0.0.1)
  PORT                               Fixed port override
  PORT_BASE                          Port scan start (default: 39085)
  PORT_SCAN_LIMIT                    Number of ports to scan (default: 40)
  MEDIA_BIN_NAME                     Output binary name under ./bin
  MEDIA_BIN                          Explicit media binary path (skips make bin-media)
  LOG_FILE                           Daemon log path (default: ./tmp/staticflow-media.log)
  PID_FILE                           Daemon pid file (default: ./tmp/staticflow-media.pid)
  STATICFLOW_LOCAL_MEDIA_ROOT        Required media root
  STATICFLOW_LOCAL_MEDIA_CACHE_DIR   Optional media cache dir
  STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG Optional; default 1

Behavior:
  - Always builds via `make bin-media` unless MEDIA_BIN is provided.
  - Starts the standalone media service on a free local high port.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon) DAEMON="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1 (use --help)" ;;
  esac
done

[[ -n "${STATICFLOW_LOCAL_MEDIA_ROOT:-}" ]] || fail "STATICFLOW_LOCAL_MEDIA_ROOT is required"

is_port_busy() {
  local port="$1"
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
}

choose_port() {
  if [[ -n "${PORT:-}" ]]; then
    if is_port_busy "$PORT"; then
      fail "PORT=$PORT is already in use"
    fi
    echo "$PORT"
    return
  fi

  local port
  for ((port=PORT_BASE; port<PORT_BASE+PORT_SCAN_LIMIT; port++)); do
    if ! is_port_busy "$port"; then
      echo "$port"
      return
    fi
  done

  fail "No free high port found in [$PORT_BASE, $((PORT_BASE + PORT_SCAN_LIMIT - 1))]"
}

build_media_bin() {
  if [[ -n "${MEDIA_BIN:-}" ]]; then
    return
  fi

  log "Building media binary via make bin-media ..."
  MEDIA_BIN_NAME="$MEDIA_BIN_NAME" make bin-media >/dev/null
}

resolve_media_bin() {
  if [[ -n "${MEDIA_BIN:-}" && -x "${MEDIA_BIN}" ]]; then
    echo "$MEDIA_BIN"
    return
  fi

  local bin_path="$ROOT_DIR/bin/$MEDIA_BIN_NAME"
  if [[ -x "$bin_path" ]]; then
    echo "$bin_path"
    return
  fi

  fail "Failed to build/find media binary: $bin_path"
}

wait_media_ready() {
  local host="$1"
  local port="$2"

  for _ in $(seq 1 80); do
    if curl -fsS "http://${host}:${port}/internal/local-media/list?limit=1" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done

  return 1
}

PORT_CHOSEN="$(choose_port)"
build_media_bin
MEDIA_BIN_PATH="$(resolve_media_bin)"
STATICFLOW_LOCAL_MEDIA_CACHE_DIR_EFFECTIVE="${STATICFLOW_LOCAL_MEDIA_CACHE_DIR:-$ROOT_DIR/tmp/local-media-cache}"
STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG_EFFECTIVE="${STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG:-1}"

mkdir -p "$ROOT_DIR/tmp" "$(dirname "$LOG_FILE")" "$(dirname "$PID_FILE")"

log "Using MEDIA_BIN=$MEDIA_BIN_PATH"
log "Using HOST=$HOST PORT=$PORT_CHOSEN"
log "Using STATICFLOW_LOCAL_MEDIA_ROOT=$STATICFLOW_LOCAL_MEDIA_ROOT"
log "Using STATICFLOW_LOCAL_MEDIA_CACHE_DIR=$STATICFLOW_LOCAL_MEDIA_CACHE_DIR_EFFECTIVE"
log "Using STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG=$STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG_EFFECTIVE"

if [[ "$DAEMON" == "true" ]]; then
  : > "$LOG_FILE"
  rm -f "$PID_FILE"
  HOST="$HOST" \
  PORT="$PORT_CHOSEN" \
  STATICFLOW_LOCAL_MEDIA_ROOT="$STATICFLOW_LOCAL_MEDIA_ROOT" \
  STATICFLOW_LOCAL_MEDIA_CACHE_DIR="$STATICFLOW_LOCAL_MEDIA_CACHE_DIR_EFFECTIVE" \
  STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG="$STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG_EFFECTIVE" \
    setsid "$MEDIA_BIN_PATH" < /dev/null >> "$LOG_FILE" 2>&1 &
  MEDIA_PID=$!
  echo "$MEDIA_PID" > "$PID_FILE"
  if ! wait_media_ready "$HOST" "$PORT_CHOSEN"; then
    fail "Media service failed to become ready: http://${HOST}:${PORT_CHOSEN}/internal/local-media/list?limit=1"
  fi
  log "Media service is ready at http://${HOST}:${PORT_CHOSEN}"
  log "Daemon pid=$MEDIA_PID log=$LOG_FILE pid_file=$PID_FILE"
  exit 0
fi

HOST="$HOST" \
PORT="$PORT_CHOSEN" \
STATICFLOW_LOCAL_MEDIA_ROOT="$STATICFLOW_LOCAL_MEDIA_ROOT" \
STATICFLOW_LOCAL_MEDIA_CACHE_DIR="$STATICFLOW_LOCAL_MEDIA_CACHE_DIR_EFFECTIVE" \
STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG="$STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG_EFFECTIVE" \
  "$MEDIA_BIN_PATH" &
MEDIA_PID=$!

cleanup() {
  if kill -0 "$MEDIA_PID" >/dev/null 2>&1; then
    log "Stopping media service (pid=$MEDIA_PID)..."
    kill "$MEDIA_PID" >/dev/null 2>&1 || true
    wait "$MEDIA_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

if ! wait_media_ready "$HOST" "$PORT_CHOSEN"; then
  fail "Media service failed to become ready: http://${HOST}:${PORT_CHOSEN}/internal/local-media/list?limit=1"
fi

log "Media service is ready."
log "Verification URL: http://${HOST}:${PORT_CHOSEN}/internal/local-media/list?limit=2"
wait "$MEDIA_PID"
