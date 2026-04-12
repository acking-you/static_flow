#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST="${HOST:-127.0.0.1}"
MEDIA_PORT_BASE="${MEDIA_PORT_BASE:-39085}"
MEDIA_PORT_SCAN_LIMIT="${MEDIA_PORT_SCAN_LIMIT:-40}"
MEDIA_PID_FILE="${MEDIA_PID_FILE:-$ROOT_DIR/tmp/staticflow-media-from-tmp.pid}"
MEDIA_LOG_FILE="${MEDIA_LOG_FILE:-$ROOT_DIR/tmp/staticflow-media-from-tmp.log}"

log() {
  echo "[start-backend-with-media] $*"
}

fail() {
  echo "[start-backend-with-media][ERROR] $*" >&2
  exit 1
}

[[ -n "${STATICFLOW_LOCAL_MEDIA_ROOT:-}" ]] || fail "STATICFLOW_LOCAL_MEDIA_ROOT is required"

is_port_busy() {
  local port="$1"
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
}

choose_media_port() {
  if [[ -n "${MEDIA_PORT:-}" ]]; then
    if is_port_busy "$MEDIA_PORT"; then
      fail "MEDIA_PORT=$MEDIA_PORT is already in use"
    fi
    echo "$MEDIA_PORT"
    return
  fi

  local port
  for ((port=MEDIA_PORT_BASE; port<MEDIA_PORT_BASE+MEDIA_PORT_SCAN_LIMIT; port++)); do
    if ! is_port_busy "$port"; then
      echo "$port"
      return
    fi
  done

  fail "No free media port found in [$MEDIA_PORT_BASE, $((MEDIA_PORT_BASE + MEDIA_PORT_SCAN_LIMIT - 1))]"
}

MEDIA_PORT_CHOSEN="$(choose_media_port)"

cleanup() {
  if [[ -f "$MEDIA_PID_FILE" ]]; then
    local media_pid
    media_pid="$(cat "$MEDIA_PID_FILE" 2>/dev/null || true)"
    if [[ -n "$media_pid" ]] && kill -0 "$media_pid" >/dev/null 2>&1; then
      log "Stopping media service daemon (pid=$media_pid)..."
      kill "$media_pid" >/dev/null 2>&1 || true
      wait "$media_pid" 2>/dev/null || true
    fi
    rm -f "$MEDIA_PID_FILE"
  fi
}
trap cleanup EXIT INT TERM

PORT="$MEDIA_PORT_CHOSEN" \
HOST="$HOST" \
LOG_FILE="$MEDIA_LOG_FILE" \
PID_FILE="$MEDIA_PID_FILE" \
  "$ROOT_DIR/scripts/start_media_service_from_tmp.sh" --daemon

export STATICFLOW_MEDIA_PROXY_BASE_URL="http://${HOST}:${MEDIA_PORT_CHOSEN}"
log "Using STATICFLOW_MEDIA_PROXY_BASE_URL=$STATICFLOW_MEDIA_PROXY_BASE_URL"
log "Media daemon log: $MEDIA_LOG_FILE"

"$ROOT_DIR/scripts/start_backend_from_tmp.sh" "$@"
