# Flow B: Source Normalization

## Goal
Produce readable canonical Markdown from any source shape.

## Strategy (adaptive, not fixed)
1. Detect source type:
   - Markdown: keep structure, apply light cleanup.
   - HTML/mixed: try one or more extraction paths and keep the best result.
2. Candidate paths for HTML:
   - semantic container extraction (`article`, `main`, content blocks),
   - readability-style main-content extraction,
   - section-by-section fallback (manual stitching) when structure is noisy.
3. Cleanup pass:
   - preserve heading hierarchy,
   - merge broken wrapped prose,
   - normalize callouts/lists/tables/code fences for Markdown readability,
   - keep image captions and surrounding explanation text.

## Quality Gate
Canonical Markdown is acceptable only if:
1. Major sections are complete.
2. Key definitions/arguments are present.
3. Figures and captions keep their information density.
4. Boilerplate/CTA noise is controlled.

## Output
- `source_canonical_<source_lang>.md`
