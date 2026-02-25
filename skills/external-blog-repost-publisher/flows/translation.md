# Flow C: Translation and Style Adaptation

## Goal
Translate with high fidelity and natural readability while preserving source voice.

## Step 1: Classify article + tone (mandatory)
Before translating, identify:
1. Article category:
   - technical deep dive / tutorial / news analysis / opinion / product narrative / others
2. Writing voice:
   - formal, concise, explanatory, witty, humorous, storytelling, etc.
3. Reader profile:
   - engineers, practitioners, general audience, mixed audience.

## Step 2: Decide translation path
1. If `source_lang == target_lang`: no translation, only readability polish.
2. If different: translate from `source_canonical_<source_lang>.md`.

## Step 3: Translate with style fidelity
1. Keep facts unchanged:
   - numbers, units, comparisons, entity names, links, code intent.
2. Keep structure meaningful:
   - translate headings, keep section logic and list semantics.
   - keep source information order unless target-language grammar forces minor reordering.
3. Preserve style:
   - keep humor/analogy/rhythm where possible,
   - avoid rigid literal wording that reads unnatural in target language.
4. Keep figure explanations concrete:
   - do not replace specific captions with vague summaries.
5. Natural expansion is allowed:
   - add concise transitions or clarifications when needed for fluent reading.
   - keep the expansion stylistically aligned with the source voice.
6. Expansion boundary:
   - do not invent facts, numbers, or positions not supported by source text.
   - do not let expansion replace or dilute source-critical information.

## Step 4: Quick quality check
1. No missing key section (especially definitions and conclusions).
2. Tone still resembles source article.
3. Target audience can read smoothly without "machine translation" feeling.
4. Expansion quality:
   - added wording feels natural, not bloated.
   - key facts/constraints are fully preserved after expansion.

## Common Failure Patterns (must avoid)
1. Reordering logic so heavily that argument flow changes.
2. Expansion that sounds fluent but weakens factual precision.
3. Adding speculative explanations that shift the author's stance.
4. Keeping words but dropping key constraints, numbers, or qualifiers.

## Output
- `content_<target_lang>.md`
- optional bilingual counterpart when user explicitly requests both languages.
