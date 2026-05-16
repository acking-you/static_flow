//! llm-access executable.

use better_mimalloc_rs::MiMalloc;

const DEFAULT_LOG_FILTER: &str = "warn,llm_access=info,llm_access_core=info,llm_access_store=info,\
                                  llm_access_kiro=info,llm_access_codex=info";

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

fn main() -> anyhow::Result<()> {
    llm_access::allocator::configure_process_allocator_for_low_rss();
    let _log_guards = static_flow_runtime::runtime_logging::init_runtime_logging(
        "llm-access",
        DEFAULT_LOG_FILTER,
    )?;
    llm_access::run_from_env()
}
