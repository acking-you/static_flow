//! Backend health endpoints and response payloads.

use axum::{extract::State, Json};
use serde::Serialize;

use crate::state::AppState;

/// Health response returned by `/api/healthz`.
#[derive(Debug, Clone, Serialize)]
pub struct HealthzResponse {
    /// Constant health status.
    pub status: String,
    /// Current process identifier.
    pub pid: u32,
    /// Effective listen port.
    pub port: u16,
    /// Backend startup timestamp in milliseconds.
    pub started_at: i64,
    /// Build id or package version.
    pub version: String,
}

/// Return minimal backend runtime metadata for upgrade checks.
pub async fn get_healthz(State(state): State<AppState>) -> Json<HealthzResponse> {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);

    Json(HealthzResponse {
        status: "ok".to_string(),
        pid: std::process::id(),
        port,
        started_at: state.runtime_metadata.started_at_ms,
        version: state.runtime_metadata.build_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::HealthzResponse;

    #[test]
    fn healthz_response_serializes_runtime_metadata() {
        let body = HealthzResponse {
            status: "ok".to_string(),
            pid: 123,
            port: 39080,
            started_at: 1,
            version: "test-build".to_string(),
        };
        let json = serde_json::to_value(&body).expect("serialize healthz");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["pid"], 123);
        assert_eq!(json["port"], 39080);
    }
}
