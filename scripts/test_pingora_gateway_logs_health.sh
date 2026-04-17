#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT_DIR/scripts/pingora_gateway.sh"
TMP_DIR="$(mktemp -d)"
CONF_FILE="$TMP_DIR/staticflow-gateway.yaml"
LOCK_FILE="$TMP_DIR/staticflow-gateway.lock"
PID_FILE="$TMP_DIR/staticflow-gateway.pid"
BLUE_PORT_FILE="$TMP_DIR/blue.port"
GREEN_PORT_FILE="$TMP_DIR/green.port"
GATEWAY_PORT_FILE="$TMP_DIR/gateway.port"

fail() {
  echo "[test-pingora-gateway-logs-health][ERROR] $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  local label="$3"
  if ! grep -Fq "$pattern" "$file"; then
    fail "$label: missing pattern '$pattern' in $file"
  fi
}

start_health_server() {
  local port_file="$1"
  local reported_port="${2:-self}"
  python3 -u - "$port_file" "$reported_port" >/dev/null 2>&1 <<'PY' &
import http.server
import json
import socketserver
import sys

port_file = sys.argv[1]
reported_port = sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/api/healthz":
            self.send_response(404)
            self.end_headers()
            return
        port = self.server.server_address[1] if reported_port == "self" else reported_port
        body = json.dumps({
            "status": "ok",
            "pid": self.server.pid,
            "port": port,
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    httpd.pid = __import__("os").getpid()
    with open(port_file, "w", encoding="utf-8") as fh:
        fh.write(str(httpd.server_address[1]))
        fh.flush()
    httpd.serve_forever()
PY
  echo "$!"
}

wait_for_file() {
  local path="$1"
  for _ in $(seq 1 50); do
    if [[ -s "$path" ]]; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

run_logs_capture() {
  local target="$1"
  local output_file="$2"
  set +e
  timeout 2 env \
    CONF_FILE="$CONF_FILE" \
    LOCK_FILE="$LOCK_FILE" \
    STATICFLOW_LOG_DIR="$TMP_DIR/runtime-logs" \
    bash "$SCRIPT" logs "$target" >"$output_file" 2>&1
  local exit_code=$?
  set -e
  [[ "$exit_code" == "0" || "$exit_code" == "124" ]] \
    || fail "logs $target returned unexpected exit code $exit_code"
}

cleanup() {
  for pid in "${BLUE_PID:-}" "${GREEN_PID:-}" "${GATEWAY_PID:-}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
      for _ in $(seq 1 40); do
        if ! kill -0 "$pid" >/dev/null 2>&1; then
          break
        fi
        sleep 0.05
      done
      if kill -0 "$pid" >/dev/null 2>&1; then
        kill -9 "$pid" >/dev/null 2>&1 || true
      fi
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

BLUE_PID="$(start_health_server "$BLUE_PORT_FILE")"
GREEN_PID="$(start_health_server "$GREEN_PORT_FILE")"
wait_for_file "$BLUE_PORT_FILE" || fail "blue health server did not publish a port"
wait_for_file "$GREEN_PORT_FILE" || fail "green health server did not publish a port"
BLUE_PORT="$(cat "$BLUE_PORT_FILE")"
GREEN_PORT="$(cat "$GREEN_PORT_FILE")"
GATEWAY_PID="$(start_health_server "$GATEWAY_PORT_FILE" "$GREEN_PORT")"
wait_for_file "$GATEWAY_PORT_FILE" || fail "gateway health server did not publish a port"
GATEWAY_PORT="$(cat "$GATEWAY_PORT_FILE")"

echo "$GATEWAY_PID" >"$PID_FILE"

mkdir -p \
  "$TMP_DIR/runtime-logs/gateway/app" \
  "$TMP_DIR/runtime-logs/gateway/access" \
  "$TMP_DIR/runtime-logs/backend/app" \
  "$TMP_DIR/runtime-logs/backend/access" \
  "$TMP_DIR/runtime-logs/backend-canary-${GREEN_PORT}/app" \
  "$TMP_DIR/runtime-logs/backend-canary-${GREEN_PORT}/access"

printf '%s\n' 'gateway app line' >"$TMP_DIR/runtime-logs/gateway/app/current.gateway-app.log"
printf '%s\n' 'gateway access line' >"$TMP_DIR/runtime-logs/gateway/access/current.gateway-access.log"
printf '%s\n' 'gateway stderr line' >"$TMP_DIR/runtime-logs/gateway/daemon-stderr.log"
printf '%s\n' 'blue app line' >"$TMP_DIR/runtime-logs/backend/app/current.backend-app.log"
printf '%s\n' 'blue access line' >"$TMP_DIR/runtime-logs/backend/access/current.backend-access.log"
printf '%s\n' 'green app line' >"$TMP_DIR/runtime-logs/backend-canary-${GREEN_PORT}/app/current.green-app.log"
printf '%s\n' 'green access line' >"$TMP_DIR/runtime-logs/backend-canary-${GREEN_PORT}/access/current.green-access.log"

cat >"$CONF_FILE" <<EOF
version: 1
daemon: false
threads: 2
pid_file: $PID_FILE
error_log: $TMP_DIR/runtime-logs/gateway/daemon-stderr.log
upgrade_sock: $TMP_DIR/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:$GATEWAY_PORT
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  upstreams:
    blue: 127.0.0.1:$BLUE_PORT
    green: 127.0.0.1:$GREEN_PORT
  active_upstream: green
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 3000
  write_idle_timeout_ms: 3000
  retry_count: 0
EOF

run_logs_capture gateway "$TMP_DIR/gateway.logs.out"
assert_contains "$TMP_DIR/gateway.logs.out" "gateway app line" "gateway logs app"
assert_contains "$TMP_DIR/gateway.logs.out" "gateway access line" "gateway logs access"
assert_contains "$TMP_DIR/gateway.logs.out" "gateway stderr line" "gateway logs stderr"

run_logs_capture blue "$TMP_DIR/blue.logs.out"
assert_contains "$TMP_DIR/blue.logs.out" "blue app line" "blue logs app"
assert_contains "$TMP_DIR/blue.logs.out" "blue access line" "blue logs access"

run_logs_capture green "$TMP_DIR/green.logs.out"
assert_contains "$TMP_DIR/green.logs.out" "green app line" "green logs app"
assert_contains "$TMP_DIR/green.logs.out" "green access line" "green logs access"

env \
  CONF_FILE="$CONF_FILE" \
  LOCK_FILE="$LOCK_FILE" \
  STATICFLOW_LOG_DIR="$TMP_DIR/runtime-logs" \
  bash "$SCRIPT" health >"$TMP_DIR/health.ok.out"
assert_contains "$TMP_DIR/health.ok.out" "gateway: ok" "gateway health ok"
assert_contains "$TMP_DIR/health.ok.out" "blue: ok" "blue health ok"
assert_contains "$TMP_DIR/health.ok.out" "green: ok" "green health ok"

kill "$GREEN_PID" >/dev/null 2>&1 || true
wait "$GREEN_PID" 2>/dev/null || true
GREEN_PID=""

set +e
env \
  CONF_FILE="$CONF_FILE" \
  LOCK_FILE="$LOCK_FILE" \
  STATICFLOW_LOG_DIR="$TMP_DIR/runtime-logs" \
  bash "$SCRIPT" health >"$TMP_DIR/health.bad.out" 2>&1
health_exit=$?
set -e
[[ "$health_exit" != "0" ]] || fail "health should fail when green listener is gone"
assert_contains "$TMP_DIR/health.bad.out" "green: unhealthy" "green health unhealthy"

cleanup
trap - EXIT INT TERM

echo "[test-pingora-gateway-logs-health] ok"
