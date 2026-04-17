//! Shared runtime logging helpers for native binaries.

use std::{env, fs, path::PathBuf};

use anyhow::Result;
use tracing::level_filters::LevelFilter;
use tracing_appender::{
    non_blocking,
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_log::LogTracer;
use tracing_subscriber::{
    filter::{filter_fn, EnvFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    Layer,
};

/// Runtime logging options shared by native binaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLogOptions {
    /// Root directory that contains per-service log folders.
    pub root_dir: PathBuf,
    /// Stable service label such as `backend` or `gateway`.
    pub service: String,
    /// Maximum number of rotated files to retain per log stream.
    pub max_files: usize,
    /// Whether logs should also be emitted to stdout.
    pub stdout: bool,
}

impl RuntimeLogOptions {
    /// Build runtime logging options for one service using env overrides.
    pub fn for_service(service: &str) -> Self {
        let root_dir = env::var("STATICFLOW_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("tmp/runtime-logs"));
        let service = env::var("STATICFLOW_LOG_SERVICE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| service.to_string());
        let stdout = env::var("STATICFLOW_LOG_STDOUT")
            .map(|value| value != "0")
            .unwrap_or(true);

        Self {
            root_dir,
            service,
            max_files: 24,
            stdout,
        }
    }

    /// Directory for application/runtime logs.
    pub fn app_dir(&self) -> PathBuf {
        self.root_dir.join(&self.service).join("app")
    }

    /// Directory for access logs.
    pub fn access_dir(&self) -> PathBuf {
        self.root_dir.join(&self.service).join("access")
    }
}

/// Non-blocking tracing guards that must stay alive for file logging.
pub struct RuntimeLogGuards {
    /// Guard for application log writes.
    pub app_guard: WorkerGuard,
    /// Guard for access log writes.
    pub access_guard: WorkerGuard,
}

/// Initialize shared runtime logging for one native service.
pub fn init_runtime_logging(service: &str, default_filter: &str) -> Result<RuntimeLogGuards> {
    let opts = RuntimeLogOptions::for_service(service);
    fs::create_dir_all(opts.app_dir())?;
    fs::create_dir_all(opts.access_dir())?;
    LogTracer::builder().ignore_crate("tracing").init()?;

    let app_writer = RollingFileAppender::builder()
        .rotation(Rotation::HOURLY)
        .max_log_files(opts.max_files)
        .filename_prefix("current")
        .filename_suffix("log")
        .build(opts.app_dir())?;
    let access_writer = RollingFileAppender::builder()
        .rotation(Rotation::HOURLY)
        .max_log_files(opts.max_files)
        .filename_prefix("current")
        .filename_suffix("log")
        .build(opts.access_dir())?;

    let (app_writer, app_guard) = non_blocking(app_writer);
    let (access_writer, access_guard) = non_blocking(access_writer);

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let app_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(app_writer)
        .with_ansi(false)
        .compact()
        .with_filter(filter_fn(|metadata| metadata.target() != "staticflow_access"));
    let access_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(access_writer)
        .with_ansi(false)
        .compact()
        .with_filter(Targets::new().with_target("staticflow_access", LevelFilter::TRACE));

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(app_layer)
        .with(access_layer);

    if opts.stdout {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_thread_ids(true)
                    .with_thread_names(true)
                    .compact(),
            )
            .try_init()?;
    } else {
        registry.try_init()?;
    }

    Ok(RuntimeLogGuards {
        app_guard,
        access_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::RuntimeLogOptions;

    #[test]
    fn runtime_log_options_default_to_24_files() {
        let opts = RuntimeLogOptions::for_service("backend");
        assert_eq!(opts.max_files, 24);
    }
}
