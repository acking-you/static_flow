# `sf-cli` Troubleshooting for External Repost Publisher

Use this reference when publication/update commands fail in external repost workflow.

## 1) CLI not found

Symptom:
- `zsh: command not found: sf-cli`

Recovery:
1. Resolve executable in fallback order:
   - `./bin/sf-cli`
   - `./target/release/sf-cli`
   - `./target/debug/sf-cli`
   - `../target/release/sf-cli`
   - `sf-cli` from `PATH`
2. If missing, build:
   - `cargo build -p sf-cli --release`

## 2) Wrong subcommand under `db`

Symptom:
- `error: unrecognized subcommand 'get-article'`

Reason:
- `get-article` is under `sf-cli api`, not `sf-cli db`.

Recovery:
1. Read article payload:
   - `sf-cli api --db-path <db_path> get-article <id>`
2. Inspect table row:
   - `sf-cli db --db-path <db_path> query-rows articles --where "id='<id>'" --format vertical --limit 1`

## 3) `db upsert-article` JSON shape mismatch

Symptoms:
- `Error: invalid article JSON`
- `invalid type: map, expected a string ...` (often `detailed_summary`)
- `missing field created_at`

Reason:
- `db upsert-article` expects `ArticleRecord` schema, not API response schema.

Recovery:
1. Normalize payload before upsert:
   - `detailed_summary` must be stored as JSON string (not nested object),
   - include required timestamps (`created_at`, `updated_at`).
2. Preserve original `created_at` for existing records.
3. If changing only a few columns, prefer `db update-rows` instead of full upsert.

## 4) Safe command selection by update scope

1. Narrow field patch (single/few columns):
   - `sf-cli db update-rows ...`
2. Bilingual field patch from files:
   - `sf-cli db update-article-bilingual ...`
3. Full overwrite/create with markdown source:
   - `sf-cli write-article ...`
4. Full-row upsert:
   - only when payload schema is fully normalized and timestamps are controlled.

## 5) Suggested diagnostics artifacts

Store in workspace (`/tmp/external_repost/<article_id>/`):
- `article_before.json`
- `article_after.json`
- `cli_diagnostics.log`
