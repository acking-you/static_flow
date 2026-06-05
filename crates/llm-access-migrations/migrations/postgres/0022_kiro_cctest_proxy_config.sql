ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cctest_proxy_base_url TEXT;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cctest_proxy_api_key TEXT;
