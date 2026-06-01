CREATE TABLE IF NOT EXISTS llm_proxy_config_node_overrides (
    proxy_config_id TEXT NOT NULL REFERENCES llm_proxy_configs(proxy_config_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    proxy_url TEXT NOT NULL,
    proxy_username TEXT,
    proxy_password TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY (proxy_config_id, node_id)
);

CREATE INDEX IF NOT EXISTS idx_llm_proxy_config_node_overrides_node
    ON llm_proxy_config_node_overrides(node_id);
