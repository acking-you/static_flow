#!/usr/bin/env bash
set -euo pipefail

LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$LIB_DIR/lib_port_process.sh"

upgrade_json_field() {
  local field="$1"
  python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field"
}

healthz_json_field() {
  local url="$1"
  local field="$2"
  curl -fsS "$url" | upgrade_json_field "$field"
}
