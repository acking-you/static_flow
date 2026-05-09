#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  submit-public-batch.sh --dir DIR --base-url API_BASE --message TEXT [options]

Required:
  --dir DIR                 Directory containing *.json auth files
  --base-url API_BASE       API base, for example http://127.0.0.1:19080/api
  --message TEXT            reviewer-facing contributor_message for the batch

Options:
  --requester-email EMAIL   Optional top-level requester_email for every item
  --github-id ID            Top-level github_id
  --frontend-page-url URL   Top-level frontend_page_url
  --batch-size N            Max items per request, default 200
  --wait-seconds N          Sleep between requests, default 60
  --prefix TEXT             Prefix for derived account_name values
  --dry-run                 Show what would be submitted without calling curl
  --help                    Show this help
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

derive_account_name() {
  local file_path stem base hash
  file_path=$1
  stem=$(basename "$file_path")
  stem=${stem%.json}
  base=$(printf '%s' "$stem" \
    | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/[^a-z0-9_-]+/_/g; s/^[_-]+//; s/[_-]+$//; s/_+/_/g')
  if [[ -z "$base" ]]; then
    base="account"
  fi
  if [[ -n "$prefix" ]]; then
    base="${prefix}_${base}"
  fi
  hash=$(printf '%s' "$stem" | sha256sum | cut -c1-8)
  if ((${#base} > 55)); then
    base=${base:0:55}
  fi
  printf '%s_%s' "$base" "$hash"
}

build_item_json() {
  local file_path account_name auth_json
  file_path=$1
  account_name=$2
  auth_json=$(jq -c . "$file_path")
  if [[ -n "${requester_email}" ]]; then
    jq -cn \
      --arg account_name "$account_name" \
      --argjson auth_json "$auth_json" \
      '{account_name: $account_name, auth_json: $auth_json}'
    return 0
  fi

  jq -cn \
    --arg account_name "$account_name" \
    --argjson auth_json "$auth_json" \
    '{account_name: $account_name, auth_json: $auth_json}'
}

dir=""
base_url=""
message=""
requester_email=""
github_id=""
frontend_page_url=""
batch_size=200
wait_seconds=60
prefix=""
dry_run=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir)
      dir=$2
      shift 2
      ;;
    --base-url)
      base_url=$2
      shift 2
      ;;
    --message)
      message=$2
      shift 2
      ;;
    --requester-email)
      requester_email=$2
      shift 2
      ;;
    --github-id)
      github_id=$2
      shift 2
      ;;
    --frontend-page-url)
      frontend_page_url=$2
      shift 2
      ;;
    --batch-size)
      batch_size=$2
      shift 2
      ;;
    --wait-seconds)
      wait_seconds=$2
      shift 2
      ;;
    --prefix)
      prefix=$2
      shift 2
      ;;
    --dry-run)
      dry_run=true
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

require_cmd jq
require_cmd curl
require_cmd sha256sum
require_cmd sed
require_cmd find
require_cmd sort
require_cmd mktemp

if [[ -z "$dir" || -z "$base_url" || -z "$message" ]]; then
  usage >&2
  exit 1
fi

if [[ ! -d "$dir" ]]; then
  echo "directory not found: $dir" >&2
  exit 1
fi

if ! [[ "$batch_size" =~ ^[0-9]+$ ]] || ((batch_size <= 0)); then
  echo "batch-size must be a positive integer" >&2
  exit 1
fi

if ! [[ "$wait_seconds" =~ ^[0-9]+$ ]] || ((wait_seconds < 0)); then
  echo "wait-seconds must be a non-negative integer" >&2
  exit 1
fi

base_url=${base_url%/}
endpoint="${base_url}/llm-gateway/account-contribution-requests/batch-submit"

mapfile -t files < <(find "$dir" -maxdepth 1 -type f -iname '*.json' | sort)
if ((${#files[@]} == 0)); then
  echo "no json files found under: $dir" >&2
  exit 1
fi

timestamp=$(date -u +%Y%m%dT%H%M%SZ)
summary_file="/tmp/llm-gateway-account-batch-submit-${timestamp}.jsonl"
: > "$summary_file"

echo "files: ${#files[@]}"
echo "endpoint: $endpoint"
echo "summary_file: $summary_file"

start=0
batch_number=0
while ((start < ${#files[@]})); do
  batch_number=$((batch_number + 1))
  batch_files=("${files[@]:start:batch_size}")
  start=$((start + ${#batch_files[@]}))

  items_file=$(mktemp)
  payload_file=$(mktemp)
  response_file=$(mktemp)
  trap 'rm -f "$items_file" "$payload_file" "$response_file"' EXIT

  for file_path in "${batch_files[@]}"; do
    jq -e . "$file_path" >/dev/null
    account_name=$(derive_account_name "$file_path")
    build_item_json "$file_path" "$account_name" >>"$items_file"
  done

  items_json=$(jq -s '.' "$items_file")
  jq -cn \
    --arg contributor_message "$message" \
    --arg requester_email "$requester_email" \
    --arg github_id "$github_id" \
    --arg frontend_page_url "$frontend_page_url" \
    --argjson items "$items_json" \
    '
      {
        contributor_message: $contributor_message,
        items: $items
      }
      + (if $requester_email != "" then {requester_email: $requester_email} else {} end)
      + (if $github_id != "" then {github_id: $github_id} else {} end)
      + (if $frontend_page_url != "" then {frontend_page_url: $frontend_page_url} else {} end)
    ' >"$payload_file"

  echo "batch ${batch_number}: ${#batch_files[@]} files"
  jq -r '.items[] | .account_name' "$payload_file"

  if $dry_run; then
    jq -cn \
      --arg type "dry_run" \
      --arg endpoint "$endpoint" \
      --argjson batch_number "$batch_number" \
      --argjson item_count "${#batch_files[@]}" \
      --slurpfile payload "$payload_file" \
      '{
        type: $type,
        endpoint: $endpoint,
        batch_number: $batch_number,
        item_count: $item_count,
        account_names: ($payload[0].items | map(.account_name))
      }' >>"$summary_file"
  else
    http_code=$(curl -sS -o "$response_file" -w '%{http_code}' \
      -H 'Content-Type: application/json' \
      --data-binary "@${payload_file}" \
      "$endpoint")
    if [[ "$http_code" != "200" ]]; then
      echo "batch ${batch_number} failed with HTTP ${http_code}" >&2
      cat "$response_file" >&2
      exit 1
    fi
    jq -cn \
      --arg type "submitted" \
      --arg endpoint "$endpoint" \
      --argjson batch_number "$batch_number" \
      --slurpfile response "$response_file" \
      '{
        type: $type,
        endpoint: $endpoint,
        batch_number: $batch_number,
        response: $response[0]
      }' >>"$summary_file"
    jq -r '"created=\(.created_count) invalid=\(.invalid_count) conflict=\(.conflict_count)"' \
      "$response_file"
  fi

  rm -f "$items_file" "$payload_file" "$response_file"
  trap - EXIT

  if ((start < ${#files[@]})) && ! $dry_run && ((wait_seconds > 0)); then
    echo "waiting ${wait_seconds}s before next batch"
    sleep "$wait_seconds"
  fi
done

echo "done"
