//! Command-line configuration for the standalone LLM access service.

use std::{
    ffi::OsString,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};
use llm_access_store::request_cache as store_request_cache;

const DEFAULT_TIERED_DUCKDB_ROLLOVER_BYTES: u64 = 64 * 1024 * 1024;

/// Backing store used for the llm-access control plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlStoreConfig {
    /// Env var name that carries the Postgres database URL.
    pub database_url_env: String,
}

/// Storage paths used by `llm-access`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// Root of the mounted persistent service state.
    pub state_root: PathBuf,
    /// Optional cluster node identity for multi-node deployments.
    pub node_identity: Option<crate::cluster::NodeIdentity>,
    /// Control-plane backing store configuration.
    pub control_store: ControlStoreConfig,
    /// Optional request-path cache configuration backed by Valkey.
    pub request_cache: Option<RequestCacheConfig>,
    /// DuckDB analytics database path.
    pub duckdb: PathBuf,
    /// Hot local usage journal directory.
    pub usage_journal_dir: PathBuf,
    /// Optional tiered DuckDB analytics storage configuration.
    pub duckdb_tiered: Option<TieredDuckDbStorageConfig>,
    /// Kiro account auth directory.
    pub kiro_auths_dir: PathBuf,
    /// Codex account auth directory.
    pub codex_auths_dir: PathBuf,
    /// Runtime log directory.
    pub logs_dir: PathBuf,
}

/// Optional request-path cache configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestCacheConfig {
    /// Env var name that contains the Valkey URL.
    pub url_env: String,
    /// Stable key prefix shared by all cache entries for this deployment.
    pub key_prefix: String,
}

/// Resolve the optional request-path cache config into the store-layer shape.
pub fn resolve_request_cache_config(
    storage: &StorageConfig,
) -> anyhow::Result<Option<store_request_cache::RequestCacheConfig>> {
    let Some(config) = &storage.request_cache else {
        return Ok(None);
    };
    let url = std::env::var(&config.url_env)
        .with_context(|| format!("missing request cache env `{}`", config.url_env))?;
    Ok(Some(store_request_cache::RequestCacheConfig {
        url,
        key_prefix: config.key_prefix.clone(),
    }))
}

/// Tiered DuckDB analytics storage paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieredDuckDbStorageConfig {
    /// Local active DuckDB directory.
    pub active_dir: PathBuf,
    /// Archived immutable DuckDB segment directory.
    pub archive_dir: PathBuf,
    /// Rollover threshold in bytes.
    pub rollover_bytes: u64,
    /// Optional local detail-pack directory for per-event detail payloads.
    pub details_dir: Option<PathBuf>,
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
    let mut postgres_control_database_url_env = None;
    let mut request_cache_url_env = None;
    let mut request_cache_key_prefix = None;
    let mut duckdb = None;
    let mut duckdb_active_dir = None;
    let mut duckdb_archive_dir = None;
    let mut duckdb_rollover_bytes = None;
    let mut usage_details_dir = None;
    let mut usage_journal_dir = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--state-root" => {
                state_root = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--state-root requires a path"))?,
                ));
            },
            "--postgres-control-database-url-env" => {
                postgres_control_database_url_env = Some(
                    args.next()
                        .ok_or_else(|| {
                            anyhow!("--postgres-control-database-url-env requires an env name")
                        })?
                        .to_string_lossy()
                        .to_string(),
                );
            },
            "--request-cache-url-env" => {
                request_cache_url_env = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--request-cache-url-env requires an env name"))?
                        .to_string_lossy()
                        .to_string(),
                );
            },
            "--request-cache-key-prefix" => {
                request_cache_key_prefix = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--request-cache-key-prefix requires a value"))?
                        .to_string_lossy()
                        .to_string(),
                );
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
            "--usage-details-dir" => {
                usage_details_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--usage-details-dir requires a path"))?,
                ));
            },
            "--usage-journal-dir" => {
                usage_journal_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--usage-journal-dir requires a path"))?,
                ));
            },
            _ => return Err(usage_error()),
        }
    }
    let state_root = state_root.ok_or_else(usage_error)?;
    let control_store = ControlStoreConfig {
        database_url_env: postgres_control_database_url_env.ok_or_else(usage_error)?,
    };
    let request_cache = match (request_cache_url_env, request_cache_key_prefix) {
        (Some(url_env), Some(key_prefix)) => Some(RequestCacheConfig {
            url_env,
            key_prefix,
        }),
        (None, None) => None,
        _ => {
            return Err(anyhow!("request cache url env and key prefix must be configured together"))
        },
    };
    let duckdb = duckdb.unwrap_or_else(|| state_root.join("analytics/usage.duckdb"));
    let usage_journal_dir = usage_journal_dir.unwrap_or_else(|| state_root.join("usage-journal"));
    let duckdb_tiered = parse_tiered_duckdb_config(
        duckdb_active_dir,
        duckdb_archive_dir,
        duckdb_rollover_bytes,
        usage_details_dir,
    )?;
    ensure_under_root(&state_root, &duckdb)?;
    if let Some(tiered) = &duckdb_tiered {
        ensure_under_root(&state_root, &tiered.archive_dir)?;
        if let Some(details_dir) = &tiered.details_dir {
            ensure_under_root(&state_root, details_dir)?;
        }
    }
    Ok(StorageConfig {
        kiro_auths_dir: state_root.join("auths/kiro"),
        codex_auths_dir: state_root.join("auths/codex"),
        logs_dir: state_root.join("logs"),
        state_root,
        node_identity: crate::cluster::load_node_identity_from_env()?,
        control_store,
        request_cache,
        duckdb,
        usage_journal_dir,
        duckdb_tiered,
    })
}

fn parse_tiered_duckdb_config(
    active_dir: Option<PathBuf>,
    archive_dir: Option<PathBuf>,
    rollover_bytes: Option<u64>,
    details_dir: Option<PathBuf>,
) -> anyhow::Result<Option<TieredDuckDbStorageConfig>> {
    let any = active_dir.is_some()
        || archive_dir.is_some()
        || rollover_bytes.is_some()
        || details_dir.is_some();
    if !any {
        return Ok(None);
    }
    Ok(Some(TieredDuckDbStorageConfig {
        active_dir: active_dir
            .ok_or_else(|| anyhow!("--duckdb-active-dir is required for tiered DuckDB storage"))?,
        archive_dir: archive_dir
            .ok_or_else(|| anyhow!("--duckdb-archive-dir is required for tiered DuckDB storage"))?,
        rollover_bytes: rollover_bytes
            .unwrap_or(DEFAULT_TIERED_DUCKDB_ROLLOVER_BYTES)
            .max(1),
        details_dir,
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
        "usage: llm-access init --state-root <path> --postgres-control-database-url-env <env> \
         --duckdb <path>\nusage: llm-access serve [--bind <addr>] --state-root <path> \
         --postgres-control-database-url-env <env> [--duckdb <path>] [--usage-journal-dir <path>] \
         [--duckdb-active-dir <path> --duckdb-archive-dir <path> --duckdb-rollover-bytes <bytes> \
         --usage-details-dir <path>]"
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
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
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
            config.storage.control_store.database_url_env,
            "LLM_ACCESS_CONTROL_DATABASE_URL"
        );
        assert_eq!(config.storage.duckdb, PathBuf::from("/mnt/llm-access/analytics/usage.duckdb"));
        assert_eq!(config.storage.duckdb_tiered, None);
        assert_eq!(
            config.storage.usage_journal_dir,
            PathBuf::from("/mnt/llm-access/usage-journal")
        );
        assert_eq!(config.storage.kiro_auths_dir, PathBuf::from("/mnt/llm-access/auths/kiro"));
        assert_eq!(config.storage.codex_auths_dir, PathBuf::from("/mnt/llm-access/auths/codex"));
    }

    #[test]
    fn parses_postgres_control_backend_from_env_name() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--usage-journal-dir",
            "/var/lib/staticflow/llm-access/usage-journal",
        ])
        .expect("parse serve config");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };
        assert_eq!(
            config.storage.control_store.database_url_env,
            "LLM_ACCESS_CONTROL_DATABASE_URL"
        );
        assert_eq!(
            config.storage.usage_journal_dir,
            PathBuf::from("/var/lib/staticflow/llm-access/usage-journal")
        );
    }

    #[test]
    fn rejects_missing_postgres_control_env() {
        let err =
            super::CliCommand::parse(["llm-access", "serve", "--state-root", "/mnt/llm-access"])
                .expect_err("missing control env must fail");

        assert!(err.to_string().contains("usage: llm-access"));
    }

    #[test]
    fn parses_serve_config_with_tiered_duckdb_paths() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--duckdb-active-dir",
            "/var/lib/staticflow/llm-access/analytics-active",
            "--duckdb-archive-dir",
            "/mnt/llm-access/analytics/segments",
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
        assert_eq!(tiered.rollover_bytes, 536_870_912);
        assert_eq!(tiered.details_dir, None);
    }

    #[test]
    fn defaults_tiered_rollover_bytes_to_64_mib() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--duckdb-active-dir",
            "/var/lib/staticflow/llm-access/analytics-active",
            "--duckdb-archive-dir",
            "/mnt/llm-access/analytics/segments",
        ])
        .expect("parse tiered serve command");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };
        let tiered = config.storage.duckdb_tiered.expect("tiered config");
        assert_eq!(tiered.rollover_bytes, 64 * 1024 * 1024);
        assert_eq!(tiered.details_dir, None);
    }

    #[test]
    fn parses_tiered_worker_config_with_external_postgres_control() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access-usage",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--duckdb-active-dir",
            "/var/lib/staticflow/llm-access/analytics-active",
            "--duckdb-archive-dir",
            "/mnt/llm-access-usage/analytics/segments",
            "--usage-details-dir",
            "/mnt/llm-access-usage/details",
        ])
        .expect("parse worker tiered serve command");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };
        assert_eq!(config.storage.state_root, PathBuf::from("/mnt/llm-access-usage"));
        assert_eq!(
            config.storage.control_store.database_url_env,
            "LLM_ACCESS_CONTROL_DATABASE_URL"
        );
        let tiered = config.storage.duckdb_tiered.expect("tiered config");
        assert_eq!(tiered.archive_dir, PathBuf::from("/mnt/llm-access-usage/analytics/segments"));
        assert_eq!(tiered.details_dir, Some(PathBuf::from("/mnt/llm-access-usage/details")));
    }

    #[test]
    fn parses_usage_journal_dir_outside_state_root() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--usage-journal-dir",
            "/var/lib/staticflow/llm-access/usage-journal",
        ])
        .expect("parse serve command");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };

        assert_eq!(
            config.storage.usage_journal_dir,
            PathBuf::from("/var/lib/staticflow/llm-access/usage-journal")
        );
    }

    #[test]
    fn rejects_state_paths_outside_state_root() {
        let err = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--duckdb",
            "/tmp/usage.duckdb",
        ])
        .expect_err("duckdb outside state root must fail");

        assert!(err.to_string().contains("must live under --state-root"));
    }

    #[test]
    fn parses_optional_request_cache_config() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--request-cache-url-env",
            "LLM_ACCESS_REQUEST_CACHE_URL",
            "--request-cache-key-prefix",
            "llma:test",
        ])
        .expect("parse serve config with request cache");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };
        let request_cache = config.storage.request_cache.expect("request cache");
        assert_eq!(request_cache.url_env, "LLM_ACCESS_REQUEST_CACHE_URL");
        assert_eq!(request_cache.key_prefix, "llma:test");
    }

    #[test]
    fn rejects_partial_request_cache_config() {
        let err = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--postgres-control-database-url-env",
            "LLM_ACCESS_CONTROL_DATABASE_URL",
            "--request-cache-url-env",
            "LLM_ACCESS_REQUEST_CACHE_URL",
        ])
        .expect_err("partial request cache config must fail");

        assert!(err
            .to_string()
            .contains("request cache url env and key prefix must be configured together"));
    }
}
