#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_DIR="${LLM_ACCESS_RELEASE_DIR:-$SCRIPT_DIR}"
SERVICE="${LLM_ACCESS_SERVICE:-llm-access.service}"
WORKER_SERVICE="${LLM_ACCESS_USAGE_WORKER_SERVICE:-llm-access-usage-worker.service}"
USAGE_MOUNT_SERVICE="${LLM_ACCESS_USAGE_MOUNT_SERVICE:-juicefs-llm-access-usage.service}"
SERVICE_USER="${LLM_ACCESS_SERVICE_USER:-ts_user}"
SERVICE_GROUP="${LLM_ACCESS_SERVICE_GROUP:-$SERVICE_USER}"
ACTIVATE_TARGET="${LLM_ACCESS_ACTIVATE_TARGET:-both}"
INSTALL_PATH="${LLM_ACCESS_INSTALL_PATH:-/usr/local/bin/llm-access}"
WORKER_INSTALL_PATH="${LLM_ACCESS_USAGE_WORKER_INSTALL_PATH:-/usr/local/bin/llm-access-usage-worker}"
SERVICE_UNIT_INSTALL_PATH="${LLM_ACCESS_SERVICE_UNIT_INSTALL_PATH:-/etc/systemd/system/llm-access.service}"
WORKER_SERVICE_UNIT_INSTALL_PATH="${LLM_ACCESS_USAGE_WORKER_SERVICE_UNIT_INSTALL_PATH:-/etc/systemd/system/llm-access-usage-worker.service}"
USAGE_MOUNT_SERVICE_UNIT_INSTALL_PATH="${LLM_ACCESS_USAGE_MOUNT_SERVICE_UNIT_INSTALL_PATH:-/etc/systemd/system/juicefs-llm-access-usage.service}"
BACKUP_DIR="${LLM_ACCESS_BACKUP_DIR:-/usr/local/bin/staticflow-backups}"
HEALTH_URL="${LLM_ACCESS_HEALTH_URL:-http://127.0.0.1:19080/healthz}"
WORKER_HEALTH_URL="${LLM_ACCESS_USAGE_WORKER_HEALTH_URL:-http://127.0.0.1:19081/admin/llm-access/usage-worker/status}"
VERSION_URL="${LLM_ACCESS_VERSION_URL:-http://127.0.0.1:19080/version}"
JOURNAL_LINES="${JOURNAL_LINES:-80}"
NEON_ENV_PATH="${LLM_ACCESS_CONTROL_DATABASE_URL_FILE:-/mnt/llm-access/config/neon.env}"
STAGED_NEON_ENV="${LLM_ACCESS_STAGED_NEON_ENV:-$RELEASE_DIR/llm-access-neon.env.latest}"
STAGED_BIN="${1:-$RELEASE_DIR/llm-access.latest}"
STAGED_WORKER_BIN="${2:-$RELEASE_DIR/llm-access-usage-worker.latest}"
MANIFEST="${LLM_ACCESS_RELEASE_MANIFEST:-$RELEASE_DIR/release.latest.env}"
STAGED_SERVICE_UNIT="${LLM_ACCESS_STAGED_SERVICE_UNIT:-$RELEASE_DIR/llm-access.service.release}"
STAGED_WORKER_SERVICE_UNIT="${LLM_ACCESS_STAGED_WORKER_SERVICE_UNIT:-$RELEASE_DIR/llm-access-usage-worker.service.release}"
STAGED_USAGE_MOUNT_SERVICE_UNIT="${LLM_ACCESS_STAGED_USAGE_MOUNT_SERVICE_UNIT:-$RELEASE_DIR/juicefs-llm-access-usage.service.release}"

log() {
  printf '[llm-access-activate] %s\n' "$*"
}

fail() {
  printf '[llm-access-activate][ERROR] %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

manifest_value() {
  local key="$1"
  [[ -f "$MANIFEST" ]] || return 0
  sed -n "s/^${key}=//p" "$MANIFEST" | tail -n 1
}

wait_for_health() {
  local url="$1"
  local attempt
  for attempt in $(seq 1 30); do
    if curl -fsS "$url" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

ensure_mount_service() {
  local service="$1"
  local mount_path="$2"

  log "restarting mount service $service"
  sudo systemctl enable "$service"
  sudo systemctl restart "$service"
  if ! findmnt -T "$mount_path" >/dev/null; then
    sudo systemctl status "$service" --no-pager -l || true
    sudo journalctl -u "$service" -n "$JOURNAL_LINES" --no-pager -l || true
    fail "$mount_path is not mounted after enabling $service"
  fi
}

install_binary() {
  local staged="$1"
  local install_path="$2"
  local backup_path="$3"
  local expected_sha="$4"

  local tmp_install
  tmp_install="$install_path.next-$timestamp"

  log "backing up current binary to $backup_path"
  if sudo test -e "$install_path"; then
    sudo cp -a "$install_path" "$backup_path"
  else
    log "current binary does not exist at $install_path; no backup created"
  fi

  log "installing staged binary to $install_path"
  sudo install -o root -g root -m 0755 "$staged" "$tmp_install"
  sudo mv -f "$tmp_install" "$install_path"

  local installed_sha
  installed_sha="$(sudo sha256sum "$install_path" | awk '{print $1}')"
  if [[ "$installed_sha" != "$expected_sha" ]]; then
    if sudo test -e "$backup_path"; then
      sudo cp -a "$backup_path" "$install_path"
    fi
    fail "installed binary sha256 mismatch for $install_path: expected $expected_sha, got $installed_sha"
  fi
}

install_service_unit() {
  local staged="$1"
  local install_path="$2"
  local backup_path="$3"
  [[ -n "$staged" ]] || return 1
  [[ -f "$staged" ]] || fail "staged service unit not found: $staged"
  [[ -r "$staged" ]] || fail "staged service unit is not readable: $staged"
  if sudo test -e "$install_path"; then
    log "backing up current service unit to $backup_path"
    sudo cp -a "$install_path" "$backup_path"
  else
    log "current service unit does not exist at $install_path; no backup created"
  fi
  log "installing staged service unit to $install_path"
  sudo install -o root -g root -m 0644 "$staged" "$install_path"
}

restart_and_verify() {
  local service="$1"
  local url="$2"
  local backup_hint="$3"

  log "restarting $service"
  sudo systemctl restart "$service"

  if ! wait_for_health "$url"; then
    sudo systemctl status "$service" --no-pager -l || true
    sudo journalctl -u "$service" -n "$JOURNAL_LINES" --no-pager -l || true
    fail "health check failed after restarting $service; rollback hint: $backup_hint"
  fi
}

for cmd in awk curl findmnt install sed seq sha256sum sudo systemctl; do
  require_cmd "$cmd"
done

case "$ACTIVATE_TARGET" in
  api|worker|both)
    ;;
  *)
    fail "unsupported activation target: $ACTIVATE_TARGET (expected api, worker, or both)"
    ;;
esac

if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  [[ -f "$STAGED_BIN" ]] || fail "staged binary not found: $STAGED_BIN"
  [[ -r "$STAGED_BIN" ]] || fail "staged binary is not readable: $STAGED_BIN"
fi
if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  [[ -f "$STAGED_WORKER_BIN" ]] || fail "staged usage worker binary not found: $STAGED_WORKER_BIN"
  [[ -r "$STAGED_WORKER_BIN" ]] || fail "staged usage worker binary is not readable: $STAGED_WORKER_BIN"
fi

expected_sha="$(manifest_value api_sha256 || true)"
expected_sha="${expected_sha:-$(manifest_value sha256 || true)}"
actual_sha=""
if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  actual_sha="$(sha256sum "$STAGED_BIN" | awk '{print $1}')"
  if [[ -n "$expected_sha" && "$actual_sha" != "$expected_sha" ]]; then
    fail "staged binary sha256 mismatch: expected $expected_sha, got $actual_sha"
  fi
fi
expected_worker_sha="$(manifest_value usage_worker_sha256 || true)"
actual_worker_sha=""
if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  actual_worker_sha="$(sha256sum "$STAGED_WORKER_BIN" | awk '{print $1}')"
  if [[ -n "$expected_worker_sha" && "$actual_worker_sha" != "$expected_worker_sha" ]]; then
    fail "staged usage worker binary sha256 mismatch: expected $expected_worker_sha, got $actual_worker_sha"
  fi
fi

release_id="$(manifest_value release_id || true)"
git_commit="$(manifest_value git_commit || true)"

log "release_id=${release_id:-unknown}"
log "git_commit=${git_commit:-unknown}"
log "activate_target=$ACTIVATE_TARGET"
if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  log "staged_binary=$STAGED_BIN"
  log "staged_sha256=$actual_sha"
fi
if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  log "staged_usage_worker_binary=$STAGED_WORKER_BIN"
  log "staged_usage_worker_sha256=$actual_worker_sha"
fi

systemctl is-active juicefs-llm-access.service >/dev/null || fail "juicefs-llm-access.service is not active"
findmnt -T /mnt/llm-access >/dev/null || fail "/mnt/llm-access is not mounted"
sudo install -d -m 0755 "$BACKUP_DIR"
if [[ -e "$STAGED_NEON_ENV" ]]; then
  [[ -r "$STAGED_NEON_ENV" ]] || fail "staged Neon config is not readable: $STAGED_NEON_ENV"
  grep -q '^LLM_ACCESS_CONTROL_DATABASE_URL=' "$STAGED_NEON_ENV" \
    || fail "staged Neon config does not define LLM_ACCESS_CONTROL_DATABASE_URL: $STAGED_NEON_ENV"
  install -d -m 0755 "$(dirname "$NEON_ENV_PATH")"
  if sudo test -e "$NEON_ENV_PATH"; then
    log "backing up shared Neon config to $BACKUP_DIR/neon.env.preinstall"
    sudo cp -a "$NEON_ENV_PATH" "$BACKUP_DIR/neon.env.preinstall"
  fi
  log "installing staged Neon config to $NEON_ENV_PATH"
  sudo install -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0600 "$STAGED_NEON_ENV" "$NEON_ENV_PATH"
fi
sudo test -r "$NEON_ENV_PATH" || fail "missing shared Neon config: $NEON_ENV_PATH"
sudo grep -q '^LLM_ACCESS_CONTROL_DATABASE_URL=' "$NEON_ENV_PATH" \
  || fail "shared Neon config does not define LLM_ACCESS_CONTROL_DATABASE_URL: $NEON_ENV_PATH"
if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  if findmnt -T /mnt/llm-access-usage >/dev/null; then
    log "/mnt/llm-access-usage is mounted before activation"
  else
    log "/mnt/llm-access-usage is not mounted before activation; continuing with install"
  fi
fi

if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]] && systemctl is-active "$SERVICE" >/dev/null; then
  log "$SERVICE is active before activation"
  curl -fsS "$HEALTH_URL" >/dev/null || log "pre-activation API health check failed; continuing with restart"
elif [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  log "$SERVICE is not active before activation; continuing with install"
fi

if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]] && systemctl is-active "$WORKER_SERVICE" >/dev/null; then
  log "$WORKER_SERVICE is active before activation"
  curl -fsS "$WORKER_HEALTH_URL" >/dev/null || log "pre-activation usage worker health check failed; continuing with restart"
elif [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  log "$WORKER_SERVICE is not active before activation; continuing with install"
fi

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup_path="$BACKUP_DIR/llm-access.$timestamp"
worker_backup_path="$BACKUP_DIR/llm-access-usage-worker.$timestamp"
neon_env_backup_path="$BACKUP_DIR/neon.env.$timestamp"
service_unit_backup_path="$BACKUP_DIR/llm-access.service.$timestamp"
worker_service_unit_backup_path="$BACKUP_DIR/llm-access-usage-worker.service.$timestamp"
usage_mount_service_unit_backup_path="$BACKUP_DIR/juicefs-llm-access-usage.service.$timestamp"

reload_required=0
installed_sha=""
installed_worker_sha=""

if [[ -e "$STAGED_NEON_ENV" && -e "$BACKUP_DIR/neon.env.preinstall" ]]; then
  sudo mv "$BACKUP_DIR/neon.env.preinstall" "$neon_env_backup_path"
fi

if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  install_binary "$STAGED_BIN" "$INSTALL_PATH" "$backup_path" "$actual_sha"
  installed_sha="$(sudo sha256sum "$INSTALL_PATH" | awk '{print $1}')"
  if [[ -n "$STAGED_SERVICE_UNIT" ]]; then
    install_service_unit "$STAGED_SERVICE_UNIT" "$SERVICE_UNIT_INSTALL_PATH" "$service_unit_backup_path"
    reload_required=1
  fi
fi

if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  install_binary "$STAGED_WORKER_BIN" "$WORKER_INSTALL_PATH" "$worker_backup_path" "$actual_worker_sha"
  installed_worker_sha="$(sudo sha256sum "$WORKER_INSTALL_PATH" | awk '{print $1}')"
  if [[ -n "$STAGED_WORKER_SERVICE_UNIT" ]]; then
    install_service_unit "$STAGED_WORKER_SERVICE_UNIT" "$WORKER_SERVICE_UNIT_INSTALL_PATH" "$worker_service_unit_backup_path"
    reload_required=1
  fi
  if [[ -n "$STAGED_USAGE_MOUNT_SERVICE_UNIT" ]]; then
    install_service_unit "$STAGED_USAGE_MOUNT_SERVICE_UNIT" "$USAGE_MOUNT_SERVICE_UNIT_INSTALL_PATH" "$usage_mount_service_unit_backup_path"
    reload_required=1
  fi
fi

if [[ "$reload_required" == "1" ]]; then
  log "reloading systemd units"
  sudo systemctl daemon-reload
fi

if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  for stale_dropin in \
    /etc/systemd/system/llm-access.service.d/readiness.conf \
    /etc/systemd/system/llm-access.service.d/tiered-duckdb.conf \
    /etc/systemd/system/llm-access.service.d/zz-usage-journal-split.conf
  do
    if sudo test -e "$stale_dropin"; then
      log "removing stale llm-access drop-in $stale_dropin"
      sudo rm -f "$stale_dropin"
      reload_required=1
    fi
  done
fi

if [[ "$reload_required" == "1" ]]; then
  log "reloading systemd units after drop-in cleanup"
  sudo systemctl daemon-reload
fi

if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  if sudo test -e /etc/systemd/system/mnt-llm\\x2daccess\\x2dusage.mount; then
    log "disabling stale mount unit mnt-llm\\x2daccess\\x2dusage.mount"
    sudo systemctl disable --now mnt-llm\\x2daccess\\x2dusage.mount || true
    sudo rm -f /etc/systemd/system/mnt-llm\\x2daccess\\x2dusage.mount
    sudo systemctl daemon-reload
  fi
  if ! findmnt -T /mnt/llm-access-usage >/dev/null; then
    sudo install -d -o ts_user -g ts_user -m 0755 /mnt/llm-access-usage
  fi
  sudo install -d -o ts_user -g ts_user -m 0755 /var/cache/juicefs/llm-access-usage
  ensure_mount_service "$USAGE_MOUNT_SERVICE" /mnt/llm-access-usage
fi

if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  restart_and_verify "$WORKER_SERVICE" "$WORKER_HEALTH_URL" "$worker_backup_path"
fi

if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  restart_and_verify "$SERVICE" "$HEALTH_URL" "$backup_path"
fi

log "activation succeeded"
if [[ "$ACTIVATE_TARGET" == "api" || "$ACTIVATE_TARGET" == "both" ]]; then
  curl -fsS "$HEALTH_URL"
  printf '\n'
  curl -fsS "$VERSION_URL"
  printf '\n'
  systemctl show "$SERVICE" -p ActiveState -p SubState -p MainPID -p ExecMainStartTimestamp -p NRestarts --no-pager
fi
if [[ "$ACTIVATE_TARGET" == "worker" || "$ACTIVATE_TARGET" == "both" ]]; then
  curl -fsS "$WORKER_HEALTH_URL"
  printf '\n'
  systemctl show "$WORKER_SERVICE" -p ActiveState -p SubState -p MainPID -p ExecMainStartTimestamp -p NRestarts --no-pager
fi

rollback_cmd=""
if [[ "$ACTIVATE_TARGET" == "both" ]]; then
  rollback_cmd="sudo cp -a \"$backup_path\" \"$INSTALL_PATH\" && sudo cp -a \"$worker_backup_path\" \"$WORKER_INSTALL_PATH\""
  if [[ -e "$neon_env_backup_path" ]]; then
    rollback_cmd="sudo cat \"$neon_env_backup_path\" | tee \"$NEON_ENV_PATH\" >/dev/null && chmod 0600 \"$NEON_ENV_PATH\" && $rollback_cmd"
  fi
  if [[ -n "$STAGED_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo cp -a \"$service_unit_backup_path\" \"$SERVICE_UNIT_INSTALL_PATH\""
  fi
  if [[ -n "$STAGED_WORKER_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo cp -a \"$worker_service_unit_backup_path\" \"$WORKER_SERVICE_UNIT_INSTALL_PATH\""
  fi
  if [[ -n "$STAGED_USAGE_MOUNT_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo cp -a \"$usage_mount_service_unit_backup_path\" \"$USAGE_MOUNT_SERVICE_UNIT_INSTALL_PATH\""
  fi
  rollback_cmd+=" && sudo systemctl daemon-reload"
  if [[ -n "$STAGED_USAGE_MOUNT_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo systemctl enable --now \"$USAGE_MOUNT_SERVICE\""
  fi
  rollback_cmd+=" && sudo systemctl restart \"$WORKER_SERVICE\" \"$SERVICE\""
elif [[ "$ACTIVATE_TARGET" == "api" ]]; then
  rollback_cmd="sudo cp -a \"$backup_path\" \"$INSTALL_PATH\""
  if [[ -e "$neon_env_backup_path" ]]; then
    rollback_cmd="sudo cat \"$neon_env_backup_path\" | tee \"$NEON_ENV_PATH\" >/dev/null && chmod 0600 \"$NEON_ENV_PATH\" && $rollback_cmd"
  fi
  if [[ -n "$STAGED_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo cp -a \"$service_unit_backup_path\" \"$SERVICE_UNIT_INSTALL_PATH\""
  fi
  rollback_cmd+=" && sudo systemctl daemon-reload && sudo systemctl restart \"$SERVICE\""
else
  rollback_cmd="sudo cp -a \"$worker_backup_path\" \"$WORKER_INSTALL_PATH\""
  if [[ -e "$neon_env_backup_path" ]]; then
    rollback_cmd="sudo cat \"$neon_env_backup_path\" | tee \"$NEON_ENV_PATH\" >/dev/null && chmod 0600 \"$NEON_ENV_PATH\" && $rollback_cmd"
  fi
  if [[ -n "$STAGED_WORKER_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo cp -a \"$worker_service_unit_backup_path\" \"$WORKER_SERVICE_UNIT_INSTALL_PATH\""
  fi
  if [[ -n "$STAGED_USAGE_MOUNT_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo cp -a \"$usage_mount_service_unit_backup_path\" \"$USAGE_MOUNT_SERVICE_UNIT_INSTALL_PATH\""
  fi
  rollback_cmd+=" && sudo systemctl daemon-reload"
  if [[ -n "$STAGED_USAGE_MOUNT_SERVICE_UNIT" ]]; then
    rollback_cmd+=" && sudo systemctl enable --now \"$USAGE_MOUNT_SERVICE\""
  fi
  rollback_cmd+=" && sudo systemctl restart \"$WORKER_SERVICE\""
fi

cat <<EOF

Installed llm-access release:
  release_id: ${release_id:-unknown}
  git_commit: ${git_commit:-unknown}
  activate_target: $ACTIVATE_TARGET
  api_sha256: ${installed_sha:-skipped}
  api_backup: ${installed_sha:+$backup_path}
  usage_worker_sha256: ${installed_worker_sha:-skipped}
  usage_worker_backup: ${installed_worker_sha:+$worker_backup_path}
  neon_env: $NEON_ENV_PATH

Rollback command if needed:
  $rollback_cmd
EOF
