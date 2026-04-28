#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT_DIR/scripts/lib_pingora_gateway_conf.sh"
CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
TEMPLATE_FILE="${PINGORA_CONF_TEMPLATE_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml.template}"

fail() {
  echo "[test-pingora-gateway-conf][ERROR] $*" >&2
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
[[ -f "$TEMPLATE_FILE" ]] || fail "template not found: $TEMPLATE_FILE"
source "$HELPER"
pingora_ensure_conf_file "$CONF_FILE" "$TEMPLATE_FILE"
[[ -f "$CONF_FILE" ]] || fail "config not found after bootstrap: $CONF_FILE"

expected_active_upstream="$(
  awk -F': ' '/^[[:space:]]*active_upstream:/{print $2; exit}' "$CONF_FILE"
)"
[[ "$expected_active_upstream" == "blue" || "$expected_active_upstream" == "green" ]] \
  || fail "unexpected active_upstream in $CONF_FILE: ${expected_active_upstream:-<empty>}"

assert_eq \
  "$(pingora_top_level_conf_value "$CONF_FILE" "pid_file")" \
  "tmp/staticflow-gateway.pid" \
  "pid_file"
assert_eq \
  "$(pingora_top_level_conf_value "$CONF_FILE" "error_log")" \
  "tmp/runtime-logs/gateway/daemon-stderr.log" \
  "error_log"
assert_eq \
  "$(pingora_staticflow_conf_value "$CONF_FILE" "listen_addr")" \
  "127.0.0.1:39180" \
  "listen_addr"
assert_eq \
  "$(pingora_staticflow_conf_value "$CONF_FILE" "active_upstream")" \
  "$expected_active_upstream" \
  "active_upstream"
assert_eq \
  "$(pingora_staticflow_conf_value "$CONF_FILE" "downstream_h2c")" \
  "true" \
  "downstream_h2c"
assert_eq \
  "$(pingora_staticflow_upstream_addr "$CONF_FILE" "blue")" \
  "127.0.0.1:39080" \
  "blue upstream"
assert_eq \
  "$(pingora_staticflow_upstream_addr "$CONF_FILE" "green")" \
  "127.0.0.1:39081" \
  "green upstream"

status_output="$(
  env PATH=/usr/bin:/bin PINGORA_CONF_TEMPLATE_FILE="$TEMPLATE_FILE" SYSTEMD_SCOPE=user \
    STATICFLOW_GATEWAY_UNIT="missing-gateway.service" \
    STATICFLOW_BACKEND_SLOT_UNIT_TEMPLATE="missing-backend-slot@%s.service" \
    bash "$ROOT_DIR/scripts/pingora_gateway.sh" status
)"
assert_eq \
  "$(printf '%s\n' "$status_output" | awk -F= '/^listen_addr=/{print $2}')" \
  "127.0.0.1:39180" \
  "status listen_addr without rg"
assert_eq \
  "$(printf '%s\n' "$status_output" | awk -F= '/^active_upstream=/{print $2}')" \
  "$expected_active_upstream" \
  "status active_upstream without rg"
assert_eq \
  "$(printf '%s\n' "$status_output" | awk -F= '/^downstream_h2c=/{print $2}')" \
  "true" \
  "status downstream_h2c without rg"
assert_eq \
  "$(printf '%s\n' "$status_output" | awk -F= '/^systemd_scope=/{print $2}')" \
  "user" \
  "status systemd_scope without rg"
assert_eq \
  "$(printf '%s\n' "$status_output" | awk -F= '/^gateway_unit=/{print $2}')" \
  "missing-gateway.service" \
  "status gateway_unit without rg"

echo "[test-pingora-gateway-conf] ok"
