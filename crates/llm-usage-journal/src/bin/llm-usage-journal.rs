//! Inspect llm-access usage journal files.

fn main() -> anyhow::Result<()> {
    llm_usage_journal::cli::run_from_env()
}
