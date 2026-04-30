#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-"$ROOT_DIR/tmp/llm-access-cloud-bundle-test"}"

rm -rf "$OUT_DIR"
"$ROOT_DIR/scripts/render_llm_access_cloud_bundle.sh" "$OUT_DIR"

test -s "$OUT_DIR/llm-access.service"
test -s "$OUT_DIR/mnt-llm\\x2daccess.mount"
test -s "$OUT_DIR/Caddyfile"

grep -F 'ExecStart=/usr/local/bin/llm-access serve' "$OUT_DIR/llm-access.service"
grep -F 'RequiresMountsFor=/mnt/llm-access' "$OUT_DIR/llm-access.service"
grep -F '@llm_access path /v1/* /cc/v1/* /api/llm-gateway/* /api/kiro-gateway/* /api/codex-gateway/* /api/llm-access/*' "$OUT_DIR/Caddyfile"
grep -F 'handle @llm_access' "$OUT_DIR/Caddyfile"
grep -F 'reverse_proxy 127.0.0.1:19080' "$OUT_DIR/Caddyfile"
grep -F 'reverse_proxy 127.0.0.1:39080' "$OUT_DIR/Caddyfile"
