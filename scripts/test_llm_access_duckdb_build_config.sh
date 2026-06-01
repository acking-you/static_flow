#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

grep -F 'DUCKDB_DOWNLOAD_LIB = { value = "1", force = false }' \
  "$ROOT_DIR/.cargo/config.toml"

grep -F 'default = ["duckdb-prebuilt"]' \
  "$ROOT_DIR/crates/llm-access-store/Cargo.toml"
grep -F 'duckdb-prebuilt = ["duckdb-runtime"]' \
  "$ROOT_DIR/crates/llm-access-store/Cargo.toml"
grep -F 'default = ["duckdb-prebuilt"]' \
  "$ROOT_DIR/crates/llm-access/Cargo.toml"
grep -F 'duckdb-prebuilt = ["duckdb-runtime"]' \
  "$ROOT_DIR/crates/llm-access/Cargo.toml"
