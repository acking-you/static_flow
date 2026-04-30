//! Command-line configuration for the standalone LLM access service.

use std::{
    ffi::OsString,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};

/// Storage paths used by `llm-access`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// Root of the mounted persistent service state.
    pub state_root: PathBuf,
    /// SQLite control-plane database path.
    pub sqlite_control: PathBuf,
    /// DuckDB analytics database path.
    pub duckdb: PathBuf,
    /// Kiro account auth directory.
    pub kiro_auths_dir: PathBuf,
    /// Codex account auth directory.
    pub codex_auths_dir: PathBuf,
    /// CDC staging directory.
    pub cdc_dir: PathBuf,
    /// Runtime log directory.
    pub logs_dir: PathBuf,
}

/// HTTP service configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    /// TCP bind address.
    pub bind_addr: SocketAddr,
    /// Storage bootstrap paths.
    pub storage: StorageConfig,
}

/// Parsed command-line command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// Initialize storage and exit.
    Init(StorageConfig),
    /// Initialize storage, then run the HTTP server.
    Serve(ServeConfig),
}

impl CliCommand {
    /// Parse CLI arguments.
    pub fn parse<I, S>(args: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut args = args.into_iter().map(Into::into);
        let _program = args.next();
        let command = args.next().ok_or_else(usage_error)?;
        match command.to_string_lossy().as_ref() {
            "init" => Ok(Self::Init(parse_storage_args(args)?)),
            "serve" => {
                let (bind_addr, storage) = parse_serve_args(args)?;
                Ok(Self::Serve(ServeConfig {
                    bind_addr,
                    storage,
                }))
            },
            _ => Err(usage_error()),
        }
    }
}

fn parse_serve_args<I>(args: I) -> anyhow::Result<(SocketAddr, StorageConfig)>
where
    I: IntoIterator<Item = OsString>,
{
    let mut bind_addr = None;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--bind requires an address"))?;
                bind_addr = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .context("failed to parse --bind address")?,
                );
            },
            _ => rest.push(arg),
        }
    }
    Ok((
        bind_addr.unwrap_or_else(|| "127.0.0.1:19080".parse().expect("valid bind addr")),
        parse_storage_args(rest)?,
    ))
}

fn parse_storage_args<I>(args: I) -> anyhow::Result<StorageConfig>
where
    I: IntoIterator<Item = OsString>,
{
    let mut state_root = None;
    let mut sqlite_control = None;
    let mut duckdb = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--state-root" => {
                state_root = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--state-root requires a path"))?,
                ));
            },
            "--sqlite-control" => {
                sqlite_control = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--sqlite-control requires a path"))?,
                ));
            },
            "--duckdb" => {
                duckdb = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--duckdb requires a path"))?,
                ));
            },
            _ => return Err(usage_error()),
        }
    }
    let state_root = state_root.ok_or_else(usage_error)?;
    let sqlite_control = sqlite_control.ok_or_else(usage_error)?;
    let duckdb = duckdb.ok_or_else(usage_error)?;
    ensure_under_root(&state_root, &sqlite_control)?;
    ensure_under_root(&state_root, &duckdb)?;
    Ok(StorageConfig {
        kiro_auths_dir: state_root.join("auths/kiro"),
        codex_auths_dir: state_root.join("auths/codex"),
        cdc_dir: state_root.join("cdc"),
        logs_dir: state_root.join("logs"),
        state_root,
        sqlite_control,
        duckdb,
    })
}

fn ensure_under_root(root: &Path, path: &Path) -> anyhow::Result<()> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(anyhow!("`{}` must live under --state-root `{}`", path.display(), root.display()))
    }
}

fn usage_error() -> anyhow::Error {
    anyhow!(
        "usage: llm-access init --state-root <path> --sqlite-control <path> --duckdb \
         <path>\nusage: llm-access serve [--bind <addr>] --state-root <path> --sqlite-control \
         <path> --duckdb <path>"
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn parses_serve_config_with_state_root_and_duckdb_path() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--bind",
            "127.0.0.1:19080",
            "--state-root",
            "/mnt/llm-access",
            "--sqlite-control",
            "/mnt/llm-access/control/llm-access.sqlite3",
            "--duckdb",
            "/mnt/llm-access/analytics/usage.duckdb",
        ])
        .expect("parse serve command");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };

        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:19080");
        assert_eq!(config.storage.state_root, PathBuf::from("/mnt/llm-access"));
        assert_eq!(
            config.storage.sqlite_control,
            PathBuf::from("/mnt/llm-access/control/llm-access.sqlite3")
        );
        assert_eq!(config.storage.duckdb, PathBuf::from("/mnt/llm-access/analytics/usage.duckdb"));
        assert_eq!(config.storage.kiro_auths_dir, PathBuf::from("/mnt/llm-access/auths/kiro"));
        assert_eq!(config.storage.codex_auths_dir, PathBuf::from("/mnt/llm-access/auths/codex"));
    }

    #[test]
    fn rejects_state_paths_outside_state_root() {
        let err = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--sqlite-control",
            "/tmp/llm-access.sqlite3",
            "--duckdb",
            "/mnt/llm-access/analytics/usage.duckdb",
        ])
        .expect_err("sqlite outside state root must fail");

        assert!(err.to_string().contains("must live under --state-root"));
    }
}
