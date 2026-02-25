#!/usr/bin/env bash
set -euo pipefail

payload_path="${1:-}"
if [[ -z "${payload_path}" || ! -f "${payload_path}" ]]; then
  echo "payload file is required" >&2
  exit 1
fi

if [[ -n "${ARTICLE_REQUEST_EXEC_COMMAND:-}" ]]; then
  eval "${ARTICLE_REQUEST_EXEC_COMMAND} \"${payload_path}\""
  exit $?
fi

if ! command -v codex >/dev/null 2>&1; then
  echo "codex command not found." >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq command not found." >&2
  exit 1
fi

sanitize_id() {
  local raw="$1"
  local safe
  safe="$(printf '%s' "${raw}" | sed -E 's/[^A-Za-z0-9._-]+/_/g')"
  if [[ -z "${safe}" ]]; then safe="unknown"; fi
  printf '%s' "${safe}"
}

request_id="$(jq -r '.request_id // empty' "${payload_path}")"
if [[ -z "${request_id}" ]]; then
  echo "payload missing request_id: ${payload_path}" >&2
  exit 1
fi

skill_path="${ARTICLE_REQUEST_SKILL_PATH:-skills/external-blog-repost-publisher/SKILL.md}"
workdir="${ARTICLE_REQUEST_WORKDIR:-$(pwd)}"
codex_sandbox="${ARTICLE_REQUEST_CODEX_SANDBOX:-danger-full-access}"
codex_json_stream="${ARTICLE_REQUEST_CODEX_JSON_STREAM:-1}"
codex_bypass="${ARTICLE_REQUEST_CODEX_BYPASS:-0}"
result_dir="${ARTICLE_REQUEST_RESULT_DIR:-/tmp/staticflow-article-request-results}"
safe_request_id="$(sanitize_id "${request_id}")"
result_path="${ARTICLE_REQUEST_RESULT_PATH:-${result_dir}/request-${safe_request_id}.json}"

mkdir -p "$(dirname "${result_path}")"
rm -f "${result_path}" >/dev/null 2>&1 || true

tmp_prompt="$(mktemp -t staticflow-article-request-prompt.XXXXXX.txt)"
cleanup() {
  rm -f "${tmp_prompt}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

cat > "${tmp_prompt}" <<EOF
You are a StaticFlow article ingestion worker.

MANDATORY:
1) Open and follow this skill file exactly: ${skill_path}
2) Read the task payload JSON from: ${payload_path}
3) Use the skill to fetch, process, and ingest the requested article into the content DB.
4) Write a JSON result to this exact local file path (UTF-8, non-empty):
   ${result_path}
5) The JSON must contain: { "ingested_article_id": "<id or null>", "reply_markdown": "<summary>" }
6) Write the result file atomically: write to a temp file then rename to target path.

Notes:
- Backend marks task success based on the result file content, not stdout JSON format.
- Keep normal Codex stdout/stderr streaming; they are used for execution trace/audit.
- Do not install/copy/remove any skill files at runtime.
- Before starting work, check if any of these files exist in the working directory
  and read them for project context: AGENTS.md, CLAUDE.md, README.md, CONTRIBUTING.md
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
ARTICLE_REQUEST_RESULT_PATH="${result_path}" \
RUST_LOG=off "${codex_cmd[@]}" < "${tmp_prompt}"
codex_status=$?
set -e

if [[ -s "${result_path}" ]]; then
  if [[ "${codex_status}" -ne 0 ]]; then
    echo "codex exited with status=${codex_status}, but result file is valid: ${result_path}" >&2
  else
    echo "article request result file ready: ${result_path}" >&2
  fi
  exit 0
fi

if [[ "${codex_status}" -ne 0 ]]; then
  echo "codex failed with status=${codex_status} and result file missing/empty: ${result_path}" >&2
else
  echo "codex completed but result file missing/empty: ${result_path}" >&2
fi
exit 1
