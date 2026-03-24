# StaticFlow Publish Checklist

## 1. CLI and DB Preflight
Resolve CLI in order:
1. `./target/release/sf-cli`
2. `./target/debug/sf-cli`
3. `../target/release/sf-cli`
4. `sf-cli`

If the checkout is newer than the chosen binary, rebuild first:
`cargo build -p sf-cli --release`

Do not prefer legacy `./bin/sf-cli` snapshots for storage-format-sensitive writes.

Smoke test:
```bash
<cli> --help
```

Build if missing:
```bash
cargo build -p sf-cli --release
```

Check DB tables:
```bash
<cli> db --db-path <db_path> list-tables
```
Required: `articles`, `images`, `taxonomies`

Current content DB may also contain:
- `article_views`, `api_behavior_events`
- `article_requests`, `article_request_ai_runs`, `article_request_ai_run_chunks`
- `interactive_pages`, `interactive_page_locales`, `interactive_assets`

Init DB only with user approval:
```bash
<cli> init --db-path <db_path>
```

If this DB recently had failed writes, duplicate-key errors, index errors, or manual table
maintenance, inspect health before publishing again:
```bash
<cli> db --db-path <db_path> audit-storage --table articles
<cli> db --db-path <db_path> audit-storage --table taxonomies
```

## 2. Metadata Gate (Before `write-article`)
Required fields in final payload:
- `summary`
- `tags`
- `category`
- `category_description`
- if bilingual output is required: `content_en` and `detailed_summary.zh/en`

Resolution priority:
1. Frontmatter
2. Explicit CLI args
3. Inference from article content + existing taxonomies

Date policy:
1. If user provides `--date`, use it.
2. Else keep frontmatter `date`.
3. Else derive from file birth time/mtime and pass `--date` (`YYYY-MM-DD`).

## 3. Article Publish Commands
Basic (frontmatter complete):
```bash
<cli> write-article --db-path <db_path> --file <post.md>
```

With custom id:
```bash
<cli> write-article --db-path <db_path> --file <post.md> --id <custom_id>
```

With explicit metadata (frontmatter incomplete):
```bash
<cli> write-article \
  --db-path <db_path> \
  --file <post.md> \
  --summary "Post summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes for Rust and WASM"
```

With explicit date:
```bash
<cli> write-article --db-path <db_path> --file <post.md> --date 2025-06-15
```

With explicit bilingual files (without frontmatter bilingual fields):
```bash
<cli> write-article \
  --db-path <db_path> \
  --file <post.md> \
  --summary "Post summary" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes for Rust and WASM" \
  --content-en-file <content_en.md> \
  --summary-zh-file <summary_zh.md> \
  --summary-en-file <summary_en.md>
```
`--summary-zh-file` and `--summary-en-file` must be provided together.

With pre-computed vectors:
```bash
<cli> write-article --db-path <db_path> --file <post.md> \
  --vector '[0.1, 0.2, ...]' --vector-en '[...]' --vector-zh '[...]'
```

With auto-embedding language hint:
```bash
<cli> write-article --db-path <db_path> --file <post.md> --language zh
```

With local image import:
```bash
<cli> write-article \
  --db-path <db_path> \
  --file <post.md> \
  --import-local-images \
  --media-root <assets_dir> \
  --generate-thumbnail \
  --thumbnail-size 256
```

Disable auto-optimize after write:
```bash
<cli> write-article --db-path <db_path> --file <post.md> --no-auto-optimize
```
Use `--no-auto-optimize` only for healthy-table batching. It is not a recovery flag for
merge/index corruption.

Image syntax supported:
- `![](relative/path.png)`
- `![[relative/path.png]]`
- `![[relative/path.png|alias]]`

## 4. Batch Publish Commands
Image directory:
```bash
<cli> write-images --db-path <db_path> --dir <image_dir> \
  [--recursive] [--generate-thumbnail] [--thumbnail-size <n>] \
  [--no-auto-optimize]
```

Notes directory:
```bash
<cli> sync-notes --db-path <db_path> --dir <notes_dir> \
  [--recursive] [--generate-thumbnail] [--thumbnail-size <n>] \
  [--language <en|zh>] [--default-category <name>] [--default-author <name>] \
  [--no-auto-optimize]
```

## 5. Verification Commands
Check article:
```bash
<cli> db --db-path <db_path> query-rows articles \
  --where "id='<article_id>'" --limit 1 --format vertical
```

Quick article via API:
```bash
<cli> api --db-path <db_path> get-article <article_id>
```
Prefer `sf-cli api` for validation because it opens a fresh DB connection. A long-running
backend process may keep a stale manifest/index view after table rebuild/swap operations.

Check taxonomy rows:
```bash
<cli> db --db-path <db_path> query-rows taxonomies --limit 20
```

Check images rows:
```bash
<cli> db --db-path <db_path> query-rows images --limit 20
```

Blob v2 note:
- `images.data` uses blob v2 in current production layout
- `images.thumbnail` remains plain `Binary`
- use `<cli> api get-image <id-or-filename>` to validate readback end-to-end

Table info:
```bash
<cli> db --db-path <db_path> describe-table articles
<cli> db --db-path <db_path> count-rows articles
<cli> db --db-path <db_path> count-rows articles --where "category='Tech'"
```

API-equivalent checks:
```bash
<cli> api --db-path <db_path> get-article <article_id>
<cli> api --db-path <db_path> list-articles [--tag "rust"] [--category "Tech"]
<cli> api --db-path <db_path> related-articles <article_id>
<cli> api --db-path <db_path> search --q "<keyword>"
<cli> api --db-path <db_path> semantic-search --q "<keyword>"
<cli> api --db-path <db_path> list-images
<cli> api --db-path <db_path> get-image <id-or-filename>
<cli> api --db-path <db_path> search-images --id <image_id>
<cli> api --db-path <db_path> search-images-text --q "<keyword>"
```

## 5b. Upsert and Bilingual Update Commands
Upsert article from JSON (full row):
```bash
<cli> db --db-path <db_path> upsert-article --json '{"id":"...","title":"...",...}'
```

Upsert image from JSON (full row):
```bash
<cli> db --db-path <db_path> upsert-image --json '{"id":"...","filename":"...",...}'
```

Update bilingual fields from files:
```bash
<cli> db --db-path <db_path> update-article-bilingual --id <article_id> \
  --content-en-file <content_en.md> \
  --summary-zh-file <summary_zh.md> \
  --summary-en-file <summary_en.md>
```

Backfill missing article vectors:
```bash
<cli> db --db-path <db_path> backfill-article-vectors [--limit 50] [--dry-run]
```

Recompute SVG image embeddings:
```bash
<cli> db --db-path <db_path> reembed-svg-images [--limit 20] [--dry-run]
```

## 6. Escalation-Only Maintenance / Repair Commands
Do not use the following as the first reaction to a failed `write-article` on a live DB:

```bash
<cli> db --db-path <db_path> delete-rows ...
<cli> db --db-path <db_path> ensure-indexes
<cli> db --db-path <db_path> optimize <table> --all --prune-now
<cli> db --db-path <db_path> cleanup-orphans [--table <table>]
<cli> db --db-path <db_path> rebuild-table-stable <table>
```

Why:
- `delete-rows` writes tombstones; it does not physically dedupe a table by itself.
- `ensure-indexes` can keep failing if the current scalar index state is already unhealthy.
- `optimize --all --prune-now` and `cleanup-orphans` are storage-maintenance commands, not
  corruption-recovery primitives.
- Rebuild/swap operations can leave a long-running backend on a stale manifest view until a
  fresh process is started.

Schema/storage repair helpers:
```bash
<cli> db --db-path <db_path> rebuild-table-stable <table>
<cli> db --db-path <db_path> migrate-images-blob-v2
```

## 7. Failure Recovery
1. Keep DB path and input files unchanged.
2. Re-run only the failed command if the failure is a normal publish/input problem.
3. Re-run verification commands.
4. Report what succeeded, what failed, and next safe command.
5. If image resolution fails, add/fix `--media-root` and retry.

Hard stop symptoms:
- `Ambiguous merge inserts`
- `RowAddrTreeMap::from_sorted_iter called with non-sorted input`
- repeated duplicate taxonomy/article key errors after input-side dedupe
- list endpoint works but single-item filtered reads fail on a running backend

If any hard stop symptom appears:
1. Stop retries and stop mutating the affected tables.
2. Capture:
   - `<cli> db --db-path <db_path> audit-storage --table <table>`
   - `<cli> db --db-path <db_path> list-indexes <table> --with-stats`
   - targeted `query-rows` / `count-rows`
3. Preserve rollback state before any repair.
4. Escalate to stable-row-id rebuild or temp-DB rewrite with fresh indexes.
5. Validate repaired data with `sf-cli api` or a fresh backend process, not only the old
   long-running backend.

## 8. Music Commands

Music DB 路径与 content DB 分离，默认 `<db_root>/lancedb-music`。

写入单曲（含自动 embedding）:
```bash
<cli> write-music --db-path <music_db_path> --file <audio.mp3|flac> \
  [--id <song_id>] [--title <title>] [--artist <artist>] [--album <album>] \
  [--album-id <album_id>] [--cover <cover.jpg>] [--content-db-path <content_db>] \
  [--lyrics <lyrics.lrc>] [--lyrics-translation <trans.lrc>] \
  [--source <source>] [--source-id <platform_id>] [--tags "tag1,tag2"]
```

批量回填 embedding:
```bash
<cli> embed-songs --db-path <music_db_path>
```

手动完成音乐许愿:
```bash
<cli> complete-wish --db-path <music_db_path> --wish-id <wish_id> \
  [--ingested-song-id <song_id>] [--ai-reply "..."] [--admin-note "..."]
```

索引维护（自动检测 music DB）:
```bash
<cli> ensure-indexes --db-path <content_db_path>
```

验证:
```bash
<cli> db --db-path <music_db_path> query-rows songs --limit 5
<cli> db --db-path <music_db_path> list-indexes songs --with-stats
```

参考: `cli/src/commands/write_music.rs:10-23` (WriteMusicOptions), `cli/src/commands/ensure_indexes.rs`
