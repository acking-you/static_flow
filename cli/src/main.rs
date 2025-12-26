mod cli;
mod commands;
mod db;
mod schema;
mod utils;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Default to info-level logs; override via RUST_LOG if needed.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = cli::Cli::parse();
    commands::run(cli).await
}
