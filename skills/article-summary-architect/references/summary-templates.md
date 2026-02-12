# Summary Thinking Reference

This file provides thinking examples for summary generation.
These are references, not mandatory templates.

## 1) Minimal Thinking Path

Before writing, lock these four answers:
1. What is the article's main problem?
2. What is the core conclusion?
3. Which reasoning path is essential to keep?
4. What boundaries or risks must be stated?

Then output:
1. One natural定位句 (e.g., "这是一篇xxx文章"/"This is a ... article").
2. 3-5 semantic sections.
3. Concise high-information bullets under each section.
4. Bilingual equivalents (`zh` and `en`).

## 2) Ordering Heuristics by Type (Flexible)

1. `*.Postmortem`
- Symptom -> root cause -> fix -> why fix works -> regression guard

2. `*.Tutorial`
- Goal -> prerequisites -> key steps -> expected output -> pitfalls

3. `*.Implementation`
- Goal -> constraints -> design path -> tradeoffs

4. `*.Comparison`
- Compared options -> dimensions -> key differences -> selection guidance

5. `*.Research`
- Question -> evidence -> finding -> confidence -> open questions

6. `*.Narrative`
- Position -> rationale -> counterpoint -> practical advice

## 3) Example (For Reference Only)

Example article type: `Tech.Postmortem`

Possible summary style:
这是一篇前端故障复盘文章。

### 现象与误判
- 页面仅显示背景层，主体不稳定。
- 直觉上像后端或网络问题，排障方向容易跑偏。

### 根因与修复
- 根因是 `Location` 依赖语义不稳定导致 effect 循环触发。
- 修复需要同时满足：稳定依赖键 + 同值写入幂等保护。

### 验证与经验
- 回归需覆盖同 URL 重入、路由往返、参数保持。
- 可复用原则：依赖表达逻辑身份，状态写入默认幂等。

The same content can be rewritten with different headings.
Variation is allowed as long as the reasoning chain stays intact.

## 4) Style Guardrails

1. Target 8-14 bullets per language.
2. Keep each bullet short but specific.
3. Avoid decorative prose and vague praise language.
4. Prefer concrete constraints over generic statements.
5. Do not output a flat bullet wall; always group by semantic sections.

## 5) Non-Technical Adaptation Rule

For non-technical domains:
1. Do not force technical framing.
2. Remove engineering-only wording unless evidence exists.
3. Do not inject fake "root cause" or "regression test" language.
4. Use natural reader-facing language rather than schema-like labels.
