# Style Contract for Implementation Deep-Dive Articles

## 1. Heading Rules

### Required
- Write headings as declarative noun phrases.
- Keep heading semantics specific and layered.
- Ensure chapter order follows total -> detail -> operation.

### Forbidden
- Question headings (for example: "Why X is not enough?").
- Emotional or exaggerated headings (for example: "ultimate", "crazy", "explosive").
- Ambiguous chapter labels (for example: "Other", "Misc", "Random Thoughts").

## 2. Narrative Rules

### Required
- Explain problem and constraints before solution details.
- Explain mechanism behavior before code references.
- Separate architecture, runtime path, and operations into explicit sections.

### Forbidden
- Using code location lists as the main narrative.
- Claiming performance gains without evidence or clear rationale.
- Repeating implementation details without stating trade-offs.

## 3. Evidence Rules

### Required
- Use diagrams for data flow or control flow.
- Use at least one comparison table for design alternatives.
- Include concrete troubleshooting scenarios.

### Forbidden
- Long unstructured source-file dumps.
- Excessive inline file:line references in explanatory paragraphs.

## 4. Tone Rules

### Required
- Keep language technical, precise, and neutral.
- Prefer concrete terms over slogans.
- Keep claims falsifiable and operational.

### Forbidden
- Marketing adjectives and hype language.
- Rhetorical flourish that weakens precision.

## 5. Definition Rules

- Define domain terms once in a dedicated section.
- Reuse the same term names consistently across sections.
- When two terms are similar, explicitly distinguish them.
