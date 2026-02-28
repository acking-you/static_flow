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
   - do not compress specific evidence into generic paraphrase.
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
7. Domain-term sanity pass (mandatory for long technical docs):
   - verify high-risk terms and phrases section-by-section,
   - fix mistranslations that change meaning (especially protocol/consistency/concurrency semantics).
8. Example-preservation pass (mandatory):
   - preserve concrete examples from source (tool names, command patterns, guardrail examples, exception cases),
   - do not reduce example-rich sections into high-level summary-only prose.
9. Narrative-frame fidelity:
   - keep the article's own narrative voice; do not rewrite body into reviewer/commentator framing,
   - avoid opening/section patterns like "this article explains..." unless the source itself uses that framing.

## Step 4: Quick quality check
1. No missing key section (especially definitions and conclusions).
2. No missing key paragraphs inside each section (paragraph-level coverage).
3. Tone still resembles source article.
4. Target audience can read smoothly without "machine translation" feeling.
5. Expansion quality:
   - added wording feels natural, not bloated.
   - key facts/constraints are fully preserved after expansion.
6. Heading and label quality:
   - title + section headings are translated/readable,
   - operation labels and figure notes remain semantically precise.
7. Professional readability:
   - final output must read like human-authored technical writing in target language,
   - avoid literal word-by-word phrasing when target language has clearer professional wording.

## Common Failure Patterns (must avoid)
1. Reordering logic so heavily that argument flow changes.
2. Expansion that sounds fluent but weakens factual precision.
3. Adding speculative explanations that shift the author's stance.
4. Keeping words but dropping key constraints, numbers, or qualifiers.
5. Preserving headings but dropping concrete examples and edge-case caveats.

## Output
- `content_<target_lang>.md`
- optional bilingual counterpart when user explicitly requests both languages.
