#!/usr/bin/env bash
set -euo pipefail

payload_path="${1:-}"
if [[ -z "${payload_path}" || ! -f "${payload_path}" ]]; then
  echo "payload file is required" >&2
  exit 1
fi

# Custom executor override (trusted, set by backend WorkerConfig)
if [[ -n "${COMMENT_AI_EXEC_COMMAND:-}" ]]; then
  exec bash -c "${COMMENT_AI_EXEC_COMMAND} \"\$1\"" -- "${payload_path}"
fi

for cmd in codex jq; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "${cmd} command not found." >&2
    exit 1
  fi
done

task_id="$(jq -r '.task_id // empty' "${payload_path}")"
if [[ -z "${task_id}" ]]; then
  echo "payload missing task_id: ${payload_path}" >&2
  exit 1
fi

# Sanitize task_id for filesystem use
safe_task_id="$(printf '%s' "${task_id}" | tr -cs 'A-Za-z0-9._-' '_')"
: "${safe_task_id:=unknown}"

skill_path="${COMMENT_AI_SKILL_PATH:-skills/comment-review-ai-responder/SKILL.md}"
workdir="${COMMENT_AI_WORKDIR:-$(pwd)}"
codex_sandbox="${COMMENT_AI_CODEX_SANDBOX:-danger-full-access}"
codex_json_stream="${COMMENT_AI_CODEX_JSON_STREAM:-1}"
codex_bypass="${COMMENT_AI_CODEX_BYPASS:-0}"
result_dir="${COMMENT_AI_RESULT_DIR:-/tmp/staticflow-comment-results}"
result_path="${COMMENT_AI_RESULT_PATH:-${result_dir}/task-${safe_task_id}.md}"

mkdir -p "$(dirname "${result_path}")"
rm -f "${result_path}"

tmp_prompt="$(mktemp -t staticflow-comment-prompt.XXXXXX.txt)"
trap 'rm -f "${tmp_prompt}"' EXIT

cat > "${tmp_prompt}" <<EOF
You are a StaticFlow comment review worker.

INSTRUCTIONS:
1) Open and follow this skill file exactly: ${skill_path}
2) Read the task payload JSON from: ${payload_path}
3) Use sf-cli and payload fields as specified by the skill.
4) Write the final markdown reply (UTF-8, non-empty) atomically (temp file then rename) to:
   ${result_path}

Notes:
- Backend judges success by result file content, not stdout.
- Keep stdout/stderr streaming for execution trace.
- If uncertain, express uncertainty in the final markdown content.
- Fetch article context via local HTTP API first; fall back to sf-cli only when HTTP fails.
- When using sf-cli fallback, read content-only fields (content/content_en) instead of full row.
- Do not install/copy/remove skill files at runtime.
- Before starting, read any of these if present in workdir: AGENTS.md, CLAUDE.md, README.md, CONTRIBUTING.md
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
  [[ "${codex_status}" -ne 0 ]] \
    && echo "codex exited status=${codex_status}, but result file exists: ${result_path}" >&2 \
    || echo "comment result ready: ${result_path}" >&2
  exit 0
fi

if [[ "${codex_status}" -ne 0 ]]; then
  echo "codex failed (status=${codex_status}), result file missing/empty: ${result_path}" >&2
else
  echo "codex completed but result file missing/empty: ${result_path}" >&2
fi
exit 1
