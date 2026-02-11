# StaticFlow Publish Checklist

## 1. Resolve CLI Binary
Try in order:
1. `./bin/sf-cli`
2. `../target/release/sf-cli`
3. `sf-cli`

Smoke test:
```bash
<cli> --help
```

Build if missing:
```bash
cargo build -p sf-cli --release
```

## 2. Verify DB Readiness
List tables:
```bash
<cli> db --db-path <db_path> list-tables
```
Required:
- `articles`
- `images`
- `taxonomies`

Initialize if approved by user:
```bash
<cli> init --db-path <db_path>
```

## 3. Required Metadata for Blog Post
Must exist before publication:
- `summary`
- `tags`
- `category`
- `category_description`

Allowed sources:
- Markdown frontmatter
- CLI args (`--summary`, `--tags`, `--category`, `--category-description`)
- Skill inference from article content + existing taxonomy rows

If any required field is missing from frontmatter, infer it and pass explicit CLI flags.

## 4. Single Post Publish Commands
Basic (only when frontmatter already contains `summary`, `tags`, `category`, and `category_description`):
```bash
<cli> write-article --db-path <db_path> --file <post.md>
```

With explicit metadata flags (required when frontmatter is incomplete):
```bash
<cli> write-article \
  --db-path <db_path> \
  --file <post.md> \
  --summary "Post summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes for Rust and WASM"
```

With local image import (still provide full metadata if frontmatter is incomplete):
```bash
<cli> write-article \
  --db-path <db_path> \
  --file <post.md> \
  --summary "Post summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes for Rust and WASM" \
  --import-local-images
```

Supports both:
- Standard Markdown image links: `![](relative/path.png)`
- Obsidian image embeds: `![[relative/path.png]]`, `![[relative/path.png|caption]]`
- Global media fallback roots: `--media-root <vault_or_assets_dir>` (repeatable)

With thumbnail generation for imported images:
```bash
<cli> write-article \
  --db-path <db_path> \
  --file <post.md> \
  --summary "Post summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes for Rust and WASM" \
  --import-local-images \
  --generate-thumbnail \
  --thumbnail-size 256
```

### How category/tags are persisted
- `articles` table:
  - `category` column stores one category name.
  - `tags` column stores the tag array.
- `taxonomies` table:
  - upsert one row for category: `kind=category`, `key=normalize(category)`.
  - upsert one row per tag: `kind=tag`, `key=normalize(tag)`.
  - `description` for category comes from `category_description` in the same publish run.

### Metadata inference policy
- `summary`: one concise sentence based on title + opening section.
- `tags`: 3-8 topic tags derived from title/headings.
- `category`: infer one primary domain category when missing.
- `category_description`: reuse existing DB taxonomy description first; otherwise generate a concise description.

## 5. Image Directory Publish
```bash
<cli> write-images --db-path <db_path> --dir <image_dir> --recursive --generate-thumbnail
```

## 6. Notes Sync Publish
```bash
<cli> sync-notes --db-path <db_path> --dir <notes_dir> --recursive --generate-thumbnail
```
See section 8 for full flag reference.

## 7. Verification Commands
Check one article:
```bash
<cli> query \
  --db-path <db_path> \
  --table articles \
  --where "id='<article_id>'" \
  --limit 1 \
  --format vertical
```

Check taxonomy rows:
```bash
<cli> query --db-path <db_path> --table taxonomies --limit 20
```

Check image rows:
```bash
<cli> query --db-path <db_path> --table images --limit 20
```

## 8. Sync Notes Publish
```bash
<cli> sync-notes --db-path <db_path> --dir <notes_dir> \
  --recursive --generate-thumbnail \
  --default-category "Notes" \
  --default-author "Unknown" \
  --language en
```

Notes-specific flags:
- `--default-category <name>`: fallback category when frontmatter is missing (default: `Notes`).
- `--default-author <name>`: fallback author when frontmatter is missing (default: `Unknown`).
- `--language <en|zh>`: language hint for auto-embedding.
- `--no-auto-optimize`: skip index optimization after sync.

## 9. Database Management Quick Reference
All via `<cli> db --db-path <db_path> <subcommand>`:

| Task | Command |
|------|---------|
| List tables | `list-tables` |
| Create table | `create-table <name> [--replace]` |
| Drop table | `drop-table <name> --yes` |
| Describe schema | `describe-table <name>` |
| Count rows | `count-rows <table> [--where <sql>]` |
| Query rows | `query-rows <table> [--where <sql>] [--columns <cols>] [--limit <n>] [--format vertical]` |
| Update rows | `update-rows <table> --set "col=expr" [--where <sql>] [--all]` |
| Delete rows | `delete-rows <table> [--where <sql>] [--all]` |
| Upsert article | `upsert-article --json '<json>'` |
| Upsert image | `upsert-image --json '<json>'` |
| Ensure indexes | `ensure-indexes [--table <name>]` |
| List indexes | `list-indexes <table> [--with-stats]` |
| Optimize | `optimize <table> [--all]` |

## 10. Local API Quick Reference
All via `<cli> api --db-path <db_path> <subcommand>`:

| Task | Command |
|------|---------|
| List articles | `list-articles [--tag <t>] [--category <c>]` |
| Get article | `get-article <id>` |
| Related articles | `related-articles <id>` |
| Keyword search | `search --q <keyword>` |
| Semantic search | `semantic-search --q <keyword> [--enhanced-highlight]` |
| List tags | `list-tags` |
| List categories | `list-categories` |
| List images | `list-images` |
| Search images | `search-images --id <image_id>` |
| Export image | `get-image <id_or_filename> [--thumb] [--out <path>]` |

## 11. Failure Recovery Pattern
If failure happens during publish:
1. Keep DB path unchanged.
2. Re-run only the failed command.
3. Re-run verification commands.
4. Report exactly what succeeded and what failed.

If local image resolution warnings appear:
1. Ask user to confirm Obsidian media root directories.
2. Re-run `write-article` with `--media-root <path>` (repeat if needed).
3. Verify no unresolved `![[...]]` remains in stored article content.
