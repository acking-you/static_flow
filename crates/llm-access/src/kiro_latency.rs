//! API-side Kiro latency routing weights.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use llm_access_core::store::{
    AdminConfigStore, KiroLatencyRankingRow, KiroLatencyRankingSnapshot, ProviderKiroRoute,
};

const ACCOUNT_WEIGHT: f64 = 0.7;
const PROXY_WEIGHT: f64 = 0.3;
const SMOOTHING_SAMPLES: f64 = 5.0;
const SNAPSHOT_STALE_MS: i64 = 5 * 60 * 1000;
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KiroLatencyDimensionStat {
    pub(crate) key: String,
    pub(crate) samples: u64,
    pub(crate) avg_first_token_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KiroLatencyRoutingSnapshot {
    pub(crate) generated_at_ms: i64,
    pub(crate) global_avg_first_token_ms: f64,
    pub(crate) accounts: Vec<KiroLatencyDimensionStat>,
    pub(crate) proxies: Vec<KiroLatencyDimensionStat>,
}

#[derive(Debug, Default)]
pub(crate) struct KiroLatencyRanker {
    snapshot: RwLock<Option<Arc<PreparedKiroLatencySnapshot>>>,
}

#[derive(Debug)]
struct PreparedKiroLatencySnapshot {
    generated_at_ms: i64,
    global_avg_first_token_ms: f64,
    accounts: HashMap<String, KiroLatencyDimensionStat>,
    proxies: HashMap<String, KiroLatencyDimensionStat>,
}

impl KiroLatencyRanker {
    pub(crate) fn replace_snapshot(&self, snapshot: KiroLatencyRoutingSnapshot) {
        let prepared = PreparedKiroLatencySnapshot::from_snapshot(snapshot);
        *self.snapshot.write().expect("kiro latency snapshot lock") = Some(Arc::new(prepared));
    }

    pub(crate) fn replace_ranking_snapshot(&self, snapshot: KiroLatencyRankingSnapshot) {
        if let Some(snapshot) = KiroLatencyRoutingSnapshot::from_ranking_snapshot(snapshot) {
            self.replace_snapshot(snapshot);
        }
    }

    pub(crate) fn route_score_ms(&self, route: &ProviderKiroRoute, now_ms: i64) -> Option<f64> {
        if !route.latency_routing_enabled {
            return None;
        }
        let snapshot = self.current_snapshot(now_ms)?;
        let account_score = snapshot
            .accounts
            .get(&route.account_name)
            .map(|stat| smoothed_latency_ms(stat, snapshot.global_avg_first_token_ms))
            .unwrap_or(snapshot.global_avg_first_token_ms);
        let proxy_score = route
            .proxy
            .as_ref()
            .and_then(|proxy| snapshot.proxies.get(&proxy.proxy_url))
            .map(|stat| smoothed_latency_ms(stat, snapshot.global_avg_first_token_ms))
            .unwrap_or(snapshot.global_avg_first_token_ms);
        Some(ACCOUNT_WEIGHT.mul_add(account_score, PROXY_WEIGHT * proxy_score))
    }

    fn current_snapshot(&self, now_ms: i64) -> Option<Arc<PreparedKiroLatencySnapshot>> {
        let snapshot = self
            .snapshot
            .read()
            .expect("kiro latency snapshot lock")
            .clone()?;
        if now_ms.saturating_sub(snapshot.generated_at_ms) > SNAPSHOT_STALE_MS {
            return None;
        }
        if !snapshot.global_avg_first_token_ms.is_finite() {
            return None;
        }
        Some(snapshot)
    }
}

impl PreparedKiroLatencySnapshot {
    fn from_snapshot(snapshot: KiroLatencyRoutingSnapshot) -> Self {
        let accounts = snapshot
            .accounts
            .into_iter()
            .filter(|stat| stat.samples > 0 && stat.avg_first_token_ms.is_finite())
            .map(|stat| (account_stat_key(&stat.key), stat))
            .collect::<HashMap<_, _>>();
        let proxies = snapshot
            .proxies
            .into_iter()
            .filter(|stat| stat.samples > 0 && stat.avg_first_token_ms.is_finite())
            .map(|stat| (stat.key.clone(), stat))
            .collect::<HashMap<_, _>>();
        Self {
            generated_at_ms: snapshot.generated_at_ms,
            global_avg_first_token_ms: snapshot.global_avg_first_token_ms,
            accounts,
            proxies,
        }
    }
}

impl KiroLatencyRoutingSnapshot {
    fn from_ranking_snapshot(snapshot: KiroLatencyRankingSnapshot) -> Option<Self> {
        let global_avg_first_token_ms = snapshot.avg_first_token_ms?;
        Some(Self {
            generated_at_ms: snapshot.generated_at_ms,
            global_avg_first_token_ms,
            accounts: snapshot
                .accounts
                .into_iter()
                .filter_map(account_stat_from_ranking_row)
                .collect(),
            proxies: snapshot
                .proxies
                .into_iter()
                .filter_map(proxy_stat_from_ranking_row)
                .collect(),
        })
    }
}

fn account_stat_from_ranking_row(row: KiroLatencyRankingRow) -> Option<KiroLatencyDimensionStat> {
    Some(KiroLatencyDimensionStat {
        key: row.account_name?,
        samples: row.first_token_samples,
        avg_first_token_ms: row.avg_first_token_ms?,
    })
}

fn proxy_stat_from_ranking_row(row: KiroLatencyRankingRow) -> Option<KiroLatencyDimensionStat> {
    Some(KiroLatencyDimensionStat {
        key: row.proxy_url?,
        samples: row.first_token_samples,
        avg_first_token_ms: row.avg_first_token_ms?,
    })
}

fn account_stat_key(key: &str) -> String {
    key.strip_prefix("account:").unwrap_or(key).to_string()
}

fn smoothed_latency_ms(stat: &KiroLatencyDimensionStat, global_avg_ms: f64) -> f64 {
    let samples = stat.samples as f64;
    (stat.avg_first_token_ms * samples + global_avg_ms * SMOOTHING_SAMPLES)
        / (samples + SMOOTHING_SAMPLES)
}

pub(crate) fn spawn_kiro_latency_refresher(
    config_store: Arc<dyn AdminConfigStore>,
    ranker: Arc<KiroLatencyRanker>,
) {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("build kiro latency refresh client");
        loop {
            if let Err(err) = refresh_once(&client, config_store.as_ref(), ranker.as_ref()).await {
                tracing::warn!(error = ?err, "failed to refresh kiro latency routing snapshot");
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

async fn refresh_once(
    client: &reqwest::Client,
    config_store: &dyn AdminConfigStore,
    ranker: &KiroLatencyRanker,
) -> anyhow::Result<()> {
    let config = config_store.get_admin_runtime_config().await?;
    let url = format!(
        "{}/internal/kiro-gateway/latency-ranking?source=hot&window=1h",
        config.usage_query_base_url.trim_end_matches('/')
    );
    let snapshot = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json::<KiroLatencyRankingSnapshot>()
        .await?;
    ranker.replace_ranking_snapshot(snapshot);
    Ok(())
}
