# Flow D: Publish and Verify

## Goal
Write updates via `sf-cli` and verify field-level correctness.

## Pre-publish
1. Remove pure traffic-driving blocks, keep substantive content.
2. Add reprint notice at top of each updated language field. → Rule 6
3. Generate/update `detailed_summary.zh/en` via `article-bilingual-translation-publisher`. → Rule 7
4. Import images and rewrite links. → Rule 8
5. Apply repost date policy. → Rule 6
6. Taxonomy pass (when in scope). → Rule 7

## Publish
1. Prefer `sf-cli write-article` for create/overwrite.
2. Use `sf-cli db update-article-bilingual` for language-field patching.
3. Preserve non-target fields exactly as requested in Flow A.

## `sf-cli` Failure Recovery
Follow `references/sf-cli-troubleshooting.md`. Prefer targeted patch commands for narrow scope. Record failures in `cli_diagnostics.log`.

## Verification Checklist
1. Target article exists; target fields updated; out-of-scope fields preserved.
2. Reprint notice with clickable source URL and source date context. → Rule 6
3. `detailed_summary.zh/en` valid when summary is in scope. → Rule 7
4. Asset integrity: no unresolved local links; referenced image IDs exist.
5. Content fidelity: critical sections/examples preserved; body is translation, not recap. → Rule 3
6. `articles.date` matches repost date policy. → Rule 6
7. Taxonomy quality (when changed): aligned with article body, no blind source copy. → Rule 7
8. Long-doc structure: balanced code fences, paired `<details>/<summary>`, no broken callout markers.
9. Source-grounding evidence exists (`source_discovery.json` + extracted artifact). → Rule 3
10. If blocked, `source_extraction_blocker.md` exists with causal chain + dominant cause. → Rule 8

## Report
Return: article id, changed vs preserved fields, source/target languages, image import status, verification result.
