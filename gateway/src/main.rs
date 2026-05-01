//! StaticFlow local Pingora gateway binary.

use std::{env, fs, path::PathBuf, sync::Arc, thread};

use anyhow::{anyhow, Context, Result};
use pingora::server::{configuration::Opt, Server};
use pingora_core::{apps::HttpServerOptions, server::configuration::ServerConf};
use pingora_proxy::http_proxy_service;
use signal_hook::{consts::signal::SIGHUP, iterator::Signals};
use static_flow_shared::runtime_logging::init_runtime_logging;
use staticflow_pingora_gateway::{
    config::{load_gateway_config_from_str, GatewayConfigStore},
    proxy::StaticFlowGateway,
};

const DEFAULT_LOG_FILTER: &str =
    "warn,staticflow_pingora_gateway=info,pingora=info,pingora_core=info,pingora_proxy=info";

fn main() -> Result<()> {
    let opt = Opt::parse_args();
    let conf_path = opt
        .conf
        .clone()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("--conf is required"))?;
    let raw_conf = fs::read_to_string(&conf_path)
        .with_context(|| format!("failed to read gateway config {}", conf_path.display()))?;
    let gateway_config = load_gateway_config_from_str(&raw_conf)?;

    if opt.test {
        println!("listen_addr={}", gateway_config.listen_addr());
        println!("active_upstream={}", gateway_config.active_upstream_name());
        println!("connect_timeout_ms={}", gateway_config.connect_timeout_ms());
        println!("read_idle_timeout_ms={}", gateway_config.read_idle_timeout_ms());
        println!("write_idle_timeout_ms={}", gateway_config.write_idle_timeout_ms());
        println!("downstream_h2c={}", gateway_config.downstream_h2c());
        println!("routing_policy={}", gateway_config.routing_policy_name());
        println!(
            "log_root={}",
            std::env::var("STATICFLOW_LOG_DIR").unwrap_or_else(|_| "tmp/runtime-logs".to_string())
        );
        return Ok(());
    }

    let _log_guards = init_runtime_logging("gateway", DEFAULT_LOG_FILTER)?;

    let mut server_conf = ServerConf::from_yaml(&raw_conf)
        .map_err(|err| anyhow!("failed to parse pingora server config: {err}"))?;
    let external_supervisor = env::var("STATICFLOW_GATEWAY_EXTERNAL_SUPERVISOR")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    if external_supervisor {
        server_conf.daemon = false;
    }
    let max_proxy_tries = gateway_config.retry_count().saturating_add(1);
    server_conf.max_retries = max_proxy_tries;

    let listen_addr = gateway_config.listen_addr().to_string();
    let active_upstream = gateway_config.active_upstream_name().to_string();
    let active_upstream_addr = gateway_config.active_upstream_addr()?.to_string();
    let connect_timeout_ms = gateway_config.connect_timeout_ms();
    let read_idle_timeout_ms = gateway_config.read_idle_timeout_ms();
    let write_idle_timeout_ms = gateway_config.write_idle_timeout_ms();
    let downstream_h2c = gateway_config.downstream_h2c();
    let routing_policy = gateway_config.routing_policy_name();
    let retry_count = gateway_config.retry_count();
    let gateway_config = Arc::new(GatewayConfigStore::load(&conf_path)?);
    install_reload_signal_handler(Arc::clone(&gateway_config))?;

    tracing::info!(
        listen_addr,
        active_upstream,
        active_upstream_addr,
        connect_timeout_ms,
        read_idle_timeout_ms,
        write_idle_timeout_ms,
        downstream_h2c,
        routing_policy,
        retry_count,
        max_proxy_tries,
        external_supervisor,
        conf = %conf_path.display(),
        "starting StaticFlow Pingora gateway"
    );

    let mut server = Server::new_with_opt_and_conf(Some(opt), server_conf);
    server.bootstrap();

    let mut proxy =
        http_proxy_service(&server.configuration, StaticFlowGateway::new(gateway_config));
    let http_logic = proxy
        .app_logic_mut()
        .ok_or_else(|| anyhow!("gateway proxy service has no HTTP app logic"))?;
    let mut http_server_options = HttpServerOptions::default();
    http_server_options.h2c = downstream_h2c;
    http_logic.server_options = Some(http_server_options);
    proxy.add_tcp(listen_addr.as_str());
    server.add_service(proxy);
    server.run_forever()
}

fn install_reload_signal_handler(config_store: Arc<GatewayConfigStore>) -> Result<()> {
    let mut signals = Signals::new([SIGHUP])?;
    thread::Builder::new()
        .name("gateway-config-reload".to_string())
        .spawn(move || {
            for _ in signals.forever() {
                match config_store.reload() {
                    Ok(config) => {
                        let active_upstream = config.active_upstream_name().to_string();
                        match config.active_upstream_addr() {
                            Ok(active_upstream_addr) => tracing::info!(
                                active_upstream,
                                active_upstream_addr,
                                connect_timeout_ms = config.connect_timeout_ms(),
                                read_idle_timeout_ms = config.read_idle_timeout_ms(),
                                write_idle_timeout_ms = config.write_idle_timeout_ms(),
                                downstream_h2c = config.downstream_h2c(),
                                routing_policy = config.routing_policy_name(),
                                retry_count = config.retry_count(),
                                conf = %config_store.path().display(),
                                "reloaded gateway config from disk"
                            ),
                            Err(err) => tracing::error!(
                                active_upstream,
                                error = %err,
                                conf = %config_store.path().display(),
                                "reloaded gateway config but upstream resolution failed"
                            ),
                        }
                    },
                    Err(err) => tracing::error!(
                        error = %err,
                        conf = %config_store.path().display(),
                        "failed to reload gateway config; keeping previous snapshot"
                    ),
                }
            }
        })?;
    Ok(())
}
