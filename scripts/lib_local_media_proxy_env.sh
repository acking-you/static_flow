#!/usr/bin/env bash

# Shared helper for backend startup scripts. The backend should not depend on
# the media service being online during startup; it only needs a stable proxy
# base URL so local-media routes start working as soon as the standalone media
# service appears on that port.

sf_apply_local_media_proxy_defaults() {
  LOCAL_MEDIA_MODE="${LOCAL_MEDIA_MODE:-enabled}"

  if [[ "$LOCAL_MEDIA_MODE" == "disabled" ]]; then
    unset STATICFLOW_MEDIA_PROXY_BASE_URL
    return 0
  fi

  local proxy_host="${STATICFLOW_MEDIA_PROXY_HOST:-127.0.0.1}"
  local proxy_port="${STATICFLOW_MEDIA_PROXY_PORT:-39085}"

  if [[ -z "${STATICFLOW_MEDIA_PROXY_BASE_URL:-}" ]]; then
    STATICFLOW_MEDIA_PROXY_BASE_URL="http://${proxy_host}:${proxy_port}"
  fi
}
