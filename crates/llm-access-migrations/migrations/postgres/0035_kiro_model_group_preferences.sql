ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_model_group_preferences_json JSONB NOT NULL DEFAULT '{}'::jsonb;

UPDATE llm_key_route_config
SET kiro_model_group_preferences_json = '{}'::jsonb
WHERE kiro_model_group_preferences_json IS NULL
   OR jsonb_typeof(kiro_model_group_preferences_json) <> 'object';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'ck_llm_key_route_config_kiro_model_group_preferences_object'
          AND conrelid = 'llm_key_route_config'::regclass
    ) THEN
        ALTER TABLE llm_key_route_config
            ADD CONSTRAINT ck_llm_key_route_config_kiro_model_group_preferences_object
            CHECK (jsonb_typeof(kiro_model_group_preferences_json) = 'object');
    END IF;
END $$;
