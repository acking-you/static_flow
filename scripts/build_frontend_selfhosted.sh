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
STANDALONE_SRC_DIR="$FRONTEND_DIR/standalone"
STANDALONE_DIST_DIR="$FRONTEND_DIR/dist/standalone"
OUTPUT_DIR=""
SKIP_NPM="false"
NPM_CACHE_DIR="${NPM_CACHE_DIR:-$ROOT_DIR/tmp/npm-cache}"
FRONTEND_DEFAULT_FEATURES="${FRONTEND_DEFAULT_FEATURES:-1}"
FRONTEND_FEATURES="${FRONTEND_FEATURES:-}"

log() { echo "[build-selfhosted] $*"; }
fail() { echo "[build-selfhosted][ERROR] $*" >&2; exit 1; }

# Newer trunk versions parse NO_COLOR as a boolean option and reject legacy
# numeric values like "1". Normalize common truthy values before invoking trunk.
if [[ "${NO_COLOR:-}" == "1" ]]; then
  export NO_COLOR="true"
fi

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
      echo ""
      echo "Feature env:"
      echo "  FRONTEND_DEFAULT_FEATURES=0   Build with --no-default-features"
      echo "  FRONTEND_FEATURES=<list>      Extra trunk --features list"
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
log "Building standalone GPT2API frontend..."
"$ROOT_DIR/scripts/build_gpt2api_frontend.sh"

cd "$FRONTEND_DIR"
TRUNK_ARGS=(build --release)
if [[ "$FRONTEND_DEFAULT_FEATURES" == "0" ]]; then
  TRUNK_ARGS+=(--no-default-features)
fi
if [[ -n "$FRONTEND_FEATURES" ]]; then
  TRUNK_ARGS+=(--features "$FRONTEND_FEATURES")
fi
NPM_CONFIG_CACHE="$NPM_CACHE_DIR" \
STATICFLOW_API_BASE="/api" \
trunk "${TRUNK_ARGS[@]}"

# Copy standalone pages that are linked from the SPA homepage.
rm -rf "$STANDALONE_DIST_DIR"
mkdir -p "$STANDALONE_DIST_DIR"
if [[ -d "$STANDALONE_SRC_DIR" ]]; then
  cp -r "$STANDALONE_SRC_DIR/." "$STANDALONE_DIST_DIR/"
  log "Copied standalone pages: $STANDALONE_SRC_DIR -> $STANDALONE_DIST_DIR"
else
  log "No standalone source directory found at $STANDALONE_SRC_DIR; leaving dist/standalone empty"
fi

log "Build complete: $FRONTEND_DIR/dist/"

# Optional: copy to custom output dir
if [[ -n "$OUTPUT_DIR" ]]; then
  mkdir -p "$OUTPUT_DIR"
  cp -r "$FRONTEND_DIR/dist/"* "$OUTPUT_DIR/"
  log "Copied to: $OUTPUT_DIR/"
fi

log "Done. Use FRONTEND_DIST_DIR=$FRONTEND_DIR/dist when starting backend."
