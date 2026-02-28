# Flow B: Source Normalization

## Goal
Produce readable canonical Markdown from any source shape.
Prefer original Markdown source over HTML extraction whenever possible.

## Step 0: Discovery and retry (mandatory for URL input)

→ Rule 2: web tools first, no `curl`/`wget` as primary path.

Retry policy:
1. Do not fail on single timeout.
2. Retry canonical extraction at least 3 times with short backoff (~2s, ~5s, ~10s).
3. Record every attempt in `source_discovery.json` (method, URL, attempt index, outcome).

Probe sequence (stop on first full-source success):
1. Direct Markdown probe (`<url>.md`, `<url>.mdx`).
2. In-page source link scan (`.md`, `.mdx`, `raw`, `view source`, `edit`, `amp`, `rss`, `feed`).
3. Repo/source inference when page links to GitHub.
4. Content-negotiation attempt (`Accept: text/markdown`, if supported).

If Markdown source is found: save as `source_raw.md`, normalize into `source_canonical_<lang>.md`.

## Step 1: HTML/mixed extraction fallback
When direct Markdown is unavailable, extract body from HTML using adaptive paths:
1. Semantic container extraction (`article`, `main`, content blocks).
2. Readability-style extraction.
3. Structured payload extraction (JSON-LD/article-body signals).
4. Section stitching fallback for noisy layouts.

Cleanup rules:
1. Preserve heading hierarchy and argument order.
2. Keep examples, figure context, and caption meaning.
3. Normalize callouts/code fences/details blocks to render-safe Markdown.
4. Rewrite local relative links to project-valid targets.
5. Keep evidence artifacts; do not fabricate missing sections.

## Step 2: Best-effort before refusal
Complete all downgrade stages before refusing:
1. Retry same source path (transient-timeout hypothesis).
2. Downgrade extractor path on same source (semantic → readability → structured).
3. Downgrade source path (canonical variants, search variants, mirrors).

Refusal is allowed only after cross-path failure convergence.

## Quality Gate
Canonical Markdown is acceptable only if:
1. Major sections are complete; key definitions/arguments present.
2. Figures and captions keep their information density.
3. Boilerplate/CTA noise is controlled.
4. Local relative links are rewritten or explicitly documented.
5. Markdown structure is stable (no unclosed fences, orphan tags, callout spillover).
6. Anti-summary gate: canonical text is the article body itself, not commentary. → Rule 3
7. Coverage gate: section headings aligned with source; image handling documented.
8. Refusal gate: if blocked, produce `source_extraction_blocker.md` with attempt matrix, causal chain, and dominant cause label. → Rule 8

## Output
- `source_discovery.json` (if URL input)
- `source_canonical_<source_lang>.md`
- `source_extraction_blocker.md` (when blocked)
