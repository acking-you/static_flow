#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCAL_SCRIPT="$ROOT_DIR/scripts/prepare_llm_access_cloud_release.sh"
REMOTE_SCRIPT="$ROOT_DIR/scripts/activate_llm_access_cloud_release.sh"
API_ONLY_SCRIPT="$ROOT_DIR/scripts/release_llm_access_cloud_api_only.sh"
WORKER_ONLY_SCRIPT="$ROOT_DIR/scripts/release_llm_access_cloud_worker_only.sh"
CONFIG_EXAMPLE="$ROOT_DIR/conf/llm-access-cloud-release.env.example"

for script in "$LOCAL_SCRIPT" "$REMOTE_SCRIPT" "$API_ONLY_SCRIPT" "$WORKER_ONLY_SCRIPT"; do
  test -x "$script"
  bash -n "$script"
done

test -s "$CONFIG_EXAMPLE"

if command -v shellcheck >/dev/null 2>&1; then
  shellcheck "$LOCAL_SCRIPT" "$REMOTE_SCRIPT" "$API_ONLY_SCRIPT" "$WORKER_ONLY_SCRIPT"
fi

grep -F 'CARGO_TARGET_DIR' "$LOCAL_SCRIPT" >/dev/null
grep -F 'cargo test -p llm-usage-journal -p llm-access-core -p llm-access-store -p llm-access' "$LOCAL_SCRIPT" >/dev/null
grep -F 'cargo clippy -p llm-usage-journal -p llm-access-core -p llm-access-store -p llm-access' "$LOCAL_SCRIPT" >/dev/null
grep -F 'cargo build -p llm-access --release' "$LOCAL_SCRIPT" >/dev/null
grep -F 'render_llm_access_cloud_bundle.sh' "$LOCAL_SCRIPT" >/dev/null
grep -F 'scp ' "$LOCAL_SCRIPT" >/dev/null
grep -F 'llm-access.latest' "$LOCAL_SCRIPT" >/dev/null
grep -F '.local/llm-access-cloud-release-aws.env' "$LOCAL_SCRIPT" >/dev/null
grep -F '.local/llm-access-cloud-release.env' "$LOCAL_SCRIPT" >/dev/null
grep -F '.local/llm-access-cloud-release-aws.env' "$API_ONLY_SCRIPT" >/dev/null
grep -F '.local/llm-access-cloud-release.env' "$API_ONLY_SCRIPT" >/dev/null
grep -F '.local/llm-access-cloud-release-aws.env' "$WORKER_ONLY_SCRIPT" >/dev/null
grep -F '.local/llm-access-cloud-release.env' "$WORKER_ONLY_SCRIPT" >/dev/null
grep -F 'source "$CONFIG_FILE"' "$LOCAL_SCRIPT" >/dev/null
grep -F 'KIRO_THINKING_SIGNATURE_SECRET' "$LOCAL_SCRIPT" >/dev/null
! grep -F 'pgrep' "$LOCAL_SCRIPT" >/dev/null
! grep -F 'another Rust/frontend build appears to be running' "$LOCAL_SCRIPT" >/dev/null
! grep -F 'GCP_HOST="${GCP_HOST:-' "$LOCAL_SCRIPT" >/dev/null
! grep -F 'GCP_SSH_KEY="${GCP_SSH_KEY:-' "$LOCAL_SCRIPT" >/dev/null
! grep -F 'REMOTE_RELEASE_DIR="${REMOTE_RELEASE_DIR:-' "$LOCAL_SCRIPT" >/dev/null

grep -F 'sudo mv -f' "$REMOTE_SCRIPT" >/dev/null
grep -F 'systemctl restart' "$REMOTE_SCRIPT" >/dev/null
grep -F 'http://127.0.0.1:19080/healthz' "$REMOTE_SCRIPT" >/dev/null
grep -F 'KIRO_THINKING_SIGNATURE_SECRET' "$REMOTE_SCRIPT" >/dev/null
grep -F 'LLM_ACCESS_ACTIVATE_TARGET=api' "$API_ONLY_SCRIPT" >/dev/null
grep -F 'LLM_ACCESS_STAGED_SERVICE_UNIT=' "$API_ONLY_SCRIPT" >/dev/null
grep -F 'render_llm_access_cloud_bundle.sh' "$API_ONLY_SCRIPT" >/dev/null
grep -F 'LLM_ACCESS_ACTIVATE_TARGET=worker' "$WORKER_ONLY_SCRIPT" >/dev/null
grep -F 'LLM_ACCESS_STAGED_WORKER_SERVICE_UNIT=' "$WORKER_ONLY_SCRIPT" >/dev/null
grep -F 'render_llm_access_cloud_bundle.sh' "$WORKER_ONLY_SCRIPT" >/dev/null

grep -F 'GCP_HOST=' "$CONFIG_EXAMPLE" >/dev/null
grep -F 'GCP_SSH_KEY=' "$CONFIG_EXAMPLE" >/dev/null
grep -F 'REMOTE_RELEASE_DIR=' "$CONFIG_EXAMPLE" >/dev/null
grep -F 'KIRO_THINKING_SIGNATURE_SECRET' "$CONFIG_EXAMPLE" >/dev/null
grep -F 'PB_MAPPER_RELAY_ADDR=' "$CONFIG_EXAMPLE" >/dev/null
grep -F 'PB_MAPPER_LOCAL_RELAY_ADDR=' "$CONFIG_EXAMPLE" >/dev/null
grep -F 'VALKEY_SSH_TARGET=' "$CONFIG_EXAMPLE" >/dev/null
