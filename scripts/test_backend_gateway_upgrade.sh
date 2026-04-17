#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT_DIR/scripts/lib_backend_gateway_upgrade.sh"

fail() {
  echo "[test-backend-gateway-upgrade][ERROR] $*" >&2
  exit 1
}

assert_eq() {
  local actual="$1"
  local expected="$2"
  local label="$3"
  if [[ "$actual" != "$expected" ]]; then
    fail "$label: expected '$expected', got '$actual'"
  fi
}

[[ -f "$HELPER" ]] || fail "helper not found: $HELPER"
source "$HELPER"

PORT_FILE="$(mktemp)"
cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -f "$PORT_FILE"
}
trap cleanup EXIT INT TERM

python3 -u - "$PORT_FILE" <<'PY' &
import http.server
import json
import socketserver
import sys

port_file = sys.argv[1]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/api/healthz":
            body = json.dumps({
                "status": "ok",
                "pid": 12345,
                "port": self.server.server_address[1],
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/html":
            body = b"<html>ok</html>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        return

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    with open(port_file, "w", encoding="utf-8") as fh:
        fh.write(str(httpd.server_address[1]))
        fh.flush()
    httpd.serve_forever()
PY
SERVER_PID=$!

for _ in $(seq 1 50); do
  if [[ -s "$PORT_FILE" ]]; then
    break
  fi
  sleep 0.05
done

[[ -s "$PORT_FILE" ]] || fail "test server did not write a port"
TEST_PORT="$(cat "$PORT_FILE")"

assert_eq \
  "$(healthz_json_field "http://127.0.0.1:${TEST_PORT}/api/healthz" pid)" \
  "12345" \
  "healthz_json_field pid"

if healthz_json_field "http://127.0.0.1:${TEST_PORT}/html" pid >/dev/null 2>&1; then
  fail "healthz_json_field should fail on non-json body"
fi

assert_eq \
  "$(listener_pid_for_port "$TEST_PORT")" \
  "$SERVER_PID" \
  "listener_pid_for_port"

echo "[test-backend-gateway-upgrade] ok"
