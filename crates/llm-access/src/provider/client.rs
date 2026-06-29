//! Provider HTTP client pool/cache construction and tuning.

use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use llm_access_core::store::ProviderProxyConfig;

use super::{
    kiro_media, ProviderClientCacheKey, ANTHROPIC_UPSTREAM_CLIENT_CACHE, CCTEST_PROXY_CLIENT,
    DEFAULT_ANTHROPIC_UPSTREAM_CLIENT, DEFAULT_PROVIDER_CLIENT,
    DEFAULT_PROVIDER_CLIENT_CACHE_CAPACITY, DEFAULT_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS,
    DEFAULT_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST, MAX_PROVIDER_CLIENT_CACHE_CAPACITY,
    MAX_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS, MAX_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST,
    MIN_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS, PROVIDER_CLIENT_CACHE,
};

pub fn build_provider_client(
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
    Ok(apply_provider_proxy(provider_client_builder(), proxy)?.build()?)
}

pub fn build_anthropic_upstream_client(
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
    let mut builder = provider_client_builder().redirect(reqwest::redirect::Policy::none());
    if proxy.is_none() {
        builder = builder.dns_resolver(Arc::new(kiro_media::PrivateFilteringDnsResolver));
    }
    Ok(apply_provider_proxy(builder, proxy)?.build()?)
}

fn provider_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .pool_idle_timeout(provider_client_pool_idle_timeout())
        .pool_max_idle_per_host(provider_client_pool_max_idle_per_host())
        .tcp_keepalive(Duration::from_secs(30))
}

fn apply_provider_proxy(
    mut builder: reqwest::ClientBuilder,
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::ClientBuilder> {
    if let Some(proxy_config) = proxy {
        let mut proxy = reqwest::Proxy::all(&proxy_config.proxy_url)?;
        if let Some(username) = proxy_config.proxy_username.as_deref() {
            proxy =
                proxy.basic_auth(username, proxy_config.proxy_password.as_deref().unwrap_or(""));
        }
        builder = builder.proxy(proxy);
    }
    Ok(builder)
}

pub fn provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let Some(proxy_config) = proxy else {
        return Ok(DEFAULT_PROVIDER_CLIENT.clone());
    };
    let cache_key = provider_client_cache_key(proxy_config);
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

pub fn anthropic_upstream_client(
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
    let Some(proxy_config) = proxy else {
        return Ok(DEFAULT_ANTHROPIC_UPSTREAM_CLIENT.clone());
    };
    let cache_key = provider_client_cache_key(proxy_config);
    {
        let mut cache = ANTHROPIC_UPSTREAM_CLIENT_CACHE
            .lock()
            .expect("anthropic upstream client cache lock");
        if let Some(client) = cache.get(&cache_key).cloned() {
            return Ok(client);
        }
    }
    let client = build_anthropic_upstream_client(Some(proxy_config))?;
    ANTHROPIC_UPSTREAM_CLIENT_CACHE
        .lock()
        .expect("anthropic upstream client cache lock")
        .put(cache_key, client.clone());
    Ok(client)
}

fn provider_client_cache_key(proxy_config: &ProviderProxyConfig) -> ProviderClientCacheKey {
    ProviderClientCacheKey {
        proxy_url: proxy_config.proxy_url.clone(),
        proxy_username: proxy_config.proxy_username.clone(),
        proxy_password: proxy_config.proxy_password.clone(),
    }
}

pub fn cctest_proxy_client() -> reqwest::Client {
    CCTEST_PROXY_CLIENT.clone()
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

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    #[tokio::test]
    async fn anthropic_upstream_client_does_not_follow_redirects() {
        let (url, target_hits) = spawn_redirect_server().await;
        let client = build_anthropic_upstream_client(None).expect("client should build");

        let response = client
            .get(url)
            .send()
            .await
            .expect("request should complete");

        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert_eq!(target_hits.load(Ordering::SeqCst), 0, "redirect target must not be requested");
    }

    #[tokio::test]
    async fn anthropic_upstream_client_rejects_localhost_dns() {
        let (url, _) = spawn_redirect_server_with_host("localhost").await;
        let client = build_anthropic_upstream_client(None).expect("client should build");

        let error = client
            .get(url)
            .send()
            .await
            .expect_err("localhost DNS must be rejected before connect");

        assert!(error_contains(&error, "private or local"), "unexpected error: {error:?}");
    }

    async fn spawn_redirect_server() -> (String, Arc<AtomicUsize>) {
        spawn_redirect_server_with_host("127.0.0.1").await
    }

    async fn spawn_redirect_server_with_host(host: &str) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("redirect test server should bind");
        let port = listener.local_addr().expect("local addr").port();
        let target_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&target_hits);
        tokio::spawn(async move {
            while let Ok((mut stream, _peer)) = listener.accept().await {
                let request_hits = Arc::clone(&server_hits);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    let Ok(read) = stream.read(&mut buffer).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let response = if request.starts_with("GET /target ") {
                        request_hits.fetch_add(1, Ordering::SeqCst);
                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                    } else {
                        concat!(
                            "HTTP/1.1 302 Found\r\n",
                            "Location: /target\r\n",
                            "Content-Length: 0\r\n",
                            "Connection: close\r\n",
                            "\r\n"
                        )
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (format!("http://{host}:{port}/start"), target_hits)
    }

    fn error_contains(error: &(dyn std::error::Error + 'static), needle: &str) -> bool {
        if error.to_string().contains(needle) {
            return true;
        }
        let mut source = error.source();
        while let Some(error) = source {
            if error.to_string().contains(needle) {
                return true;
            }
            source = error.source();
        }
        false
    }
}
