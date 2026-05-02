ALTER TABLE llm_runtime_config
ADD COLUMN duckdb_usage_memory_limit_mib INTEGER NOT NULL DEFAULT 1024
    CHECK (duckdb_usage_memory_limit_mib >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN duckdb_usage_checkpoint_threshold_mib INTEGER NOT NULL DEFAULT 16
    CHECK (duckdb_usage_checkpoint_threshold_mib >= 16);
