#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source "$ROOT_DIR/scripts/lib_pingora_gateway_conf.sh"

CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
PINGORA_CONF_TEMPLATE_FILE="${PINGORA_CONF_TEMPLATE_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml.template}"

log() { echo "[backend-slot] $*"; }
fail() { echo "[backend-slot][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/start_backend_selfhosted_slot.sh <blue|green> [launcher options...]

Examples:
  ./scripts/start_backend_selfhosted_slot.sh blue
  ./scripts/start_backend_selfhosted_slot.sh green --daemon

Environment variables:
  CONF_FILE                    Gateway YAML used to resolve slot addresses
  PINGORA_CONF_TEMPLATE_FILE   Gateway YAML template when CONF_FILE is missing
  HOST                         Override bind host instead of the slot address host
  PORT                         Override bind port instead of the slot address port
  STATICFLOW_LOG_SERVICE       Override runtime log service name
EOF
}

slot="${1:-}"
case "$slot" in
  blue|green)
    shift
    ;;
  -h|--help|"")
    usage
    exit 0
    ;;
  *)
    fail "slot must be blue or green"
    ;;
esac

pingora_ensure_conf_file "$CONF_FILE" "$PINGORA_CONF_TEMPLATE_FILE"
slot_addr="$(pingora_staticflow_upstream_addr "$CONF_FILE" "$slot")"
[[ -n "$slot_addr" ]] || fail "missing upstream address for slot=$slot in $CONF_FILE"

default_host="${slot_addr%:*}"
default_port="${slot_addr##*:}"
HOST="${HOST:-${BIND_ADDR:-$default_host}}"
PORT="${PORT:-$default_port}"

case "$slot" in
  blue)
    launcher="$ROOT_DIR/scripts/start_backend_selfhosted.sh"
    export STATICFLOW_LOG_SERVICE="${STATICFLOW_LOG_SERVICE:-backend-blue-${PORT}}"
    ;;
  green)
    launcher="$ROOT_DIR/scripts/start_backend_selfhosted_canary.sh"
    export STATICFLOW_LOG_SERVICE="${STATICFLOW_LOG_SERVICE:-backend-green-${PORT}}"
    ;;
esac

log "slot=$slot launcher=${launcher#$ROOT_DIR/} host=$HOST port=$PORT conf=${CONF_FILE#$ROOT_DIR/}"
exec "$launcher" --host "$HOST" --port "$PORT" "$@"
