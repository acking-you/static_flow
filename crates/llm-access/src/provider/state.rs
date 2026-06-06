//! `ProviderState` + `ForcedProxyRouteStore` (forced-proxy route store).

use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::store::{
    AdminConfigStore, AdminKiroStatusCacheUpdate, AuthenticatedKey, ControlStore,
    EmptyAdminConfigStore, ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderKiroAuthUpdate,
    ProviderKiroRoute, ProviderProxyConfig, ProviderRouteStore,
};
use llm_access_kiro::{
    cache_sim::{KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulator},
    scheduler::KiroRequestScheduler,
};

use super::{
    codex_session_affinity::CodexSessionAffinity,
    entry::{is_active_key, is_quota_exhausted, key_matches_route, quota_exhausted_response},
    kiro_session_affinity::KiroSessionAffinity,
    CodexAccountCooldowns, DefaultProviderDispatcher, ForcedProxyRouteStore, ProviderDispatchDeps,
    ProviderDispatcher, ProviderState, RequestLimiter,
};
use crate::{
    activity::RequestActivityTracker, geoip::GeoIpResolver, kiro_latency::KiroLatencyRanker,
};

fn protected_thinking_signature_secret_from_env() -> Option<Arc<str>> {
    std::env::var(super::KIRO_THINKING_SIGNATURE_SECRET_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Arc::from)
}

impl ProviderState {
    /// Create provider request state.
    pub fn new(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
    ) -> Self {
        Self::with_dispatcher(control_store, route_store, Arc::new(DefaultProviderDispatcher))
    }

    /// Create provider request state with an explicit admin runtime config
    /// source.
    pub fn new_with_config_store(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
    ) -> Self {
        Self::new_with_config_store_and_activity(
            control_store,
            route_store,
            admin_config_store,
            Arc::new(RequestActivityTracker::new()),
            GeoIpResolver::disabled(),
        )
    }

    pub(crate) fn new_with_config_store_and_activity(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        request_activity: Arc<RequestActivityTracker>,
        geoip: GeoIpResolver,
    ) -> Self {
        Self::new_with_config_store_activity_and_latency(
            control_store,
            route_store,
            admin_config_store,
            request_activity,
            geoip,
            Arc::new(KiroLatencyRanker::default()),
        )
    }

    pub(crate) fn new_with_config_store_activity_and_latency(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        request_activity: Arc<RequestActivityTracker>,
        geoip: GeoIpResolver,
        kiro_latency_ranker: Arc<KiroLatencyRanker>,
    ) -> Self {
        Self::with_dispatcher_and_config_store(
            control_store,
            route_store,
            admin_config_store,
            Arc::new(DefaultProviderDispatcher),
            request_activity,
            geoip,
            kiro_latency_ranker,
        )
    }

    /// Create provider request state with an explicit dispatcher.
    pub fn with_dispatcher(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        dispatcher: Arc<dyn ProviderDispatcher>,
    ) -> Self {
        Self::with_dispatcher_and_config_store(
            control_store,
            route_store,
            Arc::new(EmptyAdminConfigStore),
            dispatcher,
            Arc::new(RequestActivityTracker::new()),
            GeoIpResolver::disabled(),
            Arc::new(KiroLatencyRanker::default()),
        )
    }

    fn with_dispatcher_and_config_store(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        dispatcher: Arc<dyn ProviderDispatcher>,
        request_activity: Arc<RequestActivityTracker>,
        geoip: GeoIpResolver,
        kiro_latency_ranker: Arc<KiroLatencyRanker>,
    ) -> Self {
        Self {
            control_store,
            route_store,
            geoip,
            admin_config_store,
            dispatcher,
            kiro_cache_simulator: Arc::new(KiroCacheSimulator::default()),
            request_limiter: Arc::new(RequestLimiter::default()),
            codex_account_cooldowns: Arc::new(CodexAccountCooldowns::default()),
            codex_session_affinity: Arc::new(CodexSessionAffinity::default()),
            kiro_request_scheduler: KiroRequestScheduler::new(),
            kiro_session_affinity: Arc::new(KiroSessionAffinity::from_env()),
            kiro_latency_ranker,
            request_activity,
            protected_thinking_signature_secret: protected_thinking_signature_secret_from_env(),
        }
    }

    pub(crate) fn route_store(&self) -> Arc<dyn ProviderRouteStore> {
        Arc::clone(&self.route_store)
    }

    /// Shared Kiro cache simulator, exposed so the serve loop can snapshot it
    /// to Valkey and restore it on startup.
    pub(crate) fn kiro_cache_simulator(&self) -> Arc<KiroCacheSimulator> {
        Arc::clone(&self.kiro_cache_simulator)
    }

    pub(crate) async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        self.control_store.authenticate_bearer_secret(secret).await
    }

    pub(crate) async fn dispatch_admin_probe_with_proxy(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        proxy: ProviderProxyConfig,
    ) -> Response {
        if !is_active_key(&key) {
            return (StatusCode::FORBIDDEN, "llm key is not active").into_response();
        }
        if !key_matches_route(&key, request.uri().path()) {
            return (StatusCode::FORBIDDEN, "llm key does not match provider route")
                .into_response();
        }
        if is_quota_exhausted(&key) {
            return quota_exhausted_response(&key);
        }

        let mut deps = self.dispatch_deps();
        deps.route_store = Arc::new(ForcedProxyRouteStore {
            inner: Arc::clone(&self.route_store),
            proxy,
        });
        let _activity_guard = self.request_activity.start(&key.key_id);
        self.dispatcher.dispatch(key, request, deps).await
    }

    pub(crate) fn kiro_cache_stats(
        &self,
        config: KiroCacheSimulationConfig,
    ) -> KiroCacheRuntimeStats {
        self.kiro_cache_simulator
            .snapshot_stats(config, Instant::now())
    }

    pub(super) fn dispatch_deps(&self) -> ProviderDispatchDeps {
        ProviderDispatchDeps {
            route_store: Arc::clone(&self.route_store),
            control_store: Arc::clone(&self.control_store),
            geoip: self.geoip.clone(),
            admin_config_store: Arc::clone(&self.admin_config_store),
            kiro_cache_simulator: Arc::clone(&self.kiro_cache_simulator),
            request_limiter: Arc::clone(&self.request_limiter),
            codex_account_cooldowns: Arc::clone(&self.codex_account_cooldowns),
            codex_session_affinity: Arc::clone(&self.codex_session_affinity),
            kiro_request_scheduler: Arc::clone(&self.kiro_request_scheduler),
            kiro_session_affinity: Arc::clone(&self.kiro_session_affinity),
            kiro_latency_ranker: Arc::clone(&self.kiro_latency_ranker),
            protected_thinking_signature_secret: self.protected_thinking_signature_secret.clone(),
        }
    }
}
impl ForcedProxyRouteStore {
    fn force_codex_proxy(&self, mut route: ProviderCodexRoute) -> ProviderCodexRoute {
        route.proxy = Some(self.proxy.clone());
        route
    }

    fn force_kiro_proxy(&self, mut route: ProviderKiroRoute) -> ProviderKiroRoute {
        route.proxy = Some(self.proxy.clone());
        route
    }
}
#[async_trait]
impl ProviderRouteStore for ForcedProxyRouteStore {
    async fn resolve_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .inner
            .resolve_codex_route(key)
            .await?
            .map(|route| self.force_codex_proxy(route)))
    }

    async fn resolve_codex_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        Ok(self
            .inner
            .resolve_codex_route_candidates(key)
            .await?
            .into_iter()
            .map(|route| self.force_codex_proxy(route))
            .collect())
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .inner
            .resolve_codex_account_route(account_name)
            .await?
            .map(|route| self.force_codex_proxy(route)))
    }

    async fn resolve_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .inner
            .resolve_kiro_route(key)
            .await?
            .map(|route| self.force_kiro_proxy(route)))
    }

    async fn resolve_kiro_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        Ok(self
            .inner
            .resolve_kiro_route_candidates(key)
            .await?
            .into_iter()
            .map(|route| self.force_kiro_proxy(route))
            .collect())
    }

    async fn resolve_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .inner
            .resolve_kiro_account_route(account_name)
            .await?
            .map(|route| self.force_kiro_proxy(route)))
    }

    async fn save_kiro_auth_update(&self, update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        self.inner.save_kiro_auth_update(update).await
    }

    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        self.inner.save_codex_auth_update(update).await
    }

    async fn set_codex_account_auto_refresh_enabled(
        &self,
        account_name: &str,
        enabled: bool,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.inner
            .set_codex_account_auto_refresh_enabled(account_name, enabled, updated_at_ms)
            .await
    }

    async fn mark_kiro_account_quota_exhausted(
        &self,
        account_name: &str,
        error_message: &str,
        checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.inner
            .mark_kiro_account_quota_exhausted(account_name, error_message, checked_at_ms)
            .await
    }

    async fn save_kiro_status_cache_update(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.inner.save_kiro_status_cache_update(update).await
    }
}
