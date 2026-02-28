#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# ── Defaults (same as start_backend_from_tmp.sh) ──
DB_ROOT="${DB_ROOT:-/mnt/e/static-flow-data}"
DB_PATH="${DB_PATH:-${LANCEDB_URI:-$DB_ROOT/lancedb}}"
COMMENTS_DB_PATH="${COMMENTS_DB_PATH:-${COMMENTS_LANCEDB_URI:-$DB_ROOT/lancedb-comments}}"
MUSIC_DB_PATH="${MUSIC_DB_PATH:-${MUSIC_LANCEDB_URI:-$DB_ROOT/lancedb-music}}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-39080}"
HEAPTRACK_OUTPUT_DIR="${HEAPTRACK_OUTPUT_DIR:-$ROOT_DIR/tmp/heaptrack}"

log() { echo "[heaptrack-backend] $*"; }
fail() { echo "[heaptrack-backend][ERROR] $*" >&2; exit 1; }

# ── Pre-flight checks ──
command -v heaptrack >/dev/null 2>&1 || fail "heaptrack not found. Install: sudo apt install heaptrack heaptrack-gui"

BACKEND_BIN=""
if [[ -x "$ROOT_DIR/target/release-backend/static-flow-backend" ]]; then
  BACKEND_BIN="$ROOT_DIR/target/release-backend/static-flow-backend"
elif [[ -x "$ROOT_DIR/bin/static-flow-backend" ]]; then
  BACKEND_BIN="$ROOT_DIR/bin/static-flow-backend"
else
  fail "Backend binary not found. Run: cargo build --profile release-backend -p static-flow-backend"
fi

# Warn if binary is stripped (heaptrack needs symbols)
if file "$BACKEND_BIN" | grep -q "stripped"; then
  log "WARNING: binary is stripped — heaptrack will show raw addresses instead of function names."
  log "Rebuild with: cargo build --profile release-backend -p static-flow-backend"
fi

mkdir -p "$HEAPTRACK_OUTPUT_DIR"
[[ -d "$DB_PATH" ]] || fail "DB path not found: $DB_PATH"
mkdir -p "$COMMENTS_DB_PATH" "$MUSIC_DB_PATH"

# ── Usage ──
usage() {
  cat <<'EOF'
Usage: ./scripts/heaptrack_backend.sh [--attach PID] [--duration SECONDS]

Modes:
  (default)        Launch backend under heaptrack (LD_PRELOAD injection)
  --attach PID     Attach to a running backend process
  --duration SECS  Auto-stop after N seconds (default: run until Ctrl+C)

Environment variables (same as start_backend_from_tmp.sh):
  DB_ROOT, DB_PATH, COMMENTS_DB_PATH, MUSIC_DB_PATH, HOST, PORT
  HEAPTRACK_OUTPUT_DIR  Output directory (default: tmp/heaptrack/)

Examples:
  ./scripts/heaptrack_backend.sh                    # launch + profile
  ./scripts/heaptrack_backend.sh --duration 60      # profile for 60s
  ./scripts/heaptrack_backend.sh --attach $(pgrep -f static-flow-backend)

After profiling, analyze with:
  heaptrack_gui tmp/heaptrack/heaptrack.static-flow-backend.*.zst
  heaptrack_print tmp/heaptrack/heaptrack.static-flow-backend.*.zst
EOF
  exit 0
}

ATTACH_PID=""
DURATION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --attach)  ATTACH_PID="$2"; shift 2 ;;
    --duration) DURATION="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) fail "Unknown option: $1. Use --help for usage." ;;
  esac
done

# ── Mode: attach to running process ──
if [[ -n "$ATTACH_PID" ]]; then
  PTRACE_SCOPE=$(cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null || echo "?")
  if [[ "$PTRACE_SCOPE" != "0" ]]; then
    log "ptrace_scope=$PTRACE_SCOPE — attach requires scope=0."
    log "Run: sudo sysctl kernel.yama.ptrace_scope=0"
    fail "Cannot attach with current ptrace_scope."
  fi
  log "Attaching to PID $ATTACH_PID ..."
  log "Press Ctrl+C to stop profiling and generate report."
  heaptrack --output "$HEAPTRACK_OUTPUT_DIR/heaptrack.attach" --pid "$ATTACH_PID"
  log "Done. Output in $HEAPTRACK_OUTPUT_DIR/"
  exit 0
fi

# ── Mode: launch backend under heaptrack ──
log "Binary: $BACKEND_BIN"
log "DB: content=$DB_PATH comments=$COMMENTS_DB_PATH music=$MUSIC_DB_PATH"
log "Listen: $HOST:$PORT"
log "Output: $HEAPTRACK_OUTPUT_DIR/"
log ""

# Note: heaptrack uses LD_PRELOAD to hook libc malloc/free.
# With mimalloc as #[global_allocator], heaptrack sees mimalloc's
# mmap/munmap calls to the OS, not individual Rust-level allocations.
# This is still useful for tracking large OS-level memory requests.
log "NOTE: mimalloc bypasses libc malloc — heaptrack tracks OS-level"
log "      mmap/munmap from mimalloc, not individual Rust allocations."
log "      For Rust-level profiling, use the built-in memory profiler"
log "      (MEM_PROF_ENABLED=1) or switch to jemalloc + jeprof."
log ""

cleanup() {
  if [[ -n "${BACKEND_PID:-}" ]] && kill -0 "$BACKEND_PID" 2>/dev/null; then
    log "Stopping backend (pid=$BACKEND_PID)..."
    kill "$BACKEND_PID" 2>/dev/null || true
    wait "$BACKEND_PID" 2>/dev/null || true
  fi
  log "Heaptrack output in $HEAPTRACK_OUTPUT_DIR/"
  log "Analyze: heaptrack_gui $HEAPTRACK_OUTPUT_DIR/heaptrack.static-flow-backend.*.zst"
}
trap cleanup EXIT INT TERM

if [[ -n "$DURATION" ]]; then
  log "Will auto-stop after ${DURATION}s"
fi

RUST_ENV="development" \
BIND_ADDR="$HOST" \
PORT="$PORT" \
LANCEDB_URI="$DB_PATH" \
COMMENTS_LANCEDB_URI="$COMMENTS_DB_PATH" \
MUSIC_LANCEDB_URI="$MUSIC_DB_PATH" \
MEM_PROF_ENABLED="${MEM_PROF_ENABLED:-0}" \
heaptrack --output "$HEAPTRACK_OUTPUT_DIR/heaptrack" "$BACKEND_BIN" &
BACKEND_PID=$!

if [[ -n "$DURATION" ]]; then
  sleep "$DURATION"
  log "Duration reached (${DURATION}s), stopping..."
  kill "$BACKEND_PID" 2>/dev/null || true
  wait "$BACKEND_PID" 2>/dev/null || true
else
  log "Press Ctrl+C to stop profiling."
  wait "$BACKEND_PID" 2>/dev/null || true
fi
