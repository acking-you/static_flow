# Flow D: Publish and Verify

## Goal
Write updates via `sf-cli` and verify field-level correctness.

## Pre-publish
1. Remove pure traffic-driving blocks, keep substantive content.
2. Add reprint notice at top of each updated language field.
3. Generate/update `detailed_summary.zh/en` by default.
   - Use `article-bilingual-translation-publisher`; do not handcraft inline body brief blocks as default behavior.
4. Import images and rewrite links to `images/<id>` unless user says otherwise.

## Publish
1. Prefer `sf-cli write-article` for create/overwrite.
2. Use `sf-cli db update-article-bilingual` for language-field patching.
3. Preserve non-target fields exactly as requested in Flow A.

## Verification Checklist
1. Target article exists and target fields are updated.
2. Reprint notice exists in updated language field(s).
3. `detailed_summary.zh/en` exists and is valid when summary update is in scope.
4. No unresolved local asset links remain (unless explicitly allowed).
5. Image IDs referenced by content exist in `images` table.
6. Critical sections/captions are preserved after write-back.
7. Long-doc structure checks pass in DB-fetched content (not only local files):
   - balanced code fences,
   - paired `<details>/<summary>` tags,
   - no raw `!!!note`/broken callout markers,
   - no known high-risk mistranslation tokens.

## Report
Return:
1. article id,
2. changed vs preserved fields,
3. source/target languages,
4. image import status,
5. verification result.
