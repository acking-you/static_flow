# Flow A: Scope

## Goal
Lock write-back boundaries before touching content.

## Checklist
1. Confirm `article_id` and whether this is new insert or overwrite update.
2. Confirm language targets and field scope:
   - only `content`
   - only `content_en`
   - or both `content + content_en`
3. Confirm source language and target language(s).
4. Confirm preservation policy for non-target fields (`summary`, `tags`, `category`, etc.).
5. Create workspace:
   - `/tmp/external_repost/<article_id>/`

## Rule
If scope is unclear, do not write. Resolve scope first.
