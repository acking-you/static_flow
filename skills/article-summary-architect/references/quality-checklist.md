# Quality Checklist

Run this checklist before finalizing `detailed_summary`.

## 1) Evidence
1. Are key conclusions clearly supported by article content?
2. Are there any fabricated facts, numbers, or timelines? (must be none)
3. Are any claims overstated beyond source evidence?

## 2) Reader Value
1. Can readers quickly understand what the article gives them?
2. Does the summary keep the core problem, conclusion, and practical takeaway?
3. Is redundant or low-information text removed?

## 3) Structure and Readability
1. Does the summary have a clear, scannable structure?
2. Is the opening sentence natural and reader-facing?
3. Are bullets concise and specific (not vague slogans)?
4. Is formatting markdown-safe for frontend rendering?

## 4) Bilingual Consistency
1. Are `zh` and `en` semantically aligned?
2. Are major points present in both languages?
3. Is tone/specificity broadly consistent across languages?

## 5) Fit and Boundaries
1. Is the wording appropriate for the article domain (technical or non-technical)?
2. Are caveats/limits included when needed?
3. Is there any unnecessary schema-style exposition? (avoid unless user asks)

## 6) Rewrite Triggers
Rewrite if any of the following occurs:
1. Generic summary that could fit many unrelated articles.
2. Claims not grounded in source text.
3. Major zh/en divergence.
4. Structure too flat to scan effectively.
