#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

export BACKEND_DEFAULT_FEATURES=0
export BACKEND_FEATURES="${BACKEND_FEATURES:-}"
export BACKEND_BIN_NAME="${BACKEND_BIN_NAME:-static-flow-backend-no-media}"
export LOCAL_MEDIA_MODE=disabled
unset STATICFLOW_MEDIA_PROXY_BASE_URL

exec "$ROOT_DIR/scripts/start_backend_from_tmp.sh" "$@"
