//! Route ownership helpers for cloud path splitting.

pub use llm_access_core::routes::is_llm_access_path;

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

    #[test]
    fn recognizes_admin_llm_paths() {
        for path in ["/admin/llm-gateway/keys", "/admin/kiro-gateway/accounts"] {
            assert!(super::is_llm_access_path(path), "{path}");
        }
    }
}
