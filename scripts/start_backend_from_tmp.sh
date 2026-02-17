#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# DB path resolution (highest priority to lowest):
# 1) explicit DB_PATH / COMMENTS_DB_PATH
# 2) pre-exported backend env LANCEDB_URI / COMMENTS_LANCEDB_URI
# 3) DB_ROOT + fixed subdirs (lancedb, lancedb-comments)
# 4) built-in tmp default
DB_ROOT="${DB_ROOT:-$ROOT_DIR/tmp/cli-e2e-run}"
DB_PATH="${DB_PATH:-${LANCEDB_URI:-$DB_ROOT/lancedb}}"
COMMENTS_DB_PATH="${COMMENTS_DB_PATH:-${COMMENTS_LANCEDB_URI:-$DB_ROOT/lancedb-comments}}"
HOST="${HOST:-127.0.0.1}"
PORT_BASE="${PORT_BASE:-39080}"
PORT_SCAN_LIMIT="${PORT_SCAN_LIMIT:-120}"

log() {
  echo "[start-backend] $*"
}

fail() {
  echo "[start-backend][ERROR] $*" >&2
  exit 1
}

is_port_busy() {
  local port="$1"
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
}

choose_port() {
  if [[ -n "${PORT:-}" ]]; then
    if is_port_busy "$PORT"; then
      fail "PORT=$PORT is already in use. Please export another high port (e.g. 39123)."
    fi
    echo "$PORT"
    return
  fi

  local port
  for ((port=PORT_BASE; port<PORT_BASE+PORT_SCAN_LIMIT; port++)); do
    if ! is_port_busy "$port"; then
      echo "$port"
      return
    fi
  done

  fail "No free high port found in [$PORT_BASE, $((PORT_BASE + PORT_SCAN_LIMIT - 1))]."
}

resolve_backend_bin() {
  if [[ -n "${BACKEND_BIN:-}" && -x "${BACKEND_BIN}" ]]; then
    echo "$BACKEND_BIN"
    return
  fi

  if [[ -x "$ROOT_DIR/target/debug/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-backend"
    return
  fi

  if [[ -x "$ROOT_DIR/target/release/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/release/static-flow-backend"
    return
  fi

  log "Backend binary not found, building debug binary..."
  cargo build -p static-flow-backend >/dev/null

  if [[ -x "$ROOT_DIR/target/debug/static-flow-backend" ]]; then
    echo "$ROOT_DIR/target/debug/static-flow-backend"
    return
  fi

  fail "Failed to build/find static-flow-backend binary."
}

wait_backend_ready() {
  local host="$1"
  local port="$2"

  for _ in $(seq 1 80); do
    if curl -fsS "http://${host}:${port}/api/articles" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done

  return 1
}

print_check_urls() {
  local host="$1"
  local port="$2"
  local base="http://${host}:${port}"
  local trend_day
  trend_day="$(date +%F 2>/dev/null || echo "2026-02-16")"

  local article_id=""
  local image_id=""
  local image_name=""

  if command -v jq >/dev/null 2>&1; then
    article_id="$(curl -fsS "${base}/api/articles" | jq -r '.articles[0].id // empty' || true)"
    image_id="$(curl -fsS "${base}/api/images" | jq -r '.images[0].id // empty' || true)"
    image_name="$(curl -fsS "${base}/api/images" | jq -r '.images[0].filename // empty' || true)"
  fi

  echo
  log "Backend is ready."
  log "Public API routes are under /api; local admin runtime config is under /admin."
  echo
  log "Manual verification URLs (ALL backend routes):"

  echo
  echo "[1) GET /api/articles]"
  echo "- ${base}/api/articles"
  echo "- ${base}/api/articles?tag=mermaid"
  echo "- ${base}/api/articles?category=Web"

  if [[ -n "$article_id" ]]; then
    echo
    echo "[2) GET /api/articles/:id]"
    echo "- ${base}/api/articles/${article_id}"

    echo
    echo "[3) GET /api/articles/:id/raw/:lang]"
    echo "- ${base}/api/articles/${article_id}/raw/zh"
    echo "- ${base}/api/articles/${article_id}/raw/en"

    echo
    echo "[4) POST /api/articles/:id/view]"
    echo "- curl -X POST \"${base}/api/articles/${article_id}/view\""

    echo
    echo "[5) GET /api/articles/:id/view-trend]"
    echo "- ${base}/api/articles/${article_id}/view-trend"
    echo "- ${base}/api/articles/${article_id}/view-trend?granularity=day"
    echo "- ${base}/api/articles/${article_id}/view-trend?granularity=hour&day=${trend_day}"

    echo
    echo "[6) GET /api/articles/:id/related]"
    echo "- ${base}/api/articles/${article_id}/related"
  else
    echo
    echo "[2) GET /api/articles/:id / GET /api/articles/:id/raw/:lang / POST /api/articles/:id/view / GET /api/articles/:id/view-trend]"
    echo "- ${base}/api/articles/<article_id>"
    echo "- ${base}/api/articles/<article_id>/raw/zh"
    echo "- ${base}/api/articles/<article_id>/raw/en"
    echo "- curl -X POST \"${base}/api/articles/<article_id>/view\""
    echo "- ${base}/api/articles/<article_id>/view-trend?granularity=day"
    echo "- ${base}/api/articles/<article_id>/view-trend?granularity=hour&day=${trend_day}"

    echo
    echo "[3) GET /api/articles/:id/related]"
    echo "- ${base}/api/articles/<article_id>/related"
  fi

  echo
  echo "[6) GET /api/tags]"
  echo "- ${base}/api/tags"

  echo
  echo "[7) GET /api/categories]"
  echo "- ${base}/api/categories"

  echo
  echo "[8) GET /api/stats]"
  echo "- ${base}/api/stats"

  echo
  echo "[9) GET /api/search?q=]"
  echo "- ${base}/api/search?q=Mermaid"
  echo "- ${base}/api/search?q=%E5%9B%BE%E8%A1%A8"

  echo
  echo "[10) GET /api/semantic-search?q=]"
  echo "- ${base}/api/semantic-search?q=%E5%89%8D%E7%AB%AF%20%E6%B8%B2%E6%9F%93"
  echo "- ${base}/api/semantic-search?q=%E5%89%8D%E7%AB%AF%20%E6%B8%B2%E6%9F%93&enhanced_highlight=true"

  echo
  echo "[11) GET /api/images]"
  echo "- ${base}/api/images"

  if [[ -n "$image_id" ]]; then
    echo
    echo "[12) GET /api/images/:id-or-filename]"
    echo "- ${base}/api/images/${image_id}"
    echo "- ${base}/api/images/${image_id}?thumb=true"
    if [[ -n "$image_name" ]]; then
      echo "- ${base}/api/images/${image_name}"
      echo "- ${base}/api/images/${image_name}?thumb=true"
    fi

    echo
    echo "[13) GET /api/image-search?id=]"
    echo "- ${base}/api/image-search?id=${image_id}"
  else
    echo
    echo "[12) GET /api/images/:id-or-filename]"
    echo "- ${base}/api/images/<image_id_or_filename>"
    echo "- ${base}/api/images/<image_id_or_filename>?thumb=true"

    echo
    echo "[13) GET /api/image-search?id=]"
    echo "- ${base}/api/image-search?id=<image_id>"
  fi

  echo
  echo "[14) GET /api/image-search-text?q=]"
  echo "- ${base}/api/image-search-text?q=system%20architecture"

  echo
  echo "[15) GET /admin/view-analytics-config (local admin)]"
  echo "- ${base}/admin/view-analytics-config"

  echo
  echo "[16) POST /admin/view-analytics-config (local admin)]"
  echo "- curl -X POST \"${base}/admin/view-analytics-config\" -H \"Content-Type: application/json\" -d '{\"dedupe_window_seconds\":60,\"trend_default_days\":30,\"trend_max_days\":180}'"

  echo
  echo "[17) GET /api/comments/list + /api/comments/stats]"
  if [[ -n "$article_id" ]]; then
    echo "- ${base}/api/comments/list?article_id=${article_id}"
    echo "- ${base}/api/comments/stats?article_id=${article_id}"
  else
    echo "- ${base}/api/comments/list?article_id=<article_id>"
    echo "- ${base}/api/comments/stats?article_id=<article_id>"
  fi

  echo
  echo "[18) POST /api/comments/submit]"
  if [[ -n "$article_id" ]]; then
    echo "- curl -X POST \"${base}/api/comments/submit\" -H \"Content-Type: application/json\" -d '{\"article_id\":\"${article_id}\",\"entry_type\":\"footer\",\"comment_text\":\"示例评论\"}'"
  else
    echo "- curl -X POST \"${base}/api/comments/submit\" -H \"Content-Type: application/json\" -d '{\"article_id\":\"<article_id>\",\"entry_type\":\"footer\",\"comment_text\":\"示例评论\"}'"
  fi

  echo
  echo "[19) GET/POST /admin/comment-config (local admin)]"
  echo "- ${base}/admin/comment-config"
  echo "- curl -X POST \"${base}/admin/comment-config\" -H \"Content-Type: application/json\" -d '{\"submit_rate_limit_seconds\":60,\"list_default_limit\":20,\"cleanup_retention_days\":-1}'"

  echo
  echo "[20) Comment moderation admin routes (local admin)]"
  echo "- ${base}/admin/comments/tasks"
  echo "- ${base}/admin/comments/tasks/grouped"
  echo "- curl -X POST \"${base}/admin/comments/tasks/<task_id>/approve\" -H \"Content-Type: application/json\" -d '{\"operator\":\"ops\"}'"
  echo "- curl -X POST \"${base}/admin/comments/tasks/<task_id>/approve-and-run\" -H \"Content-Type: application/json\" -d '{\"operator\":\"ops\"}'"
  echo "- curl -X POST \"${base}/admin/comments/tasks/<task_id>/reject\" -H \"Content-Type: application/json\" -d '{\"operator\":\"ops\"}'"
  echo "- curl -X DELETE \"${base}/admin/comments/tasks/<task_id>\" -H \"Content-Type: application/json\" -d '{\"operator\":\"ops\"}'"
  echo "- ${base}/admin/comments/tasks/<task_id>/ai-output"
  echo "- ${base}/admin/comments/ai-runs?task_id=<task_id>"
  echo "- ${base}/admin/comments/published"
  echo "- ${base}/admin/comments/audit-logs"
  echo "- curl -X POST \"${base}/admin/comments/cleanup\" -H \"Content-Type: application/json\" -d '{\"status\":\"failed\",\"retention_days\":30}'"

  echo
  echo "[21) GET /admin/geoip/status (local admin)]"
  echo "- ${base}/admin/geoip/status"

  echo
  log "Tip: image endpoint returns binary; open URL directly in browser or use curl --output."
  log "Tip: /admin routes are intended for local/ops usage and should not be publicly exposed."
  log "Press Ctrl+C to stop backend."
}

[[ -d "$DB_PATH" ]] || fail "DB path not found: $DB_PATH. Run ./scripts/test_cli_e2e.sh first, or set DB_ROOT/DB_PATH to an existing content DB."
mkdir -p "$COMMENTS_DB_PATH"

PORT_CHOSEN="$(choose_port)"
BACKEND_BIN_PATH="$(resolve_backend_bin)"
COMMENT_AI_CONTENT_API_BASE_EFFECTIVE="${COMMENT_AI_CONTENT_API_BASE:-http://${HOST}:${PORT_CHOSEN}/api}"
COMMENT_AI_CODEX_SANDBOX_EFFECTIVE="${COMMENT_AI_CODEX_SANDBOX:-danger-full-access}"
COMMENT_AI_CODEX_JSON_STREAM_EFFECTIVE="${COMMENT_AI_CODEX_JSON_STREAM:-1}"
COMMENT_AI_CODEX_BYPASS_EFFECTIVE="${COMMENT_AI_CODEX_BYPASS:-0}"
COMMENT_AI_RESULT_DIR_EFFECTIVE="${COMMENT_AI_RESULT_DIR:-/tmp/staticflow-comment-results}"
COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS_EFFECTIVE="${COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS:-1}"

log "Using DB_ROOT=$DB_ROOT"
log "Using CONTENT_DB_PATH=$DB_PATH"
log "Using COMMENTS_DB_PATH=$COMMENTS_DB_PATH"
log "Using BACKEND_BIN=$BACKEND_BIN_PATH"
log "Using HOST=$HOST PORT=$PORT_CHOSEN"
log "Comment AI env: COMMENT_AI_CONTENT_API_BASE=$COMMENT_AI_CONTENT_API_BASE_EFFECTIVE COMMENT_AI_CODEX_SANDBOX=$COMMENT_AI_CODEX_SANDBOX_EFFECTIVE COMMENT_AI_CODEX_JSON_STREAM=$COMMENT_AI_CODEX_JSON_STREAM_EFFECTIVE COMMENT_AI_CODEX_BYPASS=$COMMENT_AI_CODEX_BYPASS_EFFECTIVE COMMENT_AI_RESULT_DIR=$COMMENT_AI_RESULT_DIR_EFFECTIVE COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS=$COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS_EFFECTIVE"
log "GeoIP env passthrough: GEOIP_DB_PATH=${GEOIP_DB_PATH:-<default>} ENABLE_GEOIP_AUTO_DOWNLOAD=${ENABLE_GEOIP_AUTO_DOWNLOAD:-<default>} ENABLE_GEOIP_FALLBACK_API=${ENABLE_GEOIP_FALLBACK_API:-<default>} GEOIP_PROXY_URL=${GEOIP_PROXY_URL:-<none>}"

RUST_ENV="development" \
BIND_ADDR="$HOST" \
PORT="$PORT_CHOSEN" \
LANCEDB_URI="$DB_PATH" \
COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH" \
COMMENT_AI_CONTENT_API_BASE="$COMMENT_AI_CONTENT_API_BASE_EFFECTIVE" \
COMMENT_AI_CODEX_SANDBOX="$COMMENT_AI_CODEX_SANDBOX_EFFECTIVE" \
COMMENT_AI_CODEX_JSON_STREAM="$COMMENT_AI_CODEX_JSON_STREAM_EFFECTIVE" \
COMMENT_AI_CODEX_BYPASS="$COMMENT_AI_CODEX_BYPASS_EFFECTIVE" \
COMMENT_AI_RESULT_DIR="$COMMENT_AI_RESULT_DIR_EFFECTIVE" \
COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS="$COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS_EFFECTIVE" \
"$BACKEND_BIN_PATH" &
BACKEND_PID=$!

cleanup() {
  if kill -0 "$BACKEND_PID" >/dev/null 2>&1; then
    log "Stopping backend (pid=$BACKEND_PID)..."
    kill "$BACKEND_PID" >/dev/null 2>&1 || true
    wait "$BACKEND_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

if ! wait_backend_ready "$HOST" "$PORT_CHOSEN"; then
  fail "Backend failed to become ready: http://${HOST}:${PORT_CHOSEN}/api/articles"
fi

print_check_urls "$HOST" "$PORT_CHOSEN"
wait "$BACKEND_PID"
