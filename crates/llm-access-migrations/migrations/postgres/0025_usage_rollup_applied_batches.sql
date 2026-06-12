CREATE TABLE IF NOT EXISTS llm_key_usage_rollup_applied_batches (
    batch_id TEXT PRIMARY KEY,
    digest TEXT NOT NULL,
    source_node_id TEXT,
    source_event_count BIGINT NOT NULL CHECK (source_event_count >= 0),
    delta_count BIGINT NOT NULL CHECK (delta_count >= 0),
    applied_at_ms BIGINT NOT NULL CHECK (applied_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_key_usage_rollup_applied_batches_applied_at
    ON llm_key_usage_rollup_applied_batches(applied_at_ms);
