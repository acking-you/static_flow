#!/usr/bin/env bash

sf_apply_media_service_defaults() {
  STATICFLOW_LOCAL_MEDIA_ROOT="${STATICFLOW_LOCAL_MEDIA_ROOT:-/mnt/e/videos/static}"
  STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG="${STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG:-1}"
}

sf_media_service_health_url() {
  local host="$1"
  local port="$2"
  local limit="${3:-1}"
  printf 'http://%s:%s/internal/local-media/list?limit=%s\n' "$host" "$port" "$limit"
}

sf_wait_media_service_ready() {
  local host="$1"
  local port="$2"
  local attempts="${3:-80}"
  local sleep_seconds="${4:-0.25}"
  local url

  url="$(sf_media_service_health_url "$host" "$port" 1)"

  for _ in $(seq 1 "$attempts"); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$sleep_seconds"
  done

  return 1
}
