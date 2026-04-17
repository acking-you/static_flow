#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT_DIR/scripts/pingora_gateway.sh"
TMP_DIR="$(mktemp -d)"
BIN_DIR="$TMP_DIR/bin"
GATEWAY_BIN="$TMP_DIR/staticflow-pingora-gateway"
CONF_FILE="$TMP_DIR/staticflow-gateway.yaml"
LOCK_FILE="$TMP_DIR/staticflow-gateway.lock"
PID_FILE="$TMP_DIR/staticflow-gateway.pid"
ERROR_LOG="$TMP_DIR/runtime-logs/gateway/daemon-stderr.log"
COUNTER_FILE="$TMP_DIR/build.counter"
CARGO_LOG="$TMP_DIR/cargo.log"
HEALTH_BODY="$TMP_DIR/health.json"

fail() {
  echo "[test-pingora-gateway-restart][ERROR] $*" >&2
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
  for _ in $(seq 1 80); do
    if curl -fsS "$url" >"$HEALTH_BODY" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

cleanup() {
  local pid=""
  if [[ -f "$PID_FILE" ]]; then
    pid="$(cat "$PID_FILE" 2>/dev/null || true)"
  fi
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
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

mkdir -p "$BIN_DIR" "$(dirname "$ERROR_LOG")"
PORT="$(pick_free_port)"

cat >"$CONF_FILE" <<EOF
version: 1
daemon: false
threads: 2
pid_file: $PID_FILE
error_log: $ERROR_LOG
upgrade_sock: $TMP_DIR/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:$PORT
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  upstreams:
    blue: 127.0.0.1:39080
    green: 127.0.0.1:39081
  active_upstream: green
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 3000
  write_idle_timeout_ms: 3000
  retry_count: 0
EOF

cat >"$BIN_DIR/cargo" <<EOF
#!/usr/bin/env bash
set -euo pipefail
counter_file="$COUNTER_FILE"
gateway_bin="$GATEWAY_BIN"
log_file="$CARGO_LOG"
count=0
if [[ -f "\$counter_file" ]]; then
  count="\$(cat "\$counter_file")"
fi
count="\$((count + 1))"
printf '%s\n' "\$count" >"\$counter_file"
printf '%s\n' "\$*" >>"\$log_file"
cat >"\$gateway_bin" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
build_id="__BUILD_ID__"
conf=""
while [[ \$# -gt 0 ]]; do
  case "\$1" in
    --conf)
      conf="\$2"
      shift 2
      ;;
    --test)
      exit 0
      ;;
    *)
      shift
      ;;
  esac
done
[[ -n "\$conf" ]] || exit 1
exec python3 -u - "\$conf" "\$build_id" <<'PY'
import http.server
import json
import os
import socketserver
import sys

conf_path = sys.argv[1]
build_id = sys.argv[2]
listen_addr = None
active_upstream = None
upstreams = {}
inside_upstreams = False

with open(conf_path, "r", encoding="utf-8") as fh:
    for raw in fh:
        line = raw.rstrip("\n")
        if line.startswith("  listen_addr:"):
            listen_addr = line.split(":", 1)[1].strip()
            inside_upstreams = False
        elif line.startswith("  active_upstream:"):
            active_upstream = line.split(":", 1)[1].strip()
            inside_upstreams = False
        elif line.startswith("  upstreams:"):
            inside_upstreams = True
        elif inside_upstreams and line.startswith("    "):
            key, value = line.strip().split(": ", 1)
            upstreams[key] = value
        elif inside_upstreams and not line.startswith("    "):
            inside_upstreams = False

listen_host, listen_port = listen_addr.rsplit(":", 1)
active_port = upstreams[active_upstream].rsplit(":", 1)[1]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/api/healthz":
            self.send_response(404)
            self.end_headers()
            return
        body = json.dumps({
            "status": "ok",
            "pid": os.getpid(),
            "port": active_port,
            "build": build_id,
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return

class ReusableTCPServer(socketserver.TCPServer):
    allow_reuse_address = True

with ReusableTCPServer((listen_host, int(listen_port)), Handler) as httpd:
    httpd.serve_forever()
PY
SH
python3 - "\$gateway_bin" "\$count" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
build_id = sys.argv[2]
path.write_text(path.read_text().replace("__BUILD_ID__", build_id))
path.chmod(0o755)
PY
EOF
chmod +x "$BIN_DIR/cargo"

PATH="$BIN_DIR:/usr/bin:/bin" \
CONF_FILE="$CONF_FILE" \
LOCK_FILE="$LOCK_FILE" \
GATEWAY_BIN="$GATEWAY_BIN" \
STATICFLOW_LOG_DIR="$TMP_DIR/runtime-logs" \
  bash "$SCRIPT" start >"$TMP_DIR/start.stdout" 2>"$TMP_DIR/start.stderr"

wait_for_health "http://127.0.0.1:${PORT}/api/healthz" \
  || fail "gateway did not become healthy after start"
START_PID="$(cat "$PID_FILE")"

python3 - "$HEALTH_BODY" <<'PY'
import json
import sys

body = json.load(open(sys.argv[1], "r", encoding="utf-8"))
assert body["build"] == "1", body
assert body["port"] == "39081", body
PY

PATH="$BIN_DIR:/usr/bin:/bin" \
CONF_FILE="$CONF_FILE" \
LOCK_FILE="$LOCK_FILE" \
GATEWAY_BIN="$GATEWAY_BIN" \
STATICFLOW_LOG_DIR="$TMP_DIR/runtime-logs" \
  bash "$SCRIPT" restart >"$TMP_DIR/restart.stdout" 2>"$TMP_DIR/restart.stderr"

wait_for_health "http://127.0.0.1:${PORT}/api/healthz" \
  || fail "gateway did not become healthy after restart"
RESTART_PID="$(cat "$PID_FILE")"
[[ "$RESTART_PID" != "$START_PID" ]] || fail "restart should replace the old gateway pid"
if kill -0 "$START_PID" >/dev/null 2>&1; then
  fail "old gateway pid should be gone after restart"
fi

python3 - "$HEALTH_BODY" <<'PY'
import json
import sys

body = json.load(open(sys.argv[1], "r", encoding="utf-8"))
assert body["build"] == "2", body
assert body["port"] == "39081", body
PY

grep -Fq 'build -p staticflow-pingora-gateway --profile release-backend' "$CARGO_LOG" \
  || fail "restart should force a gateway rebuild"

cleanup
trap - EXIT INT TERM

echo "[test-pingora-gateway-restart] ok"
