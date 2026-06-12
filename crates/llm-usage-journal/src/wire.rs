//! Versioned usage journal wire records.

use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{KeyUsageRollupDelta, KeyUsageRollupLastUsedCount, UsageRollupBatch},
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};
use serde::{Deserialize, Serialize};

/// Current journal file magic bytes.
pub const FILE_MAGIC_V1: &[u8; 8] = b"LLMUJNL1";

/// Current control-rollup journal file magic bytes.
pub const ROLLUP_FILE_MAGIC_V1: &[u8; 8] = b"LLMRJNL1";

/// Current journal file format version.
pub const FORMAT_VERSION_V1: u16 = 1;

/// Current usage event schema version.
pub const SCHEMA_VERSION_V1: u16 = 1;

/// File header written once at the beginning of each journal file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeaderV1 {
    /// Static magic bytes used to identify usage journal files.
    pub magic: [u8; 8],
    /// Journal container format version.
    pub format_version: u16,
    /// Usage event schema version.
    pub schema_version: u16,
    /// Monotonic file sequence assigned by the writer.
    pub file_sequence: u64,
    /// File creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Stable writer id for diagnostics.
    pub writer_id: String,
    /// Compression algorithm name.
    pub compression: String,
}

/// Header for one compressed batch block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeaderV1 {
    /// Monotonic block sequence inside the file.
    pub block_sequence: u64,
    /// Number of usage events inside the block.
    pub event_count: u32,
    /// Minimum event timestamp in the block.
    pub min_created_at_ms: i64,
    /// Maximum event timestamp in the block.
    pub max_created_at_ms: i64,
    /// Uncompressed payload byte length.
    pub uncompressed_len: u64,
    /// Compressed payload byte length.
    pub compressed_len: u64,
    /// CRC32C over header-without-crc plus compressed payload.
    pub crc32c: u32,
}

/// File footer written only when a journal file is sealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFooterV1 {
    /// Monotonic file sequence assigned by the writer.
    pub file_sequence: u64,
    /// File creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// File seal timestamp in Unix milliseconds.
    pub sealed_at_ms: i64,
    /// Total event count in this file.
    pub event_count: u64,
    /// Total block count in this file.
    pub block_count: u64,
    /// Minimum event timestamp in this file.
    pub min_created_at_ms: Option<i64>,
    /// Maximum event timestamp in this file.
    pub max_created_at_ms: Option<i64>,
    /// Total uncompressed payload bytes.
    pub uncompressed_bytes: u64,
    /// Total compressed payload bytes.
    pub compressed_bytes: u64,
}

/// One versioned usage event record stored in the journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalUsageEventV1 {
    /// Usage event schema version.
    pub schema_version: u16,
    /// Stable event id.
    pub event_id: String,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Provider type.
    pub provider_type: ProviderType,
    /// Protocol family.
    pub protocol_family: ProtocolFamily,
    /// Key id at event time.
    pub key_id: String,
    /// Key name at event time.
    pub key_name: String,
    /// Account name used by the upstream request.
    pub account_name: Option<String>,
    /// Account group id captured at event time.
    pub account_group_id_at_event: Option<String>,
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<RouteStrategy>,
    /// Incoming HTTP method.
    pub request_method: String,
    /// Operator-facing request URL.
    pub request_url: String,
    /// Client-facing endpoint.
    pub endpoint: String,
    /// Client-facing model.
    pub model: Option<String>,
    /// Upstream mapped model.
    pub mapped_model: Option<String>,
    /// Final HTTP status code.
    pub status_code: i64,
    /// Request body size in bytes.
    pub request_body_bytes: Option<i64>,
    /// Number of upstream route failovers.
    pub quota_failover_count: u64,
    /// Provider routing diagnostics JSON.
    pub routing_diagnostics_json: Option<String>,
    /// Uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Cached input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Billable tokens.
    pub billable_tokens: i64,
    /// Credit usage when known.
    pub credit_usage: Option<String>,
    /// Whether normal token usage was unavailable.
    pub usage_missing: bool,
    /// Whether credit usage was unavailable.
    pub credit_usage_missing: bool,
    /// Client IP captured from proxy headers.
    pub client_ip: String,
    /// Best-effort region label for the client IP.
    pub ip_region: String,
    /// JSON snapshot of request headers.
    pub request_headers_json: String,
    /// Last user message content when cheaply extractable.
    pub last_message_content: Option<String>,
    /// Original client request body for diagnostic events.
    pub client_request_body_json: Option<String>,
    /// Upstream request body for diagnostic events.
    pub upstream_request_body_json: Option<String>,
    /// Canonical full request body for diagnostic events.
    pub full_request_json: Option<String>,
    /// Best-effort error message surfaced for failed requests.
    #[serde(default)]
    pub error_message: Option<String>,
    /// Raw error response body surfaced for failed requests.
    #[serde(default)]
    pub error_body: Option<String>,
    /// Raw response body captured for explicit diagnostic events.
    #[serde(default)]
    pub response_body: Option<String>,
    /// Provider timing fields.
    pub timing: UsageTiming,
    /// Downstream stream outcome fields.
    pub stream: UsageStreamDetails,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LegacyJournalUsageEventV1 {
    schema_version: u16,
    event_id: String,
    created_at_ms: i64,
    provider_type: ProviderType,
    protocol_family: ProtocolFamily,
    key_id: String,
    key_name: String,
    account_name: Option<String>,
    account_group_id_at_event: Option<String>,
    route_strategy_at_event: Option<RouteStrategy>,
    request_method: String,
    request_url: String,
    endpoint: String,
    model: Option<String>,
    mapped_model: Option<String>,
    status_code: i64,
    request_body_bytes: Option<i64>,
    quota_failover_count: u64,
    routing_diagnostics_json: Option<String>,
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    credit_usage: Option<String>,
    usage_missing: bool,
    credit_usage_missing: bool,
    client_ip: String,
    ip_region: String,
    request_headers_json: String,
    last_message_content: Option<String>,
    client_request_body_json: Option<String>,
    upstream_request_body_json: Option<String>,
    full_request_json: Option<String>,
    timing: UsageTiming,
    stream: UsageStreamDetails,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LegacyJournalUsageBatchV1 {
    events: Vec<LegacyJournalUsageEventV1>,
}

impl JournalUsageEventV1 {
    /// Convert a runtime usage event into the stable journal wire shape.
    pub fn from_usage_event(event: &UsageEvent) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V1,
            event_id: event.event_id.clone(),
            created_at_ms: event.created_at_ms,
            provider_type: event.provider_type,
            protocol_family: event.protocol_family,
            key_id: event.key_id.clone(),
            key_name: event.key_name.clone(),
            account_name: event.account_name.clone(),
            account_group_id_at_event: event.account_group_id_at_event.clone(),
            route_strategy_at_event: event.route_strategy_at_event,
            request_method: event.request_method.clone(),
            request_url: event.request_url.clone(),
            endpoint: event.endpoint.clone(),
            model: event.model.clone(),
            mapped_model: event.mapped_model.clone(),
            status_code: event.status_code,
            request_body_bytes: event.request_body_bytes,
            quota_failover_count: event.quota_failover_count,
            routing_diagnostics_json: event.routing_diagnostics_json.clone(),
            input_uncached_tokens: event.input_uncached_tokens,
            input_cached_tokens: event.input_cached_tokens,
            output_tokens: event.output_tokens,
            billable_tokens: event.billable_tokens,
            credit_usage: event.credit_usage.clone(),
            usage_missing: event.usage_missing,
            credit_usage_missing: event.credit_usage_missing,
            client_ip: event.client_ip.clone(),
            ip_region: event.ip_region.clone(),
            request_headers_json: event.request_headers_json.clone(),
            last_message_content: event.last_message_content.clone(),
            client_request_body_json: event.client_request_body_json.clone(),
            upstream_request_body_json: event.upstream_request_body_json.clone(),
            full_request_json: event.full_request_json.clone(),
            error_message: event.error_message.clone(),
            error_body: event.error_body.clone(),
            response_body: event.response_body.clone(),
            timing: event.timing.clone(),
            stream: event.stream.clone(),
        }
    }

    /// Convert the journal wire shape back into a runtime usage event.
    pub fn into_usage_event(self) -> UsageEvent {
        UsageEvent {
            event_id: self.event_id,
            created_at_ms: self.created_at_ms,
            provider_type: self.provider_type,
            protocol_family: self.protocol_family,
            key_id: self.key_id,
            key_name: self.key_name,
            account_name: self.account_name,
            account_group_id_at_event: self.account_group_id_at_event,
            route_strategy_at_event: self.route_strategy_at_event,
            request_method: self.request_method,
            request_url: self.request_url,
            endpoint: self.endpoint,
            model: self.model,
            mapped_model: self.mapped_model,
            status_code: self.status_code,
            request_body_bytes: self.request_body_bytes,
            quota_failover_count: self.quota_failover_count,
            routing_diagnostics_json: self.routing_diagnostics_json,
            input_uncached_tokens: self.input_uncached_tokens,
            input_cached_tokens: self.input_cached_tokens,
            output_tokens: self.output_tokens,
            billable_tokens: self.billable_tokens,
            credit_usage: self.credit_usage,
            usage_missing: self.usage_missing,
            credit_usage_missing: self.credit_usage_missing,
            client_ip: self.client_ip,
            ip_region: self.ip_region,
            request_headers_json: self.request_headers_json,
            last_message_content: self.last_message_content,
            client_request_body_json: self.client_request_body_json,
            upstream_request_body_json: self.upstream_request_body_json,
            full_request_json: self.full_request_json,
            error_message: self.error_message,
            error_body: self.error_body,
            response_body: self.response_body,
            timing: self.timing,
            stream: self.stream,
        }
    }
}

impl LegacyJournalUsageEventV1 {
    fn into_current(self) -> JournalUsageEventV1 {
        JournalUsageEventV1 {
            schema_version: self.schema_version,
            event_id: self.event_id,
            created_at_ms: self.created_at_ms,
            provider_type: self.provider_type,
            protocol_family: self.protocol_family,
            key_id: self.key_id,
            key_name: self.key_name,
            account_name: self.account_name,
            account_group_id_at_event: self.account_group_id_at_event,
            route_strategy_at_event: self.route_strategy_at_event,
            request_method: self.request_method,
            request_url: self.request_url,
            endpoint: self.endpoint,
            model: self.model,
            mapped_model: self.mapped_model,
            status_code: self.status_code,
            request_body_bytes: self.request_body_bytes,
            quota_failover_count: self.quota_failover_count,
            routing_diagnostics_json: self.routing_diagnostics_json,
            input_uncached_tokens: self.input_uncached_tokens,
            input_cached_tokens: self.input_cached_tokens,
            output_tokens: self.output_tokens,
            billable_tokens: self.billable_tokens,
            credit_usage: self.credit_usage,
            usage_missing: self.usage_missing,
            credit_usage_missing: self.credit_usage_missing,
            client_ip: self.client_ip,
            ip_region: self.ip_region,
            request_headers_json: self.request_headers_json,
            last_message_content: self.last_message_content,
            client_request_body_json: self.client_request_body_json,
            upstream_request_body_json: self.upstream_request_body_json,
            full_request_json: self.full_request_json,
            error_message: None,
            error_body: None,
            response_body: None,
            timing: self.timing,
            stream: self.stream,
        }
    }
}

/// One compressed batch payload before compression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalUsageBatchV1 {
    /// Usage events in append order.
    pub events: Vec<JournalUsageEventV1>,
}

/// One versioned rollup delta record stored in the control-rollup journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalRollupDeltaV1 {
    /// Rollup delta schema version.
    pub schema_version: u16,
    /// Key receiving this rollup delta.
    pub key_id: String,
    /// Uncached input tokens to add.
    pub input_uncached_tokens: i64,
    /// Cached input tokens to add.
    pub input_cached_tokens: i64,
    /// Output tokens to add.
    pub output_tokens: i64,
    /// Billable tokens to add.
    pub billable_tokens: i64,
    /// Credit usage to add.
    pub credit_total: f64,
    /// Count of events whose credit usage was missing.
    pub credit_missing_events: i64,
    /// Latest usage timestamp represented by this delta.
    pub last_used_at_ms: Option<i64>,
}

/// One versioned last-used timestamp cardinality entry for rollup overlays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRollupLastUsedCountV1 {
    /// Rollup timestamp-count schema version.
    pub schema_version: u16,
    /// Key receiving this timestamp contribution.
    pub key_id: String,
    /// Usage timestamp in Unix milliseconds.
    pub last_used_at_ms: i64,
    /// Number of raw events for this key at this timestamp.
    pub count: u64,
}

/// One versioned rollup batch stored in the control-rollup journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalRollupBatchV1 {
    /// Rollup batch schema version.
    pub schema_version: u16,
    /// Stable id used by control stores for replay deduplication.
    pub batch_id: String,
    /// Optional source node id for diagnostics.
    pub source_node_id: Option<String>,
    /// Batch creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Number of raw usage events represented by this aggregated batch.
    pub source_event_count: u64,
    /// Per-key rollup deltas.
    pub deltas: Vec<JournalRollupDeltaV1>,
    /// Per-key timestamp cardinalities for exact in-memory overlay recovery.
    #[serde(default)]
    pub last_used_at_ms_counts: Vec<JournalRollupLastUsedCountV1>,
}

/// One compressed rollup block payload before compression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalRollupBatchBlockV1 {
    /// Rollup batches in append order.
    pub batches: Vec<JournalRollupBatchV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LegacyJournalRollupBatchV1 {
    schema_version: u16,
    batch_id: String,
    source_node_id: Option<String>,
    created_at_ms: i64,
    source_event_count: u64,
    deltas: Vec<JournalRollupDeltaV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LegacyJournalRollupBatchBlockV1 {
    batches: Vec<LegacyJournalRollupBatchV1>,
}

impl JournalRollupDeltaV1 {
    /// Convert a core rollup delta into the stable journal wire shape.
    pub fn from_rollup_delta(delta: &KeyUsageRollupDelta) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V1,
            key_id: delta.key_id.clone(),
            input_uncached_tokens: delta.input_uncached_tokens,
            input_cached_tokens: delta.input_cached_tokens,
            output_tokens: delta.output_tokens,
            billable_tokens: delta.billable_tokens,
            credit_total: delta.credit_total,
            credit_missing_events: delta.credit_missing_events,
            last_used_at_ms: delta.last_used_at_ms,
        }
    }

    /// Convert the journal wire shape back into a core rollup delta.
    pub fn into_rollup_delta(self) -> KeyUsageRollupDelta {
        KeyUsageRollupDelta {
            key_id: self.key_id,
            input_uncached_tokens: self.input_uncached_tokens,
            input_cached_tokens: self.input_cached_tokens,
            output_tokens: self.output_tokens,
            billable_tokens: self.billable_tokens,
            credit_total: self.credit_total,
            credit_missing_events: self.credit_missing_events,
            last_used_at_ms: self.last_used_at_ms,
        }
    }
}

impl JournalRollupLastUsedCountV1 {
    /// Convert a core timestamp-count entry into the stable journal wire shape.
    pub fn from_rollup_last_used_count(count: &KeyUsageRollupLastUsedCount) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V1,
            key_id: count.key_id.clone(),
            last_used_at_ms: count.last_used_at_ms,
            count: count.count,
        }
    }

    /// Convert the journal wire shape back into a core timestamp-count entry.
    pub fn into_rollup_last_used_count(self) -> KeyUsageRollupLastUsedCount {
        KeyUsageRollupLastUsedCount {
            key_id: self.key_id,
            last_used_at_ms: self.last_used_at_ms,
            count: self.count,
        }
    }
}

impl JournalRollupBatchV1 {
    /// Convert a core rollup batch into the stable journal wire shape.
    pub fn from_rollup_batch(batch: &UsageRollupBatch) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V1,
            batch_id: batch.batch_id.clone(),
            source_node_id: batch.source_node_id.clone(),
            created_at_ms: batch.created_at_ms,
            source_event_count: batch.source_event_count,
            deltas: batch
                .deltas
                .iter()
                .map(JournalRollupDeltaV1::from_rollup_delta)
                .collect(),
            last_used_at_ms_counts: batch
                .last_used_at_ms_counts
                .iter()
                .map(JournalRollupLastUsedCountV1::from_rollup_last_used_count)
                .collect(),
        }
    }

    /// Convert the journal wire shape back into a core rollup batch.
    pub fn into_rollup_batch(self) -> UsageRollupBatch {
        UsageRollupBatch {
            batch_id: self.batch_id,
            source_node_id: self.source_node_id,
            created_at_ms: self.created_at_ms,
            source_event_count: self.source_event_count,
            deltas: self
                .deltas
                .into_iter()
                .map(JournalRollupDeltaV1::into_rollup_delta)
                .collect(),
            last_used_at_ms_counts: self
                .last_used_at_ms_counts
                .into_iter()
                .map(JournalRollupLastUsedCountV1::into_rollup_last_used_count)
                .collect(),
        }
    }
}

impl LegacyJournalRollupBatchV1 {
    fn into_current(self) -> JournalRollupBatchV1 {
        JournalRollupBatchV1 {
            schema_version: self.schema_version,
            batch_id: self.batch_id,
            source_node_id: self.source_node_id,
            created_at_ms: self.created_at_ms,
            source_event_count: self.source_event_count,
            deltas: self.deltas,
            last_used_at_ms_counts: Vec::new(),
        }
    }
}

/// Encode one rollup batch using the stable versioned V1 wire shape.
pub fn encode_rollup_batch_v1(batch: &UsageRollupBatch) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(&JournalRollupBatchV1::from_rollup_batch(batch))
}

/// Decode one rollup block payload, accepting both current and legacy V1
/// layouts.
pub fn decode_journal_rollup_batch_block(
    bytes: &[u8],
) -> Result<JournalRollupBatchBlockV1, postcard::Error> {
    match postcard::from_bytes::<JournalRollupBatchBlockV1>(bytes) {
        Ok(block) => Ok(block),
        Err(_) => {
            let legacy = postcard::from_bytes::<LegacyJournalRollupBatchBlockV1>(bytes)?;
            Ok(JournalRollupBatchBlockV1 {
                batches: legacy
                    .batches
                    .into_iter()
                    .map(LegacyJournalRollupBatchV1::into_current)
                    .collect(),
            })
        },
    }
}

/// Decode one journal batch payload, accepting both current and legacy event
/// layouts within schema version 1.
pub fn decode_journal_usage_batch(bytes: &[u8]) -> Result<JournalUsageBatchV1, postcard::Error> {
    match postcard::from_bytes::<JournalUsageBatchV1>(bytes) {
        Ok(batch) => Ok(batch),
        Err(_) => {
            let legacy = postcard::from_bytes::<LegacyJournalUsageBatchV1>(bytes)?;
            Ok(JournalUsageBatchV1 {
                events: legacy
                    .events
                    .into_iter()
                    .map(LegacyJournalUsageEventV1::into_current)
                    .collect(),
            })
        },
    }
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        store::{KeyUsageRollupDelta, KeyUsageRollupLastUsedCount, UsageRollupBatch},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };
    use serde::{Deserialize, Serialize};

    use super::{
        decode_journal_rollup_batch_block, decode_journal_usage_batch, encode_rollup_batch_v1,
        JournalRollupBatchV1, JournalUsageBatchV1, JournalUsageEventV1, LegacyJournalUsageBatchV1,
        LegacyJournalUsageEventV1,
    };

    #[test]
    fn usage_event_converts_to_versioned_journal_event() {
        let event = test_usage_event("evt-wire-1");
        let journal = JournalUsageEventV1::from_usage_event(&event);
        assert_eq!(journal.event_id, "evt-wire-1");
        assert_eq!(journal.full_request_json.as_deref(), Some("{\"model\":\"m\"}"));
        assert_eq!(journal.schema_version, 1);
    }

    #[test]
    fn journal_batch_round_trips_through_postcard() {
        let event = JournalUsageEventV1::from_usage_event(&test_usage_event("evt-wire-2"));
        let batch = JournalUsageBatchV1 {
            events: vec![event],
        };
        let bytes = postcard::to_allocvec(&batch).expect("encode batch");
        let decoded: JournalUsageBatchV1 = postcard::from_bytes(&bytes).expect("decode batch");
        assert_eq!(decoded.events[0].event_id, "evt-wire-2");
    }

    #[test]
    fn journal_event_decodes_legacy_payload_without_error_fields() {
        let event = test_usage_event("evt-wire-legacy");
        let legacy = LegacyJournalUsageEventV1 {
            schema_version: 1,
            event_id: event.event_id.clone(),
            created_at_ms: event.created_at_ms,
            provider_type: event.provider_type,
            protocol_family: event.protocol_family,
            key_id: event.key_id.clone(),
            key_name: event.key_name.clone(),
            account_name: event.account_name.clone(),
            account_group_id_at_event: event.account_group_id_at_event.clone(),
            route_strategy_at_event: event.route_strategy_at_event,
            request_method: event.request_method.clone(),
            request_url: event.request_url.clone(),
            endpoint: event.endpoint.clone(),
            model: event.model.clone(),
            mapped_model: event.mapped_model.clone(),
            status_code: event.status_code,
            request_body_bytes: event.request_body_bytes,
            quota_failover_count: event.quota_failover_count,
            routing_diagnostics_json: event.routing_diagnostics_json.clone(),
            input_uncached_tokens: event.input_uncached_tokens,
            input_cached_tokens: event.input_cached_tokens,
            output_tokens: event.output_tokens,
            billable_tokens: event.billable_tokens,
            credit_usage: event.credit_usage.clone(),
            usage_missing: event.usage_missing,
            credit_usage_missing: event.credit_usage_missing,
            client_ip: event.client_ip.clone(),
            ip_region: event.ip_region.clone(),
            request_headers_json: event.request_headers_json.clone(),
            last_message_content: event.last_message_content.clone(),
            client_request_body_json: event.client_request_body_json.clone(),
            upstream_request_body_json: event.upstream_request_body_json.clone(),
            full_request_json: event.full_request_json.clone(),
            timing: event.timing.clone(),
            stream: event.stream.clone(),
        };
        let bytes = postcard::to_allocvec(&LegacyJournalUsageBatchV1 {
            events: vec![legacy],
        })
        .expect("encode legacy event");
        let decoded = decode_journal_usage_batch(&bytes).expect("decode legacy event");
        assert_eq!(decoded.events[0].event_id, event.event_id);
        assert_eq!(decoded.events[0].error_message, None);
        assert_eq!(decoded.events[0].error_body, None);
        assert_eq!(decoded.events[0].stream, event.stream);
    }

    #[test]
    fn rollup_batch_round_trips_last_used_counts_through_v1_wire() {
        let batch = UsageRollupBatch {
            batch_id: "rollup-wire-1".to_string(),
            source_node_id: Some("node-test".to_string()),
            created_at_ms: 1_700_000_000_000,
            source_event_count: 2,
            deltas: vec![KeyUsageRollupDelta {
                key_id: "key-wire".to_string(),
                input_uncached_tokens: 10,
                input_cached_tokens: 1,
                output_tokens: 2,
                billable_tokens: 12,
                credit_total: 0.25,
                credit_missing_events: 0,
                last_used_at_ms: Some(1_700_000_000_200),
            }],
            last_used_at_ms_counts: vec![
                KeyUsageRollupLastUsedCount {
                    key_id: "key-wire".to_string(),
                    last_used_at_ms: 1_700_000_000_100,
                    count: 1,
                },
                KeyUsageRollupLastUsedCount {
                    key_id: "key-wire".to_string(),
                    last_used_at_ms: 1_700_000_000_200,
                    count: 1,
                },
            ],
        };

        let bytes = encode_rollup_batch_v1(&batch).expect("encode rollup batch");
        let decoded: JournalRollupBatchV1 =
            postcard::from_bytes(&bytes).expect("decode rollup batch");

        assert_eq!(decoded.into_rollup_batch(), batch);
    }

    #[test]
    fn rollup_batch_decodes_legacy_v1_payload_without_last_used_counts() {
        #[derive(Debug, Serialize, Deserialize)]
        struct LegacyJournalRollupBatchV1 {
            schema_version: u16,
            batch_id: String,
            source_node_id: Option<String>,
            created_at_ms: i64,
            source_event_count: u64,
            deltas: Vec<super::JournalRollupDeltaV1>,
        }
        #[derive(Debug, Serialize, Deserialize)]
        struct LegacyJournalRollupBatchBlockV1 {
            batches: Vec<LegacyJournalRollupBatchV1>,
        }

        let legacy = LegacyJournalRollupBatchBlockV1 {
            batches: vec![LegacyJournalRollupBatchV1 {
                schema_version: 1,
                batch_id: "rollup-wire-legacy".to_string(),
                source_node_id: Some("node-test".to_string()),
                created_at_ms: 1_700_000_000_000,
                source_event_count: 1,
                deltas: vec![super::JournalRollupDeltaV1 {
                    schema_version: 1,
                    key_id: "key-wire".to_string(),
                    input_uncached_tokens: 10,
                    input_cached_tokens: 1,
                    output_tokens: 2,
                    billable_tokens: 12,
                    credit_total: 0.25,
                    credit_missing_events: 0,
                    last_used_at_ms: Some(1_700_000_000_200),
                }],
            }],
        };
        let bytes = postcard::to_allocvec(&legacy).expect("encode legacy rollup block");
        let decoded =
            decode_journal_rollup_batch_block(&bytes).expect("decode legacy rollup block");

        assert_eq!(decoded.batches[0].batch_id, "rollup-wire-legacy");
        assert!(decoded.batches[0].last_used_at_ms_counts.is_empty());
    }

    fn test_usage_event(event_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-1".to_string(),
            key_name: "for-yangshu".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: Some("group-1".to_string()),
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            mapped_model: Some("claude-opus-4-7".to_string()),
            status_code: 200,
            request_body_bytes: Some(17),
            quota_failover_count: 0,
            routing_diagnostics_json: Some("{\"route\":\"fixed\"}".to_string()),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_usage: Some("0.12".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"user-agent\":\"test\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"model\":\"m\"}".to_string()),
            upstream_request_body_json: Some("{\"upstream\":true}".to_string()),
            full_request_json: Some("{\"model\":\"m\"}".to_string()),
            error_message: None,
            error_body: None,
            response_body: None,
            timing: UsageTiming {
                latency_ms: Some(123),
                routing_wait_ms: Some(1),
                upstream_headers_ms: Some(2),
                post_headers_body_ms: Some(3),
                request_body_read_ms: Some(4),
                request_json_parse_ms: Some(5),
                pre_handler_ms: Some(6),
                first_sse_write_ms: Some(7),
                stream_finish_ms: Some(8),
            },
            stream: UsageStreamDetails {
                stream_completed_cleanly: Some(true),
                downstream_disconnect: Some(false),
                final_event_type: Some("message_stop".to_string()),
                bytes_streamed: Some(100),
            },
        }
    }
}
