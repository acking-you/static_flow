#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT_DIR/scripts/lib_script_lock.sh"

fail() {
  echo "[test-script-lock][ERROR] $*" >&2
  exit 1
}

[[ -f "$HELPER" ]] || fail "helper not found: $HELPER"
source "$HELPER"

LOCK_FILE="$(mktemp)"
cleanup() {
  if [[ -n "${CHILD_PID:-}" ]] && kill -0 "$CHILD_PID" >/dev/null 2>&1; then
    kill "$CHILD_PID" >/dev/null 2>&1 || true
    wait "$CHILD_PID" 2>/dev/null || true
  fi
  rm -f "$LOCK_FILE"
}
trap cleanup EXIT INT TERM

exec 9>"$LOCK_FILE"
flock -n 9 || fail "failed to acquire test lock"
release_lock_fd 9

sleep 5 &
CHILD_PID=$!
sleep 0.1

if lsof "$LOCK_FILE" 2>/dev/null | awk '{print $2}' | grep -qx "$CHILD_PID"; then
  fail "child process should not inherit released lock fd"
fi

echo "[test-script-lock] ok"
