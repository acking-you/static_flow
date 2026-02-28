#!/usr/bin/env bash
set -euo pipefail

payload_path="${1:-}"
if [[ -z "${payload_path}" || ! -f "${payload_path}" ]]; then
  echo "payload file is required" >&2
  exit 1
fi

# Custom executor override (trusted, set by backend WorkerConfig)
if [[ -n "${ARTICLE_REQUEST_EXEC_COMMAND:-}" ]]; then
  exec bash -c "${ARTICLE_REQUEST_EXEC_COMMAND} \"\$1\"" -- "${payload_path}"
fi

for cmd in codex jq; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "${cmd} command not found." >&2
    exit 1
  fi
done

request_id="$(jq -r '.request_id // empty' "${payload_path}")"
if [[ -z "${request_id}" ]]; then
  echo "payload missing request_id: ${payload_path}" >&2
  exit 1
fi

# Sanitize request_id for filesystem use
safe_request_id="$(printf '%s' "${request_id}" | tr -cs 'A-Za-z0-9._-' '_')"
: "${safe_request_id:=unknown}"

skill_path="${ARTICLE_REQUEST_SKILL_PATH:-skills/external-blog-repost-publisher/SKILL.md}"
workdir="${ARTICLE_REQUEST_WORKDIR:-$(pwd)}"
codex_sandbox="${ARTICLE_REQUEST_CODEX_SANDBOX:-danger-full-access}"
codex_json_stream="${ARTICLE_REQUEST_CODEX_JSON_STREAM:-1}"
codex_bypass="${ARTICLE_REQUEST_CODEX_BYPASS:-0}"
result_dir="${ARTICLE_REQUEST_RESULT_DIR:-/tmp/staticflow-article-request-results}"
result_path="${ARTICLE_REQUEST_RESULT_PATH:-${result_dir}/request-${safe_request_id}.json}"

mkdir -p "$(dirname "${result_path}")"
rm -f "${result_path}"

tmp_prompt="$(mktemp -t staticflow-article-request-prompt.XXXXXX.txt)"
trap 'rm -f "${tmp_prompt}"' EXIT

cat > "${tmp_prompt}" <<EOF
You are a StaticFlow article ingestion worker.

INSTRUCTIONS:
1) Open and follow this skill file exactly: ${skill_path}
2) Read the task payload JSON from: ${payload_path}
3) Execute the skill workflow to fetch, process, and ingest the article.
4) Write a JSON result (UTF-8, non-empty) atomically (temp file then rename) to:
   ${result_path}
5) Result schema: { "ingested_article_id": "<id or null>", "reply_markdown": "<task_status_markdown>" }
6) \`reply_markdown\` is an operator-facing status summary; it does not relax any skill rules.

Notes:
- Backend judges success by result file content, not stdout.
- Keep stdout/stderr streaming for execution trace.
- Do not install/copy/remove skill files at runtime.
- Before starting, read any of these if present in workdir: AGENTS.md, CLAUDE.md, README.md, CONTRIBUTING.md

FOLLOW-UP CONTEXT (if applicable):
- If payload contains "parent_request_id" + "parent_context", this is a follow-up.
- "parent_context" is ordered from direct parent to oldest ancestor.
- Each entry: "request_id", "article_url", "request_message", "ai_reply", "ingested_article_id".
- Use the chain to understand cumulative intent.
- If a previous round produced an article, prefer updating it unless user asks for a new one.
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
  [[ "${codex_status}" -ne 0 ]] \
    && echo "codex exited status=${codex_status}, but result file exists: ${result_path}" >&2 \
    || echo "article request result ready: ${result_path}" >&2
  exit 0
fi

if [[ "${codex_status}" -ne 0 ]]; then
  echo "codex failed (status=${codex_status}), result file missing/empty: ${result_path}" >&2
else
  echo "codex completed but result file missing/empty: ${result_path}" >&2
fi
exit 1
