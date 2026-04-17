#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATEWAY_BIN="${GATEWAY_BIN:-$ROOT_DIR/target/release-backend/staticflow-pingora-gateway}"
TMP_DIR="$(mktemp -d)"
UPSTREAM_PORT_FILE="$TMP_DIR/upstream.port"
CONF_FILE="$TMP_DIR/staticflow-gateway.yaml"
RESPONSE_HEADERS="$TMP_DIR/response.headers"
RESPONSE_BODY="$TMP_DIR/response.json"

fail() {
  echo "[test-pingora-gateway-header-passthrough][ERROR] $*" >&2
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

wait_for_url() {
  local url="$1"
  for _ in $(seq 1 80); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

cleanup() {
  for pid in "${GATEWAY_PID:-}" "${UPSTREAM_PID:-}"; do
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

cargo build -p staticflow-pingora-gateway --profile release-backend >/dev/null
[[ -x "$GATEWAY_BIN" ]] || fail "gateway binary not found: $GATEWAY_BIN"

python3 -u - "$UPSTREAM_PORT_FILE" <<'PY' >/dev/null 2>&1 &
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
                "pid": self.server.pid,
                "port": self.server.server_address[1],
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        if self.path == "/echo":
            headers = {key.lower(): value for key, value in self.headers.items()}
            body = json.dumps(headers, sort_keys=True).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("X-Request-Id", "backend-response-req")
            self.send_header("X-Trace-Id", "backend-response-trace")
            self.send_header("X-Test-Upstream", "echo")
            self.end_headers()
            self.wfile.write(body)
            return

        self.send_response(404)
        self.end_headers()

    def log_message(self, format, *args):
        return

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    httpd.pid = self_pid = __import__("os").getpid()
    with open(port_file, "w", encoding="utf-8") as fh:
        fh.write(str(httpd.server_address[1]))
        fh.flush()
    httpd.serve_forever()
PY
UPSTREAM_PID=$!

for _ in $(seq 1 50); do
  if [[ -s "$UPSTREAM_PORT_FILE" ]]; then
    break
  fi
  sleep 0.05
done
[[ -s "$UPSTREAM_PORT_FILE" ]] || fail "upstream mock server did not publish a port"
UPSTREAM_PORT="$(cat "$UPSTREAM_PORT_FILE")"
GATEWAY_PORT="$(pick_free_port)"

cat >"$CONF_FILE" <<EOF
version: 1
daemon: false
threads: 2
pid_file: $TMP_DIR/staticflow-gateway.pid
error_log: $TMP_DIR/runtime-logs/gateway/daemon-stderr.log
upgrade_sock: $TMP_DIR/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:$GATEWAY_PORT
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  upstreams:
    blue: 127.0.0.1:$UPSTREAM_PORT
    green: 127.0.0.1:$UPSTREAM_PORT
  active_upstream: blue
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 3000
  write_idle_timeout_ms: 3000
  retry_count: 0
EOF

STATICFLOW_LOG_DIR="$TMP_DIR/runtime-logs" \
STATICFLOW_LOG_SERVICE="gateway-test" \
STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR=1 \
  "$GATEWAY_BIN" --conf "$CONF_FILE" >/dev/null 2>&1 &
GATEWAY_PID=$!

wait_for_url "http://127.0.0.1:${GATEWAY_PORT}/api/healthz" \
  || fail "gateway did not become ready on port $GATEWAY_PORT"

curl -fsS \
  -D "$RESPONSE_HEADERS" \
  -H 'X-Forwarded-For: 198.51.100.1, 198.51.100.2' \
  -H 'X-Forwarded-Host: original.example.com' \
  -H 'X-Forwarded-Proto: https' \
  -H 'X-Request-Id: client-req-123' \
  -H 'X-Trace-Id: client-trace-456' \
  -H 'Cookie: session_id=abc123; theme=light' \
  -o "$RESPONSE_BODY" \
  "http://127.0.0.1:${GATEWAY_PORT}/echo" >/dev/null

python3 - "$RESPONSE_BODY" <<'PY'
import json
import sys

body = json.load(open(sys.argv[1], "r", encoding="utf-8"))
assert body["x-forwarded-for"] == "198.51.100.1, 198.51.100.2", body
assert body["x-forwarded-host"] == "original.example.com", body
assert body["x-forwarded-proto"] == "https", body
assert body["x-request-id"] == "client-req-123", body
assert body["x-trace-id"] == "client-trace-456", body
assert body["cookie"] == "session_id=abc123; theme=light", body
PY

grep -Fqi 'x-request-id: backend-response-req' "$RESPONSE_HEADERS" \
  || fail "gateway should preserve upstream response x-request-id"
grep -Fqi 'x-trace-id: backend-response-trace' "$RESPONSE_HEADERS" \
  || fail "gateway should preserve upstream response x-trace-id"
grep -Fqi 'x-test-upstream: echo' "$RESPONSE_HEADERS" \
  || fail "gateway should preserve unrelated upstream response headers"

cleanup
trap - EXIT INT TERM

echo "[test-pingora-gateway-header-passthrough] ok"
