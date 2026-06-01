CREATE TABLE IF NOT EXISTS llm_proxy_config_endpoint_checks (
    proxy_config_id TEXT NOT NULL REFERENCES llm_proxy_configs(proxy_config_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('codex', 'kiro')),
    target_url TEXT NOT NULL,
    reachable BOOLEAN NOT NULL,
    status_code INTEGER CHECK (status_code IS NULL OR (status_code >= 100 AND status_code <= 599)),
    latency_ms BIGINT NOT NULL CHECK (latency_ms >= 0),
    error_message TEXT,
    checked_at_ms BIGINT NOT NULL CHECK (checked_at_ms >= 0),
    PRIMARY KEY (proxy_config_id, node_id, provider_type)
);

CREATE INDEX IF NOT EXISTS idx_llm_proxy_config_endpoint_checks_node
    ON llm_proxy_config_endpoint_checks(node_id);
