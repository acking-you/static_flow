ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_full_request_logging_enabled BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE IF EXISTS llm_account_contribution_requests
    ADD COLUMN IF NOT EXISTS show_on_public_wall BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS duckdb_usage_memory_limit_mib BIGINT NOT NULL DEFAULT 1024;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS duckdb_usage_checkpoint_threshold_mib BIGINT NOT NULL DEFAULT 16;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_max_file_bytes BIGINT NOT NULL DEFAULT 67108864;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_max_file_age_ms BIGINT NOT NULL DEFAULT 300000;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_max_files BIGINT NOT NULL DEFAULT 128;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_block_target_uncompressed_bytes BIGINT NOT NULL DEFAULT 1048576;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_block_max_events BIGINT NOT NULL DEFAULT 1024;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_fsync_interval_ms BIGINT NOT NULL DEFAULT 250;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_zstd_level BIGINT NOT NULL DEFAULT 3;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_consumer_lease_ms BIGINT NOT NULL DEFAULT 300000;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_journal_delete_bad_files BIGINT NOT NULL DEFAULT 0;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_query_bind_addr TEXT NOT NULL DEFAULT '127.0.0.1:19081';

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_query_base_url TEXT NOT NULL DEFAULT 'http://127.0.0.1:19081';

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_weight_free BIGINT NOT NULL DEFAULT 1;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_weight_plus BIGINT NOT NULL DEFAULT 10;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_weight_pro5x BIGINT NOT NULL DEFAULT 50;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_weight_pro20x BIGINT NOT NULL DEFAULT 200;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_analytics_retention_days BIGINT NOT NULL DEFAULT 7;
