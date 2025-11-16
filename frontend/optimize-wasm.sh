#!/bin/bash
# Manually run wasm-opt with bulk-memory enabled
# This script is called by Trunk's post_build hook

set -e

DIST_DIR="${TRUNK_STAGING_DIR:-dist}"
WASM_OPT="${HOME}/.cache/trunk/wasm-opt-version_116/bin/wasm-opt"

# Find all .wasm files in dist
for wasm in "$DIST_DIR"/*.wasm; do
  if [ -f "$wasm" ]; then
    echo "Optimizing $wasm with bulk-memory support..."
    temp="${wasm}.tmp"
    "$WASM_OPT" \
      --enable-bulk-memory \
      --enable-mutable-globals \
      --enable-nontrapping-float-to-int \
      --enable-sign-ext \
      -Oz \
      "$wasm" \
      -o "$temp"
    mv "$temp" "$wasm"
    echo "✓ Optimized $(basename $wasm)"
  fi
done
