#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source "$ROOT_DIR/scripts/lib_pingora_gateway_conf.sh"
source "$ROOT_DIR/scripts/lib_script_lock.sh"

CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
PINGORA_CONF_TEMPLATE_FILE="${PINGORA_CONF_TEMPLATE_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml.template}"
GATEWAY_BIN="${GATEWAY_BIN:-$ROOT_DIR/target/release-backend/staticflow-pingora-gateway}"
STATICFLOW_LOG_DIR="${STATICFLOW_LOG_DIR:-$ROOT_DIR/tmp/runtime-logs}"
STATICFLOW_LOG_SERVICE="${STATICFLOW_LOG_SERVICE:-gateway}"
STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR="${STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR:-1}"
LOCK_FILE="${LOCK_FILE:-$ROOT_DIR/tmp/staticflow-gateway.lock}"
SYSTEMD_SCOPE="${SYSTEMD_SCOPE:-system}"
SYSTEMCTL_BIN="${SYSTEMCTL_BIN:-systemctl}"
JOURNALCTL_BIN="${JOURNALCTL_BIN:-journalctl}"
STATICFLOW_GATEWAY_UNIT="${STATICFLOW_GATEWAY_UNIT:-staticflow-gateway.service}"
STATICFLOW_BACKEND_SLOT_UNIT_TEMPLATE="${STATICFLOW_BACKEND_SLOT_UNIT_TEMPLATE:-staticflow-backend-slot@%s.service}"
DEFAULT_LOG_LINES="${DEFAULT_LOG_LINES:-200}"

log() { echo "[gateway] $*"; }
fail() { echo "[gateway][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage:
  ./scripts/pingora_gateway.sh run
  ./scripts/pingora_gateway.sh check
  ./scripts/pingora_gateway.sh start
  ./scripts/pingora_gateway.sh stop
  ./scripts/pingora_gateway.sh restart
  ./scripts/pingora_gateway.sh reload
  ./scripts/pingora_gateway.sh status [gateway|blue|green|all]
  ./scripts/pingora_gateway.sh logs [gateway|blue|green] [--lines N] [--follow]
  ./scripts/pingora_gateway.sh health
  ./scripts/pingora_gateway.sh switch <blue|green>
  ./scripts/pingora_gateway.sh start-backend <blue|green>
  ./scripts/pingora_gateway.sh stop-backend <blue|green>
  ./scripts/pingora_gateway.sh restart-backend <blue|green>

Environment variables:
  CONF_FILE                           Gateway YAML path
  PINGORA_CONF_TEMPLATE_FILE          Gateway YAML template path when CONF_FILE is missing
  GATEWAY_BIN                         Gateway binary path used by `run` and `check`
  STATICFLOW_LOG_DIR                  Runtime log root exported to the gateway binary
  STATICFLOW_LOG_SERVICE              Runtime log service name exported to the gateway binary
  LOCK_FILE                           Lock file path used to serialize mutating operations
  SYSTEMD_SCOPE                       `system` or `user` (default: system)
  SYSTEMCTL_BIN                       systemctl binary path
  JOURNALCTL_BIN                      journalctl binary path
  STATICFLOW_GATEWAY_UNIT             Gateway systemd unit name
  STATICFLOW_BACKEND_SLOT_UNIT_TEMPLATE printf-style backend slot unit template
  DEFAULT_LOG_LINES                   Default `logs` line count (default: 200)
EOF
}

ensure_gateway_conf() {
  pingora_ensure_conf_file "$CONF_FILE" "$PINGORA_CONF_TEMPLATE_FILE"
}

ensure_layout() {
  mkdir -p "$ROOT_DIR/tmp" "$STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE"
}

require_gateway_bin() {
  [[ -x "$GATEWAY_BIN" ]] || fail "gateway binary not found or not executable: $GATEWAY_BIN"
}

check_gateway_conf() {
  local conf_file="${1:-$CONF_FILE}"
  require_gateway_bin
  STATICFLOW_LOG_DIR="$STATICFLOW_LOG_DIR" \
  STATICFLOW_LOG_SERVICE="$STATICFLOW_LOG_SERVICE" \
  STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR="$STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR" \
    "$GATEWAY_BIN" --conf "$conf_file" --test
}

listen_addr() {
  pingora_staticflow_conf_value "$CONF_FILE" "listen_addr"
}

listen_addr_from_file() {
  pingora_staticflow_conf_value "$1" "listen_addr"
}

active_upstream() {
  pingora_staticflow_conf_value "$CONF_FILE" "active_upstream"
}

active_upstream_from_file() {
  pingora_staticflow_conf_value "$1" "active_upstream"
}

downstream_h2c() {
  local value
  value="$(pingora_staticflow_conf_value "$CONF_FILE" "downstream_h2c")"
  echo "${value:-true}"
}

slot_addr_from_file() {
  local conf_file="$1"
  local slot="$2"
  pingora_staticflow_upstream_addr "$conf_file" "$slot"
}

slot_port_from_file() {
  local conf_file="$1"
  local slot="$2"
  local addr
  addr="$(slot_addr_from_file "$conf_file" "$slot")"
  [[ -n "$addr" ]] || fail "missing address for slot $slot in $conf_file"
  echo "${addr##*:}"
}

slot_addr() {
  slot_addr_from_file "$CONF_FILE" "$1"
}

slot_port() {
  slot_port_from_file "$CONF_FILE" "$1"
}

gateway_base_url() {
  echo "http://$(listen_addr)"
}

gateway_base_url_from_file() {
  echo "http://$(listen_addr_from_file "$1")"
}

json_field() {
  local field="$1"
  python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field"
}

healthz_body() {
  local base_url="$1"
  curl -fsS "${base_url}/api/healthz"
}

healthz_field_from_body() {
  local body="$1"
  local field="$2"
  printf '%s' "$body" | json_field "$field"
}

wait_gateway_port() {
  local gateway_base="$1"
  local target_port="$2"
  local body="" response_port=""
  for _ in $(seq 1 120); do
    if body="$(healthz_body "$gateway_base" 2>/dev/null)"; then
      response_port="$(healthz_field_from_body "$body" port 2>/dev/null || true)"
      if [[ "$response_port" == "$target_port" ]]; then
        return 0
      fi
    fi
    sleep 0.25
  done
  return 1
}

wait_slot_health() {
  local slot="$1"
  local base_url="http://$(slot_addr "$slot")"
  local expected_port
  expected_port="$(slot_port "$slot")"
  wait_gateway_port "$base_url" "$expected_port"
}

systemctl_cmd() {
  local -a args=()
  case "$SYSTEMD_SCOPE" in
    system)
      ;;
    user)
      args+=(--user)
      ;;
    *)
      fail "SYSTEMD_SCOPE must be system or user"
      ;;
  esac
  "$SYSTEMCTL_BIN" "${args[@]}" "$@"
}

journalctl_cmd() {
  local -a args=(--no-pager)
  case "$SYSTEMD_SCOPE" in
    system)
      ;;
    user)
      args+=(--user)
      ;;
    *)
      fail "SYSTEMD_SCOPE must be system or user"
      ;;
  esac
  "$JOURNALCTL_BIN" "${args[@]}" "$@"
}

validate_systemd_access() {
  command -v "$SYSTEMCTL_BIN" >/dev/null 2>&1 || fail "systemctl not found: $SYSTEMCTL_BIN"
  command -v "$JOURNALCTL_BIN" >/dev/null 2>&1 || fail "journalctl not found: $JOURNALCTL_BIN"
  if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
    systemctl_cmd show-environment >/dev/null 2>&1 \
      || fail "systemd --user is not available for the current session"
  fi
}

gateway_unit() {
  echo "$STATICFLOW_GATEWAY_UNIT"
}

backend_slot_unit() {
  local slot="$1"
  case "$slot" in
    blue|green)
      printf "$STATICFLOW_BACKEND_SLOT_UNIT_TEMPLATE" "$slot"
      ;;
    *)
      fail "slot must be blue or green"
      ;;
  esac
}

unit_for_target() {
  local target="$1"
  case "$target" in
    gateway)
      gateway_unit
      ;;
    blue|green)
      backend_slot_unit "$target"
      ;;
    *)
      fail "target must be gateway, blue, or green"
      ;;
  esac
}

unit_load_state() {
  local unit="$1"
  systemctl_cmd show --property=LoadState --value "$unit" 2>/dev/null || true
}

unit_active_state() {
  local unit="$1"
  systemctl_cmd show --property=ActiveState --value "$unit" 2>/dev/null || true
}

unit_sub_state() {
  local unit="$1"
  systemctl_cmd show --property=SubState --value "$unit" 2>/dev/null || true
}

require_unit_registered() {
  local unit="$1"
  local label="$2"
  local load_state
  load_state="$(unit_load_state "$unit")"
  [[ -n "$load_state" && "$load_state" != "not-found" ]] \
    || fail "$label unit is not registered for SYSTEMD_SCOPE=$SYSTEMD_SCOPE: $unit"
}

wait_unit_active() {
  local unit="$1"
  for _ in $(seq 1 80); do
    if systemctl_cmd is-active --quiet "$unit"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_unit_inactive() {
  local unit="$1"
  for _ in $(seq 1 80); do
    if ! systemctl_cmd is-active --quiet "$unit"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

acquire_lock() {
  mkdir -p "$(dirname "$LOCK_FILE")"
  exec 9>"$LOCK_FILE"
  flock -n 9 || fail "another gateway operation is already running"
}

release_lock() {
  release_lock_fd 9
}

with_lock() {
  acquire_lock
  "$@"
  release_lock
}

print_service_state() {
  local target="$1"
  local unit
  unit="$(unit_for_target "$target")"
  echo "$target: unit=$unit load=$(unit_load_state "$unit") active=$(unit_active_state "$unit") sub=$(unit_sub_state "$unit")"
}

print_gateway_health() {
  local gateway_base="" expected_port="" body="" response_port="" response_pid=""
  gateway_base="$(gateway_base_url)"
  expected_port="$(slot_port "$(active_upstream)")"
  if body="$(healthz_body "$gateway_base" 2>/dev/null)"; then
    response_port="$(healthz_field_from_body "$body" port 2>/dev/null || true)"
    response_pid="$(healthz_field_from_body "$body" pid 2>/dev/null || true)"
    if [[ "$response_port" == "$expected_port" ]]; then
      echo "gateway_health=ok url=$gateway_base expected_port=$expected_port response_port=$response_port response_pid=${response_pid:-}"
      return 0
    fi
    echo "gateway_health=mismatch url=$gateway_base expected_port=$expected_port response_port=${response_port:-<none>} response_pid=${response_pid:-}"
    return 1
  fi
  echo "gateway_health=unreachable url=$gateway_base expected_port=$expected_port"
  return 1
}

print_slot_health() {
  local slot="$1"
  local base_url="" expected_port="" body="" response_port="" response_pid=""
  base_url="http://$(slot_addr "$slot")"
  expected_port="$(slot_port "$slot")"
  if body="$(healthz_body "$base_url" 2>/dev/null)"; then
    response_port="$(healthz_field_from_body "$body" port 2>/dev/null || true)"
    response_pid="$(healthz_field_from_body "$body" pid 2>/dev/null || true)"
    if [[ "$response_port" == "$expected_port" ]]; then
      echo "${slot}_health=ok url=$base_url expected_port=$expected_port response_port=$response_port response_pid=${response_pid:-}"
      return 0
    fi
    echo "${slot}_health=mismatch url=$base_url expected_port=$expected_port response_port=${response_port:-<none>} response_pid=${response_pid:-}"
    return 1
  fi
  echo "${slot}_health=unreachable url=$base_url expected_port=$expected_port"
  return 1
}

report_health() {
  local exit_code=0
  print_service_state gateway
  print_service_state blue
  print_service_state green
  print_gateway_health || exit_code=1
  print_slot_health blue || exit_code=1
  print_slot_health green || exit_code=1
  return "$exit_code"
}

status_summary() {
  echo "conf=$CONF_FILE"
  echo "systemd_scope=$SYSTEMD_SCOPE"
  echo "gateway_unit=$(gateway_unit)"
  echo "blue_unit=$(backend_slot_unit blue)"
  echo "green_unit=$(backend_slot_unit green)"
  echo "listen_addr=$(listen_addr)"
  echo "active_upstream=$(active_upstream)"
  echo "downstream_h2c=$(downstream_h2c)"
  report_health || true
}

show_unit_status() {
  local target="$1"
  local unit
  unit="$(unit_for_target "$target")"
  require_unit_registered "$unit" "$target"
  systemctl_cmd --no-pager --full status "$unit"
}

tail_unit_logs() {
  local target="$1"
  local lines="$2"
  local follow="$3"
  local unit
  unit="$(unit_for_target "$target")"
  require_unit_registered "$unit" "$target"
  if [[ "$follow" == "true" ]]; then
    journalctl_cmd -u "$unit" -n "$lines" -f
    return
  fi
  journalctl_cmd -u "$unit" -n "$lines"
}

switch_upstream() {
  local source_file="$1"
  local target_file="$2"
  local next="$3"
  python3 - "$source_file" "$target_file" "$next" <<'PY'
import pathlib
import re
import sys

source_path = pathlib.Path(sys.argv[1])
target_path = pathlib.Path(sys.argv[2])
next_value = sys.argv[3]
text = source_path.read_text()
text, count = re.subn(r'(^\s*active_upstream:\s*).+$', rf'\1{next_value}', text, flags=re.M)
if count != 1:
    raise SystemExit("expected exactly one active_upstream line")
target_path.write_text(text)
PY
}

require_gateway_active() {
  local unit
  unit="$(gateway_unit)"
  require_unit_registered "$unit" "gateway"
  systemctl_cmd is-active --quiet "$unit" \
    || fail "gateway unit is not active: $unit"
}

start_gateway_service() {
  local unit gateway_base target_port
  unit="$(gateway_unit)"
  validate_systemd_access
  ensure_gateway_conf
  check_gateway_conf >/dev/null
  require_unit_registered "$unit" "gateway"
  gateway_base="$(gateway_base_url)"
  target_port="$(slot_port "$(active_upstream)")"
  log "starting gateway unit=$unit scope=$SYSTEMD_SCOPE"
  systemctl_cmd start "$unit"
  wait_unit_active "$unit" || fail "gateway unit did not become active: $unit"
  wait_gateway_port "$gateway_base" "$target_port" \
    || fail "gateway failed health verification after start: unit=$unit target_port=$target_port"
}

stop_gateway_service() {
  local unit
  unit="$(gateway_unit)"
  validate_systemd_access
  require_unit_registered "$unit" "gateway"
  log "stopping gateway unit=$unit scope=$SYSTEMD_SCOPE"
  systemctl_cmd stop "$unit"
  wait_unit_inactive "$unit" || fail "gateway unit did not stop: $unit"
}

restart_gateway_service() {
  local unit gateway_base target_port
  unit="$(gateway_unit)"
  validate_systemd_access
  ensure_gateway_conf
  check_gateway_conf >/dev/null
  require_unit_registered "$unit" "gateway"
  gateway_base="$(gateway_base_url)"
  target_port="$(slot_port "$(active_upstream)")"
  log "restarting gateway unit=$unit scope=$SYSTEMD_SCOPE"
  systemctl_cmd restart "$unit"
  wait_unit_active "$unit" || fail "gateway unit did not become active after restart: $unit"
  wait_gateway_port "$gateway_base" "$target_port" \
    || fail "gateway failed health verification after restart: unit=$unit target_port=$target_port"
}

reload_gateway_service() {
  local unit gateway_base target_port
  unit="$(gateway_unit)"
  validate_systemd_access
  ensure_gateway_conf
  check_gateway_conf >/dev/null
  require_gateway_active
  gateway_base="$(gateway_base_url)"
  target_port="$(slot_port "$(active_upstream)")"
  log "reloading gateway unit=$unit scope=$SYSTEMD_SCOPE active_upstream=$(active_upstream)"
  systemctl_cmd reload "$unit"
  wait_gateway_port "$gateway_base" "$target_port" \
    || fail "gateway failed health verification after reload: unit=$unit target_port=$target_port"
}

control_backend_slot() {
  local action="$1"
  local slot="$2"
  local unit
  unit="$(backend_slot_unit "$slot")"
  validate_systemd_access
  ensure_gateway_conf
  require_unit_registered "$unit" "$slot"
  case "$action" in
    start)
      log "starting backend slot=$slot unit=$unit scope=$SYSTEMD_SCOPE"
      systemctl_cmd start "$unit"
      wait_unit_active "$unit" || fail "backend unit did not become active: $unit"
      wait_slot_health "$slot" || fail "backend slot failed health verification after start: $slot"
      ;;
    restart)
      log "restarting backend slot=$slot unit=$unit scope=$SYSTEMD_SCOPE"
      systemctl_cmd restart "$unit"
      wait_unit_active "$unit" || fail "backend unit did not become active after restart: $unit"
      wait_slot_health "$slot" || fail "backend slot failed health verification after restart: $slot"
      ;;
    stop)
      if [[ "$(active_upstream)" == "$slot" ]]; then
        fail "refusing to stop active backend slot $slot"
      fi
      log "stopping backend slot=$slot unit=$unit scope=$SYSTEMD_SCOPE"
      systemctl_cmd stop "$unit"
      wait_unit_inactive "$unit" || fail "backend unit did not stop: $unit"
      ;;
    *)
      fail "unknown backend slot action: $action"
      ;;
  esac
}

switch_active_upstream() {
  local next_slot="$1"
  local old_slot="" tmp_conf="" backup_conf="" target_port="" old_port="" gateway_base=""
  validate_systemd_access
  ensure_gateway_conf
  require_gateway_active
  old_slot="$(active_upstream)"
  [[ "$old_slot" != "$next_slot" ]] || fail "active_upstream is already $next_slot"
  tmp_conf="$(mktemp "$ROOT_DIR/tmp/staticflow-gateway.XXXXXX.yaml")"
  backup_conf="${tmp_conf}.bak"
  cp "$CONF_FILE" "$backup_conf"
  switch_upstream "$CONF_FILE" "$tmp_conf" "$next_slot"
  check_gateway_conf "$tmp_conf" >/dev/null
  target_port="$(slot_port_from_file "$tmp_conf" "$next_slot")"
  old_port="$(slot_port "$old_slot")"
  gateway_base="$(gateway_base_url_from_file "$tmp_conf")"
  mv "$tmp_conf" "$CONF_FILE"
  if ! systemctl_cmd reload "$(gateway_unit)" || ! wait_gateway_port "$gateway_base" "$target_port"; then
    mv "$backup_conf" "$CONF_FILE"
    systemctl_cmd reload "$(gateway_unit)" || true
    wait_gateway_port "$gateway_base" "$old_port" || true
    fail "gateway switch verification failed after switching active_upstream to $next_slot; reverted to $old_slot"
  fi
  rm -f "$backup_conf"
  log "active_upstream switched: $old_slot -> $next_slot"
}

run_gateway_foreground() {
  ensure_gateway_conf
  ensure_layout
  require_gateway_bin
  export STATICFLOW_LOG_DIR STATICFLOW_LOG_SERVICE STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR
  exec "$GATEWAY_BIN" --conf "$CONF_FILE"
}

status_target="${2:-summary}"

case "${1:-}" in
  run)
    run_gateway_foreground
    ;;
  check)
    ensure_gateway_conf
    check_gateway_conf
    ;;
  start)
    with_lock start_gateway_service
    ;;
  stop)
    with_lock stop_gateway_service
    ;;
  restart)
    with_lock restart_gateway_service
    ;;
  reload)
    with_lock reload_gateway_service
    ;;
  status)
    ensure_gateway_conf
    validate_systemd_access
    case "$status_target" in
      ""|summary)
        status_summary
        ;;
      gateway|blue|green)
        show_unit_status "$status_target"
        ;;
      all)
        status_summary
        printf '\n'
        show_unit_status gateway
        printf '\n'
        show_unit_status blue
        printf '\n'
        show_unit_status green
        ;;
      *)
        fail "usage: $0 status [gateway|blue|green|all]"
        ;;
    esac
    ;;
  logs)
    ensure_gateway_conf
    validate_systemd_access
    target="gateway"
    lines="$DEFAULT_LOG_LINES"
    follow="false"
    shift
    if [[ $# -gt 0 && "$1" != --* ]]; then
      target="$1"
      shift
    fi
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --follow|-f)
          follow="true"
          shift
          ;;
        --lines|-n)
          [[ $# -ge 2 ]] || fail "--lines requires a value"
          lines="$2"
          shift 2
          ;;
        *)
          fail "usage: $0 logs [gateway|blue|green] [--lines N] [--follow]"
          ;;
      esac
    done
    tail_unit_logs "$target" "$lines" "$follow"
    ;;
  health)
    ensure_gateway_conf
    validate_systemd_access
    report_health
    ;;
  switch)
    [[ $# -eq 2 ]] || fail "usage: $0 switch <blue|green>"
    [[ "$2" == "blue" || "$2" == "green" ]] || fail "slot must be blue or green"
    with_lock switch_active_upstream "$2"
    ;;
  start-backend)
    [[ $# -eq 2 ]] || fail "usage: $0 start-backend <blue|green>"
    with_lock control_backend_slot start "$2"
    ;;
  stop-backend)
    [[ $# -eq 2 ]] || fail "usage: $0 stop-backend <blue|green>"
    with_lock control_backend_slot stop "$2"
    ;;
  restart-backend)
    [[ $# -eq 2 ]] || fail "usage: $0 restart-backend <blue|green>"
    with_lock control_backend_slot restart "$2"
    ;;
  -h|--help|"")
    usage
    ;;
  *)
    fail "usage: $0 {run|check|start|stop|restart|reload|status [gateway|blue|green|all]|logs [gateway|blue|green] [--lines N] [--follow]|health|switch <blue|green>|start-backend <blue|green>|stop-backend <blue|green>|restart-backend <blue|green>}"
    ;;
esac
