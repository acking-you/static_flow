#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source "$ROOT_DIR/scripts/lib_pingora_gateway_conf.sh"
source "$ROOT_DIR/scripts/lib_port_process.sh"
source "$ROOT_DIR/scripts/lib_script_lock.sh"

CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
GATEWAY_BIN="${GATEWAY_BIN:-$ROOT_DIR/target/release-backend/staticflow-pingora-gateway}"
STATICFLOW_LOG_DIR="${STATICFLOW_LOG_DIR:-$ROOT_DIR/tmp/runtime-logs}"
STATICFLOW_LOG_SERVICE="${STATICFLOW_LOG_SERVICE:-gateway}"
STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR="${STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR:-1}"
FORCE_BUILD_GATEWAY="${FORCE_BUILD_GATEWAY:-0}"
LOCK_FILE="${LOCK_FILE:-$ROOT_DIR/tmp/staticflow-gateway.lock}"

log() { echo "[gateway] $*"; }
fail() { echo "[gateway][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/pingora_gateway.sh {run|start|restart|check|reload|status|stop|switch <blue|green>|stop-backend <blue|green>|logs <gateway|blue|green>|health}

Environment variables:
  CONF_FILE               Gateway YAML path
  GATEWAY_BIN             Gateway binary path
  STATICFLOW_LOG_DIR      Runtime log root
  STATICFLOW_LOG_SERVICE  Gateway runtime log folder name
  STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR  Force shell-managed gateway lifecycle (default: 1)
  FORCE_BUILD_GATEWAY     Rebuild gateway binary even when it already exists (default: 0)
  LOCK_FILE               Lock file path used to serialize gateway operations
EOF
}

ensure_layout() {
  mkdir -p "$ROOT_DIR/tmp" "$STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE"
  mkdir -p "$(dirname "$(pid_file)")"
  mkdir -p "$(dirname "$(error_log_file)")"
}

build_gateway_bin() {
  ensure_layout
  if [[ "$FORCE_BUILD_GATEWAY" != "1" && -x "$GATEWAY_BIN" ]]; then
    log "reusing gateway binary: $GATEWAY_BIN"
    return
  fi
  log "building gateway binary: $GATEWAY_BIN"
  cargo build -p staticflow-pingora-gateway --profile release-backend >/dev/null
}

force_build_gateway_bin() {
  local previous_force_build="${FORCE_BUILD_GATEWAY:-0}"
  FORCE_BUILD_GATEWAY=1
  build_gateway_bin
  FORCE_BUILD_GATEWAY="$previous_force_build"
}

check_gateway_conf() {
  local conf_file="${1:-$CONF_FILE}"
  build_gateway_bin
  log "checking gateway config: $conf_file"
  STATICFLOW_LOG_DIR="$STATICFLOW_LOG_DIR" \
  STATICFLOW_LOG_SERVICE="$STATICFLOW_LOG_SERVICE" \
  STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR="$STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR" \
    "$GATEWAY_BIN" --conf "$conf_file" --test
}

top_level_conf_value() {
  local conf_file="$1"
  local key="$2"
  pingora_top_level_conf_value "$conf_file" "$key"
}

staticflow_conf_value() {
  local conf_file="$1"
  local key="$2"
  pingora_staticflow_conf_value "$conf_file" "$key"
}

listen_addr() {
  staticflow_conf_value "$CONF_FILE" "listen_addr"
}

listen_addr_from_file() {
  staticflow_conf_value "$1" "listen_addr"
}

pid_file() {
  top_level_conf_value "$CONF_FILE" "pid_file"
}

error_log_file() {
  top_level_conf_value "$CONF_FILE" "error_log"
}

current_pid() {
  local file
  file="$(pid_file)"
  [[ -f "$file" ]] && cat "$file"
}

active_upstream() {
  pingora_staticflow_conf_value "$CONF_FILE" "active_upstream"
}

active_upstream_from_file() {
  local conf_file="$1"
  pingora_staticflow_conf_value "$conf_file" "active_upstream"
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

json_field() {
  local field="$1"
  python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field"
}

wait_for_process() {
  local pid="$1"
  for _ in $(seq 1 40); do
    if kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_for_exit() {
  local pid="$1"
  for _ in $(seq 1 40); do
    if ! kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_gateway_port() {
  local gateway_base="$1"
  local target_port="$2"
  local body="" port=""
  for _ in $(seq 1 80); do
    if body="$(curl -fsS "${gateway_base}/api/healthz" 2>/dev/null)"; then
      port="$(printf '%s' "$body" | json_field port 2>/dev/null || true)"
      if [[ "$port" == "$target_port" ]]; then
        return 0
      fi
    fi
    sleep 0.25
  done
  return 1
}

slot_log_service() {
  local slot="$1"
  case "$slot" in
    blue)
      echo "backend"
      ;;
    green)
      echo "backend-canary-$(slot_port_from_file "$CONF_FILE" "$slot")"
      ;;
    *)
      fail "slot must be blue or green"
      ;;
  esac
}

resolve_log_files() {
  local target="$1"
  local -n out_files_ref="$2"
  local log_dir="" daemon_log=""
  local -a app_logs=() access_logs=()
  shopt -s nullglob
  case "$target" in
    gateway)
      log_dir="$STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE"
      app_logs=("$log_dir"/app/current*.log)
      access_logs=("$log_dir"/access/current*.log)
      daemon_log="$log_dir/daemon-stderr.log"
      [[ ${#app_logs[@]} -gt 0 ]] || fail "missing gateway app logs under $log_dir/app/current*.log"
      [[ ${#access_logs[@]} -gt 0 ]] || fail "missing gateway access logs under $log_dir/access/current*.log"
      [[ -f "$daemon_log" ]] || fail "missing gateway daemon log: $daemon_log"
      out_files_ref=("${app_logs[@]}" "${access_logs[@]}" "$daemon_log")
      ;;
    blue|green)
      log_dir="$STATICFLOW_LOG_DIR/$(slot_log_service "$target")"
      app_logs=("$log_dir"/app/current*.log)
      access_logs=("$log_dir"/access/current*.log)
      [[ ${#app_logs[@]} -gt 0 ]] || fail "missing $target app logs under $log_dir/app/current*.log"
      [[ ${#access_logs[@]} -gt 0 ]] || fail "missing $target access logs under $log_dir/access/current*.log"
      out_files_ref=("${app_logs[@]}" "${access_logs[@]}")
      ;;
    *)
      fail "logs target must be gateway, blue, or green"
      ;;
  esac
  shopt -u nullglob
}

tail_logs() {
  local target="$1"
  local -a log_files=()
  resolve_log_files "$target" log_files
  exec tail -n 200 -F "${log_files[@]}"
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

report_gateway_health() {
  local active_slot="" expected_port="" gateway_base="" listen_port="" pid_file_value="" listener_pid=""
  local body="" response_port="" response_pid=""
  active_slot="$(active_upstream)"
  expected_port="$(slot_port_from_file "$CONF_FILE" "$active_slot")"
  gateway_base="http://$(listen_addr)"
  listen_port="${gateway_base##*:}"
  pid_file_value="$(current_pid || true)"
  listener_pid="$(listener_pid_for_port "$listen_port" || true)"
  if [[ -n "$listener_pid" ]] && body="$(healthz_body "$gateway_base" 2>/dev/null)"; then
    response_port="$(healthz_field_from_body "$body" port 2>/dev/null || true)"
    response_pid="$(healthz_field_from_body "$body" pid 2>/dev/null || true)"
    if [[ "$response_port" == "$expected_port" ]]; then
      echo "gateway: ok listen_port=$listen_port listener_pid=${listener_pid:-} pid_file=${pid_file_value:-} active_upstream=$active_slot response_pid=${response_pid:-} response_port=${response_port:-}"
      return 0
    fi
  fi
  echo "gateway: unhealthy listen_port=$listen_port listener_pid=${listener_pid:-<none>} pid_file=${pid_file_value:-<none>} active_upstream=$active_slot expected_port=$expected_port"
  return 1
}

report_slot_health() {
  local slot="$1"
  local addr="" port="" base_url="" listener_pid="" body="" response_port="" response_pid=""
  addr="$(slot_addr_from_file "$CONF_FILE" "$slot")"
  port="${addr##*:}"
  base_url="http://$addr"
  listener_pid="$(listener_pid_for_port "$port" || true)"
  if [[ -n "$listener_pid" ]] && body="$(healthz_body "$base_url" 2>/dev/null)"; then
    response_port="$(healthz_field_from_body "$body" port 2>/dev/null || true)"
    response_pid="$(healthz_field_from_body "$body" pid 2>/dev/null || true)"
    if [[ "$response_port" == "$port" ]]; then
      echo "$slot: ok addr=$addr listener_pid=${listener_pid:-} response_pid=${response_pid:-} response_port=${response_port:-}"
      return 0
    fi
  fi
  echo "$slot: unhealthy addr=$addr listener_pid=${listener_pid:-<none>}"
  return 1
}

report_health() {
  local exit_code=0
  report_gateway_health || exit_code=1
  report_slot_health blue || exit_code=1
  report_slot_health green || exit_code=1
  return "$exit_code"
}

stop_backend_slot() {
  local slot="$1"
  local active_slot="" port="" pid=""
  active_slot="$(active_upstream)"
  [[ "$slot" != "$active_slot" ]] || fail "refusing to stop active slot $slot"
  port="$(slot_port_from_file "$CONF_FILE" "$slot")"
  pid="$(listener_pid_for_port "$port")"
  [[ -n "$pid" ]] || fail "no backend listener found for slot $slot on port $port"
  log "stopping backend slot=$slot pid=$pid port=$port"
  kill -TERM "$pid"
  if ! wait_for_exit "$pid"; then
    log "backend slot=$slot pid=$pid did not exit after SIGTERM; forcing SIGKILL"
    kill -KILL "$pid" 2>/dev/null || true
    wait_for_exit "$pid" || fail "backend slot=$slot pid $pid did not exit after SIGKILL"
  fi
  log "backend slot=$slot stopped"
}

clear_stale_pid() {
  local pid
  pid="$(current_pid || true)"
  if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$(pid_file)"
  fi
}

start_gateway() {
  local pid=""
  build_gateway_bin
  clear_stale_pid
  if pid="$(current_pid || true)" && [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    fail "gateway is already running (pid=$pid)"
  fi
  check_gateway_conf >/dev/null
  export STATICFLOW_LOG_DIR STATICFLOW_LOG_SERVICE STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR
  log "starting gateway on $(active_upstream) via $CONF_FILE"
  release_lock_fd 9
  nohup "$GATEWAY_BIN" --conf "$CONF_FILE" >>"$(error_log_file)" 2>&1 &
  pid="$!"
  echo "$pid" >"$(pid_file)"
  sleep 0.5
  wait_for_process "$pid" || {
    rm -f "$(pid_file)"
    fail "gateway failed to stay alive after start; inspect $(error_log_file)"
  }
  log "gateway started pid=$pid"
}

stop_gateway() {
  local pid=""
  require_running
  pid="$(current_pid || true)"
  log "stopping gateway pid=$pid"
  kill -TERM "$pid"
  if ! wait_for_exit "$pid"; then
    log "gateway pid=$pid did not exit after SIGTERM; forcing SIGKILL"
    kill -KILL "$pid" 2>/dev/null || true
    wait_for_exit "$pid" || fail "gateway pid $pid did not exit after SIGKILL"
  fi
  rm -f "$(pid_file)"
  log "gateway stopped"
}

restart_gateway() {
  local gateway_base="" target_port=""
  require_running
  force_build_gateway_bin
  check_gateway_conf >/dev/null
  gateway_base="http://$(listen_addr)"
  target_port="$(slot_port_from_file "$CONF_FILE" "$(active_upstream)")"
  stop_gateway
  start_gateway
  wait_gateway_port "$gateway_base" "$target_port" \
    || fail "gateway failed health verification after restart; inspect $(error_log_file)"
  log "gateway restarted with rebuilt binary"
}

reload_gateway() {
  local pid=""
  pid="$(current_pid || true)"
  if [[ -z "$pid" ]]; then
    echo "[gateway][ERROR] gateway is not running" >&2
    return 1
  fi
  log "sending SIGHUP to gateway pid=$pid active_upstream=$(active_upstream)"
  kill -HUP "$pid"
  wait_for_process "$pid" || {
    echo "[gateway][ERROR] gateway pid $pid exited after SIGHUP" >&2
    return 1
  }
  log "gateway config reload signal delivered to pid=$pid"
}

switch_upstream() {
  local source_file="$1"
  local target_file="$2"
  local next="$3"
  python3 - "$source_file" "$target_file" "$next" <<'PY'
import pathlib, re, sys
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

require_running() {
  clear_stale_pid
  local pid
  pid="$(current_pid || true)"
  [[ -n "$pid" ]] || fail "gateway is not running"
  kill -0 "$pid" 2>/dev/null || fail "gateway pid $pid is stale"
}

acquire_lock() {
  mkdir -p "$(dirname "$LOCK_FILE")"
  exec 9>"$LOCK_FILE"
  flock -n 9 || fail "another gateway operation is already running"
}

acquire_lock

case "${1:-}" in
  run)
    build_gateway_bin
    export STATICFLOW_LOG_DIR STATICFLOW_LOG_SERVICE
    export STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR
    release_lock_fd 9
    exec "$GATEWAY_BIN" --conf "$CONF_FILE"
    ;;
  start)
    start_gateway
    ;;
  restart)
    restart_gateway
    ;;
  check)
    check_gateway_conf
    ;;
  reload)
    require_running
    check_gateway_conf >/dev/null
    reload_gateway
    ;;
  status)
    clear_stale_pid
    pid="$(current_pid || true)"
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      pid="${pid} (stale)"
    fi
    echo "conf=$CONF_FILE"
    echo "pid=${pid:-}"
    echo "pid_file=$(pid_file)"
    echo "listen_addr=$(listen_addr)"
    echo "active_upstream=$(active_upstream)"
    ;;
  stop)
    stop_gateway
    ;;
  stop-backend)
    [[ $# -eq 2 ]] || fail "usage: $0 stop-backend <blue|green>"
    [[ "$2" == "blue" || "$2" == "green" ]] || fail "slot must be blue or green"
    stop_backend_slot "$2"
    ;;
  logs)
    [[ $# -eq 2 ]] || fail "usage: $0 logs <gateway|blue|green>"
    release_lock_fd 9
    tail_logs "$2"
    ;;
  health)
    [[ $# -eq 1 ]] || fail "usage: $0 health"
    report_health
    ;;
  switch)
    [[ $# -eq 2 ]] || fail "usage: $0 switch <blue|green>"
    [[ "$2" == "blue" || "$2" == "green" ]] || fail "slot must be blue or green"
    require_running
    old_slot="$(active_upstream)"
    [[ "$old_slot" != "$2" ]] || fail "active_upstream is already $2"
    tmp_conf="$(mktemp "$ROOT_DIR/tmp/staticflow-gateway.XXXXXX.yaml")"
    backup_conf="${tmp_conf}.bak"
    cp "$CONF_FILE" "$backup_conf"
    switch_upstream "$CONF_FILE" "$tmp_conf" "$2"
    if ! check_gateway_conf "$tmp_conf" >/dev/null; then
      rm -f "$tmp_conf"
      fail "gateway config check failed after switching active_upstream to $2"
    fi
    target_port="$(slot_port_from_file "$tmp_conf" "$2")"
    gateway_base="http://$(listen_addr_from_file "$tmp_conf")"
    mv "$tmp_conf" "$CONF_FILE"
    if ! reload_gateway || ! wait_gateway_port "$gateway_base" "$target_port"; then
      mv "$backup_conf" "$CONF_FILE"
      check_gateway_conf >/dev/null
      reload_gateway || true
      wait_gateway_port "$gateway_base" "$(slot_port_from_file "$CONF_FILE" "$old_slot")" || true
      fail "gateway switch verification failed after switching active_upstream to $2; reverted to $old_slot"
    fi
    rm -f "$backup_conf"
    log "active_upstream switched: $old_slot -> $2"
    ;;
  -h|--help|"")
    usage
    ;;
  *)
    fail "usage: $0 {run|start|restart|check|reload|status|stop|switch <blue|green>|stop-backend <blue|green>|logs <gateway|blue|green>|health}"
    ;;
esac
