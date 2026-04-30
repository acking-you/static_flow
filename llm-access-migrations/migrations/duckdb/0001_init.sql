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
    endpoint VARCHAR NOT NULL,
    model VARCHAR,
    mapped_model VARCHAR,
    status_code INTEGER NOT NULL,
    latency_ms INTEGER,
    routing_wait_ms INTEGER,
    upstream_headers_ms INTEGER,
    post_headers_body_ms INTEGER,
    first_sse_write_ms INTEGER,
    stream_finish_ms INTEGER,
    request_body_bytes BIGINT,
    input_uncached_tokens BIGINT NOT NULL,
    input_cached_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    billable_tokens BIGINT NOT NULL,
    credit_usage DECIMAL(24, 12),
    usage_missing BOOLEAN NOT NULL,
    credit_usage_missing BOOLEAN NOT NULL,
    client_ip VARCHAR,
    ip_region VARCHAR
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_events_source_event_id
    ON usage_events(source_event_id);

CREATE INDEX IF NOT EXISTS idx_usage_events_source_seq
    ON usage_events(source_seq);

CREATE INDEX IF NOT EXISTS idx_usage_events_created_date
    ON usage_events(created_date);

CREATE INDEX IF NOT EXISTS idx_usage_events_key_date
    ON usage_events(key_id, created_date);

CREATE INDEX IF NOT EXISTS idx_usage_events_provider_date
    ON usage_events(provider_type, created_date);

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

CREATE TABLE IF NOT EXISTS cdc_event_log (
    source_seq BIGINT PRIMARY KEY,
    event_id VARCHAR NOT NULL,
    source_instance VARCHAR NOT NULL,
    entity VARCHAR NOT NULL,
    op VARCHAR NOT NULL,
    primary_key VARCHAR NOT NULL,
    schema_version INTEGER NOT NULL,
    payload_json VARCHAR NOT NULL,
    created_at_ms BIGINT NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_cdc_event_log_entity_seq
    ON cdc_event_log(entity, source_seq);

CREATE TABLE IF NOT EXISTS cdc_apply_audit (
    audit_id VARCHAR PRIMARY KEY,
    consumer_name VARCHAR NOT NULL,
    source_seq BIGINT NOT NULL,
    event_id VARCHAR NOT NULL,
    entity VARCHAR NOT NULL,
    op VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    error_message VARCHAR,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_cdc_apply_audit_consumer_seq
    ON cdc_apply_audit(consumer_name, source_seq);
