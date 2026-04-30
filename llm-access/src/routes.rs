//! Route ownership helpers for cloud path splitting.

/// Return whether a request path is owned by the standalone `llm-access`
/// service instead of the local StaticFlow backend.
pub fn is_llm_access_path(path: &str) -> bool {
    path == "/healthz"
        || path == "/version"
        || path.starts_with("/v1/")
        || path.starts_with("/cc/v1/")
        || path.starts_with("/api/llm-gateway/")
        || path.starts_with("/api/kiro-gateway/")
        || path.starts_with("/api/codex-gateway/")
        || path.starts_with("/api/llm-access/")
}

#[cfg(test)]
mod tests {
    #[test]
    fn recognizes_public_llm_provider_paths() {
        for path in [
            "/v1/chat/completions",
            "/v1/responses",
            "/v1/models",
            "/cc/v1/messages",
            "/api/llm-gateway/v1/responses",
            "/api/kiro-gateway/v1/messages",
            "/api/codex-gateway/v1/responses",
            "/api/llm-access/status",
        ] {
            assert!(super::is_llm_access_path(path), "{path}");
        }
    }

    #[test]
    fn leaves_non_llm_staticflow_paths_on_local_backend() {
        for path in ["/", "/api/articles", "/api/music/songs", "/admin/local-media"] {
            assert!(!super::is_llm_access_path(path), "{path}");
        }
    }
}
