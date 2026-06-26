//! Routing, concurrency, and logging coverage for Codex image dispatch.

use axum::http::StatusCode;
use llm_access_codex_image::{
    dispatch::{eligible_image_routes, should_failover_status},
    limiter::{
        image_account_limiter_scope, image_key_limiter_scope, ImageAccountLimiter, ImageKeyLimiter,
    },
    logging::{build_image_log_event, ImageLogInput, UpstreamLogInput},
};
use llm_access_core::{provider::RouteStrategy, store::ProviderCodexRoute};

fn route(account_name: &str, key_enabled: bool, account_enabled: bool) -> ProviderCodexRoute {
    ProviderCodexRoute {
        account_name: account_name.to_string(),
        account_group_id_at_event: None,
        route_strategy_at_event: RouteStrategy::Auto,
        auth_json: r#"{"access_token":"token"}"#.to_string(),
        map_gpt53_codex_to_spark: false,
        auth_refresh_enabled: true,
        codex_fast_enabled: true,
        codex_strict_session_rejection_enabled: false,
        codex_image_generation_enabled: key_enabled,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        account_request_max_concurrency: None,
        account_request_min_start_interval_ms: None,
        account_codex_image_generation_enabled: account_enabled,
        account_codex_image_generation_max_concurrency:
            llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY,
        cached_error_message: None,
        proxy: None,
    }
}

#[test]
fn image_route_selection_rejects_disabled_key_and_disabled_accounts() {
    let err = eligible_image_routes(vec![route("codex-a", false, true)])
        .expect_err("key-level image switch must be enforced");
    assert_eq!(err.status, StatusCode::FORBIDDEN);

    let err = eligible_image_routes(vec![route("codex-a", true, false)])
        .expect_err("account-level image switch must be enforced");
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);

    let selected =
        eligible_image_routes(vec![route("codex-a", true, false), route("codex-b", true, true)])
            .expect("image-enabled accounts should be selected");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].account_name, "codex-b");
}

#[tokio::test]
async fn image_account_limiter_uses_independent_scope_and_default_limit() {
    assert_eq!(image_account_limiter_scope("codex-a"), "account:codex-image:codex-a");

    let limiter = ImageAccountLimiter::default();
    let first = limiter
        .try_acquire("codex-a", None)
        .expect("first permit should be available");
    let second = limiter
        .try_acquire("codex-a", None)
        .expect("second permit should be available");
    let third = limiter
        .try_acquire("codex-a", None)
        .expect("third permit should be available");
    assert!(limiter.try_acquire("codex-a", None).is_none());
    drop(first);
    assert!(limiter.try_acquire("codex-a", None).is_some());
    drop(second);
    drop(third);
}

#[tokio::test]
async fn image_account_limiter_honors_lowered_account_limit() {
    let limiter = ImageAccountLimiter::default();
    let first = limiter
        .try_acquire("codex-a", Some(3))
        .expect("first permit should be available");
    let second = limiter
        .try_acquire("codex-a", Some(3))
        .expect("second permit should be available");

    assert!(limiter.try_acquire("codex-a", Some(1)).is_none());
    drop(first);
    assert!(limiter.try_acquire("codex-a", Some(1)).is_none());
    drop(second);
    assert!(limiter.try_acquire("codex-a", Some(1)).is_some());
}

#[test]
fn image_failover_only_retries_transient_or_auth_failures() {
    assert!(!should_failover_status(StatusCode::BAD_REQUEST));
    assert!(!should_failover_status(StatusCode::NOT_FOUND));
    assert!(!should_failover_status(StatusCode::UNPROCESSABLE_ENTITY));
    assert!(should_failover_status(StatusCode::UNAUTHORIZED));
    assert!(should_failover_status(StatusCode::FORBIDDEN));
    assert!(should_failover_status(StatusCode::TOO_MANY_REQUESTS));
    assert!(should_failover_status(StatusCode::INTERNAL_SERVER_ERROR));
    assert!(should_failover_status(StatusCode::SERVICE_UNAVAILABLE));
    assert!(should_failover_status(StatusCode::BAD_GATEWAY));
}

#[tokio::test]
async fn image_key_limiter_uses_independent_scope_and_route_limits() {
    assert_eq!(image_key_limiter_scope("key-1"), "key:codex-image:key-1");

    let limiter = ImageKeyLimiter::default();
    let first = limiter
        .try_acquire("key-1", Some(1), None)
        .expect("first key permit should be available");
    let blocked = limiter
        .try_acquire("key-1", Some(1), None)
        .expect_err("key max concurrency should block");
    assert_eq!(blocked.reason, "key_max_concurrency");
    drop(first);

    limiter
        .try_acquire("key-2", None, Some(60_000))
        .expect("first interval permit should be available");
    let blocked = limiter
        .try_acquire("key-2", None, Some(60_000))
        .expect_err("key min start interval should block");
    assert_eq!(blocked.reason, "key_min_start_interval");
}

#[test]
fn image_log_event_redacts_prompt_and_image_payloads() {
    let event = build_image_log_event(ImageLogInput {
        request_id: "req-1",
        key_id: "key-1",
        key_name: "Key One",
        account_name: Some("codex-a"),
        endpoint: "edits",
        prompt: "draw an exact scene with confidential text",
        size: Some("1024x1024"),
        quality: Some("high"),
        n: 2,
        input_images: &["data:image/png;base64,AAABBBCCC", "https://example.com/input.png"],
        upstream: UpstreamLogInput {
            status: Some(200),
            duration_ms: 17,
            failover_count: 1,
            error_class: None,
            response_image_count: Some(2),
            response_image_bytes: Some(4096),
            usage_tokens: None,
            usage_missing: true,
        },
    });
    let encoded = serde_json::to_string(&event).expect("encode log event");

    assert!(!encoded.contains("AAABBBCCC"));
    assert!(!encoded.contains("data:image"));
    assert!(!encoded.contains("example.com"));
    assert!(!encoded.contains("input.png"));
    assert!(!encoded.contains("confidential text"));
    assert!(encoded.contains("prompt_hash"));
    assert!(encoded.contains("input_image_count"));
}
