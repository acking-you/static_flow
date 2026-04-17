#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:39180}"
ROLLBACK_NEEDED="0"
OLD_SLOT=""
NEW_SLOT=""
OLD_PID=""
NEW_PORT=""
CANDIDATE_BACKEND_BIN=""

log() { echo "[upgrade] $*"; }
fail() { echo "[upgrade][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/backend_gateway_upgrade.sh

Environment variables:
  CONF_FILE    Gateway YAML path
  GATEWAY_URL  Gateway base URL used for post-switch verification
EOF
}

active_slot() {
  rg '^[[:space:]]+active_upstream:' "$CONF_FILE" | awk '{print $2}'
}

slot_addr() {
  local slot="$1"
  rg "^[[:space:]]+${slot}:" "$CONF_FILE" | awk '{print $2}'
}

slot_port() {
  local addr
  addr="$(slot_addr "$1")"
  [[ -n "$addr" ]] || fail "missing address for slot $1"
  echo "${addr##*:}"
}

other_slot() {
  case "$1" in
    blue) echo green ;;
    green) echo blue ;;
    *) fail "unknown slot $1" ;;
  esac
}

wait_health() {
  local url="$1"
  for _ in $(seq 1 80); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

json_field() {
  local field="$1"
  python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field"
}

build_candidate_backend() {
  log "building candidate backend artifact via cargo"
  cargo build --profile release-backend -p static-flow-backend
  CANDIDATE_BACKEND_BIN="$ROOT_DIR/target/release-backend/static-flow-backend"
  [[ -x "$CANDIDATE_BACKEND_BIN" ]] || fail "missing candidate backend binary: $CANDIDATE_BACKEND_BIN"
}

rollback() {
  if [[ "$ROLLBACK_NEEDED" != "1" ]]; then
    return 0
  fi
  log "rolling back gateway to slot=$OLD_SLOT"
  if bash "$ROOT_DIR/scripts/pingora_gateway.sh" switch "$OLD_SLOT"; then
    log "rollback finished; old backend remains on pid=$OLD_PID"
  else
    echo "[upgrade][ERROR] gateway rollback failed; inspect gateway logs immediately" >&2
  fi
}

trap rollback EXIT

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "")
    ;;
  *)
    fail "usage: $0"
    ;;
esac

OLD_SLOT="$(active_slot)"
NEW_SLOT="$(other_slot "$OLD_SLOT")"
old_port="$(slot_port "$OLD_SLOT")"
NEW_PORT="$(slot_port "$NEW_SLOT")"

log "old_slot=$OLD_SLOT new_slot=$NEW_SLOT old_port=$old_port new_port=$NEW_PORT"

wait_health "http://127.0.0.1:${old_port}/api/healthz" || fail "current active backend on port $old_port failed healthz before upgrade"
build_candidate_backend

if [[ "$NEW_SLOT" == "blue" ]]; then
  log "starting candidate backend via primary launcher on port=$NEW_PORT"
  BACKEND_BIN="$CANDIDATE_BACKEND_BIN" \
    bash "$ROOT_DIR/scripts/start_backend_selfhosted.sh" --daemon --port "$NEW_PORT"
else
  log "starting candidate backend via canary launcher on port=$NEW_PORT"
  BACKEND_BIN="$CANDIDATE_BACKEND_BIN" \
    bash "$ROOT_DIR/scripts/start_backend_selfhosted_canary.sh" --daemon --port "$NEW_PORT"
fi

wait_health "http://127.0.0.1:${NEW_PORT}/api/healthz" || fail "candidate backend failed healthz on port $NEW_PORT"
OLD_PID="$(curl -fsS "http://127.0.0.1:${old_port}/api/healthz" | json_field pid)"
log "candidate backend healthy; old_pid=$OLD_PID"

log "switching gateway active_upstream: $OLD_SLOT -> $NEW_SLOT"
bash "$ROOT_DIR/scripts/pingora_gateway.sh" switch "$NEW_SLOT"
ROLLBACK_NEEDED="1"
wait_health "${GATEWAY_URL}/api/healthz" || fail "gateway did not recover after switch"

gateway_port="$(curl -fsS "${GATEWAY_URL}/api/healthz" | json_field port)"
[[ "$gateway_port" == "$NEW_PORT" ]] || fail "gateway still points to old backend"

log "gateway now serves new port=$gateway_port; stopping old pid=$OLD_PID"
kill -TERM "$OLD_PID" || log "warning: failed to stop old pid=$OLD_PID"
ROLLBACK_NEEDED="0"
log "upgrade completed: active_upstream=$NEW_SLOT new_port=$NEW_PORT"
