---
name: article-summary-architect
description: >-
  Generate bilingual article briefs for StaticFlow posts using a
  thinking-first workflow: classify article type first, then compress
  the content into concise, evidence-grounded bullet points.
---

# Article Summary Architect

Create robust, reader-friendly quick briefs (`detailed_summary`) for blog posts and notes.

## When To Use

Use this skill when the user asks to:
1. Add or rewrite `detailed_summary` in article frontmatter.
2. Improve summary quality/readability/structure.
3. Improve consistency without forcing one rigid writing template.
4. Repair a misleading or noisy "quick brief".

---

## Load Extra Context

Read these files before writing summaries:
1. `references/type-taxonomy.md`
2. `references/summary-templates.md`
3. `references/quality-checklist.md`

If a summary is being prepared for publication, coordinate with:
- `../staticflow-cli-publisher/SKILL.md`

---

## Core Workflow (Mandatory)

### Step 1: Classify Article Type First

Use a two-level label:
- Domain: `Tech | Product | Business | Learning | Opinion`
- Intent: `Postmortem | Tutorial | Implementation | Comparison | Research | Checklist | Narrative`

Output exactly one primary type:
- `Domain.Intent` (example: `Tech.Postmortem`)

Classification method:
1. Rule-based scoring first (title, headings, section signals, artifact style).
2. Apply model correction only when top-1 and top-2 scores are too close.
3. If uncertain after correction, fallback to rule-based top-1.

Do not emit mixed types in internal reasoning.
Type classification is for internal guidance by default, not mandatory output text.

### Step 2: Decide What Must Survive Compression

Before writing bullets, answer these questions from the article body:
1. What exact problem/question is this article solving?
2. What is the core conclusion?
3. What is the minimum reasoning path that makes the conclusion trustworthy?
4. What validation or boundary conditions matter to readers?

Do not force a fixed section template.
Section headers are optional and only used when they improve readability.

### Step 3: Produce Concise Bullet Plan

Write a compact bullet plan first, then finalize bilingual output:
1. Start with one natural定位句:
   - zh example: `这是一篇前端故障复盘文章。`
   - en example: `This is a frontend incident postmortem article.`
2. Use 3-5 semantic sections, each with 2-4 concise bullets.
3. Keep only high-information bullets.
4. Default to 8-14 bullets per language (adjust by article complexity).
5. Prefer short, concrete points over long narrative paragraphs.

Type-informed ordering guidance (not a hard template):
- `*.Postmortem`: symptom -> root cause -> fix -> why fix works -> regression guard
- `*.Tutorial`: goal -> prerequisites -> key steps -> expected output -> pitfalls
- `*.Comparison`: compared options -> dimensions -> differences -> selection advice

Use `references/summary-templates.md` as examples, not as mandatory structure.
Do not output rigid labels like `Type: Domain.Intent` unless the user explicitly asks for it.

### Step 4: Enforce Quality Gates

Before finalizing:
1. Each key conclusion must map to evidence in article body.
2. No fabricated metrics or claims.
3. No irrelevant technical fields forced into non-technical posts.
4. Bilingual outputs must be semantically aligned.
5. Keep high information density while preserving readability and natural tone.

Apply all checks in `references/quality-checklist.md`.

---

## Output Contract

Return frontmatter-ready content:

```yaml
detailed_summary:
  zh: |
    这是一篇……文章。
    ### 小节A
    - ...
    ### 小节B
    - ...
  en: |
    This is a ... article.
    ### Section A
    - ...
    ### Section B
    - ...
```

Guidelines:
1. Target 8-14 bullets per language (not fixed).
2. Keep concise, specific, evidence-grounded statements.
3. Maintain markdown compatibility with frontend popup rendering.
4. Must include semantic sectioning; avoid a flat bullet wall.
5. Avoid rigid formatting rules; prioritize scannability.

---

## Error Handling

1. If source article lacks enough evidence for a claim, explicitly mark uncertainty.
2. If article type is ambiguous, keep one primary internal type for reasoning and avoid noisy type exposition unless user asks.
3. If existing summary is partially useful, refactor instead of discarding all content.

---

## Integration Rule With Publisher

When used before publication:
1. Generate or refresh `detailed_summary.zh/en` first.
2. Then hand off to `staticflow-cli-publisher` for write/sync.
3. In publish report, include:
   - Brief quality check result
   - Whether summary was regenerated or reused
