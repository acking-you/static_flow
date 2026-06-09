ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS preferred_pool_strategy TEXT NOT NULL DEFAULT 'balanced';

UPDATE llm_key_route_config
SET preferred_pool_strategy = 'balanced'
WHERE preferred_pool_strategy IS NULL
   OR BTRIM(preferred_pool_strategy) = '';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'llm_key_route_config_preferred_pool_strategy_check'
    ) THEN
        ALTER TABLE llm_key_route_config
            ADD CONSTRAINT llm_key_route_config_preferred_pool_strategy_check
            CHECK (preferred_pool_strategy IN ('balanced', 'credit_first'));
    END IF;
END $$;
