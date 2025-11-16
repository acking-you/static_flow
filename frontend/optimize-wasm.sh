#!/bin/bash
# 1. Manually run wasm-opt with bulk-memory enabled
# 2. Fix absolute paths for GitHub Pages subpath deployment
# This script is called by Trunk's post_build hook

set -e

DIST_DIR="${TRUNK_STAGING_DIR:-dist}"
WASM_OPT="${HOME}/.cache/trunk/wasm-opt-version_116/bin/wasm-opt"

# ====================
# Step 1: Optimize WASM
# ====================
echo "=== Step 1: WASM Optimization ==="

if [ ! -f "$WASM_OPT" ]; then
  echo "Warning: wasm-opt not found at $WASM_OPT, skipping optimization"
else
  # Find all .wasm files in dist
  for wasm in "$DIST_DIR"/*.wasm; do
    if [ -f "$wasm" ]; then
      echo "Optimizing $wasm with bulk-memory support..."
      temp="${wasm}.tmp"

      # Run wasm-opt, but don't fail the entire script if it errors
      if "$WASM_OPT" \
        --enable-bulk-memory \
        --enable-mutable-globals \
        --enable-nontrapping-float-to-int \
        --enable-sign-ext \
        -Oz \
        "$wasm" \
        -o "$temp" 2>&1; then
        mv "$temp" "$wasm"
        echo "✓ Optimized $(basename $wasm)"
      else
        echo "Warning: wasm-opt failed for $(basename $wasm), using unoptimized version"
        rm -f "$temp"
      fi
    fi
  done
fi

# ====================
# Step 2: Fix Paths for Subpath Deployment
# ====================
echo ""
echo "=== Step 2: Path Fixing for /static_flow/ ==="

HTML_FILE="${DIST_DIR}/index.html"

if [ ! -f "$HTML_FILE" ]; then
  echo "Warning: $HTML_FILE not found, skipping path fixes"
  exit 0
fi

echo "Fixing absolute paths in index.html..."

# Replace absolute paths with subpath-prefixed paths
# This is needed because Trunk generates absolute paths like '/xxx.js'
# but we need '/static_flow/xxx.js' for GitHub Pages subpath deployment
sed -i.bak \
  -e "s|from '/static-flow-frontend-|from '/static_flow/static-flow-frontend-|g" \
  -e "s|module_or_path: '/static-flow-frontend-|module_or_path: '/static_flow/static-flow-frontend-|g" \
  -e "s|href=\"/static-flow-frontend-|href=\"/static_flow/static-flow-frontend-|g" \
  -e "s|href=\"/styles-|href=\"/static_flow/styles-|g" \
  -e "s|href=\"/apple-touch-icon-|href=\"/static_flow/apple-touch-icon-|g" \
  -e "s|href=\"/favicon-|href=\"/static_flow/favicon-|g" \
  -e "s|src=\"/static/|src=\"/static_flow/static/|g" \
  -e "s|href=\"/static/|href=\"/static_flow/static/|g" \
  -e "s|url(/static/|url(/static_flow/static/|g" \
  "$HTML_FILE"

rm -f "${HTML_FILE}.bak"

echo "✓ Fixed paths in index.html"
echo ""
echo "=== Build Post-Processing Complete ==="
