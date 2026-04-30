//! llm-access executable.

fn main() -> anyhow::Result<()> {
    llm_access::run_from_env()
}
