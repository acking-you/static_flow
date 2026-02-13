---
name: staticflow-cli-publisher
description: >-
  Publish and maintain StaticFlow content in LanceDB via `sf-cli`: write
  articles/images, sync notes, infer missing metadata, run local API checks,
  and perform DB maintenance (including immediate prune).
---

# StaticFlow CLI Publisher

Use this skill to publish Markdown/blog notes into LanceDB and verify results.

## When To Use
1. Publish one Markdown article (`write-article`).
2. Batch import images (`write-images`) or sync a notes directory (`sync-notes`).
3. Query/update/delete data in `articles`, `images`, and `taxonomies`.
4. Run backend-equivalent local API queries for verification/debug.
5. Reclaim storage immediately (`db optimize --all --prune-now`).

## Execution Policy (Mandatory)
- Context-first: read article and metadata from current context/local files first.
- LLM-native generation: generate missing metadata and summary directly in-session.
- Do not call external model APIs, proxy scripts, or sub-agent model commands.
- Use CLI only for fetch fallback, write/sync, and verification.
- Keep intermediate artifacts under `tmp/`.
- Never run destructive operations unless explicitly requested.

## Load Extra Context
- Required: `references/publish-checklist.md`
- If summary generation is needed: `../article-summary-architect/SKILL.md`
- Optional docs: `README.md`, `docs/cli-user-guide.zh.md`

## Preconditions
1. Resolve CLI in this order:
   - `./bin/sf-cli`
   - `./target/release/sf-cli`
   - `./target/debug/sf-cli`
   - `../target/release/sf-cli`
   - `sf-cli` from `PATH`
2. Verify CLI works: `<cli> --help`
   - Build if needed: `cargo build -p sf-cli --release`
3. Verify DB path exists.
4. Verify DB tables:
   - `<cli> db --db-path <db_path> list-tables`
   - required: `articles`, `images`, `taxonomies`
5. If DB is uninitialized, ask user before:
   - `<cli> init --db-path <db_path>`

## Publication Workflow

### A) Single Article (`write-article`)
1. Read Markdown body/frontmatter.
2. Ensure required metadata in final payload:
   - `summary`
   - `tags`
   - `category`
   - `category_description`
3. Metadata priority:
   - frontmatter
   - explicit user CLI args
   - skill inference from content + existing taxonomy records
4. Preserve source date:
   - use `--date` when user provides it
   - else keep frontmatter `date`
   - else derive from file birth time/mtime (`YYYY-MM-DD`) and pass `--date`
5. Summary gate (before publish):
   - regenerate `detailed_summary.zh/en` when missing/stale/unstructured
   - follow `article-summary-architect` quality contract
6. Publish command:
   - frontmatter complete:
     - `<cli> write-article --db-path <db_path> --file <post.md>`
   - frontmatter incomplete:
     - `<cli> write-article --db-path <db_path> --file <post.md> --summary "..." --tags "a,b" --category "..." --category-description "..."`
7. Local image import (optional):
   - `--import-local-images`
   - optional `--media-root <path>` (repeatable)
   - optional `--generate-thumbnail --thumbnail-size <n>`
   - supports `![](path)`, `![[path]]`, `![[path|alias]]`
8. Verify publication:
   - article row exists
   - taxonomy rows updated
   - images imported when expected
9. Report:
   - article id
   - inferred metadata (if any)
   - summary generated/reused
   - image import count and warnings

### B) Image Batch (`write-images`)
`<cli> write-images --db-path <db_path> --dir <image_dir> [--recursive] [--generate-thumbnail] [--thumbnail-size <n>] [--no-auto-optimize]`

### C) Notes Sync (`sync-notes`)
`<cli> sync-notes --db-path <db_path> --dir <notes_dir> [--recursive] [--generate-thumbnail] [--thumbnail-size <n>] [--language <en|zh>] [--default-category <name>] [--default-author <name>] [--no-auto-optimize]`

## DB and API Quick Map
- DB ops: `<cli> db --db-path <db_path> <subcommand>`
  - common: `list-tables`, `query-rows`, `update-rows`, `delete-rows`, `ensure-indexes`, `optimize`
- API-equivalent ops: `<cli> api --db-path <db_path> <subcommand>`
  - common: `get-article`, `search`, `semantic-search`, `list-tags`, `list-categories`, `search-images`

## One-Click Immediate Prune
- Single table:
  - `<cli> db --db-path <db_path> optimize <table> --all --prune-now`
- All managed tables:
  - `for t in articles images taxonomies; do <cli> db --db-path <db_path> optimize "$t" --all --prune-now; done`

## Error Handling
- Missing metadata: infer and continue, but report inferred fields explicitly.
- Image resolution warnings: rerun with `--media-root`.
- Partial success: rerun only failed steps, then verify again.
- Never run `drop-table` or `delete-rows --all` unless user explicitly requests it.
