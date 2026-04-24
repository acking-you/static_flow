pub(crate) fn codex_default_instructions() -> &'static str {
    include_str!("codex_default_instructions.md").trim_end_matches('\n')
}
