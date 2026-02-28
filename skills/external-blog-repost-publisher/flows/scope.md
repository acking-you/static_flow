# Flow A: Scope

## Goal
Lock write-back boundaries before touching content.

## Checklist
1. Confirm `article_id` and whether this is new insert or overwrite update.
2. Confirm language targets and field scope (`content` only / `content_en` only / both).
3. Confirm source language and target language(s). → Rule 5
4. Confirm preservation policy for non-target fields.
5. Create workspace: `/tmp/external_repost/<article_id>/`
6. Confirm repost date + attribution policy. → Rule 6
7. Confirm taxonomy strategy. → Rule 7

## Rule
If scope is unclear, do not write. Resolve scope first.
