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
- Required for translation + detailed summary generation: `../article-bilingual-translation-publisher/SKILL.md`

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
   - **Markdown source discovery is mandatory for URL inputs** (see Flow B Step 0):
     probe for direct Markdown before falling back to HTML extraction.
4. Translation path is language-aware:
   - If `source_lang == target_lang`, skip translation.
   - If different, build high-quality canonical Markdown in source language first, then translate.
5. English source retention is mandatory:
   - If source language is English, `content_en` must be written and preserved as source-truth English content.
   - Do not publish a Chinese-only result for English-source materials.
   - If user requests Chinese import for an English source, publish `content` (Chinese) and `content_en` (original/refined English) together.
6. Preserve critical information:
   - definitions, arguments, numbers, comparisons, captions, links, code intent.
   - Do not replace specific evidence with generic text.
7. Natural expansion is allowed when it improves readability:
   - expansion should be fluent and audience-friendly.
   - expansion must not introduce conflicting claims or drop source facts.
8. Keep original argument flow as much as possible:
   - minor reordering is fine for target-language grammar.
9. Execution boundary for this skill:
   - Do not explore unrelated repository code/context (backend/frontend/source code deep dives).
   - Use publishing artifacts + `sf-cli` verification as the primary and sufficient context.
   - If a task requires code/context exploration, switch out of publisher workflow instead of expanding this skill's scope.
10. Update only requested fields; preserve all others.
11. Remove promotional traffic-driving blocks, keep substantive content.
12. Images should be ingested and rewritten to `images/<id>` unless user explicitly allows remote links.
13. Reprint notice must appear at top of each updated language field.
14. Generate/update `detailed_summary.zh/en` by default:
   - use `article-bilingual-translation-publisher` workflow for summary generation.
   - do not handcraft ad-hoc inline brief blocks in article body by default.
   - only skip summary update when user explicitly asks to keep existing summary.
15. Always verify after write-back.
16. Local relative-link cleaning is mandatory during normalization:
   - detect Markdown links using local relative paths (for example `./x.md`, `../y/`, `article/...`, `articles/...`),
   - preserve external links,
   - rewrite local links to project-valid targets (typically `/posts/<id>` for article routes, `images/<id>` for ingested images),
   - ensure no unresolved local-relative links remain unless user explicitly allows them.
17. Long-article quality gates are mandatory (especially spec/docs pages):
   - structure gate: code fences are balanced, `<details>/<summary>` tags are paired and render-safe, and no broken block spills over into following sections,
   - callout gate: raw markers like `!!!note` must be rendered into readable Markdown form (or preserved safely when target renderer supports them),
   - heading gate: translated output must cover title and section headings; do not leave major heading clusters untranslated,
   - fidelity gate: no paragraph-level information loss (definitions, constraints, examples, caveats, captions),
   - terminology gate: run targeted checks for domain-critical phrases to avoid semantic mistranslation (for example `Writer Fencing`-class terms).
18. Publish path must be deterministic:
   - if this publisher skill is selected, avoid unrelated codebase exploration and execute the skill flow directly,
   - if deep repository exploration is required, switch workflow instead of stretching this publisher scope.

## Minimal Workflow

1. Run Flow A to lock scope (`id`, language fields, overwrite policy).
2. Run Flow B to produce readable canonical Markdown from source.
   - Step 0: probe for direct Markdown source (URL suffix, HTML button scan, GitHub repo inference, content negotiation).
   - Step 1: if no Markdown found, fall back to adaptive HTML extraction + cleanup.
3. Run Flow C for style-aware translation (only when needed).
4. Run Flow D for publish and verification.
   - For English-source materials, ensure `content_en` is written in this run.
   - Ensure `detailed_summary.zh/en` is updated unless user explicitly disables it.
   - For long documents, run structure + terminology checks before write-back and again after write-back.
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
- `source_discovery.json` (Markdown source probe results)
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
