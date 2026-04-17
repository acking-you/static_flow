#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT_DIR/scripts/pingora_gateway.sh"
TMP_DIR="$(mktemp -d)"
CONF_FILE="$TMP_DIR/staticflow-gateway.yaml"
LOCK_FILE="$TMP_DIR/staticflow-gateway.lock"
BLUE_PORT_FILE="$TMP_DIR/blue.port"
GREEN_PORT_FILE="$TMP_DIR/green.port"
ACTIVE_STDERR="$TMP_DIR/active.stderr"
MISSING_STDERR="$TMP_DIR/missing.stderr"

fail() {
  echo "[test-pingora-gateway-stop-backend][ERROR] $*" >&2
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

start_listener() {
  local port_file="$1"
  python3 -u - "$port_file" >/dev/null 2>&1 <<'PY' &
import http.server
import socketserver
import sys

port_file = sys.argv[1]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, format, *args):
        return

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
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

cleanup() {
  for pid in "${BLUE_PID:-}" "${GREEN_PID:-}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

BLUE_PID="$(start_listener "$BLUE_PORT_FILE")"
GREEN_PID="$(start_listener "$GREEN_PORT_FILE")"
wait_for_file "$BLUE_PORT_FILE" || fail "blue listener did not publish a port"
wait_for_file "$GREEN_PORT_FILE" || fail "green listener did not publish a port"
BLUE_PORT="$(cat "$BLUE_PORT_FILE")"
GREEN_PORT="$(cat "$GREEN_PORT_FILE")"

cat >"$CONF_FILE" <<EOF
version: 1
daemon: false
threads: 2
pid_file: $TMP_DIR/staticflow-gateway.pid
error_log: $TMP_DIR/staticflow-gateway.log
upgrade_sock: $TMP_DIR/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:39180
  upstreams:
    blue: 127.0.0.1:$BLUE_PORT
    green: 127.0.0.1:$GREEN_PORT
  active_upstream: blue
EOF

if CONF_FILE="$CONF_FILE" LOCK_FILE="$LOCK_FILE" bash "$SCRIPT" stop-backend blue >"$TMP_DIR/active.stdout" 2>"$ACTIVE_STDERR"; then
  fail "stop-backend blue should refuse to stop the active slot"
fi
assert_contains "$ACTIVE_STDERR" "refusing to stop active slot blue" "active slot refusal"
kill -0 "$BLUE_PID" >/dev/null 2>&1 || fail "active slot process should remain alive"

CONF_FILE="$CONF_FILE" LOCK_FILE="$LOCK_FILE" bash "$SCRIPT" stop-backend green >"$TMP_DIR/green.stdout" 2>"$TMP_DIR/green.stderr"
for _ in $(seq 1 50); do
  if ! kill -0 "$GREEN_PID" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
if kill -0 "$GREEN_PID" >/dev/null 2>&1; then
  fail "non-active slot process should be stopped"
fi
wait "$GREEN_PID" 2>/dev/null || true
GREEN_PID=""
kill -0 "$BLUE_PID" >/dev/null 2>&1 || fail "stopping green should not kill blue"

if CONF_FILE="$CONF_FILE" LOCK_FILE="$LOCK_FILE" bash "$SCRIPT" stop-backend green >"$TMP_DIR/missing.stdout" 2>"$MISSING_STDERR"; then
  fail "stop-backend green should fail after the listener is already gone"
fi
assert_contains "$MISSING_STDERR" "no backend listener found for slot green on port $GREEN_PORT" "missing listener failure"

echo "[test-pingora-gateway-stop-backend] ok"
