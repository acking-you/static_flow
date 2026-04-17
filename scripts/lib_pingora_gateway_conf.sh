#!/usr/bin/env bash
set -euo pipefail

pingora_top_level_conf_value() {
  local conf_file="$1"
  local key="$2"
  awk -v key="$key" '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    /^[^[:space:]]/ && $1 == key ":" { print $2; exit }
  ' "$conf_file"
}

pingora_staticflow_conf_value() {
  local conf_file="$1"
  local key="$2"
  awk -v key="$key" '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    /^staticflow:[[:space:]]*$/ { in_static = 1; next }
    in_static && /^[^[:space:]]/ { exit }
    in_static && $1 == key ":" { print $2; exit }
  ' "$conf_file"
}

pingora_staticflow_upstream_addr() {
  local conf_file="$1"
  local slot="$2"
  awk -v slot="$slot" '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    /^staticflow:[[:space:]]*$/ { in_static = 1; next }
    in_static && /^[^[:space:]]/ { exit }
    in_static {
      indent = match($0, /[^[:space:]]/) - 1
      if ($1 == "upstreams:") {
        in_upstreams = 1
        upstream_indent = indent
        next
      }
      if (in_upstreams) {
        if (indent <= upstream_indent) {
          in_upstreams = 0
        } else if ($1 == slot ":") {
          print $2
          exit
        }
      }
    }
  ' "$conf_file"
}
