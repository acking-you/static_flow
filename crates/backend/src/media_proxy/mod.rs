pub mod config;
pub mod forward;
pub mod handlers;

use std::sync::Arc;

use anyhow::Result;

use self::config::read_media_proxy_config_from_env;

#[derive(Clone)]
pub struct MediaProxyState {
    client: reqwest::Client,
    config: config::MediaProxyConfig,
}

impl MediaProxyState {
    pub fn from_env() -> Result<Option<Arc<Self>>> {
        let Some(config) = read_media_proxy_config_from_env()? else {
            tracing::info!("local media proxy is not configured; admin media routes stay inactive");
            return Ok(None);
        };

        let client = reqwest::Client::builder().build()?;
        tracing::info!(base_url = %config.base_url, "local media proxy initialized");
        Ok(Some(Arc::new(Self {
            client,
            config,
        })))
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn config(&self) -> &config::MediaProxyConfig {
        &self.config
    }
}
