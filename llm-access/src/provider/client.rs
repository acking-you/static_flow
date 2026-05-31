//! Provider HTTP client pool/cache construction and tuning.

use std::{num::NonZeroUsize, time::Duration};

use llm_access_core::store::ProviderProxyConfig;

use super::{
    ProviderClientCacheKey, DEFAULT_PROVIDER_CLIENT, DEFAULT_PROVIDER_CLIENT_CACHE_CAPACITY,
    DEFAULT_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS,
    DEFAULT_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST, MAX_PROVIDER_CLIENT_CACHE_CAPACITY,
    MAX_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS, MAX_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST,
    MIN_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS, PROVIDER_CLIENT_CACHE,
};

pub fn build_provider_client(
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .pool_idle_timeout(provider_client_pool_idle_timeout())
        .pool_max_idle_per_host(provider_client_pool_max_idle_per_host())
        .tcp_keepalive(Duration::from_secs(30));
    if let Some(proxy_config) = proxy {
        let mut proxy = reqwest::Proxy::all(&proxy_config.proxy_url)?;
        if let Some(username) = proxy_config.proxy_username.as_deref() {
            proxy =
                proxy.basic_auth(username, proxy_config.proxy_password.as_deref().unwrap_or(""));
        }
        builder = builder.proxy(proxy);
    }
    Ok(builder.build()?)
}
pub fn provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let Some(proxy_config) = proxy else {
        return Ok(DEFAULT_PROVIDER_CLIENT.clone());
    };
    let cache_key = ProviderClientCacheKey {
        proxy_url: proxy_config.proxy_url.clone(),
        proxy_username: proxy_config.proxy_username.clone(),
        proxy_password: proxy_config.proxy_password.clone(),
    };
    {
        let mut cache = PROVIDER_CLIENT_CACHE
            .lock()
            .expect("provider client cache lock");
        if let Some(client) = cache.get(&cache_key).cloned() {
            return Ok(client);
        }
    }
    let client = build_provider_client(Some(proxy_config))?;
    PROVIDER_CLIENT_CACHE
        .lock()
        .expect("provider client cache lock")
        .put(cache_key, client.clone());
    Ok(client)
}
pub fn provider_client_cache_capacity() -> NonZeroUsize {
    let capacity = std::env::var("LLM_ACCESS_PROVIDER_CLIENT_CACHE_CAPACITY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.clamp(1, MAX_PROVIDER_CLIENT_CACHE_CAPACITY))
        .unwrap_or(DEFAULT_PROVIDER_CLIENT_CACHE_CAPACITY);
    NonZeroUsize::new(capacity).expect("provider client cache capacity is non-zero")
}
pub fn provider_client_pool_idle_timeout() -> Duration {
    let seconds = std::env::var("LLM_ACCESS_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| {
            value.clamp(
                MIN_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS,
                MAX_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS,
            )
        })
        .unwrap_or(DEFAULT_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS);
    Duration::from_secs(seconds)
}
pub fn provider_client_pool_max_idle_per_host() -> usize {
    std::env::var("LLM_ACCESS_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.min(MAX_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST))
        .unwrap_or(DEFAULT_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST)
}
