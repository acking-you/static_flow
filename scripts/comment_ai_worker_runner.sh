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

skill_path="${COMMENT_AI_SKILL_PATH:-skills/comment-review-ai-responder/SKILL.md}"
workdir="${COMMENT_AI_WORKDIR:-$(pwd)}"
codex_sandbox="${COMMENT_AI_CODEX_SANDBOX:-danger-full-access}"
codex_json_stream="${COMMENT_AI_CODEX_JSON_STREAM:-1}"
codex_bypass="${COMMENT_AI_CODEX_BYPASS:-0}"

tmp_schema="$(mktemp -t staticflow-comment-schema.XXXXXX.json)"
tmp_prompt="$(mktemp -t staticflow-comment-prompt.XXXXXX.txt)"
tmp_output="$(mktemp -t staticflow-comment-output.XXXXXX.json)"

cleanup() {
  rm -f "${tmp_schema}" "${tmp_prompt}" "${tmp_output}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

cat > "${tmp_schema}" <<'JSON'
{
  "type": "object",
  "properties": {
    "final_reply_markdown": { "type": "string" }
  },
  "required": ["final_reply_markdown"],
  "additionalProperties": false
}
JSON

cat > "${tmp_prompt}" <<EOF
You are a StaticFlow comment review worker.

MANDATORY:
1) Open and follow this skill file exactly: ${skill_path}
2) Read the task payload JSON from: ${payload_path}
3) Use sf-cli and payload fields as specified by the skill.
4) Return ONLY one JSON object that matches the required schema.

Notes:
- The backend parses stdout as JSON.
- Do not output markdown fences.
- If the answer is uncertain, say uncertainty inside final_reply_markdown.
- Fetch article context via local HTTP API first, fallback to sf-cli only when HTTP fails.
- When using sf-cli fallback, read content-only fields ('content' or 'content_en') instead of full row.
- Do not install/copy/remove any skill files at runtime.
EOF

codex_cmd=(
  codex exec
  --skip-git-repo-check
  --cd "${workdir}"
  --ephemeral
  --output-schema "${tmp_schema}"
  --output-last-message "${tmp_output}"
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

if ! RUST_LOG=off "${codex_cmd[@]}" < "${tmp_prompt}" 1>&2; then
  echo "codex exec failed for payload=${payload_path}. skill=${skill_path}" >&2
  exit 1
fi

if [[ ! -s "${tmp_output}" ]]; then
  echo "codex exec succeeded but produced empty output file: ${tmp_output}" >&2
  exit 1
fi

cat "${tmp_output}"
