CREATE TABLE IF NOT EXISTS usage_events (
    source_seq BIGINT NOT NULL,
    source_event_id VARCHAR NOT NULL,
    event_id VARCHAR PRIMARY KEY,
    created_at_ms BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    created_date DATE NOT NULL,
    created_hour TIMESTAMP NOT NULL,
    provider_type VARCHAR NOT NULL,
    protocol_family VARCHAR NOT NULL,
    key_id VARCHAR NOT NULL,
    key_name VARCHAR NOT NULL,
    key_status_at_event VARCHAR NOT NULL,
    account_name VARCHAR,
    account_group_id_at_event VARCHAR,
    route_strategy_at_event VARCHAR,
    request_method VARCHAR NOT NULL DEFAULT 'POST',
    request_url VARCHAR NOT NULL DEFAULT '',
    endpoint VARCHAR NOT NULL,
    model VARCHAR,
    mapped_model VARCHAR,
    status_code INTEGER NOT NULL,
    latency_ms INTEGER,
    routing_wait_ms INTEGER,
    upstream_headers_ms INTEGER,
    post_headers_body_ms INTEGER,
    request_body_read_ms INTEGER,
    request_json_parse_ms INTEGER,
    pre_handler_ms INTEGER,
    first_sse_write_ms INTEGER,
    stream_finish_ms INTEGER,
    request_body_bytes BIGINT,
    quota_failover_count BIGINT NOT NULL DEFAULT 0,
    routing_diagnostics_json VARCHAR,
    input_uncached_tokens BIGINT NOT NULL,
    input_cached_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    billable_tokens BIGINT NOT NULL,
    credit_usage DECIMAL(24, 12),
    usage_missing BOOLEAN NOT NULL,
    credit_usage_missing BOOLEAN NOT NULL,
    client_ip VARCHAR,
    ip_region VARCHAR,
    request_headers_json VARCHAR NOT NULL DEFAULT '{}',
    last_message_content VARCHAR,
    client_request_body_json VARCHAR,
    upstream_request_body_json VARCHAR,
    full_request_json VARCHAR
);
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS request_method VARCHAR DEFAULT 'POST';
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS request_url VARCHAR DEFAULT '';
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS request_body_read_ms INTEGER;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS request_json_parse_ms INTEGER;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS pre_handler_ms INTEGER;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS quota_failover_count BIGINT DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS routing_diagnostics_json VARCHAR;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS request_headers_json VARCHAR DEFAULT '{}';
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS last_message_content VARCHAR;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS client_request_body_json VARCHAR;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS upstream_request_body_json VARCHAR;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS full_request_json VARCHAR;

CREATE TABLE IF NOT EXISTS usage_event_details (
    event_id VARCHAR PRIMARY KEY,
    request_headers_json VARCHAR,
    routing_diagnostics_json VARCHAR,
    last_message_content VARCHAR,
    client_request_body_json VARCHAR,
    upstream_request_body_json VARCHAR,
    full_request_json VARCHAR
);

CREATE TABLE IF NOT EXISTS usage_rollups_hourly (
    bucket_hour TIMESTAMP NOT NULL,
    provider_type VARCHAR NOT NULL,
    protocol_family VARCHAR NOT NULL,
    key_id VARCHAR NOT NULL,
    key_name VARCHAR NOT NULL,
    account_name VARCHAR,
    account_group_id_at_event VARCHAR,
    route_strategy_at_event VARCHAR,
    endpoint VARCHAR NOT NULL,
    model VARCHAR,
    mapped_model VARCHAR,
    status_code_class INTEGER NOT NULL,
    request_count BIGINT NOT NULL,
    input_uncached_tokens BIGINT NOT NULL,
    input_cached_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    billable_tokens BIGINT NOT NULL,
    credit_usage DECIMAL(24, 12),
    credit_usage_missing_count BIGINT NOT NULL,
    avg_latency_ms DOUBLE,
    max_latency_ms INTEGER,
    p95_latency_ms DOUBLE,
    PRIMARY KEY (
        bucket_hour,
        provider_type,
        key_id,
        account_name,
        endpoint,
        model,
        status_code_class
    )
);

CREATE TABLE IF NOT EXISTS usage_rollups_daily (
    bucket_date DATE NOT NULL,
    provider_type VARCHAR NOT NULL,
    protocol_family VARCHAR NOT NULL,
    key_id VARCHAR NOT NULL,
    key_name VARCHAR NOT NULL,
    account_name VARCHAR,
    account_group_id_at_event VARCHAR,
    route_strategy_at_event VARCHAR,
    endpoint VARCHAR NOT NULL,
    model VARCHAR,
    mapped_model VARCHAR,
    status_code_class INTEGER NOT NULL,
    request_count BIGINT NOT NULL,
    input_uncached_tokens BIGINT NOT NULL,
    input_cached_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    billable_tokens BIGINT NOT NULL,
    credit_usage DECIMAL(24, 12),
    credit_usage_missing_count BIGINT NOT NULL,
    avg_latency_ms DOUBLE,
    max_latency_ms INTEGER,
    p95_latency_ms DOUBLE,
    PRIMARY KEY (
        bucket_date,
        provider_type,
        key_id,
        account_name,
        endpoint,
        model,
        status_code_class
    )
);
