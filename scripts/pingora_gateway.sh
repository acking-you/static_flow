#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
GATEWAY_BIN="${GATEWAY_BIN:-$ROOT_DIR/target/release-backend/staticflow-pingora-gateway}"
STATICFLOW_LOG_DIR="${STATICFLOW_LOG_DIR:-$ROOT_DIR/tmp/runtime-logs}"
STATICFLOW_LOG_SERVICE="${STATICFLOW_LOG_SERVICE:-gateway}"

log() { echo "[gateway] $*"; }
fail() { echo "[gateway][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/pingora_gateway.sh {run|start|check|reload|status|stop|switch <blue|green>}

Environment variables:
  CONF_FILE               Gateway YAML path
  GATEWAY_BIN             Gateway binary path
  STATICFLOW_LOG_DIR      Runtime log root
  STATICFLOW_LOG_SERVICE  Gateway runtime log folder name
EOF
}

ensure_layout() {
  mkdir -p "$ROOT_DIR/tmp" "$STATICFLOW_LOG_DIR/$STATICFLOW_LOG_SERVICE"
  mkdir -p "$(dirname "$(pid_file)")"
  mkdir -p "$(dirname "$(error_log_file)")"
  mkdir -p "$(dirname "$(upgrade_sock)")"
}

build_gateway_bin() {
  ensure_layout
  log "building gateway binary: $GATEWAY_BIN"
  cargo build -p staticflow-pingora-gateway --profile release-backend >/dev/null
}

check_gateway() {
  build_gateway_bin
  log "checking gateway config: $CONF_FILE"
  STATICFLOW_LOG_DIR="$STATICFLOW_LOG_DIR" \
  STATICFLOW_LOG_SERVICE="$STATICFLOW_LOG_SERVICE" \
    "$GATEWAY_BIN" --conf "$CONF_FILE" --test
}

conf_value() {
  local key="$1"
  rg "^${key}:" "$CONF_FILE" | awk '{print $2}'
}

pid_file() {
  conf_value "pid_file"
}

error_log_file() {
  conf_value "error_log"
}

upgrade_sock() {
  conf_value "upgrade_sock"
}

current_pid() {
  local file
  file="$(pid_file)"
  [[ -f "$file" ]] && cat "$file"
}

active_upstream() {
  rg '^[[:space:]]+active_upstream:' "$CONF_FILE" | awk '{print $2}'
}

reload_gateway() {
  local old_pid="" new_pid=""
  old_pid="$(current_pid || true)"
  if [[ -z "$old_pid" ]]; then
    echo "[gateway][ERROR] gateway is not running" >&2
    return 1
  fi
  log "starting graceful reload (old_pid=$old_pid active_upstream=$(active_upstream))"
  STATICFLOW_LOG_DIR="$STATICFLOW_LOG_DIR" \
  STATICFLOW_LOG_SERVICE="$STATICFLOW_LOG_SERVICE" \
    "$GATEWAY_BIN" --daemon --upgrade --conf "$CONF_FILE"

  for _ in $(seq 1 40); do
    new_pid="$(current_pid || true)"
    if [[ -n "$new_pid" && "$new_pid" != "$old_pid" ]]; then
      break
    fi
    sleep 0.25
  done

  if [[ -z "$new_pid" || "$new_pid" == "$old_pid" ]]; then
    echo "[gateway][ERROR] new gateway pid did not appear after upgrade start" >&2
    return 1
  fi
  kill -QUIT "$old_pid"
  log "graceful reload handed over to new_pid=$new_pid"
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
  local pid
  pid="$(current_pid || true)"
  [[ -n "$pid" ]] || fail "gateway is not running"
  kill -0 "$pid" 2>/dev/null || fail "gateway pid $pid is stale"
}

case "${1:-}" in
  run)
    build_gateway_bin
    export STATICFLOW_LOG_DIR STATICFLOW_LOG_SERVICE
    exec "$GATEWAY_BIN" --conf "$CONF_FILE"
    ;;
  start)
    build_gateway_bin
    if pid="$(current_pid || true)" && [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      fail "gateway is already running (pid=$pid)"
    fi
    export STATICFLOW_LOG_DIR STATICFLOW_LOG_SERVICE
    log "starting gateway daemon on $(active_upstream) via $CONF_FILE"
    "$GATEWAY_BIN" --daemon --conf "$CONF_FILE"
    ;;
  check)
    check_gateway
    ;;
  reload)
    require_running
    check_gateway >/dev/null
    reload_gateway
    ;;
  status)
    pid="$(current_pid || true)"
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      pid="${pid} (stale)"
    fi
    echo "conf=$CONF_FILE"
    echo "pid=${pid:-}"
    echo "pid_file=$(pid_file)"
    echo "listen_addr=$(rg '^[[:space:]]+listen_addr:' "$CONF_FILE" | awk '{print $2}')"
    echo "active_upstream=$(active_upstream)"
    ;;
  stop)
    require_running
    pid="$(current_pid || true)"
    log "stopping gateway pid=$pid"
    kill -QUIT "$pid"
    ;;
  switch)
    [[ $# -eq 2 ]] || fail "usage: $0 switch <blue|green>"
    [[ "$2" == "blue" || "$2" == "green" ]] || fail "slot must be blue or green"
    require_running
    old_slot="$(active_upstream)"
    [[ "$old_slot" != "$2" ]] || fail "active_upstream is already $2"
    tmp_conf="$(mktemp "$ROOT_DIR/tmp/staticflow-gateway.XXXXXX.yaml")"
    cp "$CONF_FILE" "${tmp_conf}.bak"
    switch_upstream "$CONF_FILE" "$tmp_conf" "$2"
    mv "$tmp_conf" "$CONF_FILE"
    if ! check_gateway >/dev/null; then
      mv "${tmp_conf}.bak" "$CONF_FILE"
      fail "gateway config check failed after switching active_upstream to $2"
    fi
    if ! reload_gateway; then
      mv "${tmp_conf}.bak" "$CONF_FILE"
      check_gateway >/dev/null
      reload_gateway || true
      fail "gateway reload failed after switching active_upstream to $2; reverted to $old_slot"
    fi
    rm -f "${tmp_conf}.bak"
    log "active_upstream switched: $old_slot -> $2"
    ;;
  -h|--help|"")
    usage
    ;;
  *)
    fail "usage: $0 {run|start|check|reload|status|stop|switch <blue|green>}"
    ;;
esac
