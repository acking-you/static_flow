#!/bin/bash
# 1. Manually run wasm-opt with bulk-memory enabled
# 2. Inject <base> tag for GitHub Pages subpath deployment
# This script is called in GitHub Actions workflow

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
# Step 2: Inject <base> tag for GitHub Pages subpath
# ====================
echo ""
echo "=== Step 2: Injecting <base> tag for subpath deployment ==="

HTML_FILE="${DIST_DIR}/index.html"

if [ ! -f "$HTML_FILE" ]; then
  echo "Warning: $HTML_FILE not found, skipping base tag injection"
  exit 0
fi

echo "Injecting <base href=\"/static_flow/\"> into index.html..."

# Inject <base> tag right after <head> opening tag
# This tells the browser that all relative URLs should be resolved relative to /static_flow/
sed -i.bak \
  's|<head>|<head>\n    <base href="/static_flow/">|' \
  "$HTML_FILE"

rm -f "${HTML_FILE}.bak"

echo "✓ Injected <base> tag into index.html"
echo ""
echo "=== Build Post-Processing Complete ==="
