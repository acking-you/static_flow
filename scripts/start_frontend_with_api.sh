#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$ROOT_DIR/frontend"

BACKEND_HOST="${BACKEND_HOST:-127.0.0.1}"
BACKEND_PORT="${BACKEND_PORT:-39080}"
API_BASE="${API_BASE:-http://${BACKEND_HOST}:${BACKEND_PORT}/api}"
FRONTEND_PORT="${FRONTEND_PORT:-38080}"
OPEN_BROWSER="${OPEN_BROWSER:-false}"
TRUNK_EXTRA_ARGS=()

# Use project-local npm cache by default so script stays robust even when
# ~/.npm contains root-owned leftovers from old npm/npx runs.
NPM_CACHE_DIR="${NPM_CACHE_DIR:-$ROOT_DIR/tmp/npm-cache}"

log() {
  echo "[start-frontend] $*"
}

fail() {
  echo "[start-frontend][ERROR] $*" >&2
  exit 1
}

usage() {
  cat <<'EOF_USAGE'
Usage:
  ./scripts/start_frontend_with_api.sh [options] [-- <extra trunk args>]

Options:
  --api-base <url>   Backend API base URL (default: http://127.0.0.1:39080/api)
  --port <port>      Frontend serve port (default: 38080)
  --open             Open browser automatically
  -h, --help         Show this help

Environment variables (optional):
  API_BASE=<url>
  BACKEND_HOST=<host>
  BACKEND_PORT=<port>
  HOST=<host> / PORT=<port> / PORT_BASE=<port> (compatible with backend script vars)
  FRONTEND_PORT=<port>
  OPEN_BROWSER=true|false
  NPM_CACHE_DIR=<dir> (default: ./tmp/npm-cache, avoid ~/.npm permission issues)

Examples:
  ./scripts/start_frontend_with_api.sh --open   # defaults to backend script port (39080)
  ./scripts/start_frontend_with_api.sh --api-base "https://<cloud-host>:8888/api" --port 38123
  ./scripts/start_frontend_with_api.sh --api-base "https://api.example.com/api" -- --no-autoreload

Notes:
  - Default API base follows backend script port convention: `http://127.0.0.1:${PORT_BASE:-39080}/api`.
  - This script sets STATICFLOW_API_BASE before running `trunk serve`.
  - This script also sets NPM_CONFIG_CACHE to a project-local directory by default.
  - Missing npm deps are auto-installed (checks @tailwindcss/cli + tailwindcss).
  - STATICFLOW_API_BASE is read at compile time in frontend code. If you change API URL,
    restart this script so trunk rebuilds with the new value.
EOF_USAGE
}

is_port_busy() {
  local port="$1"
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
}

validate_api_base() {
  local api_base="$1"
  if [[ "$api_base" != http://* && "$api_base" != https://* ]]; then
    fail "--api-base must start with http:// or https://, got: $api_base"
  fi

  if [[ "$api_base" != */api ]]; then
    log "Warning: --api-base does not end with '/api': $api_base"
    log "         frontend usually expects a base like .../api"
  fi
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --api-base)
        [[ $# -ge 2 ]] || fail "--api-base requires a value"
        API_BASE="$2"
        shift 2
        ;;
      --port)
        [[ $# -ge 2 ]] || fail "--port requires a value"
        FRONTEND_PORT="$2"
        shift 2
        ;;
      --open)
        OPEN_BROWSER="true"
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      --)
        shift
        TRUNK_EXTRA_ARGS=("$@")
        break
        ;;
      *)
        fail "Unknown option: $1 (use --help)"
        ;;
    esac
  done
}

parse_args "$@"

command -v trunk >/dev/null 2>&1 || fail "trunk not found. Install with: cargo install trunk"
command -v npm >/dev/null 2>&1 || fail "npm not found. Please install Node.js + npm first"
[[ -d "$FRONTEND_DIR" ]] || fail "frontend directory not found: $FRONTEND_DIR"

validate_api_base "$API_BASE"

if ! [[ "$FRONTEND_PORT" =~ ^[0-9]+$ ]] || (( FRONTEND_PORT < 1 || FRONTEND_PORT > 65535 )); then
  fail "Invalid --port: $FRONTEND_PORT"
fi

if is_port_busy "$FRONTEND_PORT"; then
  fail "Port $FRONTEND_PORT is already in use. Please choose another port."
fi

mkdir -p "$NPM_CACHE_DIR"

ensure_frontend_deps() {
  local need_install="false"

  if [[ ! -d "$FRONTEND_DIR/node_modules" ]]; then
    need_install="true"
  elif [[ ! -d "$FRONTEND_DIR/node_modules/@tailwindcss/cli" ]]; then
    need_install="true"
  elif [[ ! -d "$FRONTEND_DIR/node_modules/tailwindcss" ]]; then
    need_install="true"
  fi

  if [[ "$need_install" == "true" ]]; then
    log "Installing frontend npm dependencies (this may take a while)..."
    (
      cd "$FRONTEND_DIR"
      NPM_CONFIG_CACHE="$NPM_CACHE_DIR" npm install
    )
  fi
}

ensure_frontend_deps

TRUNK_CMD=(trunk serve --release --port "$FRONTEND_PORT" --dist dist-dev)
if [[ "$OPEN_BROWSER" == "true" ]]; then
  TRUNK_CMD+=(--open)
fi
if [[ ${#TRUNK_EXTRA_ARGS[@]} -gt 0 ]]; then
  TRUNK_CMD+=("${TRUNK_EXTRA_ARGS[@]}")
fi

if [[ "${NO_COLOR:-}" == "1" ]]; then
  export NO_COLOR="true"
fi

log "Project root: $ROOT_DIR"
log "Frontend dir: $FRONTEND_DIR"
log "Using STATICFLOW_API_BASE=$API_BASE"
log "Using NPM cache: $NPM_CACHE_DIR"
log "Frontend URL: http://127.0.0.1:$FRONTEND_PORT"
log "Tip: press Ctrl+C to stop frontend"

cd "$FRONTEND_DIR"
NPM_CONFIG_CACHE="$NPM_CACHE_DIR" \
STATICFLOW_API_BASE="$API_BASE" \
"${TRUNK_CMD[@]}"
