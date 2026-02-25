---
name: external-blog-repost-publisher
description: >-
  Repost external blog articles into StaticFlow with style-aware translation,
  adaptive source normalization, image ingestion, and verified publish updates.
---

# External Blog Repost Publisher

Use this skill to ingest or update external articles in StaticFlow while keeping:
- attribution and traceability,
- factual fidelity,
- readable Markdown,
- source writing style and target-reader comfort.

## Companion Skills
- Required: `../staticflow-cli-publisher/SKILL.md`
- Translation quality reference: `../article-bilingual-translation-publisher/SKILL.md`

## Flow Routing

| Task | Flow | File |
|---|---|---|
| Scope and write-back boundaries | A | [flows/scope.md](flows/scope.md) |
| Source normalization (Markdown/HTML/mixed) | B | [flows/source-normalization.md](flows/source-normalization.md) |
| Translation + style adaptation | C | [flows/translation.md](flows/translation.md) |
| Publish + verification | D | [flows/publish-verify.md](flows/publish-verify.md) |

## Core Rules

1. First classify the article and its tone before translation.
   - Identify article category (technical deep dive, tutorial, news analysis, opinion, etc.).
   - Identify tone/voice (formal, concise, witty, narrative, humorous, marketing-lite, etc.).
   - Translation must preserve this voice while staying natural for target readers.
2. Never lock output to a single fixed style (for example, always "technical blog style").
3. Source handling must be adaptive, not hard-coded.
   - HTML-to-Markdown is strategy-based; pick the path that yields best readability/fidelity.
4. Translation path is language-aware:
   - If `source_lang == target_lang`, skip translation.
   - If different, build high-quality canonical Markdown in source language first, then translate.
5. Preserve critical information:
   - definitions, arguments, numbers, comparisons, captions, links, code intent.
   - Do not replace specific evidence with generic text.
6. Natural expansion is allowed when it improves readability:
   - expansion should be fluent and audience-friendly.
   - expansion must not introduce conflicting claims or drop source facts.
7. Keep original argument flow as much as possible:
   - minor reordering is fine for target-language grammar.
8. Update only requested fields; preserve all others.
9. Remove promotional traffic-driving blocks, keep substantive content.
10. Images should be ingested and rewritten to `images/<id>` unless user explicitly allows remote links.
11. Reprint notice must appear at top of each updated language field.
12. Always verify after write-back.

## Minimal Workflow

1. Run Flow A to lock scope (`id`, language fields, overwrite policy).
2. Run Flow B to produce readable canonical Markdown from source.
3. Run Flow C for style-aware translation (only when needed).
4. Run Flow D for publish and verification.
5. Report: changed fields, preserved fields, fidelity/style checks, verification result.

## Inputs

At least one source input is required:
- External URL,
- local Markdown,
- or local HTML.

Optional:
- target `article_id`,
- db path,
- target language(s),
- field scope (`content` only vs `content + content_en`),
- metadata override.

## Suggested Workspace
- `/tmp/external_repost/<article_id>/`

Suggested artifacts:
- `source_url.txt`
- `source_raw.*`
- `source_canonical_<lang>.md`
- `content_<lang>.md` (for updated language fields)
- `image_map.tsv`
- `publish_verify.json` (or equivalent check output)

## Safety Notes
- Do not publish raw HTML into `articles.content`.
- Do not run destructive DB/table operations in this workflow.
- If style fidelity or factual fidelity is not acceptable, do not publish; iterate or report blocker.
