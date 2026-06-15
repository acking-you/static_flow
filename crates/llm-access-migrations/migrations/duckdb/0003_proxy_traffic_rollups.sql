CREATE TABLE IF NOT EXISTS proxy_traffic_rollups_hourly (
    bucket_hour TIMESTAMP NOT NULL,
    provider_type VARCHAR NOT NULL,
    proxy_key VARCHAR NOT NULL,
    proxy_source VARCHAR,
    proxy_config_id VARCHAR,
    proxy_config_name VARCHAR,
    proxy_url VARCHAR,
    request_count BIGINT NOT NULL,
    request_bytes BIGINT NOT NULL,
    response_bytes BIGINT NOT NULL,
    total_bytes BIGINT NOT NULL,
    PRIMARY KEY (bucket_hour, provider_type, proxy_key)
);

WITH proxy_traffic_events AS (
    SELECT
        created_at_ms,
        date_trunc('hour', to_timestamp(created_at_ms / 1000.0)) AS bucket_hour,
        provider_type,
        CASE
            WHEN proxy_config_id_at_event IS NOT NULL AND length(trim(proxy_config_id_at_event)) > 0
                THEN 'proxy:id:' || trim(proxy_config_id_at_event)
            WHEN proxy_url_at_event IS NOT NULL AND length(trim(proxy_url_at_event)) > 0
                THEN 'proxy:url:' || trim(proxy_url_at_event)
            WHEN proxy_source_at_event IS NOT NULL AND length(trim(proxy_source_at_event)) > 0
                THEN 'proxy:source:' || trim(proxy_source_at_event)
            ELSE 'proxy:unknown'
        END AS proxy_key,
        nullif(trim(proxy_source_at_event), '') AS proxy_source,
        nullif(trim(proxy_config_id_at_event), '') AS proxy_config_id,
        nullif(trim(proxy_config_name_at_event), '') AS proxy_config_name,
        nullif(trim(proxy_url_at_event), '') AS proxy_url,
        greatest(COALESCE(request_body_bytes, 0), 0) AS request_bytes,
        greatest(COALESCE(bytes_streamed, 0), 0) AS response_bytes
    FROM usage_events
    WHERE NOT EXISTS (SELECT 1 FROM proxy_traffic_rollups_hourly LIMIT 1)
)
INSERT INTO proxy_traffic_rollups_hourly (
    bucket_hour,
    provider_type,
    proxy_key,
    proxy_source,
    proxy_config_id,
    proxy_config_name,
    proxy_url,
    request_count,
    request_bytes,
    response_bytes,
    total_bytes
)
SELECT
    bucket_hour,
    provider_type,
    proxy_key,
    arg_max(proxy_source, created_at_ms) AS proxy_source,
    arg_max(proxy_config_id, created_at_ms) AS proxy_config_id,
    arg_max(proxy_config_name, created_at_ms) AS proxy_config_name,
    arg_max(proxy_url, created_at_ms) AS proxy_url,
    CAST(count(*) AS BIGINT) AS request_count,
    CAST(COALESCE(sum(request_bytes), 0) AS BIGINT) AS request_bytes,
    CAST(COALESCE(sum(response_bytes), 0) AS BIGINT) AS response_bytes,
    CAST(COALESCE(sum(request_bytes + response_bytes), 0) AS BIGINT) AS total_bytes
FROM proxy_traffic_events
GROUP BY
    bucket_hour,
    provider_type,
    proxy_key
ON CONFLICT (bucket_hour, provider_type, proxy_key) DO UPDATE SET
    request_count = proxy_traffic_rollups_hourly.request_count + excluded.request_count,
    request_bytes = proxy_traffic_rollups_hourly.request_bytes + excluded.request_bytes,
    response_bytes = proxy_traffic_rollups_hourly.response_bytes + excluded.response_bytes,
    total_bytes = proxy_traffic_rollups_hourly.total_bytes + excluded.total_bytes,
    proxy_source = COALESCE(excluded.proxy_source, proxy_traffic_rollups_hourly.proxy_source),
    proxy_config_id = COALESCE(excluded.proxy_config_id, proxy_traffic_rollups_hourly.proxy_config_id),
    proxy_config_name = COALESCE(excluded.proxy_config_name, proxy_traffic_rollups_hourly.proxy_config_name),
    proxy_url = COALESCE(excluded.proxy_url, proxy_traffic_rollups_hourly.proxy_url);
