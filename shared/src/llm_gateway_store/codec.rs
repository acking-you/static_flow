use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    builder::{
        BooleanBuilder, Int32Builder, StringBuilder, TimestampMillisecondBuilder, UInt64Builder,
    },
    Array, ArrayRef, BooleanArray, Int32Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt64Array,
};

use super::{
    schema::{
        llm_gateway_keys_schema, llm_gateway_runtime_config_schema, llm_gateway_usage_events_schema,
    },
    types::{LlmGatewayKeyRecord, LlmGatewayRuntimeConfigRecord, LlmGatewayUsageEventRecord},
};

pub fn build_keys_batch(records: &[LlmGatewayKeyRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_keys_schema();
    let mut id = StringBuilder::new();
    let mut name = StringBuilder::new();
    let mut secret = StringBuilder::new();
    let mut key_hash = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut public_visible = BooleanBuilder::new();
    let mut quota_billable_limit = UInt64Builder::new();
    let mut usage_input_uncached_tokens = UInt64Builder::new();
    let mut usage_input_cached_tokens = UInt64Builder::new();
    let mut usage_output_tokens = UInt64Builder::new();
    let mut usage_billable_tokens = UInt64Builder::new();
    let mut last_used_at = TimestampMillisecondBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        name.append_value(&record.name);
        secret.append_value(&record.secret);
        key_hash.append_value(&record.key_hash);
        status.append_value(&record.status);
        public_visible.append_value(record.public_visible);
        quota_billable_limit.append_value(record.quota_billable_limit);
        usage_input_uncached_tokens.append_value(record.usage_input_uncached_tokens);
        usage_input_cached_tokens.append_value(record.usage_input_cached_tokens);
        usage_output_tokens.append_value(record.usage_output_tokens);
        usage_billable_tokens.append_value(record.usage_billable_tokens);
        append_optional_ts(&mut last_used_at, record.last_used_at);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(name.finish()),
        Arc::new(secret.finish()),
        Arc::new(key_hash.finish()),
        Arc::new(status.finish()),
        Arc::new(public_visible.finish()),
        Arc::new(quota_billable_limit.finish()),
        Arc::new(usage_input_uncached_tokens.finish()),
        Arc::new(usage_input_cached_tokens.finish()),
        Arc::new(usage_output_tokens.finish()),
        Arc::new(usage_billable_tokens.finish()),
        Arc::new(last_used_at.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build llm gateway keys batch")
}

pub fn build_usage_events_batch(records: &[LlmGatewayUsageEventRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_usage_events_schema();
    let mut id = StringBuilder::new();
    let mut key_id = StringBuilder::new();
    let mut key_name = StringBuilder::new();
    let mut request_method = StringBuilder::new();
    let mut request_url = StringBuilder::new();
    let mut latency_ms = Int32Builder::new();
    let mut endpoint = StringBuilder::new();
    let mut model = StringBuilder::new();
    let mut status_code = Int32Builder::new();
    let mut input_uncached_tokens = UInt64Builder::new();
    let mut input_cached_tokens = UInt64Builder::new();
    let mut output_tokens = UInt64Builder::new();
    let mut billable_tokens = UInt64Builder::new();
    let mut usage_missing = BooleanBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut request_headers_json = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        key_id.append_value(&record.key_id);
        key_name.append_value(&record.key_name);
        request_method.append_value(&record.request_method);
        request_url.append_value(&record.request_url);
        latency_ms.append_value(record.latency_ms);
        endpoint.append_value(&record.endpoint);
        append_optional_str(&mut model, record.model.as_deref());
        status_code.append_value(record.status_code);
        input_uncached_tokens.append_value(record.input_uncached_tokens);
        input_cached_tokens.append_value(record.input_cached_tokens);
        output_tokens.append_value(record.output_tokens);
        billable_tokens.append_value(record.billable_tokens);
        usage_missing.append_value(record.usage_missing);
        client_ip.append_value(&record.client_ip);
        ip_region.append_value(&record.ip_region);
        request_headers_json.append_value(&record.request_headers_json);
        created_at.append_value(record.created_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(key_id.finish()),
        Arc::new(key_name.finish()),
        Arc::new(request_method.finish()),
        Arc::new(request_url.finish()),
        Arc::new(latency_ms.finish()),
        Arc::new(endpoint.finish()),
        Arc::new(model.finish()),
        Arc::new(status_code.finish()),
        Arc::new(input_uncached_tokens.finish()),
        Arc::new(input_cached_tokens.finish()),
        Arc::new(output_tokens.finish()),
        Arc::new(billable_tokens.finish()),
        Arc::new(usage_missing.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(request_headers_json.finish()),
        Arc::new(created_at.finish()),
    ])
    .context("failed to build llm gateway usage events batch")
}

pub fn build_runtime_config_batch(
    records: &[LlmGatewayRuntimeConfigRecord],
) -> Result<RecordBatch> {
    let schema = llm_gateway_runtime_config_schema();
    let mut id = StringBuilder::new();
    let mut auth_cache_ttl_seconds = UInt64Builder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        auth_cache_ttl_seconds.append_value(record.auth_cache_ttl_seconds);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(auth_cache_ttl_seconds.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build llm gateway runtime config batch")
}

pub fn batches_to_keys(batches: &[RecordBatch]) -> Result<Vec<LlmGatewayKeyRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let name = required_str_col(batch, "name")?;
        let secret = required_str_col(batch, "secret")?;
        let key_hash = required_str_col(batch, "key_hash")?;
        let status = required_str_col(batch, "status")?;
        let public_visible = required_bool_col(batch, "public_visible")?;
        let quota_billable_limit = required_u64_col(batch, "quota_billable_limit")?;
        let usage_input_uncached_tokens = required_u64_col(batch, "usage_input_uncached_tokens")?;
        let usage_input_cached_tokens = required_u64_col(batch, "usage_input_cached_tokens")?;
        let usage_output_tokens = required_u64_col(batch, "usage_output_tokens")?;
        let usage_billable_tokens = batch
            .column_by_name("usage_billable_tokens")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let last_used_at = optional_ts_col(batch, "last_used_at")?;
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;

        for idx in 0..batch.num_rows() {
            let raw_billable_tokens = usage_input_uncached_tokens
                .value(idx)
                .saturating_add(usage_output_tokens.value(idx));
            rows.push(LlmGatewayKeyRecord {
                id: id.value(idx).to_string(),
                name: name.value(idx).to_string(),
                secret: secret.value(idx).to_string(),
                key_hash: key_hash.value(idx).to_string(),
                status: status.value(idx).to_string(),
                public_visible: public_visible.value(idx),
                quota_billable_limit: quota_billable_limit.value(idx),
                usage_input_uncached_tokens: usage_input_uncached_tokens.value(idx),
                usage_input_cached_tokens: usage_input_cached_tokens.value(idx),
                usage_output_tokens: usage_output_tokens.value(idx),
                usage_billable_tokens: usage_billable_tokens
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(raw_billable_tokens),
                last_used_at: value_ts_opt(last_used_at, idx),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_usage_events(batches: &[RecordBatch]) -> Result<Vec<LlmGatewayUsageEventRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let key_id = required_str_col(batch, "key_id")?;
        let key_name = batch
            .column_by_name("key_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_method = batch
            .column_by_name("request_method")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_url = batch
            .column_by_name("request_url")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let latency_ms = batch
            .column_by_name("latency_ms")
            .and_then(|column| column.as_any().downcast_ref::<Int32Array>());
        let endpoint = required_str_col(batch, "endpoint")?;
        let model = optional_str_col(batch, "model")?;
        let status_code = required_i32_col(batch, "status_code")?;
        let input_uncached_tokens = required_u64_col(batch, "input_uncached_tokens")?;
        let input_cached_tokens = required_u64_col(batch, "input_cached_tokens")?;
        let output_tokens = required_u64_col(batch, "output_tokens")?;
        let billable_tokens = required_u64_col(batch, "billable_tokens")?;
        let usage_missing = required_bool_col(batch, "usage_missing")?;
        let client_ip = batch
            .column_by_name("client_ip")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let ip_region = batch
            .column_by_name("ip_region")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_headers_json = batch
            .column_by_name("request_headers_json")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let created_at = required_ts_col(batch, "created_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayUsageEventRecord {
                id: id.value(idx).to_string(),
                key_id: key_id.value(idx).to_string(),
                key_name: key_name
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| key_id.value(idx).to_string()),
                request_method: request_method
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "POST".to_string()),
                request_url: request_url
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| endpoint.value(idx).to_string()),
                latency_ms: latency_ms
                    .and_then(|column| value_i32_opt(column, idx))
                    .unwrap_or_default(),
                endpoint: endpoint.value(idx).to_string(),
                model: value_string_opt(model, idx),
                status_code: status_code.value(idx),
                input_uncached_tokens: input_uncached_tokens.value(idx),
                input_cached_tokens: input_cached_tokens.value(idx),
                output_tokens: output_tokens.value(idx),
                billable_tokens: billable_tokens.value(idx),
                usage_missing: usage_missing.value(idx),
                client_ip: client_ip
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "unknown".to_string()),
                ip_region: ip_region
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "Unknown".to_string()),
                request_headers_json: request_headers_json
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "{}".to_string()),
                created_at: created_at.value(idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_runtime_config(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewayRuntimeConfigRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let auth_cache_ttl_seconds = required_u64_col(batch, "auth_cache_ttl_seconds")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayRuntimeConfigRecord {
                id: id.value(idx).to_string(),
                auth_cache_ttl_seconds: auth_cache_ttl_seconds.value(idx),
                updated_at: updated_at.value(idx),
            });
        }
    }
    Ok(rows)
}

fn required_str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .with_context(|| format!("column `{name}` is not StringArray"))
}

fn optional_str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    required_str_col(batch, name)
}

fn required_bool_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BooleanArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
        .with_context(|| format!("column `{name}` is not BooleanArray"))
}

fn required_u64_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
        .with_context(|| format!("column `{name}` is not UInt64Array"))
}

fn required_i32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .with_context(|| format!("column `{name}` is not Int32Array"))
}

fn required_ts_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<TimestampMillisecondArray>())
        .with_context(|| format!("column `{name}` is not TimestampMillisecondArray"))
}

fn optional_ts_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    required_ts_col(batch, name)
}

fn value_string_opt(array: &StringArray, idx: usize) -> Option<String> {
    if array.is_null(idx) {
        None
    } else {
        let value = array.value(idx).trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}

fn value_ts_opt(array: &TimestampMillisecondArray, idx: usize) -> Option<i64> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn value_u64_opt(array: &UInt64Array, idx: usize) -> Option<u64> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn value_i32_opt(array: &Int32Array, idx: usize) -> Option<i32> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn append_optional_str(builder: &mut StringBuilder, value: Option<&str>) {
    match value {
        Some(value) if !value.trim().is_empty() => builder.append_value(value),
        _ => builder.append_null(),
    }
}

fn append_optional_ts(builder: &mut TimestampMillisecondBuilder, value: Option<i64>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}

fn total_rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(RecordBatch::num_rows).sum()
}
