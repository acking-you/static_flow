# Interactive Repost Repair Design

Date: 2026-04-03

## Scope

Repair the already-published interactive repost for:

- article id: `using-the-most-unhinged-avx-512-instruction-to-make-the-fastest-phrase-search-algo`
- page id: `ipg-using-the-most-unhinged-avx-512-instruction-to-make-the-fastest-phrase-search-algo`

Also tighten the `interactive-page-repost-publisher` skill so future publishes cannot repeat the same failure mode.

## Confirmed User Intent

The desired final shape is:

- `en` raw entry remains a pure English source-faithful mirror
- `zh` raw entry becomes a pure Chinese localized mirror
- bilingual comparison belongs in the article track, not inside the raw interactive entry
- the article page must be readable and searchable, with clean bilingual content
- existing ids and routes must remain stable

## Root Cause

The previous publish failed for two separate reasons:

1. Wrong localized-entry design
   - The `zh` localized entry was implemented as English source HTML plus an appended Chinese translation block.
   - That violates the intended meaning of a localized raw entry.
   - Result: Chinese and English were mixed in the same raw interactive page.

2. Wrong translation source of truth
   - The Chinese article and localized content reused already-published Markdown from LanceDB.
   - That stored Markdown was already contaminated by broken Jekyll macro conversion and malformed benchmark/table sections.
   - Result: the new publish preserved existing corruption instead of rebuilding from clean source HTML.

These are design failures, not styling bugs.

## Design

### 1. Article Track

The article row keeps the same `articles.id`, but its bilingual fields are regenerated from clean source inputs:

- `content`
  - fresh Chinese translation generated from source HTML / clean English extraction
- `content_en`
  - normalized English source article regenerated from source HTML
- `detailed_summary.zh/en`
  - regenerated from the rebuilt article content

Rules:

- Do not reuse the current broken `articles.content` as translation input.
- Do not reuse the current broken `articles.content_en` as normalization input unless it passes structure checks against source HTML.
- Bilingual comparison is allowed here because the article page is the search-and-reference track.

### 2. Interactive Track

The interactive page keeps the same `interactive_pages.id`.

#### Base locale

- `en` entry remains source-faithful.
- Preserve source DOM structure, code highlighting hooks, TOC behavior, images, and page-local scripts.
- All runtime assets must remain local under `/api/interactive-pages/<page_id>/assets/...`.

#### Chinese locale

- `zh` entry is a standalone Chinese localized mirror.
- It must not contain the full English source article inline.
- It should preserve the same page structure as closely as practical:
  - same heading tree
  - same code blocks
  - same tables
  - same image positions
  - same CSS classes and styling hooks when possible
- If some source strings are intentionally left untranslated, they must be limited to protocol names, code, identifiers, and proper nouns.

#### Explicit rejection

The following pattern is forbidden:

- English raw page with a second Chinese section appended below it
- Chinese raw page with English raw page embedded inside it
- Any raw localized entry that contains two full article languages mixed together

### 3. Bilingual Comparison Placement

Comparison belongs in article rendering, not raw entry rendering.

Acceptable locations:

- `articles.content` / `articles.content_en`
- article detail bilingual UI
- an explicit comparison view outside the raw interactive entry

Forbidden location:

- `/interactive-pages/<page_id>/entry?lang=<locale>`

### 4. Skill Contract Changes

`interactive-page-repost-publisher` must be updated so the following rules are explicit:

1. Raw localized entry must be single-locale
   - `/entry?lang=zh` must read as Chinese-first
   - `/entry?lang=en` must read as English-first
   - do not embed a full secondary-language article inside a raw localized entry

2. Bilingual comparison belongs to the article track
   - article body may be bilingual
   - raw interactive entry may not be bilingual-by-stacking

3. Translation source must be clean
   - if the currently published article content is malformed, do not reuse it as the translation source
   - rebuild from source HTML, clean source Markdown, or a verified clean extraction

4. Existing broken reposts require rebuild, not patch stacking
   - when the target article already contains malformed Markdown or corrupted bilingual content, regenerate the bilingual article artifacts from clean source before republishing

5. Localized locale QA must include language purity
   - verify the localized raw entry is not a mixed full-page bilingual stack
   - verify no obvious malformed macro residue, broken tables, or raw template markers remain

### 5. Write Strategy

Use the existing ids and replace the wrong data in place:

- keep `articles.id`
- keep `interactive_pages.id`
- replace `interactive_page_locales.zh`
- replace article bilingual fields
- preserve unrelated metadata where still correct

Avoid any unnecessary route or schema changes.

## Verification Requirements

### DB

- `articles.id = <article_id>` still exists
- `articles.article_kind = interactive_repost`
- `articles.interactive_page_id = ipg-<article_id>`
- `interactive_pages.status = ready`
- `interactive_pages.translation_scope = article_and_interactive`
- `interactive_page_locales` contains `zh`

### Route behavior

- `/interactive-pages/<page_id>?lang=en` returns 200
- `/interactive-pages/<page_id>?lang=zh` returns 200
- `/interactive-pages/<page_id>/entry?lang=en` returns 200
- `/interactive-pages/<page_id>/entry?lang=zh` returns 200

### Content quality

- `en` raw entry does not contain appended Chinese full-article content
- `zh` raw entry does not contain appended English full-article content
- `zh` raw entry has no malformed Jekyll macro residue
- bilingual article fields are clean enough to render tables, code fences, and section structure correctly
- all runtime assets are local

## Constraints

- Do not change article id or page id.
- Do not add a new third route just for this repair.
- Do not apply cosmetic patching on top of the current broken Chinese entry.
- Do not use the currently broken stored Markdown as the authoritative translation source.

## Out of Scope

- General taxonomy-table repair
- CLI merge semantics changes unrelated to this repost
- New frontend feature work beyond what is required to display the repaired article and mirror correctly

## Implementation Notes

- The current taxonomy merge issue is a separate storage/write-path problem and should not redefine the mirror/content design.
- If it blocks final article-field rewrite, the implementation plan must isolate the safe write path rather than silently accepting mixed-language raw entries.
