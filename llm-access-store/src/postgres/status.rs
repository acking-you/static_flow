//! Codex rate-limit status storage: in-memory + row-backed rate-limit status
//! caches, plus the `PublicStatusStore` impl.

use async_trait::async_trait;

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[async_trait]
impl PublicStatusStore for PostgresControlRepository {
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus> {
        if let Some(snapshot) = self.load_codex_rate_limit_status_cached().await? {
            return Ok(snapshot);
        }
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .map(|record| record.codex_status_refresh_max_interval_seconds.max(0) as u64)
            .unwrap_or(DEFAULT_CODEX_STATUS_REFRESH_SECONDS);
        Ok(CodexRateLimitStatus::loading(refresh_interval_seconds))
    }

    async fn save_codex_rate_limit_status(
        &self,
        snapshot: CodexRateLimitStatus,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_codex_status_cache (id, snapshot_json, updated_at_ms)
                 VALUES ('default', $1::jsonb, $2)
                 ON CONFLICT(id) DO UPDATE SET
                    snapshot_json = EXCLUDED.snapshot_json,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &serde_json::to_string(&snapshot)
                        .context("serialize postgres codex rate-limit snapshot")?,
                    &now_ms(),
                ],
            )
            .await
            .context("upsert postgres codex rate-limit status snapshot")?;
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            let lookup = crate::request_cache::CachedCodexStatusLookup {
                snapshot: Some(snapshot.clone()),
            };
            if let Err(err) = cache
                .set_json(&cache_key, &lookup, cache.codex_status_ttl())
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex status write-through failed"
                );
            }
        }
        self.store_cached_codex_rate_limit_status(Some(snapshot))
            .await;
        Ok(())
    }
}
impl PostgresControlRepository {
    pub(crate) async fn cached_codex_rate_limit_status(&self) -> Option<CodexRateLimitStatus> {
        let guard = self.codex_status_cache.read().await;
        let cached = guard.as_ref()?;
        if cached.loaded_at.elapsed() > CODEX_STATUS_CACHE_TTL {
            return None;
        }
        Some(cached.snapshot.clone())
    }
    pub(crate) async fn store_cached_codex_rate_limit_status(
        &self,
        snapshot: Option<CodexRateLimitStatus>,
    ) {
        let mut guard = self.codex_status_cache.write().await;
        *guard = snapshot.map(|snapshot| CachedCodexRateLimitStatus {
            snapshot,
            loaded_at: Instant::now(),
        });
    }
    pub(crate) async fn load_codex_rate_limit_status_cached(
        &self,
    ) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        if let Some(snapshot) = self.cached_codex_rate_limit_status().await {
            return Ok(Some(snapshot));
        }
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            match cache
                .get_json::<crate::request_cache::CachedCodexStatusLookup>(&cache_key)
                .await
            {
                Ok(Some(lookup)) => {
                    self.store_cached_codex_rate_limit_status(lookup.snapshot.clone())
                        .await;
                    return Ok(lookup.snapshot);
                },
                Ok(None) => {},
                Err(err) => {
                    tracing::warn!(
                        key = %cache_key,
                        error = %err,
                        "request cache codex status read failed; falling back to postgres"
                    );
                },
            }
        }
        let snapshot = self.load_codex_rate_limit_status_row().await?;
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            let lookup = crate::request_cache::CachedCodexStatusLookup {
                snapshot: snapshot.clone(),
            };
            if let Err(err) = cache
                .set_json(&cache_key, &lookup, cache.codex_status_ttl())
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex status write failed"
                );
            }
        }
        self.store_cached_codex_rate_limit_status(snapshot.clone())
            .await;
        Ok(snapshot)
    }
    pub(crate) async fn load_codex_rate_limit_status_row(
        &self,
    ) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        self.ensure_connection_alive()?;
        let snapshot_json = self
            .client
            .query_opt(
                "SELECT snapshot_json::text FROM llm_codex_status_cache WHERE id = 'default'",
                &[],
            )
            .await
            .context("load codex rate-limit status snapshot")?
            .map(|row| row.get::<_, String>(0));
        snapshot_json
            .map(|json| {
                serde_json::from_str::<CodexRateLimitStatus>(&json)
                    .context("decode codex rate-limit status snapshot")
            })
            .transpose()
    }
}
