CREATE TABLE IF NOT EXISTS llm_proxy_config_traffic_snapshots (
    proxy_config_id TEXT PRIMARY KEY REFERENCES llm_proxy_configs(proxy_config_id) ON DELETE CASCADE,
    refreshed_at_ms BIGINT NOT NULL CHECK (refreshed_at_ms >= 0),
    window_start_ms BIGINT NOT NULL CHECK (window_start_ms >= 0),
    window_end_ms BIGINT NOT NULL CHECK (window_end_ms > window_start_ms),
    retention_days BIGINT NOT NULL CHECK (retention_days > 0),
    event_count BIGINT NOT NULL CHECK (event_count >= 0),
    request_bytes BIGINT NOT NULL CHECK (request_bytes >= 0),
    response_bytes BIGINT NOT NULL CHECK (response_bytes >= 0),
    total_bytes BIGINT NOT NULL CHECK (total_bytes >= 0)
);
