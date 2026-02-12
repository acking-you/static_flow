# Type Taxonomy

This document defines the cross-domain article type system used by `article-summary-architect`.

## 1) Label Format

Primary label format:
- `Domain.Intent`

Examples:
- `Tech.Postmortem`
- `Learning.Tutorial`
- `Opinion.Narrative`

Only one primary label is allowed in final output.

## 2) Domain Layer

1. `Tech`
- Core content is engineering systems, source code, architecture, debugging, infra, or data systems.

2. `Product`
- Core content is product strategy, user flows, feature design, prioritization, roadmap tradeoffs.

3. `Business`
- Core content is market analysis, business model, go-to-market, operations, org/process decisions.

4. `Learning`
- Core content is knowledge building, study notes, concept maps, personal learning paths.

5. `Opinion`
- Core content is viewpoint, argumentation, reflective writing, essays, narrative reasoning.

## 3) Intent Layer

1. `Postmortem`
- Trigger signals: incident timeline, symptom, root cause, fix, prevention, regression.

2. `Tutorial`
- Trigger signals: prerequisites, step-by-step operations, command sequence, expected output.

3. `Implementation`
- Trigger signals: design constraints, data/control flow, component boundaries, tradeoff details.

4. `Comparison`
- Trigger signals: dimensions, alternatives, pros/cons, selection rationale.

5. `Research`
- Trigger signals: question framing, source evidence, unknowns, confidence/limitations.

6. `Checklist`
- Trigger signals: ordered action items, pass/fail criteria, runbook style.

7. `Narrative`
- Trigger signals: argument-driven or story-driven progression with reflections and recommendations.

## 4) Rule-First Classification

## Signals to score
1. Title keywords.
2. Section heading keywords.
3. Artifact signals (code block density, command density, comparison tables, references).
4. Language cues (`root cause`, `tradeoff`, `step`, `why`, `lessons learned`, etc.).

## Decision
1. Choose top-1 by score.
2. If top-1 and top-2 are close (`delta < 0.15`), use model correction.
3. If still ambiguous, fallback to rule top-1.

## Output
- Always emit one label.
- Keep this label as internal guidance by default.
- In final summary text, prefer natural reader-facing phrasing (for example: "这是一篇xxx文章"), not schema-like type exposition.

## 5) Anti-Patterns

Do not:
1. Output mixed labels.
2. Choose `Tech.*` only because code snippets exist.
3. Force `Postmortem` without incident + cause + fix evidence.
4. Reclassify based on author preference if text evidence disagrees.
