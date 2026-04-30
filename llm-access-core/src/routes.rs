//! Canonical route surface owned by the standalone LLM access service.

use crate::provider::{ProtocolFamily, ProviderType};

/// HTTP route declaration used by compatibility tests and router wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSpec {
    /// HTTP method or `ANY` for wildcard proxy routes.
    pub method: &'static str,
    /// Axum-compatible route pattern.
    pub path: &'static str,
}

/// Provider and wire-protocol required by a provider-facing route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRouteRequirement {
    /// Required backend provider family.
    pub provider_type: ProviderType,
    /// Required client-facing protocol family.
    pub protocol_family: ProtocolFamily,
}

const CODEX_OPENAI_ROUTE: ProviderRouteRequirement = ProviderRouteRequirement {
    provider_type: ProviderType::Codex,
    protocol_family: ProtocolFamily::OpenAi,
};

const KIRO_ANTHROPIC_ROUTE: ProviderRouteRequirement = ProviderRouteRequirement {
    provider_type: ProviderType::Kiro,
    protocol_family: ProtocolFamily::Anthropic,
};

/// Public/provider routes that must be handled by `llm-access`.
pub const PUBLIC_PROVIDER_ROUTES: &[RouteSpec] = &[
    RouteSpec {
        method: "ANY",
        path: "/api/llm-gateway/v1/*path",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/access",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/model-catalog.json",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/status",
    },
    RouteSpec {
        method: "POST",
        path: "/api/llm-gateway/public-usage/query",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/support-config",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/support-assets/:file_name",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/account-contributions",
    },
    RouteSpec {
        method: "GET",
        path: "/api/llm-gateway/sponsors",
    },
    RouteSpec {
        method: "POST",
        path: "/api/llm-gateway/token-requests/submit",
    },
    RouteSpec {
        method: "POST",
        path: "/api/llm-gateway/account-contribution-requests/submit",
    },
    RouteSpec {
        method: "POST",
        path: "/api/llm-gateway/sponsor-requests/submit",
    },
    RouteSpec {
        method: "GET",
        path: "/api/kiro-gateway/access",
    },
    RouteSpec {
        method: "GET",
        path: "/api/kiro-gateway/v1/models",
    },
    RouteSpec {
        method: "POST",
        path: "/api/kiro-gateway/v1/messages",
    },
    RouteSpec {
        method: "POST",
        path: "/api/kiro-gateway/v1/messages/count_tokens",
    },
    RouteSpec {
        method: "POST",
        path: "/api/kiro-gateway/cc/v1/messages",
    },
    RouteSpec {
        method: "POST",
        path: "/api/kiro-gateway/cc/v1/messages/count_tokens",
    },
];

/// Admin routes that must keep working for the current frontend.
pub const ADMIN_ROUTES: &[RouteSpec] = &[
    RouteSpec {
        method: "GET|POST",
        path: "/admin/llm-gateway/config",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/llm-gateway/proxy-configs",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/proxy-configs/import-legacy-kiro",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/llm-gateway/proxy-configs/:proxy_id",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/proxy-configs/:proxy_id/check/:provider_type",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/llm-gateway/proxy-bindings",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/proxy-bindings/:provider_type",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/llm-gateway/account-groups",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/llm-gateway/account-groups/:group_id",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/llm-gateway/keys",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/llm-gateway/keys/:key_id",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/llm-gateway/usage",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/llm-gateway/usage/:event_id",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/llm-gateway/token-requests",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/token-requests/:request_id/approve-and-issue",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/token-requests/:request_id/reject",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/llm-gateway/account-contribution-requests",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/account-contribution-requests/:request_id/approve-and-issue",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/account-contribution-requests/:request_id/reject",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/llm-gateway/sponsor-requests",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/sponsor-requests/:request_id/approve",
    },
    RouteSpec {
        method: "DELETE",
        path: "/admin/llm-gateway/sponsor-requests/:request_id",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/llm-gateway/accounts",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/llm-gateway/accounts/:name",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/llm-gateway/accounts/:name/refresh",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/kiro-gateway/account-groups",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/kiro-gateway/account-groups/:group_id",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/kiro-gateway/keys",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/kiro-gateway/keys/:key_id",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/kiro-gateway/usage",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/kiro-gateway/usage/:event_id",
    },
    RouteSpec {
        method: "GET",
        path: "/admin/kiro-gateway/accounts/statuses",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/kiro-gateway/accounts",
    },
    RouteSpec {
        method: "POST",
        path: "/admin/kiro-gateway/accounts/import-local",
    },
    RouteSpec {
        method: "PATCH|DELETE",
        path: "/admin/kiro-gateway/accounts/:name",
    },
    RouteSpec {
        method: "GET|POST",
        path: "/admin/kiro-gateway/accounts/:name/balance",
    },
];

/// Return whether a path is owned by `llm-access`.
pub fn is_llm_access_path(path: &str) -> bool {
    path == "/healthz"
        || path == "/version"
        || path.starts_with("/v1/")
        || path.starts_with("/cc/v1/")
        || path.starts_with("/api/llm-gateway/")
        || path.starts_with("/api/kiro-gateway/")
        || path.starts_with("/api/codex-gateway/")
        || path.starts_with("/api/llm-access/")
        || path.starts_with("/admin/llm-gateway/")
        || path.starts_with("/admin/kiro-gateway/")
}

/// Return the provider/protocol contract for provider data-plane routes.
pub fn provider_route_requirement(path: &str) -> Option<ProviderRouteRequirement> {
    if path.starts_with("/cc/v1/")
        || path.starts_with("/api/kiro-gateway/v1/")
        || path.starts_with("/api/kiro-gateway/cc/v1/")
    {
        Some(KIRO_ANTHROPIC_ROUTE)
    } else if path.starts_with("/v1/")
        || path.starts_with("/api/codex-gateway/")
        || path.starts_with("/api/llm-gateway/v1/")
    {
        Some(CODEX_OPENAI_ROUTE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_llm_access_path, provider_route_requirement, ProviderRouteRequirement, ADMIN_ROUTES,
        PUBLIC_PROVIDER_ROUTES,
    };
    use crate::provider::{ProtocolFamily, ProviderType};

    #[test]
    fn route_contract_contains_required_public_provider_paths() {
        let paths = PUBLIC_PROVIDER_ROUTES
            .iter()
            .map(|route| route.path)
            .collect::<Vec<_>>();
        assert!(paths.contains(&"/api/llm-gateway/v1/*path"));
        assert!(paths.contains(&"/api/kiro-gateway/v1/messages"));
        assert!(paths.contains(&"/api/kiro-gateway/cc/v1/messages"));
        assert!(paths.contains(&"/api/llm-gateway/public-usage/query"));
    }

    #[test]
    fn route_contract_contains_required_admin_paths() {
        let paths = ADMIN_ROUTES
            .iter()
            .map(|route| route.path)
            .collect::<Vec<_>>();
        assert!(paths.contains(&"/admin/llm-gateway/keys"));
        assert!(paths.contains(&"/admin/llm-gateway/accounts/:name/refresh"));
        assert!(paths.contains(&"/admin/kiro-gateway/keys/:key_id"));
        assert!(paths.contains(&"/admin/kiro-gateway/accounts/:name/balance"));
    }

    #[test]
    fn route_ownership_matches_llm_path_prefixes() {
        assert!(is_llm_access_path("/api/llm-gateway/status"));
        assert!(is_llm_access_path("/api/kiro-gateway/cc/v1/messages"));
        assert!(is_llm_access_path("/admin/kiro-gateway/accounts"));
        assert!(!is_llm_access_path("/api/articles"));
        assert!(!is_llm_access_path("/admin/local-media"));
    }

    #[test]
    fn provider_route_requirement_matches_provider_protocol_prefixes() {
        let codex = ProviderRouteRequirement {
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
        };
        let kiro = ProviderRouteRequirement {
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
        };

        assert_eq!(provider_route_requirement("/v1/responses"), Some(codex));
        assert_eq!(provider_route_requirement("/api/llm-gateway/v1/responses"), Some(codex));
        assert_eq!(provider_route_requirement("/api/codex-gateway/v1/responses"), Some(codex));
        assert_eq!(provider_route_requirement("/cc/v1/messages"), Some(kiro));
        assert_eq!(provider_route_requirement("/api/kiro-gateway/v1/messages"), Some(kiro));
        assert_eq!(provider_route_requirement("/api/kiro-gateway/cc/v1/messages"), Some(kiro));
        assert_eq!(provider_route_requirement("/api/llm-gateway/status"), None);
        assert_eq!(provider_route_requirement("/api/kiro-gateway/access"), None);
    }
}
