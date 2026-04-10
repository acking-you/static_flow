#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JUICEFS_BIN="${JUICEFS_BIN:-$ROOT_DIR/bin/juicefs}"
JUICEFS_ROOT="${JUICEFS_ROOT:-$ROOT_DIR/tmp/juicefs-cos-sqlite}"
JUICEFS_META_PATH="${JUICEFS_META_PATH:-$JUICEFS_ROOT/meta/juicefs.db}"
JUICEFS_META_URL="${JUICEFS_META_URL:-sqlite3://$JUICEFS_META_PATH}"
JUICEFS_FS_NAME="${JUICEFS_FS_NAME:-staticflow-cos-test}"
JUICEFS_STORAGE="${JUICEFS_STORAGE:-cos}"
JUICEFS_BUCKET_URL="${JUICEFS_BUCKET_URL:-}"
JUICEFS_MOUNT_DIR="${JUICEFS_MOUNT_DIR:-$JUICEFS_ROOT/mnt}"
JUICEFS_CACHE_DIR="${JUICEFS_CACHE_DIR:-$JUICEFS_ROOT/cache}"
JUICEFS_LOG_FILE="${JUICEFS_LOG_FILE:-$JUICEFS_ROOT/log/juicefs.log}"
JUICEFS_CACHE_SIZE_MB="${JUICEFS_CACHE_SIZE_MB:-1024}"

FORCE_FORMAT="false"
ONLY_UMOUNT="false"

log() { echo "[juicefs-run] $*"; }
fail() { echo "[juicefs-run][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: ./scripts/run_juicefs_cos_sqlite.sh [--force-format] [--umount]

Environment variables:
  JUICEFS_BIN            Built JuiceFS binary (default: ./bin/juicefs)
  JUICEFS_ROOT           Working root for meta/cache/log/mount
  JUICEFS_META_PATH      SQLite metadata DB path
  JUICEFS_META_URL       Metadata URL override (default: sqlite3://$JUICEFS_META_PATH)
  JUICEFS_FS_NAME        Volume name used during format
  JUICEFS_STORAGE        Object storage type (default: cos)
  JUICEFS_BUCKET_URL     Required bucket URL, e.g. https://bucket.cos.ap-guangzhou.myqcloud.com
  JUICEFS_ACCESS_KEY     Required object storage access key (or ACCESS_KEY)
  JUICEFS_SECRET_KEY     Required object storage secret key (or SECRET_KEY)
  JUICEFS_MOUNT_DIR      Mount point
  JUICEFS_CACHE_DIR      Local cache directory
  JUICEFS_LOG_FILE       Background mount log file
  JUICEFS_CACHE_SIZE_MB  Disk cache size in MiB (default: 1024)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --force-format) FORCE_FORMAT="true"; shift ;;
    --umount) ONLY_UMOUNT="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1 (use --help)" ;;
  esac
done

[[ -x "$JUICEFS_BIN" ]] || fail "JuiceFS binary not found or not executable: $JUICEFS_BIN"
mkdir -p "$(dirname "$JUICEFS_META_PATH")" "$JUICEFS_MOUNT_DIR" "$JUICEFS_CACHE_DIR" "$(dirname "$JUICEFS_LOG_FILE")"

if [[ "$ONLY_UMOUNT" == "true" ]]; then
  if mountpoint -q "$JUICEFS_MOUNT_DIR"; then
    "$JUICEFS_BIN" umount "$JUICEFS_MOUNT_DIR"
    log "Unmounted $JUICEFS_MOUNT_DIR"
  else
    log "Mount point is not active: $JUICEFS_MOUNT_DIR"
  fi
  exit 0
fi

ACCESS_KEY="${JUICEFS_ACCESS_KEY:-${ACCESS_KEY:-}}"
SECRET_KEY="${JUICEFS_SECRET_KEY:-${SECRET_KEY:-}}"
[[ -n "$JUICEFS_BUCKET_URL" ]] || fail "JUICEFS_BUCKET_URL is required"
[[ -n "$ACCESS_KEY" ]] || fail "JUICEFS_ACCESS_KEY or ACCESS_KEY is required"
[[ -n "$SECRET_KEY" ]] || fail "JUICEFS_SECRET_KEY or SECRET_KEY is required"

if mountpoint -q "$JUICEFS_MOUNT_DIR"; then
  log "Mount point is already active: $JUICEFS_MOUNT_DIR"
  "$JUICEFS_BIN" status "$JUICEFS_META_URL"
  exit 0
fi

if [[ "$FORCE_FORMAT" == "true" || ! -f "$JUICEFS_META_PATH" ]]; then
  log "Formatting volume $JUICEFS_FS_NAME with $JUICEFS_STORAGE + sqlite metadata..."
  ACCESS_KEY="$ACCESS_KEY" SECRET_KEY="$SECRET_KEY" \
    "$JUICEFS_BIN" format \
      --storage "$JUICEFS_STORAGE" \
      --bucket "$JUICEFS_BUCKET_URL" \
      --trash-days 0 \
      "$JUICEFS_META_URL" \
      "$JUICEFS_FS_NAME"
else
  log "Reusing existing metadata at $JUICEFS_META_PATH"
fi

log "Mounting $JUICEFS_META_URL at $JUICEFS_MOUNT_DIR ..."
"$JUICEFS_BIN" mount \
  --cache-dir "$JUICEFS_CACHE_DIR" \
  --cache-size "$JUICEFS_CACHE_SIZE_MB" \
  --log "$JUICEFS_LOG_FILE" \
  --no-syslog \
  --no-usage-report \
  -d \
  "$JUICEFS_META_URL" \
  "$JUICEFS_MOUNT_DIR"

log "Mounted successfully. Status:"
"$JUICEFS_BIN" status "$JUICEFS_META_URL"
