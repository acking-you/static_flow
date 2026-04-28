#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT_DIR/scripts/pingora_gateway.sh"
TMP_DIR="$(mktemp -d)"
CONF_FILE="$TMP_DIR/staticflow-gateway.yaml"
TEMPLATE_FILE="$TMP_DIR/staticflow-gateway.yaml.template"

fail() {
  echo "[test-pingora-gateway-conf-bootstrap][ERROR] $*" >&2
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

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

cat >"$TEMPLATE_FILE" <<'EOF'
version: 1
daemon: false
threads: 2
pid_file: tmp/staticflow-gateway.pid
error_log: tmp/runtime-logs/gateway/daemon-stderr.log
upgrade_sock: tmp/staticflow-gateway-upgrade.sock

staticflow:
  listen_addr: 127.0.0.1:39180
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  downstream_h2c: true
  upstreams:
    blue: 127.0.0.1:39080
    green: 127.0.0.1:39081
  active_upstream: blue
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
EOF

STATUS_OUTPUT="$(
  env \
    PATH=/usr/bin:/bin \
    CONF_FILE="$CONF_FILE" \
    PINGORA_CONF_TEMPLATE_FILE="$TEMPLATE_FILE" \
    bash "$SCRIPT" status
)"

[[ -f "$CONF_FILE" ]] || fail "status should create missing conf from template"
cmp -s "$CONF_FILE" "$TEMPLATE_FILE" || fail "generated conf should match template"
assert_contains <(printf '%s\n' "$STATUS_OUTPUT") "conf=$CONF_FILE" "status output conf path"
assert_contains "$CONF_FILE" "active_upstream: blue" "generated conf active slot"
assert_contains "$CONF_FILE" "downstream_h2c: true" "generated conf h2c flag"

python3 - "$CONF_FILE" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
path.write_text(path.read_text().replace("active_upstream: blue", "active_upstream: green"))
PY

env \
  PATH=/usr/bin:/bin \
  CONF_FILE="$CONF_FILE" \
  PINGORA_CONF_TEMPLATE_FILE="$TEMPLATE_FILE" \
  bash "$SCRIPT" status >"$TMP_DIR/status-second.out"

assert_contains "$CONF_FILE" "active_upstream: green" "existing conf should not be overwritten"

echo "[test-pingora-gateway-conf-bootstrap] ok"
