#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$ROOT_DIR/frontend/gpt2api-app"
NPM_CACHE_DIR="${NPM_CACHE_DIR:-$ROOT_DIR/tmp/npm-cache}"

cd "$APP_DIR"
mkdir -p "$NPM_CACHE_DIR"
if [[ ! -d node_modules ]]; then
  NPM_CONFIG_CACHE="$NPM_CACHE_DIR" npm install
fi
NPM_CONFIG_CACHE="$NPM_CACHE_DIR" npm run build
