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
    CAST(count(*) AS BIGINT) AS request_count,
    CAST(COALESCE(sum(greatest(COALESCE(request_body_bytes, 0), 0)), 0) AS BIGINT)
        AS request_bytes,
    CAST(COALESCE(sum(greatest(COALESCE(bytes_streamed, 0), 0)), 0) AS BIGINT)
        AS response_bytes,
    CAST(COALESCE(sum(
        greatest(COALESCE(request_body_bytes, 0), 0)
        + greatest(COALESCE(bytes_streamed, 0), 0)
    ), 0) AS BIGINT) AS total_bytes
FROM usage_events
WHERE NOT EXISTS (SELECT 1 FROM proxy_traffic_rollups_hourly LIMIT 1)
GROUP BY
    bucket_hour,
    provider_type,
    proxy_key,
    proxy_source,
    proxy_config_id,
    proxy_config_name,
    proxy_url
ON CONFLICT (bucket_hour, provider_type, proxy_key) DO UPDATE SET
    request_count = proxy_traffic_rollups_hourly.request_count + excluded.request_count,
    request_bytes = proxy_traffic_rollups_hourly.request_bytes + excluded.request_bytes,
    response_bytes = proxy_traffic_rollups_hourly.response_bytes + excluded.response_bytes,
    total_bytes = proxy_traffic_rollups_hourly.total_bytes + excluded.total_bytes,
    proxy_source = COALESCE(excluded.proxy_source, proxy_traffic_rollups_hourly.proxy_source),
    proxy_config_id = COALESCE(excluded.proxy_config_id, proxy_traffic_rollups_hourly.proxy_config_id),
    proxy_config_name = COALESCE(excluded.proxy_config_name, proxy_traffic_rollups_hourly.proxy_config_name),
    proxy_url = COALESCE(excluded.proxy_url, proxy_traffic_rollups_hourly.proxy_url);
