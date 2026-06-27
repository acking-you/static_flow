#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_FILE=""
REMOTE_SCRIPT_NAME="activate_llm_access_cloud_release.sh"
RENDER_DIR="$(mktemp -d "$ROOT_DIR/tmp/llm-access-cloud-worker-only.XXXXXX")"

cleanup() {
  rm -rf "$RENDER_DIR"
}
trap cleanup EXIT

fail() {
  printf '[llm-access-release-worker][ERROR] %s\n' "$*" >&2
  exit 1
}

expand_path() {
  case "$1" in
    "~")
      printf '%s\n' "$HOME"
      ;;
    "~/"*)
      printf '%s/%s\n' "$HOME" "${1#"~/"}"
      ;;
    *)
      printf '%s\n' "$1"
      ;;
  esac
}

default_config_file() {
  local preferred="$ROOT_DIR/.local/llm-access-cloud-release-aws.env"
  local legacy="$ROOT_DIR/.local/llm-access-cloud-release.env"
  if [[ -n "${LLM_ACCESS_CLOUD_RELEASE_CONFIG:-}" ]]; then
    printf '%s\n' "$LLM_ACCESS_CLOUD_RELEASE_CONFIG"
  elif [[ -r "$preferred" ]]; then
    printf '%s\n' "$preferred"
  else
    printf '%s\n' "$legacy"
  fi
}

require_var() {
  local name="$1"
  [[ -n "${!name:-}" ]] || fail "missing required config value: $name in $CONFIG_FILE"
}

CONFIG_FILE="$(default_config_file)"
[[ -r "$CONFIG_FILE" ]] || fail "missing config file: $CONFIG_FILE"
# shellcheck source=/dev/null
source "$CONFIG_FILE"

require_var GCP_SSH_KEY
require_var REMOTE_RELEASE_DIR
if [[ -z "${GCP_DEST:-}" ]]; then
  require_var GCP_USER
  require_var GCP_HOST
  GCP_DEST="$GCP_USER@$GCP_HOST"
fi

GCP_SSH_KEY="$(expand_path "$GCP_SSH_KEY")"
SSH_OPTS=(-i "$GCP_SSH_KEY" -o IdentitiesOnly=yes -o BatchMode=yes)

LLM_ACCESS_ACTIVATE_TARGET=worker "$ROOT_DIR/scripts/prepare_llm_access_cloud_release.sh"
"$ROOT_DIR/scripts/render_llm_access_cloud_bundle.sh" "$RENDER_DIR"

scp "${SSH_OPTS[@]}" \
  "$RENDER_DIR/llm-access-usage-worker.service" \
  "$GCP_DEST:$REMOTE_RELEASE_DIR/llm-access-usage-worker.service.release"
scp "${SSH_OPTS[@]}" \
  "$RENDER_DIR/juicefs-llm-access-usage.service" \
  "$GCP_DEST:$REMOTE_RELEASE_DIR/juicefs-llm-access-usage.service.release"

ssh "${SSH_OPTS[@]}" "$GCP_DEST" \
  "LLM_ACCESS_ACTIVATE_TARGET=worker LLM_ACCESS_STAGED_WORKER_SERVICE_UNIT=$REMOTE_RELEASE_DIR/llm-access-usage-worker.service.release LLM_ACCESS_STAGED_USAGE_MOUNT_SERVICE_UNIT=$REMOTE_RELEASE_DIR/juicefs-llm-access-usage.service.release $REMOTE_RELEASE_DIR/$REMOTE_SCRIPT_NAME"
