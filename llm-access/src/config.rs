//! Command-line configuration for the standalone LLM access service.

use std::{
    ffi::OsString,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};

const DEFAULT_TIERED_DUCKDB_ROLLOVER_BYTES: u64 = 256 * 1024 * 1024;

/// Storage paths used by `llm-access`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// Root of the mounted persistent service state.
    pub state_root: PathBuf,
    /// SQLite control-plane database path.
    pub sqlite_control: PathBuf,
    /// DuckDB analytics database path.
    pub duckdb: PathBuf,
    /// Optional tiered DuckDB analytics storage configuration.
    pub duckdb_tiered: Option<TieredDuckDbStorageConfig>,
    /// Kiro account auth directory.
    pub kiro_auths_dir: PathBuf,
    /// Codex account auth directory.
    pub codex_auths_dir: PathBuf,
    /// Runtime log directory.
    pub logs_dir: PathBuf,
}

/// Tiered DuckDB analytics storage paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieredDuckDbStorageConfig {
    /// Local active DuckDB directory.
    pub active_dir: PathBuf,
    /// Archived immutable DuckDB segment directory.
    pub archive_dir: PathBuf,
    /// Segment catalog directory.
    pub catalog_dir: PathBuf,
    /// Rollover threshold in bytes.
    pub rollover_bytes: u64,
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
    let mut duckdb_active_dir = None;
    let mut duckdb_archive_dir = None;
    let mut duckdb_catalog_dir = None;
    let mut duckdb_rollover_bytes = None;
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
            "--duckdb-active-dir" => {
                duckdb_active_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--duckdb-active-dir requires a path"))?,
                ));
            },
            "--duckdb-archive-dir" => {
                duckdb_archive_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--duckdb-archive-dir requires a path"))?,
                ));
            },
            "--duckdb-catalog-dir" => {
                duckdb_catalog_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--duckdb-catalog-dir requires a path"))?,
                ));
            },
            "--duckdb-rollover-bytes" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--duckdb-rollover-bytes requires a byte count"))?;
                duckdb_rollover_bytes = Some(
                    value
                        .to_string_lossy()
                        .parse::<u64>()
                        .context("failed to parse --duckdb-rollover-bytes")?,
                );
            },
            _ => return Err(usage_error()),
        }
    }
    let state_root = state_root.ok_or_else(usage_error)?;
    let sqlite_control = sqlite_control.ok_or_else(usage_error)?;
    let duckdb = duckdb.unwrap_or_else(|| state_root.join("analytics/usage.duckdb"));
    let duckdb_tiered = parse_tiered_duckdb_config(
        duckdb_active_dir,
        duckdb_archive_dir,
        duckdb_catalog_dir,
        duckdb_rollover_bytes,
    )?;
    ensure_under_root(&state_root, &sqlite_control)?;
    ensure_under_root(&state_root, &duckdb)?;
    if let Some(tiered) = &duckdb_tiered {
        ensure_under_root(&state_root, &tiered.archive_dir)?;
        ensure_under_root(&state_root, &tiered.catalog_dir)?;
    }
    Ok(StorageConfig {
        kiro_auths_dir: state_root.join("auths/kiro"),
        codex_auths_dir: state_root.join("auths/codex"),
        logs_dir: state_root.join("logs"),
        state_root,
        sqlite_control,
        duckdb,
        duckdb_tiered,
    })
}

fn parse_tiered_duckdb_config(
    active_dir: Option<PathBuf>,
    archive_dir: Option<PathBuf>,
    catalog_dir: Option<PathBuf>,
    rollover_bytes: Option<u64>,
) -> anyhow::Result<Option<TieredDuckDbStorageConfig>> {
    let any = active_dir.is_some()
        || archive_dir.is_some()
        || catalog_dir.is_some()
        || rollover_bytes.is_some();
    if !any {
        return Ok(None);
    }
    Ok(Some(TieredDuckDbStorageConfig {
        active_dir: active_dir
            .ok_or_else(|| anyhow!("--duckdb-active-dir is required for tiered DuckDB storage"))?,
        archive_dir: archive_dir
            .ok_or_else(|| anyhow!("--duckdb-archive-dir is required for tiered DuckDB storage"))?,
        catalog_dir: catalog_dir
            .ok_or_else(|| anyhow!("--duckdb-catalog-dir is required for tiered DuckDB storage"))?,
        rollover_bytes: rollover_bytes
            .unwrap_or(DEFAULT_TIERED_DUCKDB_ROLLOVER_BYTES)
            .max(1),
    }))
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
         <path> [--duckdb <path>] [--duckdb-active-dir <path> --duckdb-archive-dir <path> \
         --duckdb-catalog-dir <path> --duckdb-rollover-bytes <bytes>]"
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
        assert_eq!(config.storage.duckdb_tiered, None);
        assert_eq!(config.storage.kiro_auths_dir, PathBuf::from("/mnt/llm-access/auths/kiro"));
        assert_eq!(config.storage.codex_auths_dir, PathBuf::from("/mnt/llm-access/auths/codex"));
    }

    #[test]
    fn parses_serve_config_with_tiered_duckdb_paths() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--sqlite-control",
            "/mnt/llm-access/control/llm-access.sqlite3",
            "--duckdb-active-dir",
            "/var/lib/staticflow/llm-access/analytics-active",
            "--duckdb-archive-dir",
            "/mnt/llm-access/analytics/segments",
            "--duckdb-catalog-dir",
            "/mnt/llm-access/analytics/catalog",
            "--duckdb-rollover-bytes",
            "536870912",
        ])
        .expect("parse tiered serve command");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };
        assert_eq!(config.storage.duckdb, PathBuf::from("/mnt/llm-access/analytics/usage.duckdb"));
        let tiered = config.storage.duckdb_tiered.expect("tiered config");
        assert_eq!(
            tiered.active_dir,
            PathBuf::from("/var/lib/staticflow/llm-access/analytics-active")
        );
        assert_eq!(tiered.archive_dir, PathBuf::from("/mnt/llm-access/analytics/segments"));
        assert_eq!(tiered.catalog_dir, PathBuf::from("/mnt/llm-access/analytics/catalog"));
        assert_eq!(tiered.rollover_bytes, 536_870_912);
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
