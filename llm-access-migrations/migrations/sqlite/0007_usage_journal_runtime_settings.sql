ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_enabled INTEGER NOT NULL DEFAULT 1
    CHECK (usage_journal_enabled IN (0, 1));

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_max_file_bytes INTEGER NOT NULL DEFAULT 67108864
    CHECK (usage_journal_max_file_bytes >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_max_file_age_ms INTEGER NOT NULL DEFAULT 300000
    CHECK (usage_journal_max_file_age_ms >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_max_files INTEGER NOT NULL DEFAULT 128
    CHECK (usage_journal_max_files >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_block_target_uncompressed_bytes INTEGER NOT NULL DEFAULT 1048576
    CHECK (usage_journal_block_target_uncompressed_bytes >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_block_max_events INTEGER NOT NULL DEFAULT 1024
    CHECK (usage_journal_block_max_events >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_fsync_interval_ms INTEGER NOT NULL DEFAULT 250
    CHECK (usage_journal_fsync_interval_ms >= 0);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_zstd_level INTEGER NOT NULL DEFAULT 3
    CHECK (usage_journal_zstd_level >= 0);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_consumer_lease_ms INTEGER NOT NULL DEFAULT 300000
    CHECK (usage_journal_consumer_lease_ms >= 1);

ALTER TABLE llm_runtime_config
ADD COLUMN usage_journal_delete_bad_files INTEGER NOT NULL DEFAULT 0
    CHECK (usage_journal_delete_bad_files IN (0, 1));

ALTER TABLE llm_runtime_config
ADD COLUMN usage_query_bind_addr TEXT NOT NULL DEFAULT '127.0.0.1:19081';

ALTER TABLE llm_runtime_config
ADD COLUMN usage_query_base_url TEXT NOT NULL DEFAULT 'http://127.0.0.1:19081';
