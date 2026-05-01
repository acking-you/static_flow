//! llm-access executable.

const DEFAULT_LOG_FILTER: &str = "warn,llm_access=info,llm_access_core=info,llm_access_store=info,\
                                  llm_access_kiro=info,llm_access_codex=info";

fn main() -> anyhow::Result<()> {
    let _log_guards = static_flow_runtime::runtime_logging::init_runtime_logging(
        "llm-access",
        DEFAULT_LOG_FILTER,
    )?;
    llm_access::run_from_env()
}
