use std::{collections::BTreeMap, env};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct MediaProxyConfig {
    pub base_url: reqwest::Url,
}

pub fn read_media_proxy_config_from_env() -> Result<Option<MediaProxyConfig>> {
    let env_map = env::vars().collect::<BTreeMap<_, _>>();
    read_media_proxy_config_from_map(&env_map)
}

fn read_media_proxy_config_from_map(
    env_map: &BTreeMap<String, String>,
) -> Result<Option<MediaProxyConfig>> {
    let Some(raw_base_url) = env_map
        .get("STATICFLOW_MEDIA_PROXY_BASE_URL")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let normalized = format!("{}/", raw_base_url.trim_end_matches('/'));
    let base_url = reqwest::Url::parse(&normalized).with_context(|| {
        format!("failed to parse STATICFLOW_MEDIA_PROXY_BASE_URL: {raw_base_url}")
    })?;
    Ok(Some(MediaProxyConfig {
        base_url,
    }))
}

#[cfg(test)]
pub(crate) fn read_media_proxy_config_for_test(vars: &[(&str, &str)]) -> Result<MediaProxyConfig> {
    let env_map = vars
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>();
    read_media_proxy_config_from_map(&env_map)?
        .context("STATICFLOW_MEDIA_PROXY_BASE_URL must be configured for test")
}

#[cfg(test)]
mod tests {
    use super::read_media_proxy_config_for_test;

    #[test]
    fn media_proxy_config_reads_base_url() {
        let cfg = read_media_proxy_config_for_test(&[(
            "STATICFLOW_MEDIA_PROXY_BASE_URL",
            "http://127.0.0.1:39085",
        )])
        .expect("config should parse");
        assert_eq!(cfg.base_url.as_str(), "http://127.0.0.1:39085/");
    }
}
