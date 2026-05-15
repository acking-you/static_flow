ALTER TABLE llm_runtime_config
ADD COLUMN usage_analytics_retention_days INTEGER NOT NULL DEFAULT 7
    CHECK (usage_analytics_retention_days >= 1 AND usage_analytics_retention_days <= 365);
