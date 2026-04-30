//! Default Codex system instructions embedded into model catalogs and requests.

/// Return the default Codex system instructions embedded in client payloads.
pub fn codex_default_instructions() -> &'static str {
    include_str!("codex_default_instructions.md").trim_end_matches('\n')
}
