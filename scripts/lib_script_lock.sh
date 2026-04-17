#!/usr/bin/env bash
set -euo pipefail

release_lock_fd() {
  local fd="${1:-9}"
  eval "flock -u ${fd} 2>/dev/null || true"
  eval "exec ${fd}>&-"
}
