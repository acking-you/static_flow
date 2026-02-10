mod cli;
mod commands;
mod db;
mod schema;
mod utils;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str = "warn,sf_cli=info,static_flow_shared=info";

#[tokio::main]
async fn main() -> Result<()> {
    // Default: keep third-party noise low while preserving project logs.
    // Override with RUST_LOG when debugging.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = cli::Cli::parse();
    commands::run(cli).await
}
