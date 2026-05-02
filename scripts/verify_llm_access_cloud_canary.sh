#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-https://ackingliu.top}"
LLM_HEALTH_URL="${LLM_HEALTH_URL:-$BASE_URL/api/llm-gateway/status}"
STATICFLOW_HEALTH_URL="${STATICFLOW_HEALTH_URL:-$BASE_URL/api/healthz}"

curl_common=(
  -o /dev/null
  -sS
  -w 'code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n'
)

echo "[llm-access] health"
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl "${curl_common[@]}" "$LLM_HEALTH_URL"

echo "[staticflow] health"
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl "${curl_common[@]}" "$STATICFLOW_HEALTH_URL"

echo "[routing] non-llm article API should still be reachable"
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl "${curl_common[@]}" "$BASE_URL/api/articles"
