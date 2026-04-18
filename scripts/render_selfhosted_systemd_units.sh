#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TEMPLATE_DIR="$ROOT_DIR/deployment-examples/systemd"
UNIT_DIR=""
WORKDIR="$ROOT_DIR"
COMMON_ENV=""
GATEWAY_ENV=""
BACKEND_ENV_PATTERN=""
UNIT_PREFIX="staticflow"
DESCRIPTION_PREFIX="StaticFlow"

log() { echo "[systemd-render] $*"; }
fail() { echo "[systemd-render][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/render_selfhosted_systemd_units.sh [options]

Options:
  --unit-dir <dir>              Output directory for rendered unit files
  --workdir <dir>               StaticFlow checkout/workdir path
  --common-env <path>           Shared EnvironmentFile path
  --gateway-env <path>          Gateway EnvironmentFile path
  --backend-env-pattern <path>  Backend slot EnvironmentFile pattern, e.g. /etc/staticflow/backend-slot-%i.env
  --unit-prefix <name>          Unit file prefix (default: staticflow)
  --description-prefix <text>   Human-readable description prefix (default: StaticFlow)
  -h, --help                    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --unit-dir)
      [[ $# -ge 2 ]] || fail "--unit-dir requires a value"
      UNIT_DIR="$2"
      shift 2
      ;;
    --workdir)
      [[ $# -ge 2 ]] || fail "--workdir requires a value"
      WORKDIR="$2"
      shift 2
      ;;
    --common-env)
      [[ $# -ge 2 ]] || fail "--common-env requires a value"
      COMMON_ENV="$2"
      shift 2
      ;;
    --gateway-env)
      [[ $# -ge 2 ]] || fail "--gateway-env requires a value"
      GATEWAY_ENV="$2"
      shift 2
      ;;
    --backend-env-pattern)
      [[ $# -ge 2 ]] || fail "--backend-env-pattern requires a value"
      BACKEND_ENV_PATTERN="$2"
      shift 2
      ;;
    --unit-prefix)
      [[ $# -ge 2 ]] || fail "--unit-prefix requires a value"
      UNIT_PREFIX="$2"
      shift 2
      ;;
    --description-prefix)
      [[ $# -ge 2 ]] || fail "--description-prefix requires a value"
      DESCRIPTION_PREFIX="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

[[ -n "$UNIT_DIR" ]] || fail "--unit-dir is required"
[[ -n "$COMMON_ENV" ]] || fail "--common-env is required"
[[ -n "$GATEWAY_ENV" ]] || fail "--gateway-env is required"
[[ -n "$BACKEND_ENV_PATTERN" ]] || fail "--backend-env-pattern is required"
[[ -f "$TEMPLATE_DIR/staticflow-gateway.service.template" ]] || fail "missing gateway service template"
[[ -f "$TEMPLATE_DIR/staticflow-backend-slot@.service.template" ]] || fail "missing backend slot service template"

mkdir -p "$UNIT_DIR"

render_template() {
  local template_file="$1"
  local output_file="$2"
  sed \
    -e "s|@WORKDIR@|$WORKDIR|g" \
    -e "s|@COMMON_ENV@|$COMMON_ENV|g" \
    -e "s|@GATEWAY_ENV@|$GATEWAY_ENV|g" \
    -e "s|@BACKEND_ENV_PATTERN@|$BACKEND_ENV_PATTERN|g" \
    -e "s|@DESCRIPTION_PREFIX@|$DESCRIPTION_PREFIX|g" \
    "$template_file" >"$output_file"
}

gateway_unit="$UNIT_DIR/${UNIT_PREFIX}-gateway.service"
backend_unit="$UNIT_DIR/${UNIT_PREFIX}-backend-slot@.service"

render_template \
  "$TEMPLATE_DIR/staticflow-gateway.service.template" \
  "$gateway_unit"
render_template \
  "$TEMPLATE_DIR/staticflow-backend-slot@.service.template" \
  "$backend_unit"

log "rendered $gateway_unit"
log "rendered $backend_unit"
