---
name: external-blog-repost-publisher
description: >-
  Import external blog posts into StaticFlow as compliant reprints:
  fetch source Markdown, sanitize promotional sections, add reprint notice,
  apply date policy, ingest referenced images, rewrite image links to
  `images/<id>`, fill missing bilingual fields, publish to LanceDB, and verify.
---

# External Blog Repost Publisher

Use this skill to ingest an external article into StaticFlow as a reprint with
clear attribution and reproducible CLI steps.

## When To Use
Use this skill when the user asks to:
1. Import an article from an external URL/repository into LanceDB.
2. Keep original author attribution but remove promotion/traffic-driving content.
3. Add a reprint notice at the beginning of the article.
4. Normalize article date policy (usually use current date for site display).
5. Publish and verify through `sf-cli`.

## Required Companion Skills
1. Required: `../staticflow-cli-publisher/SKILL.md`
2. Required when bilingual fields are missing:
   `../article-bilingual-translation-publisher/SKILL.md`

## Hard Rules
1. Keep explicit attribution:
   - original source URL
   - original author
2. Preserve technical content integrity:
   - keep core arguments, code, examples, and non-promotional links.
3. Remove promotional/traffic-driving content:
   - paid courses, QR/WeChat follow blocks, invite/referral sections,
     unrelated ads, or self-marketing CTAs.
4. Reprint notice must be the first visible block in both `content` and
   `content_en` (when English exists).
5. Date policy:
   - default: set `articles.date` to current local date (`YYYY-MM-DD`).
   - if original date is retained, mention it explicitly in the top reprint notice.
   - recommended: always include original publication date in the notice.
6. Store intermediate artifacts under `tmp/`; do not run destructive DB commands.
7. Every article image must be imported into `images` and rewritten to
   `images/<sha256_id>` unless user explicitly requires keeping remote URLs.
8. Never trust file extension alone for image ingestion:
   - require HTTP 200
   - reject `text/*` or `application/json` response types
   - reject placeholder payloads such as `404: Not Found`
   - require local decode check (`file`/image decoder) before import

## Inputs
At least one source input is required:
1. External URL (GitHub/GitBook/blog URL), or
2. Local Markdown file path.

Optional inputs:
1. Target article id/slug.
2. Target DB path (default content DB).
3. Category/tags/summary overrides.
4. Explicit date policy override (use current date vs retain original date).

## Recommended Workflow

### Step 1. Resolve source and workspace
1. Create workspace:
   - `tmp/external_repost/<article_id>/`
2. Save source material:
   - `source.md`
   - `source_url.txt`
3. If source is GitHub `blob` URL, convert to raw content URL for fetching.

### Step 2. Extract baseline metadata
Collect:
1. `title`
2. original author
3. original publication date (if available)
4. candidate slug/id
5. source language

If any required publish field is missing, infer it from content:
1. `summary`
2. `tags`
3. `category`
4. `category_description`

### Step 3. Sanitize reprint body
Prepare `sanitized.md`:
1. Remove promotion/lead-generation sections.
2. Remove author marketing blocks unrelated to technical substance.
3. Keep all technical code, diagrams, and references needed for comprehension.
4. Keep Markdown render-safe:
   - heading hierarchy
   - fenced code blocks
   - tables
   - links/images

### Step 4. Inject top reprint notice
Insert a notice as the first block in `sanitized.md`.

Chinese example:
```md
> Reprint Notice (转载提示): This article is reprinted from [original URL](...).
> Original author: **AUTHOR**. Original publication date: **YYYY-MM-DD**.
> Promotional/traffic-driving parts were removed according to this site's policy.
```

English example (for `content_en`):
```md
> Reprint Notice: This article is reprinted from [original URL](...).
> Original author: **AUTHOR**. Original publication date: **YYYY-MM-DD**.
> Promotional and traffic-driving sections from the original were removed.
```

### Step 5. Apply date policy
1. Default policy:
   - `articles.date = <today>`
2. Keep original date only when user explicitly asks.
3. Regardless of DB date choice, preserve original date in reprint notice when known.

### Step 6. Ingest and rewrite images (mandatory when images exist)
Prepare image-safe markdown before publish:
1. Extract all markdown image references from `sanitized.md`:
   - standard markdown images: `![alt](url)`
   - Obsidian embeds if present: `![[path]]`, `![[path|alias]]`
2. Resolve image sources:
   - absolute URLs (`http/https`): download to
     `tmp/external_repost/<article_id>/assets/`
   - root-relative paths (for example `/blog_imgs/a.png`): resolve against
     source site origin and download locally
3. Validate downloads before import:
   - status must be HTTP 200
   - payload must be image-like (not text/html/plain/json)
   - file must decode as image (`file` or image loader check)
4. Rewrite markdown image links to local relative paths (for example
   `assets/<filename>`). This is required because leading `/...` paths are not
   treated as local import targets by `sf-cli`.
5. Keep a local mapping artifact:
   - `image_map.tsv` with columns:
     `original_url_or_path<TAB>local_path<TAB>final_image_id`

### Step 7. Generate/refresh bilingual fields
1. If `content_en` or `detailed_summary.zh/en` is missing or stale, run
   `article-bilingual-translation-publisher` flow.
2. Keep bilingual summary aligned to final sanitized content.

### Step 8. Publish to LanceDB
Preferred path:
1. Write processed markdown to local file (for reproducibility).
2. Publish with `sf-cli write-article` (plus explicit metadata args when
   needed).
3. If the article contains local image links, use `--import-local-images`
   and `--media-root`.

Example:
```bash
<cli> write-article \
  --db-path <db_path> \
  --file tmp/external_repost/<article_id>/sanitized.md \
  --import-local-images \
  --media-root tmp/external_repost/<article_id> \
  --generate-thumbnail \
  --summary "..." \
  --tags "tag1,tag2" \
  --category "..." \
  --category-description "..."
```

If only targeted patching is needed on an existing row, use guarded updates:
```bash
<cli> db --db-path <db_path> update-rows articles \
  --set "date='<yyyy-mm-dd>'" \
  --set "content=replace(content, '<old>', '<new>')" \
  --where "id='<article_id>'"
```

### Step 9. Verify publication
Run:
1. `get-article <article_id>`
2. check top notice in `content` and `content_en`
3. check `date` policy result
4. check author/source attribution presence
5. ensure promotional sections are absent
6. verify image links in final `content` are rewritten to `images/<64hex>`
7. query `images` table for those ids and confirm rows exist
8. ensure no unresolved markdown image links remain (`http...`, `/blog_imgs/...`,
   `assets/...`) unless explicitly allowed

## Output Report Requirements
Always report:
1. article id
2. source URL
3. original author/date captured
4. applied DB date
5. sanitized scope (what was removed)
6. image ingestion summary:
   - discovered image links
   - imported image count
   - rewritten link count
   - final image ids
7. bilingual field status (`content_en`, `detailed_summary.zh/en`)
8. verification command results
9. artifact paths under `tmp/external_repost/<article_id>/`

## Failure and Recovery
1. If source fetch fails, save error details and keep a manual fallback path.
2. If publish fails, keep sanitized artifacts and provide exact rerun commands.
3. If post-publish rendering breaks, patch the markdown structure first, then re-publish.
4. Never delete tables/rows as part of this workflow unless user explicitly requests it.
5. If image import writes wrong bytes (for example placeholder `404` body):
   - redownload to a new local folder path
   - rewrite markdown to this new local path
   - re-run publish with `--import-local-images`
   - verify final `images/<id>` links changed to the new correct ids
