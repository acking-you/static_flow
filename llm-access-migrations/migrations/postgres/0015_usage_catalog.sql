CREATE TABLE IF NOT EXISTS llm_usage_segments (
    segment_id TEXT PRIMARY KEY,
    archive_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('archived')),
    start_ms BIGINT,
    end_ms BIGINT,
    row_count BIGINT NOT NULL CHECK (row_count >= 0),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    sealed_at_ms BIGINT NOT NULL CHECK (sealed_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_usage_segments_state_time
    ON llm_usage_segments (state, end_ms DESC, start_ms DESC, segment_id DESC);

CREATE TABLE IF NOT EXISTS llm_usage_segment_events (
    event_id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL REFERENCES llm_usage_segments(segment_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_llm_usage_segment_events_segment_id
    ON llm_usage_segment_events (segment_id);

CREATE TABLE IF NOT EXISTS llm_usage_segment_key_rollups (
    segment_id TEXT NOT NULL REFERENCES llm_usage_segments(segment_id) ON DELETE CASCADE,
    key_id TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    row_count BIGINT NOT NULL CHECK (row_count >= 0),
    input_uncached_tokens BIGINT NOT NULL,
    input_cached_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    billable_tokens BIGINT NOT NULL,
    credit_total TEXT NOT NULL,
    credit_missing_events BIGINT NOT NULL CHECK (credit_missing_events >= 0),
    last_used_at_ms BIGINT,
    PRIMARY KEY (segment_id, key_id, provider_type)
);

CREATE INDEX IF NOT EXISTS idx_llm_usage_segment_key_rollups_key
    ON llm_usage_segment_key_rollups (key_id, provider_type);
