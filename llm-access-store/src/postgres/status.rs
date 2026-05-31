//! The `PublicStatusStore` impl (provider status snapshots).

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    CodexRateLimitStatus, PublicStatusStore, DEFAULT_CODEX_STATUS_REFRESH_SECONDS,
};

use super::{now_ms, PostgresControlRepository};

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
