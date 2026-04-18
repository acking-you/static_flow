#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TMP_DIR="$(mktemp -d "$ROOT_DIR/tmp/systemd-stack-test.XXXXXX")"
UNIT_PREFIX="staticflow-systemd-test"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
COMMON_ENV="$TMP_DIR/common.env"
GATEWAY_ENV="$TMP_DIR/gateway.env"
BACKEND_BLUE_ENV="$TMP_DIR/backend-slot-blue.env"
BACKEND_GREEN_ENV="$TMP_DIR/backend-slot-green.env"
CONF_FILE="$TMP_DIR/staticflow-gateway.yaml"
BUNDLE_DIR="$TMP_DIR/release"
GATEWAY_UNIT="${UNIT_PREFIX}-gateway.service"
BACKEND_TEMPLATE_UNIT="${UNIT_PREFIX}-backend-slot@.service"
BACKEND_BLUE_UNIT="${UNIT_PREFIX}-backend-slot@blue.service"
BACKEND_GREEN_UNIT="${UNIT_PREFIX}-backend-slot@green.service"
LOCK_FILE="$TMP_DIR/staticflow-gateway.lock"
RUN_LOG_ROOT="$TMP_DIR/runtime-logs"

fail() {
  echo "[test-systemd-stack][ERROR] $*" >&2
  exit 1
}

pick_free_port() {
  python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

wait_for_health() {
  local url="$1"
  for _ in $(seq 1 120); do
    if curl -fsS "$url" >"$TMP_DIR/health.json" 2>/dev/null; then
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

run_gateway_cmd() {
  env \
    CONF_FILE="$CONF_FILE" \
    LOCK_FILE="$LOCK_FILE" \
    GATEWAY_BIN="$BUNDLE_DIR/bin/staticflow-pingora-gateway" \
    STATICFLOW_LOG_DIR="$RUN_LOG_ROOT" \
    SYSTEMD_SCOPE=user \
    STATICFLOW_GATEWAY_UNIT="$GATEWAY_UNIT" \
    STATICFLOW_BACKEND_SLOT_UNIT_TEMPLATE="${UNIT_PREFIX}-backend-slot@%s.service" \
    "$ROOT_DIR/scripts/pingora_gateway.sh" "$@"
}

cleanup() {
  systemctl --user stop "$GATEWAY_UNIT" "$BACKEND_BLUE_UNIT" "$BACKEND_GREEN_UNIT" >/dev/null 2>&1 || true
  rm -f \
    "$UNIT_DIR/$GATEWAY_UNIT" \
    "$UNIT_DIR/$BACKEND_TEMPLATE_UNIT"
  systemctl --user daemon-reload >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

command -v systemctl >/dev/null 2>&1 || fail "systemctl not found"
systemctl --user show-environment >/dev/null 2>&1 || fail "systemd --user is not available"

GATEWAY_PORT="$(pick_free_port)"
BLUE_PORT="$(pick_free_port)"
GREEN_PORT="$(pick_free_port)"

while [[ "$BLUE_PORT" == "$GATEWAY_PORT" || "$GREEN_PORT" == "$GATEWAY_PORT" || "$GREEN_PORT" == "$BLUE_PORT" ]]; do
  BLUE_PORT="$(pick_free_port)"
  GREEN_PORT="$(pick_free_port)"
done

"$ROOT_DIR/scripts/prepare_selfhosted_systemd_bundle.sh" \
  --output-dir "$BUNDLE_DIR" >/dev/null

mkdir -p "$UNIT_DIR" "$RUN_LOG_ROOT"

cat >"$CONF_FILE" <<EOF
version: 1
daemon: false
threads: 2
pid_file: $TMP_DIR/staticflow-gateway.pid
error_log: $RUN_LOG_ROOT/gateway/daemon-stderr.log
upgrade_sock: $TMP_DIR/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:$GATEWAY_PORT
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  upstreams:
    blue: 127.0.0.1:$BLUE_PORT
    green: 127.0.0.1:$GREEN_PORT
  active_upstream: blue
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
EOF

cat >"$COMMON_ENV" <<EOF
DB_ROOT=/mnt/wsl/data4tb/static-flow-data
FRONTEND_DIST_DIR=$BUNDLE_DIR/frontend/dist
STATICFLOW_LOG_DIR=$RUN_LOG_ROOT
BACKEND_BIN=$BUNDLE_DIR/bin/static-flow-backend
GATEWAY_BIN=$BUNDLE_DIR/bin/staticflow-pingora-gateway
SITE_BASE_URL=http://127.0.0.1:$GATEWAY_PORT
LOCAL_MEDIA_MODE=enabled
EOF

cat >"$GATEWAY_ENV" <<EOF
CONF_FILE=$CONF_FILE
LOCK_FILE=$LOCK_FILE
STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR=1
EOF

cat >"$BACKEND_BLUE_ENV" <<EOF
CONF_FILE=$CONF_FILE
STATICFLOW_LOG_SERVICE=backend-blue-$BLUE_PORT
EOF

cat >"$BACKEND_GREEN_ENV" <<EOF
CONF_FILE=$CONF_FILE
STATICFLOW_LOG_SERVICE=backend-green-$GREEN_PORT
EOF

"$ROOT_DIR/scripts/render_selfhosted_systemd_units.sh" \
  --unit-dir "$UNIT_DIR" \
  --workdir "$ROOT_DIR" \
  --common-env "$COMMON_ENV" \
  --gateway-env "$GATEWAY_ENV" \
  --backend-env-pattern "$TMP_DIR/backend-slot-%i.env" \
  --unit-prefix "$UNIT_PREFIX" \
  --description-prefix "StaticFlow Test" >/dev/null

systemctl --user daemon-reload >/dev/null
run_gateway_cmd start-backend blue >/dev/null
run_gateway_cmd start-backend green >/dev/null
run_gateway_cmd start >/dev/null

wait_for_health "http://127.0.0.1:$BLUE_PORT/api/healthz" \
  || fail "blue backend did not become healthy"
wait_for_health "http://127.0.0.1:$GREEN_PORT/api/healthz" \
  || fail "green backend did not become healthy"
wait_for_health "http://127.0.0.1:$GATEWAY_PORT/api/healthz" \
  || fail "gateway did not become healthy"

gateway_port="$(json_field port <"$TMP_DIR/health.json")"
[[ "$gateway_port" == "$BLUE_PORT" ]] \
  || fail "gateway should start on blue port=$BLUE_PORT, got $gateway_port"

status_output="$(run_gateway_cmd status)"
printf '%s\n' "$status_output" | grep -Fq "systemd_scope=user" \
  || fail "status summary should report user scope"
printf '%s\n' "$status_output" | grep -Fq "active_upstream=blue" \
  || fail "status summary should report blue as active before switch"

run_gateway_cmd status gateway >/dev/null
run_gateway_cmd status blue >/dev/null
run_gateway_cmd logs gateway --lines 20 >/dev/null
run_gateway_cmd health >/dev/null

run_gateway_cmd switch green >/dev/null

wait_for_health "http://127.0.0.1:$GATEWAY_PORT/api/healthz" \
  || fail "gateway did not recover after switching to green"

gateway_port="$(json_field port <"$TMP_DIR/health.json")"
[[ "$gateway_port" == "$GREEN_PORT" ]] \
  || fail "gateway should switch to green port=$GREEN_PORT, got $gateway_port"

status_output="$(run_gateway_cmd status)"
printf '%s\n' "$status_output" | grep -Fq "active_upstream=green" \
  || fail "status summary should report green as active after switch"

run_gateway_cmd restart >/dev/null
wait_for_health "http://127.0.0.1:$GATEWAY_PORT/api/healthz" \
  || fail "gateway did not recover after restart"

systemctl --user is-active "$GATEWAY_UNIT" "$BACKEND_BLUE_UNIT" "$BACKEND_GREEN_UNIT" >/dev/null

echo "[test-systemd-stack] ok gateway_port=$GATEWAY_PORT blue_port=$BLUE_PORT green_port=$GREEN_PORT"
