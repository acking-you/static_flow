#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT_DIR/scripts/lib_media_service_common.sh"

fail() {
  echo "[test-media-service-common][ERROR] $*" >&2
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

defaults_case="$(
  (
    unset STATICFLOW_LOCAL_MEDIA_ROOT STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG
    sf_apply_media_service_defaults
    printf 'root=%s\n' "${STATICFLOW_LOCAL_MEDIA_ROOT:-}"
    printf 'auto_download=%s\n' "${STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG:-}"
  )
)"
assert_eq "$(printf '%s\n' "$defaults_case" | awk -F= '/^root=/{print $2}')" "/mnt/e/videos/static" "default media root"
assert_eq "$(printf '%s\n' "$defaults_case" | awk -F= '/^auto_download=/{print $2}')" "1" "default auto download"

assert_eq \
  "$(sf_media_service_health_url 127.0.0.1 39085 2)" \
  "http://127.0.0.1:39085/internal/local-media/list?limit=2" \
  "health url"

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
import socketserver
import sys

port_file = sys.argv[1]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/internal/local-media/list"):
            body = b'{"configured":true,"entries":[]}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
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

sf_wait_media_service_ready 127.0.0.1 "$TEST_PORT" 20 0.05 || fail "wait helper should succeed"

kill "$SERVER_PID" >/dev/null 2>&1 || true
wait "$SERVER_PID" 2>/dev/null || true
unset SERVER_PID

if sf_wait_media_service_ready 127.0.0.1 "$TEST_PORT" 2 0.02; then
  fail "wait helper should fail after the test server stops"
fi

echo "[test-media-service-common] ok"
