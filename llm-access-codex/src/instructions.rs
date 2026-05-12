//! Default Codex system instructions embedded into model catalogs and requests.

/// Return the default Codex system instructions embedded in client payloads.
pub fn codex_default_instructions() -> &'static str {
    include_str!("codex_default_instructions.md").trim_end_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::codex_default_instructions;

    #[test]
    fn codex_default_instructions_match_latest_upstream_prompt_shape() {
        let prompt = codex_default_instructions();

        assert!(prompt.starts_with(
            "You are a coding agent running in the Codex CLI, a terminal-based coding assistant."
        ));
        assert!(prompt.contains("\n## Personality\n"));
    }
}
