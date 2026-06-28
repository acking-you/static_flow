ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS same_account_retry_count BIGINT DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS same_account_retry_delay_ms BIGINT DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS same_account_retry_reasons_json VARCHAR DEFAULT '[]';
