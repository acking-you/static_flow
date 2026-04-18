#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

OUTPUT_DIR="${OUTPUT_DIR:-$ROOT_DIR/tmp/systemd-release/current}"
FRONTEND_DIST_SOURCE="${FRONTEND_DIST_SOURCE:-$ROOT_DIR/frontend/dist}"
BUILD_MODE="${BUILD_MODE:-if-missing}"
COPY_FRONTEND="true"
BACKEND_DEFAULT_FEATURES="${BACKEND_DEFAULT_FEATURES:-1}"
BACKEND_FEATURES="${BACKEND_FEATURES:-}"

log() { echo "[systemd-bundle] $*"; }
fail() { echo "[systemd-bundle][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/prepare_selfhosted_systemd_bundle.sh [options]

Options:
  --output-dir <dir>     Bundle output directory
  --rebuild              Force rebuilding backend and gateway binaries
  --skip-frontend        Do not copy frontend/dist into the bundle
  -h, --help             Show this help

Environment variables:
  OUTPUT_DIR             Bundle output directory
  FRONTEND_DIST_SOURCE   Frontend dist source (default: ./frontend/dist)
  BUILD_MODE             always|if-missing (default: if-missing)
  BACKEND_DEFAULT_FEATURES
  BACKEND_FEATURES
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output-dir)
      [[ $# -ge 2 ]] || fail "--output-dir requires a value"
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --rebuild)
      BUILD_MODE="always"
      shift
      ;;
    --skip-frontend)
      COPY_FRONTEND="false"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

backend_bin="$ROOT_DIR/bin/static-flow-backend"
gateway_bin="$ROOT_DIR/target/release-backend/staticflow-pingora-gateway"

if [[ "$BUILD_MODE" == "always" || ! -x "$backend_bin" ]]; then
  log "building backend release binary via make bin-backend"
  BACKEND_DEFAULT_FEATURES="$BACKEND_DEFAULT_FEATURES" \
  BACKEND_FEATURES="$BACKEND_FEATURES" \
    make bin-backend >/dev/null
fi

if [[ "$BUILD_MODE" == "always" || ! -x "$gateway_bin" ]]; then
  log "building gateway release binary via cargo"
  cargo build -p staticflow-pingora-gateway --profile release-backend >/dev/null
fi

[[ -x "$backend_bin" ]] || fail "missing backend binary: $backend_bin"
[[ -x "$gateway_bin" ]] || fail "missing gateway binary: $gateway_bin"

tmp_dir="$(mktemp -d "$ROOT_DIR/tmp/systemd-bundle.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

mkdir -p "$tmp_dir/bin" "$tmp_dir/conf/pingora"

cp "$backend_bin" "$tmp_dir/bin/static-flow-backend"
cp "$gateway_bin" "$tmp_dir/bin/staticflow-pingora-gateway"
cp "$ROOT_DIR/conf/pingora/staticflow-gateway.yaml.template" \
  "$tmp_dir/conf/pingora/staticflow-gateway.yaml.template"

if [[ "$COPY_FRONTEND" == "true" ]]; then
  [[ -f "$FRONTEND_DIST_SOURCE/index.html" ]] \
    || fail "frontend dist not found: $FRONTEND_DIST_SOURCE/index.html"
  mkdir -p "$tmp_dir/frontend/dist"
  cp -a "$FRONTEND_DIST_SOURCE"/. "$tmp_dir/frontend/dist/"
fi

mkdir -p "$(dirname "$OUTPUT_DIR")"
rm -rf "$OUTPUT_DIR"
mv "$tmp_dir" "$OUTPUT_DIR"
trap - EXIT

log "bundle ready under $OUTPUT_DIR"
log "backend_bin=$OUTPUT_DIR/bin/static-flow-backend"
log "gateway_bin=$OUTPUT_DIR/bin/staticflow-pingora-gateway"
if [[ "$COPY_FRONTEND" == "true" ]]; then
  log "frontend_dist=$OUTPUT_DIR/frontend/dist"
fi
