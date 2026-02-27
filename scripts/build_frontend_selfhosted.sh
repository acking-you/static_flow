#!/usr/bin/env bash
set -euo pipefail

# Build frontend for self-hosted mode (backend serves SPA + API on same origin).
#
# Key difference from GitHub Pages build:
#   STATICFLOW_API_BASE=/api  (relative, same-origin)
#
# Output: frontend/dist/  (ready to be served by backend via FRONTEND_DIST_DIR)
#
# Usage:
#   ./scripts/build_frontend_selfhosted.sh
#   ./scripts/build_frontend_selfhosted.sh --out /path/to/output
#   ./scripts/build_frontend_selfhosted.sh --skip-npm  # skip npm install

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$ROOT_DIR/frontend"
OUTPUT_DIR=""
SKIP_NPM="false"
NPM_CACHE_DIR="${NPM_CACHE_DIR:-$ROOT_DIR/tmp/npm-cache}"

log() { echo "[build-selfhosted] $*"; }
fail() { echo "[build-selfhosted][ERROR] $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      [[ $# -ge 2 ]] || fail "--out requires a path"
      OUTPUT_DIR="$2"; shift 2 ;;
    --skip-npm)
      SKIP_NPM="true"; shift ;;
    -h|--help)
      echo "Usage: $0 [--out <dir>] [--skip-npm]"
      echo "  --out <dir>   Copy dist to a custom directory after build"
      echo "  --skip-npm    Skip npm install (use if deps are already installed)"
      exit 0 ;;
    *) fail "Unknown option: $1" ;;
  esac
done

command -v trunk >/dev/null 2>&1 || fail "trunk not found. Install with: cargo install trunk"
[[ -d "$FRONTEND_DIR" ]] || fail "frontend directory not found: $FRONTEND_DIR"

# Ensure npm deps
if [[ "$SKIP_NPM" != "true" ]]; then
  if [[ ! -d "$FRONTEND_DIR/node_modules/@tailwindcss/cli" ]]; then
    log "Installing frontend npm dependencies..."
    mkdir -p "$NPM_CACHE_DIR"
    (cd "$FRONTEND_DIR" && NPM_CONFIG_CACHE="$NPM_CACHE_DIR" npm install)
  fi
fi

log "Building frontend for self-hosted mode (API_BASE=/api)..."

cd "$FRONTEND_DIR"
NPM_CONFIG_CACHE="$NPM_CACHE_DIR" \
STATICFLOW_API_BASE="/api" \
trunk build --release

log "Build complete: $FRONTEND_DIR/dist/"

# Optional: copy to custom output dir
if [[ -n "$OUTPUT_DIR" ]]; then
  mkdir -p "$OUTPUT_DIR"
  cp -r "$FRONTEND_DIR/dist/"* "$OUTPUT_DIR/"
  log "Copied to: $OUTPUT_DIR/"
fi

log "Done. Use FRONTEND_DIST_DIR=$FRONTEND_DIR/dist when starting backend."
