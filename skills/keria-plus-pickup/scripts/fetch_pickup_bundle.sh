#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  fetch_pickup_bundle.sh --codes-file FILE --output-dir DIR [options]

Required:
  --codes-file FILE        Text file with one pickup code per line
  --output-dir DIR         Directory to store ZIP, headers, and unpacked files

Options:
  --base-url URL           Default: https://plus.keria.cc.cd
  --output-format FORMAT   Default: plus_json
  --progress-id ID         Default: UTC timestamp
  --dry-run                Print the request plan without calling curl
  --help                   Show this help
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

base_url="https://plus.keria.cc.cd"
codes_file=""
output_dir=""
output_format="plus_json"
progress_id="$(date -u +%Y%m%dT%H%M%SZ)"
dry_run=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-url)
      base_url=$2
      shift 2
      ;;
    --codes-file)
      codes_file=$2
      shift 2
      ;;
    --output-dir)
      output_dir=$2
      shift 2
      ;;
    --output-format)
      output_format=$2
      shift 2
      ;;
    --progress-id)
      progress_id=$2
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

require_cmd curl
require_cmd unzip
require_cmd python3

if [[ -z "$codes_file" || -z "$output_dir" ]]; then
  usage >&2
  exit 1
fi

if [[ ! -f "$codes_file" ]]; then
  echo "codes file not found: $codes_file" >&2
  exit 1
fi

mkdir -p "$output_dir"
headers_file="$output_dir/pickup.headers.txt"
zip_file="$output_dir/pickup-${output_format}.zip"
unpack_dir="$output_dir/unpacked"

code_count=$(python3 - "$codes_file" <<'PY'
from pathlib import Path
import sys
text = Path(sys.argv[1]).read_text(encoding='utf-8')
count = len([line.strip() for line in text.splitlines() if line.strip()])
print(count)
PY
)

endpoint="${base_url%/}/pickup"
echo "endpoint: $endpoint"
echo "codes_file: $codes_file"
echo "code_count: $code_count"
echo "output_dir: $output_dir"
echo "output_format: $output_format"
echo "progress_id: $progress_id"

if $dry_run; then
  exit 0
fi

curl -sS \
  -D "$headers_file" \
  -o "$zip_file" \
  -H 'accept: */*' \
  -H "origin: ${base_url%/}" \
  -H "referer: ${base_url%/}/" \
  -H 'user-agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36 Edg/147.0.0.0' \
  -F "codes=<${codes_file}" \
  -F "output_format=${output_format}" \
  -F "progress_id=${progress_id}" \
  "$endpoint"

mkdir -p "$unpack_dir"
unzip -o "$zip_file" -d "$unpack_dir" >/dev/null

echo "zip_file: $zip_file"
echo "headers_file: $headers_file"
echo "unpack_dir: $unpack_dir"

result_file="$unpack_dir/取件结果.txt"
if [[ -f "$result_file" ]]; then
  echo "result_file: $result_file"
  cat "$result_file"
fi
