#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

DB_PATH="${DB_PATH:-$ROOT_DIR/tmp/cli-e2e-run/lancedb}"
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
  log "This backend is read-only API (all routes are GET)."
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
    echo "[3) GET /api/articles/:id/related]"
    echo "- ${base}/api/articles/${article_id}/related"
  else
    echo
    echo "[2) GET /api/articles/:id]"
    echo "- ${base}/api/articles/<article_id>"

    echo
    echo "[3) GET /api/articles/:id/related]"
    echo "- ${base}/api/articles/<article_id>/related"
  fi

  echo
  echo "[4) GET /api/tags]"
  echo "- ${base}/api/tags"

  echo
  echo "[5) GET /api/categories]"
  echo "- ${base}/api/categories"

  echo
  echo "[6) GET /api/search?q=]"
  echo "- ${base}/api/search?q=Mermaid"
  echo "- ${base}/api/search?q=%E5%9B%BE%E8%A1%A8"

  echo
  echo "[7) GET /api/semantic-search?q=]"
  echo "- ${base}/api/semantic-search?q=%E5%89%8D%E7%AB%AF%20%E6%B8%B2%E6%9F%93"
  echo "- ${base}/api/semantic-search?q=%E5%89%8D%E7%AB%AF%20%E6%B8%B2%E6%9F%93&enhanced_highlight=true"

  echo
  echo "[8) GET /api/images]"
  echo "- ${base}/api/images"

  if [[ -n "$image_id" ]]; then
    echo
    echo "[9) GET /api/images/:id-or-filename]"
    echo "- ${base}/api/images/${image_id}"
    echo "- ${base}/api/images/${image_id}?thumb=true"
    if [[ -n "$image_name" ]]; then
      echo "- ${base}/api/images/${image_name}"
      echo "- ${base}/api/images/${image_name}?thumb=true"
    fi

    echo
    echo "[10) GET /api/image-search?id=]"
    echo "- ${base}/api/image-search?id=${image_id}"
  else
    echo
    echo "[9) GET /api/images/:id-or-filename]"
    echo "- ${base}/api/images/<image_id_or_filename>"
    echo "- ${base}/api/images/<image_id_or_filename>?thumb=true"

    echo
    echo "[10) GET /api/image-search?id=]"
    echo "- ${base}/api/image-search?id=<image_id>"
  fi

  echo
  log "Tip: image endpoint returns binary; open URL directly in browser or use curl --output."
  log "Press Ctrl+C to stop backend."
}

[[ -d "$DB_PATH" ]] || fail "DB path not found: $DB_PATH. Run ./scripts/test_cli_e2e.sh first."

PORT_CHOSEN="$(choose_port)"
BACKEND_BIN_PATH="$(resolve_backend_bin)"

log "Using DB_PATH=$DB_PATH"
log "Using BACKEND_BIN=$BACKEND_BIN_PATH"
log "Using HOST=$HOST PORT=$PORT_CHOSEN"

RUST_ENV="development" \
BIND_ADDR="$HOST" \
PORT="$PORT_CHOSEN" \
LANCEDB_URI="$DB_PATH" \
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
