#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT_DIR/scripts/lib_local_media_proxy_env.sh"

fail() {
  echo "[test-local-media-proxy-env][ERROR] $*" >&2
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

assert_empty() {
  local actual="${1:-}"
  local label="$2"
  if [[ -n "$actual" ]]; then
    fail "$label: expected empty, got '$actual'"
  fi
}

run_case() {
  local mode="$1"
  local base_url="$2"
  local host="$3"
  local port="$4"

  (
    unset STATICFLOW_MEDIA_PROXY_BASE_URL STATICFLOW_MEDIA_PROXY_HOST STATICFLOW_MEDIA_PROXY_PORT LOCAL_MEDIA_MODE
    [[ -n "$mode" ]] && export LOCAL_MEDIA_MODE="$mode"
    [[ -n "$base_url" ]] && export STATICFLOW_MEDIA_PROXY_BASE_URL="$base_url"
    [[ -n "$host" ]] && export STATICFLOW_MEDIA_PROXY_HOST="$host"
    [[ -n "$port" ]] && export STATICFLOW_MEDIA_PROXY_PORT="$port"
    source "$HELPER"
    sf_apply_local_media_proxy_defaults
    printf 'mode=%s\n' "${LOCAL_MEDIA_MODE:-}"
    printf 'base_url=%s\n' "${STATICFLOW_MEDIA_PROXY_BASE_URL:-}"
  )
}

[[ -f "$HELPER" ]] || fail "helper not found: $HELPER"

default_case="$(run_case "" "" "" "")"
assert_eq "$(printf '%s\n' "$default_case" | awk -F= '/^mode=/{print $2}')" "enabled" "default mode"
assert_eq "$(printf '%s\n' "$default_case" | awk -F= '/^base_url=/{print $2}')" "http://127.0.0.1:39085" "default base url"

custom_case="$(run_case "" "" "127.0.0.2" "39123")"
assert_eq "$(printf '%s\n' "$custom_case" | awk -F= '/^base_url=/{print $2}')" "http://127.0.0.2:39123" "custom host/port"

explicit_case="$(run_case "" "http://127.0.0.1:49000" "" "")"
assert_eq "$(printf '%s\n' "$explicit_case" | awk -F= '/^base_url=/{print $2}')" "http://127.0.0.1:49000" "explicit base url"

disabled_case="$(run_case "disabled" "http://127.0.0.1:49000" "" "")"
assert_eq "$(printf '%s\n' "$disabled_case" | awk -F= '/^mode=/{print $2}')" "disabled" "disabled mode"
assert_empty "$(printf '%s\n' "$disabled_case" | awk -F= '/^base_url=/{print $2}')" "disabled base url"

echo "[test-local-media-proxy-env] ok"
