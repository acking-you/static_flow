# StaticFlow Publish Checklist

## 1. CLI and DB Preflight
Resolve CLI in order:
1. `./bin/sf-cli`
2. `./target/release/sf-cli`
3. `./target/debug/sf-cli`
4. `../target/release/sf-cli`
5. `sf-cli`

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

Init DB only with user approval:
```bash
<cli> init --db-path <db_path>
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

Check taxonomy rows:
```bash
<cli> db --db-path <db_path> query-rows taxonomies --limit 20
```

Check images rows:
```bash
<cli> db --db-path <db_path> query-rows images --limit 20
```

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

## 6. Immediate Storage Reclaim (Prune Now)
Single table:
```bash
<cli> db --db-path <db_path> cleanup-orphans --table <table>
```

All managed tables:
```bash
<cli> db --db-path <db_path> cleanup-orphans
```

## 7. Failure Recovery
1. Keep DB path and input files unchanged.
2. Re-run only the failed command.
3. Re-run verification commands.
4. Report what succeeded, what failed, and next safe command.
5. If image resolution fails, add/fix `--media-root` and retry.

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
