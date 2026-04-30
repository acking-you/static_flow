//! Command-line utility for bootstrapping standalone LLM access storage.

use std::{env, path::PathBuf};

use anyhow::{anyhow, Context};

fn main() -> anyhow::Result<()> {
    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        return Err(usage_error());
    };
    if command != "init" {
        return Err(usage_error());
    }

    let mut sqlite_control = None;
    let mut duckdb_schema_sql = None;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--sqlite-control" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--sqlite-control requires a path"))?;
                sqlite_control = Some(PathBuf::from(value));
            },
            "--duckdb-schema-sql" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--duckdb-schema-sql requires a path"))?;
                duckdb_schema_sql = Some(PathBuf::from(value));
            },
            _ => return Err(usage_error()),
        }
    }

    let sqlite_control = sqlite_control.ok_or_else(usage_error)?;
    let duckdb_schema_sql = duckdb_schema_sql.ok_or_else(usage_error)?;

    llm_access_store::initialize_sqlite_target_path(&sqlite_control)
        .with_context(|| format!("failed to initialize `{}`", sqlite_control.display()))?;
    llm_access_store::write_duckdb_schema_file(&duckdb_schema_sql)
        .with_context(|| format!("failed to write `{}`", duckdb_schema_sql.display()))?;

    println!(
        "initialized sqlite_control={} duckdb_schema_sql={}",
        sqlite_control.display(),
        duckdb_schema_sql.display()
    );
    Ok(())
}

fn usage_error() -> anyhow::Error {
    anyhow!("usage: llm-access-store init --sqlite-control <path> --duckdb-schema-sql <path>")
}
