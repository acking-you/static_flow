#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JUICEFS_DIR="${JUICEFS_DIR:-$ROOT_DIR/deps/juicefs}"
JUICEFS_BIN_OUT="${JUICEFS_BIN_OUT:-$ROOT_DIR/bin/juicefs}"

log() { echo "[juicefs-build] $*"; }
fail() { echo "[juicefs-build][ERROR] $*" >&2; exit 1; }

[[ -d "$JUICEFS_DIR" ]] || fail "JuiceFS source not found: $JUICEFS_DIR"
mkdir -p "$(dirname "$JUICEFS_BIN_OUT")"

log "Building release binary from ${JUICEFS_DIR#$ROOT_DIR/} ..."
make juicefs -C "$JUICEFS_DIR"
install -m 0755 "$JUICEFS_DIR/juicefs" "$JUICEFS_BIN_OUT"
log "Binary ready: $JUICEFS_BIN_OUT"
