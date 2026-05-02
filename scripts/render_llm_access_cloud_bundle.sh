#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:?usage: scripts/render_llm_access_cloud_bundle.sh <out-dir>}"

mkdir -p "$OUT_DIR"
cp "$ROOT_DIR/deployment-examples/systemd/llm-access.service.template" "$OUT_DIR/llm-access.service"
cp "$ROOT_DIR/deployment-examples/systemd/llm-access-juicefs.mount.template" "$OUT_DIR/mnt-llm\\x2daccess.mount"
cp "$ROOT_DIR/deployment-examples/systemd/staticflow-wait-llm-access-state" "$OUT_DIR/staticflow-wait-llm-access-state"
cp "$ROOT_DIR/deployment-examples/caddy/llm-access-path-split.Caddyfile" "$OUT_DIR/Caddyfile"
