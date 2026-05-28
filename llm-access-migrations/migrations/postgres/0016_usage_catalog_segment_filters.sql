ALTER TABLE llm_usage_segments
    ADD COLUMN IF NOT EXISTS input_uncached_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS input_cached_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS output_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS billable_tokens BIGINT NOT NULL DEFAULT 0;

ALTER TABLE llm_usage_segment_key_rollups
    ADD COLUMN IF NOT EXISTS first_used_at_ms BIGINT;

UPDATE llm_usage_segments AS s
SET input_uncached_tokens = rollups.input_uncached_tokens,
    input_cached_tokens = rollups.input_cached_tokens,
    output_tokens = rollups.output_tokens,
    billable_tokens = rollups.billable_tokens
FROM (
    SELECT
        segment_id,
        COALESCE(SUM(input_uncached_tokens), 0)::BIGINT AS input_uncached_tokens,
        COALESCE(SUM(input_cached_tokens), 0)::BIGINT AS input_cached_tokens,
        COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
        COALESCE(SUM(billable_tokens), 0)::BIGINT AS billable_tokens
    FROM llm_usage_segment_key_rollups
    GROUP BY segment_id
) AS rollups
WHERE s.segment_id = rollups.segment_id;

UPDATE llm_usage_segment_key_rollups AS r
SET first_used_at_ms = s.start_ms
FROM llm_usage_segments AS s
WHERE r.segment_id = s.segment_id
  AND r.first_used_at_ms IS NULL;

CREATE INDEX IF NOT EXISTS idx_llm_usage_segment_key_rollups_scope_time
    ON llm_usage_segment_key_rollups (
        key_id,
        provider_type,
        last_used_at_ms DESC,
        first_used_at_ms DESC,
        segment_id
    );

CREATE TABLE IF NOT EXISTS llm_usage_segment_field_rollups (
    segment_id TEXT NOT NULL REFERENCES llm_usage_segments(segment_id) ON DELETE CASCADE,
    key_id TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    field_name TEXT NOT NULL CHECK (
        field_name IN ('model', 'account_name', 'endpoint', 'status_code', 'status_kind')
    ),
    field_value TEXT NOT NULL,
    row_count BIGINT NOT NULL CHECK (row_count >= 0),
    input_uncached_tokens BIGINT NOT NULL,
    input_cached_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    billable_tokens BIGINT NOT NULL,
    first_used_at_ms BIGINT,
    last_used_at_ms BIGINT,
    PRIMARY KEY (segment_id, key_id, provider_type, field_name, field_value)
);

CREATE INDEX IF NOT EXISTS idx_llm_usage_segment_field_rollups_lookup
    ON llm_usage_segment_field_rollups (
        field_name,
        field_value,
        key_id,
        provider_type,
        last_used_at_ms DESC,
        first_used_at_ms DESC,
        segment_id
    );
