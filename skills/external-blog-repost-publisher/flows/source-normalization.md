# Flow B: Source Normalization

## Goal
Produce readable canonical Markdown from any source shape.
Prefer original Markdown source over HTML extraction whenever possible.

## Step 0: Markdown Source Discovery (mandatory for URL inputs)

Before extracting content from HTML, actively probe for a direct Markdown source.
A clean Markdown source is always superior to HTML-to-Markdown conversion.

### Probe sequence (run in order, stop on first success)

1. **URL suffix probe** — request `<original_url>.md` and check response:
   - `Content-Type: text/markdown` → direct Markdown source confirmed.
   - Also try `.mdx` if `.md` returns 404.
   - Example: `https://clickhouse.com/blog/fivetran-connector-beta` → append `.md` → returns full Markdown with frontmatter.

2. **HTML button/link scan** — fetch the original HTML page and search for:
   - Buttons or links with text matching: `View as Markdown`, `Edit on GitHub`, `Edit this page`, `View source`, `Raw`, `Suggest an edit`.
   - `<a>` tags whose `href` contains `.md`, `.mdx`, or `raw.githubusercontent.com`.
   - `data-*` attributes on buttons that encode a source URL.
   - Note: these elements are often in dropdown menus or hidden behind JS toggles; scan the full HTML source, not just visible text.

3. **GitHub repo inference** — if the page contains a GitHub repository link:
   - Extract the repo owner/name (e.g. `ClickHouse/clickhouse-docs`).
   - Derive a candidate path from the URL slug:
     - strip domain and leading path segments (e.g. `/blog/my-post` → `my-post`),
     - try common content directories: `blog/`, `content/blog/`, `_posts/`, `src/content/`, `docs/`, `website/blog/`.
   - Use GitHub API or raw URL to verify: `https://raw.githubusercontent.com/<owner>/<repo>/<default_branch>/<candidate_path>.md`
   - If the repo has a clear blog content structure, navigate it to find the matching file.

4. **Content negotiation** — send `Accept: text/markdown` header to the original URL:
   - Some CMS platforms (Hugo, Docusaurus, custom) honor this and return Markdown directly.

### Discovery output
- If a Markdown source is found, save it as `source_raw.md` and skip HTML extraction entirely.
- Record the discovery method and source URL in `source_discovery.json`:
  ```json
  {
    "method": "url_suffix_probe",
    "markdown_url": "https://example.com/blog/post.md",
    "original_url": "https://example.com/blog/post",
    "has_frontmatter": true
  }
  ```
- If all probes fail, proceed to HTML extraction (Step 1 below) as normal.

## Step 1: Detect and extract (adaptive, not fixed)
1. Detect source type:
   - Markdown (from discovery or local file): keep structure, apply light cleanup.
   - HTML/mixed: try one or more extraction paths and keep the best result.
2. Candidate paths for HTML:
   - semantic container extraction (`article`, `main`, content blocks),
   - readability-style main-content extraction,
   - section-by-section fallback (manual stitching) when structure is noisy.
3. Cleanup pass:
   - preserve heading hierarchy,
   - merge broken wrapped prose,
   - normalize callouts/lists/tables/code fences for Markdown readability,
   - normalize admonitions/callouts (for example `!!! note`) into render-safe Markdown blocks when needed,
   - keep `<details>/<summary>` blocks valid and readable (no fence/tag breakage),
   - keep image captions and surrounding explanation text,
   - normalize local relative links to project-valid paths:
     - keep external links unchanged,
     - rewrite local article links to `/posts/<id>` style targets,
     - rewrite local image links to `images/<id>` after image ingestion/mapping.

## Quality Gate
Canonical Markdown is acceptable only if:
1. Major sections are complete.
2. Key definitions/arguments are present.
3. Figures and captions keep their information density.
4. Boilerplate/CTA noise is controlled.
5. Local relative links are either rewritten or explicitly documented as intentionally preserved.
6. Markdown structure is stable end-to-end:
   - no unclosed/duplicated code fences,
   - no orphan `<details>` or `<summary>` tags,
   - no callout/body spillover that accidentally turns later prose into a code block.

## Output
- `source_discovery.json` (discovery metadata, if URL input)
- `source_canonical_<source_lang>.md`
