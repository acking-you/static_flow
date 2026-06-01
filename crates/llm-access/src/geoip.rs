//! GeoIP lookup for standalone llm-access request metadata.

use std::{
    env,
    io::Read,
    net::IpAddr,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use maxminddb::{geoip2, Reader};
use serde_json::Value;

const DEFAULT_GEOIP_DB_PATH: &str = "/var/lib/staticflow/llm-access/geoip/GeoLite2-City.mmdb";
const DEFAULT_GEOIP_DB_URL: &str =
    "https://cdn.jsdelivr.net/npm/geolite2-city/GeoLite2-City.mmdb.gz";
const DEFAULT_GEOIP_FALLBACK_API_URL: &str = "https://ipwho.is/{ip}";

/// Shared GeoIP resolver used by data-plane request accounting.
#[derive(Clone)]
pub(crate) struct GeoIpResolver {
    mode: Arc<GeoIpResolverMode>,
}

struct GeoIpResolverInner {
    db_path: PathBuf,
    db_url: String,
    auto_download: bool,
    fallback_api_enabled: bool,
    fallback_api_url: String,
    require_region_detail: bool,
    client: reqwest::Client,
    reader: RwLock<Option<Reader<Vec<u8>>>>,
}

enum GeoIpResolverMode {
    Real(Box<GeoIpResolverInner>),
    Disabled,
    #[cfg(test)]
    Fixed(String),
}

#[derive(Clone, Debug)]
struct GeoRegion {
    country: String,
    subdivision: Option<String>,
    city: Option<String>,
}

impl GeoIpResolver {
    /// Build the resolver from process environment.
    pub(crate) fn from_env() -> Result<Self> {
        let db_path = env::var("GEOIP_DB_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_geoip_db_path);
        let db_url = env::var("GEOIP_DB_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_GEOIP_DB_URL.to_string());
        let auto_download = parse_bool_env("ENABLE_GEOIP_AUTO_DOWNLOAD", true);
        let fallback_api_enabled = parse_bool_env("ENABLE_GEOIP_FALLBACK_API", true);
        let fallback_api_url = env::var("GEOIP_FALLBACK_API_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_GEOIP_FALLBACK_API_URL.to_string());
        let require_region_detail = parse_bool_env("GEOIP_REQUIRE_REGION_DETAIL", true);
        let timeout = env::var("GEOIP_HTTP_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(120)
            .max(3);
        let proxy_url = env::var("GEOIP_PROXY_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let mut client_builder = reqwest::Client::builder().timeout(Duration::from_secs(timeout));
        if let Some(proxy_url) = proxy_url.as_deref() {
            let proxy = reqwest::Proxy::all(proxy_url)
                .with_context(|| format!("invalid GEOIP_PROXY_URL: {proxy_url}"))?;
            client_builder = client_builder.proxy(proxy);
        }
        let client = client_builder
            .build()
            .context("failed to build geoip http client")?;

        Ok(Self {
            mode: Arc::new(GeoIpResolverMode::Real(Box::new(GeoIpResolverInner {
                db_path,
                db_url,
                auto_download,
                fallback_api_enabled,
                fallback_api_url,
                require_region_detail,
                client,
                reader: RwLock::new(None),
            }))),
        })
    }

    /// Build a resolver that always returns `Unknown`.
    pub(crate) fn disabled() -> Self {
        Self {
            mode: Arc::new(GeoIpResolverMode::Disabled),
        }
    }

    #[cfg(test)]
    pub(crate) fn fixed_for_tests(region: &str) -> Self {
        Self {
            mode: Arc::new(GeoIpResolverMode::Fixed(region.to_string())),
        }
    }

    /// Try to initialize the local MMDB reader without making startup fatal.
    pub(crate) async fn warmup(&self) {
        let GeoIpResolverMode::Real(inner) = self.mode.as_ref() else {
            return;
        };
        if let Err(err) = inner.ensure_reader().await {
            tracing::warn!("geoip warmup skipped: {err}");
        }
    }

    /// Resolve a client IP into a stable region string for usage records.
    pub(crate) async fn resolve_region(&self, ip: &str) -> String {
        match self.mode.as_ref() {
            GeoIpResolverMode::Real(inner) => inner.resolve_region(ip).await,
            GeoIpResolverMode::Disabled => "Unknown".to_string(),
            #[cfg(test)]
            GeoIpResolverMode::Fixed(region) => region.clone(),
        }
    }
}

impl GeoIpResolverInner {
    async fn resolve_region(&self, ip: &str) -> String {
        let parsed_ip = match ip.parse::<IpAddr>() {
            Ok(ip) => ip,
            Err(_) => return "Unknown".to_string(),
        };

        if is_private_or_loopback_ip(parsed_ip) {
            return "LAN".to_string();
        }

        let local_region = match self.lookup_local_region(parsed_ip).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("geoip local lookup unavailable: {err}");
                None
            },
        };
        if let Some(region) = local_region.as_ref() {
            if let Some(value) = self.format_region(region) {
                return value;
            }
        }

        if self.fallback_api_enabled {
            match self.lookup_region_via_fallback(parsed_ip).await {
                Ok(Some(region)) => {
                    if let Some(value) = self.format_region(&region) {
                        return value;
                    }
                },
                Ok(None) => {},
                Err(err) => tracing::warn!("geoip fallback api lookup failed: {err}"),
            }
        }

        "Unknown".to_string()
    }

    async fn lookup_local_region(&self, ip: IpAddr) -> Result<Option<GeoRegion>> {
        self.ensure_reader().await?;

        let reader_guard = self
            .reader
            .read()
            .map_err(|_| anyhow::anyhow!("geoip reader lock poisoned"))?;
        let Some(reader) = reader_guard.as_ref() else {
            return Ok(None);
        };

        let city: geoip2::City<'_> = match reader.lookup(ip) {
            Ok(value) => match value.decode() {
                Ok(Some(city)) => city,
                Ok(None) => return Ok(None),
                Err(err) => {
                    tracing::warn!("geoip local decode failed: {err}");
                    return Ok(None);
                },
            },
            Err(err) => {
                tracing::warn!("geoip local lookup failed: {err}");
                return Ok(None);
            },
        };

        Ok(build_region_from_city(city))
    }

    async fn lookup_region_via_fallback(&self, ip: IpAddr) -> Result<Option<GeoRegion>> {
        let url = build_fallback_url(&self.fallback_api_url, &ip.to_string());
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to call geoip fallback api")?
            .error_for_status()
            .context("geoip fallback api returned bad status")?;
        let value: Value = response
            .json()
            .await
            .context("failed to decode geoip fallback response json")?;

        if let Some(success) = value.get("success") {
            if matches!(success.as_bool(), Some(false)) {
                return Ok(None);
            }
            if success
                .as_str()
                .map(|item| item.eq_ignore_ascii_case("fail"))
                .unwrap_or(false)
            {
                return Ok(None);
            }
        }
        if value
            .get("status")
            .and_then(|item| item.as_str())
            .map(|item| !item.eq_ignore_ascii_case("success"))
            .unwrap_or(false)
        {
            return Ok(None);
        }

        let country = read_first_non_empty(&value, &[
            "country",
            "country_name",
            "countryCode",
            "country_code",
        ]);
        let subdivision = read_first_non_empty(&value, &[
            "regionName",
            "region_name",
            "region",
            "state",
            "province",
            "subdivision",
        ]);
        let city = read_first_non_empty(&value, &["city", "district", "town"]);

        let Some(country) = country else {
            return Ok(None);
        };
        Ok(Some(GeoRegion {
            country,
            subdivision,
            city,
        }))
    }

    fn format_region(&self, region: &GeoRegion) -> Option<String> {
        let country = normalize_geo_component(Some(region.country.clone()))?;
        let mut subdivision = normalize_geo_component(region.subdivision.clone());
        let mut city = normalize_geo_component(region.city.clone());

        if let (Some(sub), Some(city_name)) = (subdivision.as_ref(), city.as_ref()) {
            if sub.eq_ignore_ascii_case(city_name) {
                city = None;
            }
        }

        if self.require_region_detail && subdivision.is_none() && city.is_none() {
            return None;
        }

        match (subdivision.take(), city.take()) {
            (Some(subdivision), Some(city)) => Some(format!("{country}/{subdivision}/{city}")),
            (Some(subdivision), None) => Some(format!("{country}/{subdivision}")),
            (None, Some(city)) => Some(format!("{country}/{city}")),
            (None, None) => Some(country),
        }
    }

    async fn ensure_reader(&self) -> Result<()> {
        {
            let reader = self
                .reader
                .read()
                .map_err(|_| anyhow::anyhow!("geoip reader lock poisoned"))?;
            if reader.is_some() {
                return Ok(());
            }
        }

        self.ensure_db_file().await?;

        let data = tokio::fs::read(&self.db_path)
            .await
            .with_context(|| format!("failed to read geoip db {}", self.db_path.display()))?;
        let reader = Reader::from_source(data).context("failed to open geoip mmdb")?;

        let mut writer = self
            .reader
            .write()
            .map_err(|_| anyhow::anyhow!("geoip reader lock poisoned"))?;
        *writer = Some(reader);
        tracing::info!("geoip reader initialized from {}", self.db_path.display());
        Ok(())
    }

    async fn ensure_db_file(&self) -> Result<()> {
        if self.db_path.exists() {
            return Ok(());
        }

        if !self.auto_download {
            anyhow::bail!(
                "geoip db missing at {} and auto download is disabled",
                self.db_path.display()
            );
        }

        let parent = self
            .db_path
            .parent()
            .context("invalid geoip db path without parent")?;
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create geoip dir {}", parent.display()))?;

        tracing::info!(
            "geoip db not found, downloading from {} to {}",
            self.db_url,
            self.db_path.display()
        );

        let response = self
            .client
            .get(&self.db_url)
            .send()
            .await
            .context("failed to download geoip db")?
            .error_for_status()
            .context("geoip db download returned bad status")?;
        let compressed = response
            .bytes()
            .await
            .context("failed to read geoip db body")?;
        let decompressed = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let mut decoder = GzDecoder::new(compressed.as_ref());
            let mut output = Vec::new();
            decoder
                .read_to_end(&mut output)
                .context("failed to decompress geoip db")?;
            Ok(output)
        })
        .await
        .context("geoip decompression task join failed")??;

        let tmp_path = self.db_path.with_extension("mmdb.tmp-download");
        tokio::fs::write(&tmp_path, &decompressed)
            .await
            .with_context(|| format!("failed to write temp geoip db {}", tmp_path.display()))?;
        tokio::fs::rename(&tmp_path, &self.db_path)
            .await
            .with_context(|| {
                format!(
                    "failed to move temp geoip db {} -> {}",
                    tmp_path.display(),
                    self.db_path.display()
                )
            })?;

        tracing::info!("geoip db downloaded to {}", self.db_path.display());
        Ok(())
    }
}

fn build_region_from_city(city: geoip2::City<'_>) -> Option<GeoRegion> {
    let country = normalize_geo_component(
        city.country
            .names
            .simplified_chinese
            .or(city.country.names.english)
            .or(city.registered_country.names.simplified_chinese)
            .or(city.registered_country.names.english)
            .map(str::to_string)
            .or_else(|| city.country.iso_code.map(str::to_string))
            .or_else(|| city.registered_country.iso_code.map(str::to_string)),
    )?;
    let subdivision = city
        .subdivisions
        .first()
        .and_then(|item| item.names.simplified_chinese.or(item.names.english))
        .map(str::to_string);
    let city_name = city
        .city
        .names
        .simplified_chinese
        .or(city.city.names.english)
        .map(str::to_string);

    Some(GeoRegion {
        country,
        subdivision,
        city: city_name,
    })
}

fn normalize_geo_component(value: Option<String>) -> Option<String> {
    let value = value?;
    let normalized = value.trim();
    if normalized.is_empty() {
        return None;
    }
    let lowered = normalized.to_ascii_lowercase();
    if matches!(lowered.as_str(), "unknown" | "n/a" | "na" | "none" | "null" | "-") {
        return None;
    }
    Some(normalized.to_string())
}

fn read_first_non_empty(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(candidate) = value.get(*key).and_then(|item| item.as_str()) {
            if let Some(component) = normalize_geo_component(Some(candidate.to_string())) {
                return Some(component);
            }
        }
    }
    None
}

fn build_fallback_url(template: &str, ip: &str) -> String {
    if template.contains("{ip}") {
        template.replace("{ip}", ip)
    } else if template.ends_with('/') {
        format!("{template}{ip}")
    } else {
        format!("{template}/{ip}")
    }
}

fn parse_bool_env(key: &str, default_value: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default_value)
}

fn default_geoip_db_path() -> PathBuf {
    PathBuf::from(DEFAULT_GEOIP_DB_PATH)
}

fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (v4.octets()[0] == 169 && v4.octets()[1] == 254)
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_geoip_db_path_uses_local_vm_disk() {
        assert_eq!(
            default_geoip_db_path(),
            PathBuf::from("/var/lib/staticflow/llm-access/geoip/GeoLite2-City.mmdb")
        );
    }

    #[tokio::test]
    async fn disabled_resolver_returns_unknown_without_io() {
        let resolver = GeoIpResolver::disabled();

        assert_eq!(resolver.resolve_region("208.77.246.15").await, "Unknown");
    }
}
