ALTER TABLE IF EXISTS llm_anthropic_upstream_channels
    ADD COLUMN IF NOT EXISTS model_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS last_models_status TEXT,
    ADD COLUMN IF NOT EXISTS last_models_latency_ms BIGINT CHECK (
        last_models_latency_ms IS NULL OR last_models_latency_ms >= 0
    ),
    ADD COLUMN IF NOT EXISTS last_models_checked_at_ms BIGINT CHECK (
        last_models_checked_at_ms IS NULL OR last_models_checked_at_ms >= 0
    ),
    ADD COLUMN IF NOT EXISTS last_models_error TEXT,
    ADD COLUMN IF NOT EXISTS last_test_model TEXT,
    ADD COLUMN IF NOT EXISTS last_test_status TEXT,
    ADD COLUMN IF NOT EXISTS last_test_latency_ms BIGINT CHECK (
        last_test_latency_ms IS NULL OR last_test_latency_ms >= 0
    ),
    ADD COLUMN IF NOT EXISTS last_test_at_ms BIGINT CHECK (
        last_test_at_ms IS NULL OR last_test_at_ms >= 0
    ),
    ADD COLUMN IF NOT EXISTS last_test_error TEXT;

UPDATE llm_anthropic_upstream_channels
SET model_ids = '[]'::jsonb
WHERE model_ids IS NULL
   OR jsonb_typeof(model_ids) <> 'array';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'ck_llm_anthropic_upstream_channels_model_ids_array'
    ) THEN
        ALTER TABLE llm_anthropic_upstream_channels
            ADD CONSTRAINT ck_llm_anthropic_upstream_channels_model_ids_array
            CHECK (jsonb_typeof(model_ids) = 'array');
    END IF;
END $$;
