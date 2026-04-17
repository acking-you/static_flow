//! StaticFlow local Pingora gateway binary.

use std::{fs, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use pingora::server::{configuration::Opt, Server};
use pingora_core::server::configuration::ServerConf;
use pingora_proxy::http_proxy_service;
use static_flow_shared::runtime_logging::init_runtime_logging;
use staticflow_pingora_gateway::{
    config::{load_gateway_config, load_gateway_config_from_str},
    proxy::StaticFlowGateway,
};

const DEFAULT_LOG_FILTER: &str =
    "warn,staticflow_pingora_gateway=info,pingora=info,pingora_core=info,pingora_proxy=info";

#[tokio::main]
async fn main() -> Result<()> {
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
        println!(
            "log_root={}",
            std::env::var("STATICFLOW_LOG_DIR").unwrap_or_else(|_| "tmp/runtime-logs".to_string())
        );
        return Ok(());
    }

    let _log_guards = init_runtime_logging("gateway", DEFAULT_LOG_FILTER)?;

    let mut server_conf = ServerConf::from_yaml(&raw_conf)
        .map_err(|err| anyhow!("failed to parse pingora server config: {err}"))?;
    server_conf.max_retries = gateway_config.retry_count();

    let listen_addr = gateway_config.listen_addr().to_string();
    let active_upstream = gateway_config.active_upstream_name().to_string();
    let active_upstream_addr = gateway_config.active_upstream_addr()?.to_string();
    let connect_timeout_ms = gateway_config.connect_timeout_ms();
    let read_idle_timeout_ms = gateway_config.read_idle_timeout_ms();
    let write_idle_timeout_ms = gateway_config.write_idle_timeout_ms();
    let retry_count = gateway_config.retry_count();
    let gateway_config = Arc::new(load_gateway_config(&conf_path)?);

    tracing::info!(
        listen_addr,
        active_upstream,
        active_upstream_addr,
        connect_timeout_ms,
        read_idle_timeout_ms,
        write_idle_timeout_ms,
        retry_count,
        conf = %conf_path.display(),
        "starting StaticFlow Pingora gateway"
    );

    let mut server = Server::new_with_opt_and_conf(Some(opt), server_conf);
    server.bootstrap();

    let mut proxy =
        http_proxy_service(&server.configuration, StaticFlowGateway::new(gateway_config));
    proxy.add_tcp(listen_addr.as_str());
    server.add_service(proxy);
    server.run_forever()
}
