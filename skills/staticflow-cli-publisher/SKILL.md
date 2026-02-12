---
name: staticflow-cli-publisher
description: >-
  Manage StaticFlow blog content lifecycle through `sf-cli` and LanceDB.
  Trigger this skill when the user wants to:
  (1) publish/write/sync a Markdown blog post or Obsidian note into the database,
  (2) batch import images or an image directory into LanceDB,
  (3) sync a notes/vault directory (bulk Markdown + images),
  (4) query, inspect, update, or delete articles/images/taxonomies in LanceDB,
  (5) manage LanceDB tables (init, create, drop, describe, optimize, index),
  (6) reclaim LanceDB storage immediately with one-command prune (`db optimize --all --prune-now`),
  (7) run backend-equivalent API queries locally (list articles, search, semantic search, related articles, image search),
  (8) verify publication results or debug data integrity issues,
  (9) enforce reasoning-driven bilingual `detailed_summary` quality before publication.
  The skill auto-infers missing metadata (`summary`, `tags`, `category`, `category_description`)
  from article content, handles local image import from Markdown/Obsidian `![[]]` syntax,
  and enforces a strict precondition → publish → verify workflow.
---

# StaticFlow CLI Publisher

Execute content publication and database management workflows for StaticFlow using `sf-cli`.

## Load Extra Context
- Read `references/publish-checklist.md` before running any publish commands.
- If article summary needs generation/refinement, read and apply:
  - `../article-summary-architect/SKILL.md`
- Read project docs only as needed:
  - `README.md`
  - `docs/cli-user-guide.zh.md`

## Enforce Preconditions
1. Resolve the CLI binary path in this order:
   - `./bin/sf-cli`
   - `../target/release/sf-cli`
   - `sf-cli` from `PATH`
2. Verify CLI is runnable (`<cli> --help`). If not installed, build it from repo root:
   - `cargo build -p sf-cli --release`
3. Verify the target LanceDB directory exists.
4. Verify DB is initialized by checking managed tables:
   - `<cli> db --db-path <db_path> list-tables`
   Required tables: `articles`, `images`, `taxonomies`.
5. If DB is not initialized, stop and ask before running:
   - `<cli> init --db-path <db_path>`

---

## Publication Workflows

### Reasoning-First Detailed Summary Gate (Run Before Publish)

For long-form Markdown publication, ensure `detailed_summary.zh/en` quality before write:
1. If `detailed_summary` is missing, stale, or unstructured, invoke `article-summary-architect`.
2. Generate/refresh bilingual summaries with internal type-aware reasoning.
3. Require concise, evidence-grounded content that includes:
   - natural opening定位句（如“这是一篇xxx文章”）
   - 3-5 semantic sections with bullets
   - core problem and conclusion
   - minimal trustworthy reasoning path
   - validation/boundary notes when relevant
4. Do not force a rigid template; keep style natural and readable.
5. Continue to publish only after summary quality checks pass.
6. Include summary status in the final publish report:
   - generated vs reused
   - quality-gate result

### Enforce Metadata Requirements
At publish time, `summary`, `tags`, `category`, and `category_description` must all be present in the final write payload.
Also preserve source article date: the stored `articles.date` must match the note's creation date.

For blog publication, resolve metadata from these sources in order:
1. Frontmatter in the Markdown file.
2. Explicit user-provided CLI args.
3. Skill-driven inference from article content (automatic).

Required fields:
- `summary`
- `tags`
- `category`
- `category_description` (collect together with `category`)

If any field is missing:
1. Auto-generate missing fields from content and existing DB taxonomy records.
2. Pass generated values via CLI flags.
3. Continue publication without blocking on user input.
4. Report generated values in the final publish summary.

### Preserve Source Creation Date
Keep publication date consistent with the original note:
1. If the user provides `--date`, use it (highest priority).
2. Otherwise if frontmatter `date` exists, keep it unchanged.
3. If frontmatter `date` is missing, derive from source file timestamp (`YYYY-MM-DD`, prefer file birth time when available, otherwise mtime) and pass it via `--date`.
4. Do not silently fallback to "today" when date preservation is required by the user.

### Metadata Inference Rules
- `summary`: generate 1 concise sentence from title + opening paragraphs.
- `tags`: derive 3-8 domain tags from title/headings/keywords; output comma-separated.
- `category`: reuse frontmatter category when present; otherwise infer one primary topic.
- `category_description`: prefer existing taxonomy description for the same category; otherwise generate a concise human-readable description.

### Choose Publication Path
- Single Markdown post → `write-article`
- Image directory → `write-images`
- Notes/vault directory (bulk Markdown + images) → `sync-notes`

### Publish Single Markdown Post
1. Validate required metadata (`summary`, `tags`, `category`, `category_description`).
2. Detect local image usage in Markdown body and frontmatter `featured_image`.
3. Confirm where metadata comes from:
   - Preferred: frontmatter fields in the Markdown file.
   - Fallback: explicit CLI flags (`--summary`, `--tags`, `--category`, `--category-description`, optional `--date`) using inferred values when needed.
4. If frontmatter is complete, basic publish is valid:
   - `<cli> write-article --db-path <db_path> --file <markdown>`
5. If any required field is missing from frontmatter, use explicit flags:
   - `<cli> write-article --db-path <db_path> --file <markdown> --summary \"...\" --tags \"tag1,tag2\" --category \"...\" --category-description \"...\"`
6. Optional flags:
   - `--id <custom_id>`: override the default article ID (file stem).
   - `--date <YYYY-MM-DD>`: override frontmatter date for publication date control.
   - `--language <en|zh>`: language hint for auto-embedding.
   - `--vector <json>` / `--vector-en <json>` / `--vector-zh <json>`: pre-computed embedding vectors.
   - `--no-auto-optimize`: skip automatic index optimization after write.
7. If local images are referenced, append local-image import options:
   - `--import-local-images`
   - Optional: `--generate-thumbnail --thumbnail-size <n>`
   - Optional: `--media-root <path>` (repeatable) for Obsidian vault-wide attachment folders.
8. Category description behavior:
   - Publish category and category description together in the same command run.
   - Write `category_description` into `taxonomies.description` for `kind=category`.
9. Where category/tag data is written:
   - Article row: `articles.category`, `articles.tags`.
   - Taxonomy rows: `taxonomies` upsert with `kind=category` and `kind=tag`.

### Publish Image Assets
```
<cli> write-images --db-path <db_path> --dir <image_dir> \
  [--recursive] [--generate-thumbnail] [--thumbnail-size <n>] \
  [--no-auto-optimize]
```

### Sync Notes Directory
Bulk sync a notes/vault directory. Processes all Markdown files and their referenced images.
```
<cli> sync-notes --db-path <db_path> --dir <notes_dir> \
  [--recursive] [--generate-thumbnail] [--thumbnail-size <n>] \
  [--language <en|zh>] \
  [--default-category <name>]    # default: "Notes"
  [--default-author <name>]      # default: "Unknown"
  [--no-auto-optimize]
```

### Local Image Capability Guard
Before publishing a single post with local images, verify the CLI supports:
- `write-article --import-local-images`
- Obsidian image embeds `![[path]]` and `![[path|alias]]` in addition to standard `![](path)`.
- Optional global fallback roots via `--media-root <path>` for vault-wide attachment folders.

If the option is missing:
1. Implement the capability in CLI (Markdown + Obsidian local image detection, image upsert, link rewrite to `images/<sha256>`).
2. Build and re-run checks.
3. Proceed with publication only after the option exists.

---

## Database Management (`db` subcommand)

Use `<cli> db --db-path <db_path> <subcommand>` for direct table operations.

### Table Operations
- `list-tables [--limit <n>]` — list all tables.
- `create-table <name> [--replace]` — create a managed table (`articles`, `images`, or `taxonomies`). `--replace` drops existing first.
- `drop-table <name> --yes` — drop a table (requires explicit `--yes` confirmation).
- `describe-table <name>` — show table schema and row count.

### Row Operations
- `count-rows <table> [--where <sql>]` — count rows with optional filter.
- `query-rows <table> [--where <sql>] [--columns <col1,col2>] [--limit <n>] [--offset <n>] [--format table|vertical]` — query rows with projection, filter, and pagination.
- `update-rows <table> --set "<col>=<expr>" [--set ...] [--where <sql>] [--all]` — update rows by SQL expression. `--all` required when no `--where` is given.
- `delete-rows <table> [--where <sql>] [--all]` — delete rows. `--all` required when no `--where` is given.
- `upsert-article --json '<ArticleRecord JSON>'` — upsert one article row from JSON payload.
- `upsert-image --json '<ImageRecord JSON>'` — upsert one image row from JSON payload.

### Index Operations
- `ensure-indexes [--table <name>]` — ensure all expected indexes for managed tables.
- `list-indexes <table> [--with-stats]` — list indexes and optional coverage stats.
- `drop-index <table> <index_name>` — drop an index by name.
- `optimize <table> [--all] [--prune-now]` — optimize index coverage (default) or full table optimization; `--prune-now` triggers immediate aggressive prune (`older_than=0`, `delete_unverified=true`).

### One-Click Immediate Prune
Use this when the user asks to reclaim space now (without waiting retention windows):

```
<cli> db --db-path <db_path> optimize <table> --all --prune-now
```

Run across all managed tables:

```
for t in articles images taxonomies; do
  <cli> db --db-path <db_path> optimize "$t" --all --prune-now
done
```

---

## Local API Queries (`api` subcommand)

Use `<cli> api --db-path <db_path> <subcommand>` to run backend-equivalent queries locally without a running server.

### Article Queries
- `list-articles [--tag <tag>] [--category <cat>]` — list articles with optional filters.
- `get-article <id>` — get a single article by ID.
- `related-articles <id>` — find related articles.
- `search --q <keyword>` — full-text keyword search.
- `semantic-search --q <keyword> [--enhanced-highlight]` — vector-based semantic search with optional high-precision highlight reranking.

### Taxonomy Queries
- `list-tags` — list all tags.
- `list-categories` — list all categories.

### Image Queries
- `list-images` — list all images.
- `search-images --id <image_id>` — find similar images by ID.
- `get-image <id_or_filename> [--thumb] [--out <path>]` — export an image to file. `--thumb` returns thumbnail when available.

---

## Post-Publish Verification
After each publish, run targeted checks:
1. Verify article row exists:
   - `<cli> query --db-path <db_path> --table articles --where "id='<article_id>'" --limit 1 --format vertical`
2. Verify taxonomy rows for category and tags:
   - `<cli> query --db-path <db_path> --table taxonomies --limit 20`
3. If local images were imported, verify `images` rows increased and links are rewritten in stored content.
4. Report concise results: published IDs, imported image count, and any warnings.

## Error Handling Rules
- Never silently skip missing required metadata.
- If metadata is inferred, report exactly what was inferred.
- If image resolution warnings appear, ask the user to confirm one or more media roots, then rerun publish with `--media-root`.
- Never run destructive DB commands (`drop-table`, `delete-rows --all`) unless explicitly asked.
- If publish partially succeeds, report exact completed steps and next safe recovery command.
