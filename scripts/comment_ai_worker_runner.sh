#!/usr/bin/env bash
set -euo pipefail

payload_path="${1:-}"
if [[ -z "${payload_path}" || ! -f "${payload_path}" ]]; then
  echo "payload file is required" >&2
  exit 1
fi

# Preferred: user provides an explicit runner command.
# The payload path will be appended as the final argument.
if [[ -n "${COMMENT_AI_EXEC_COMMAND:-}" ]]; then
  eval "${COMMENT_AI_EXEC_COMMAND} \"${payload_path}\""
  exit $?
fi

if ! command -v codex >/dev/null 2>&1; then
  echo "codex command not found. Install/configure Codex on this backend host." >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq command not found. Install jq because runner requires payload parsing." >&2
  exit 1
fi

sanitize_task_id() {
  local raw="$1"
  local safe
  safe="$(printf '%s' "${raw}" | sed -E 's/[^A-Za-z0-9._-]+/_/g')"
  if [[ -z "${safe}" ]]; then
    safe="unknown-task"
  fi
  printf '%s' "${safe}"
}

task_id="$(jq -r '.task_id // empty' "${payload_path}")"
if [[ -z "${task_id}" ]]; then
  echo "payload missing task_id: ${payload_path}" >&2
  exit 1
fi

skill_path="${COMMENT_AI_SKILL_PATH:-skills/comment-review-ai-responder/SKILL.md}"
workdir="${COMMENT_AI_WORKDIR:-$(pwd)}"
codex_sandbox="${COMMENT_AI_CODEX_SANDBOX:-danger-full-access}"
codex_json_stream="${COMMENT_AI_CODEX_JSON_STREAM:-1}"
codex_bypass="${COMMENT_AI_CODEX_BYPASS:-0}"
result_dir="${COMMENT_AI_RESULT_DIR:-/tmp/staticflow-comment-results}"
safe_task_id="$(sanitize_task_id "${task_id}")"
result_path="${COMMENT_AI_RESULT_PATH:-${result_dir}/task-${safe_task_id}.md}"

mkdir -p "$(dirname "${result_path}")"
rm -f "${result_path}" >/dev/null 2>&1 || true

tmp_prompt="$(mktemp -t staticflow-comment-prompt.XXXXXX.txt)"
cleanup() {
  rm -f "${tmp_prompt}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

cat > "${tmp_prompt}" <<EOF
You are a StaticFlow comment review worker.

MANDATORY:
1) Open and follow this skill file exactly: ${skill_path}
2) Read the task payload JSON from: ${payload_path}
3) Use sf-cli and payload fields as specified by the skill.
4) Write FINAL markdown reply to this exact local file path (UTF-8, non-empty):
   ${result_path}
5) Write the result file atomically: write to a temp file then rename to target path.

Notes:
- Backend marks task success based on the result file content, not stdout JSON format.
- Keep normal Codex stdout/stderr streaming; they are used for execution trace/audit.
- If the answer is uncertain, say uncertainty inside the final markdown content.
- Fetch article context via local HTTP API first, fallback to sf-cli only when HTTP fails.
- When using sf-cli fallback, read content-only fields ('content' or 'content_en') instead of full row.
- Do not install/copy/remove any skill files at runtime.
EOF

codex_cmd=(
  codex exec
  --skip-git-repo-check
  --cd "${workdir}"
  --ephemeral
)

if [[ "${codex_bypass}" == "1" ]]; then
  codex_cmd+=(--dangerously-bypass-approvals-and-sandbox)
else
  codex_cmd+=(--sandbox "${codex_sandbox}")
fi

if [[ "${codex_json_stream}" == "1" ]]; then
  codex_cmd+=(--json)
fi

codex_cmd+=(-)

set +e
COMMENT_AI_RESULT_PATH="${result_path}" \
RUST_LOG=off "${codex_cmd[@]}" < "${tmp_prompt}"
codex_status=$?
set -e

if [[ -s "${result_path}" ]]; then
  if [[ "${codex_status}" -ne 0 ]]; then
    echo "codex exited with status=${codex_status}, but result file is valid: ${result_path}" >&2
  else
    echo "comment result file ready: ${result_path}" >&2
  fi
  exit 0
fi

if [[ "${codex_status}" -ne 0 ]]; then
  echo "codex failed with status=${codex_status} and result file missing/empty: ${result_path}" >&2
else
  echo "codex completed but result file missing/empty: ${result_path}" >&2
fi
exit 1
