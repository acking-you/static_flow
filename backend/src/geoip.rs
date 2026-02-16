use std::{
    env,
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use maxminddb::{geoip2, Reader};
use tokio::sync::RwLock;

const DEFAULT_GEOIP_DB_URL: &str =
    "https://cdn.jsdelivr.net/npm/geolite2-city/GeoLite2-City.mmdb.gz";
const DEFAULT_GEOIP_DB_NAME: &str = "GeoLite2-City.mmdb";

#[derive(Clone)]
pub struct GeoIpResolver {
    inner: Arc<GeoIpResolverInner>,
}

struct GeoIpResolverInner {
    db_path: PathBuf,
    db_url: String,
    auto_download: bool,
    client: reqwest::Client,
    reader: RwLock<Option<Reader<Vec<u8>>>>,
}

impl GeoIpResolver {
    pub fn from_env() -> Result<Self> {
        let db_path = env::var("GEOIP_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_geoip_db_path());
        let db_url = env::var("GEOIP_DB_URL").unwrap_or_else(|_| DEFAULT_GEOIP_DB_URL.to_string());
        let auto_download = parse_bool_env("ENABLE_GEOIP_AUTO_DOWNLOAD", true);
        let timeout = env::var("GEOIP_HTTP_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(120)
            .max(3);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .no_proxy()
            .build()
            .context("failed to build geoip http client")?;

        Ok(Self {
            inner: Arc::new(GeoIpResolverInner {
                db_path,
                db_url,
                auto_download,
                client,
                reader: RwLock::new(None),
            }),
        })
    }

    pub async fn warmup(&self) {
        if let Err(err) = self.ensure_reader().await {
            tracing::warn!("geoip warmup skipped: {err}");
        }
    }

    pub async fn resolve_region(&self, ip: &str) -> String {
        let parsed_ip = match ip.parse::<IpAddr>() {
            Ok(ip) => ip,
            Err(_) => return "Unknown".to_string(),
        };

        if let Err(err) = self.ensure_reader().await {
            tracing::warn!("geoip reader unavailable: {err}");
            return "Unknown".to_string();
        }

        let reader_guard = self.inner.reader.read().await;
        let Some(reader) = reader_guard.as_ref() else {
            return "Unknown".to_string();
        };

        let city: geoip2::City<'_> = match reader.lookup(parsed_ip) {
            Ok(value) => match value.decode() {
                Ok(Some(city)) => city,
                Ok(None) => return "Unknown".to_string(),
                Err(err) => {
                    tracing::warn!("geoip decode failed: {err}");
                    return "Unknown".to_string();
                },
            },
            Err(err) => {
                tracing::warn!("geoip lookup failed: {err}");
                return "Unknown".to_string();
            },
        };

        let country = city
            .country
            .iso_code
            .map(str::to_string)
            .or_else(|| city.registered_country.iso_code.map(str::to_string))
            .unwrap_or_else(|| "Unknown".to_string());

        let subdivision = city
            .subdivisions
            .first()
            .and_then(|item| item.names.simplified_chinese.or(item.names.english))
            .map(|value| value.to_string());
        let city_name = city
            .city
            .names
            .simplified_chinese
            .or(city.city.names.english)
            .map(|value| value.to_string());

        match (subdivision, city_name) {
            (Some(subdivision), Some(city_name)) => format!("{country}/{subdivision}/{city_name}"),
            (Some(subdivision), None) => format!("{country}/{subdivision}"),
            (None, Some(city_name)) => format!("{country}/{city_name}"),
            (None, None) => country,
        }
    }

    async fn ensure_reader(&self) -> Result<()> {
        {
            let reader = self.inner.reader.read().await;
            if reader.is_some() {
                return Ok(());
            }
        }

        self.ensure_db_file().await?;

        let data = tokio::fs::read(&self.inner.db_path)
            .await
            .with_context(|| format!("failed to read geoip db {}", self.inner.db_path.display()))?;
        let reader = Reader::from_source(data).context("failed to open geoip mmdb")?;

        let mut writer = self.inner.reader.write().await;
        *writer = Some(reader);
        tracing::info!("geoip reader initialized from {}", self.inner.db_path.display());
        Ok(())
    }

    async fn ensure_db_file(&self) -> Result<()> {
        if self.inner.db_path.exists() {
            return Ok(());
        }

        if !self.inner.auto_download {
            anyhow::bail!(
                "geoip db missing at {} and auto download is disabled",
                self.inner.db_path.display()
            );
        }

        let parent = self
            .inner
            .db_path
            .parent()
            .context("invalid geoip db path without parent")?;
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create geoip dir {}", parent.display()))?;

        tracing::info!(
            "geoip db not found, downloading from {} to {}",
            self.inner.db_url,
            self.inner.db_path.display()
        );

        let response = self
            .inner
            .client
            .get(&self.inner.db_url)
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

        let tmp_path = self.inner.db_path.with_extension("mmdb.tmp-download");
        tokio::fs::write(&tmp_path, &decompressed)
            .await
            .with_context(|| format!("failed to write temp geoip db {}", tmp_path.display()))?;
        tokio::fs::rename(&tmp_path, &self.inner.db_path)
            .await
            .with_context(|| {
                format!(
                    "failed to move temp geoip db {} -> {}",
                    tmp_path.display(),
                    self.inner.db_path.display()
                )
            })?;

        tracing::info!("geoip db downloaded to {}", self.inner.db_path.display());
        Ok(())
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
    let home = env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(".").to_path_buf());
    home.join(".static-flow")
        .join("geoip")
        .join(DEFAULT_GEOIP_DB_NAME)
}
