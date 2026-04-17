#!/usr/bin/env bash
set -euo pipefail

listener_pid_for_port() {
  local port="$1"

  if command -v lsof >/dev/null 2>&1; then
    lsof -t -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -n 1
    return 0
  fi

  ss -tlnp 2>/dev/null | awk -v port="$port" '
    $4 ~ ":" port "$" {
      if (match($0, /pid=[0-9]+/)) {
        print substr($0, RSTART + 4, RLENGTH - 4)
        exit
      }
    }
  '
}
