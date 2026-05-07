//! Versioned usage journal wire records.

use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};
use serde::{Deserialize, Serialize};

/// Current journal file magic bytes.
pub const FILE_MAGIC_V1: &[u8; 8] = b"LLMUJNL1";

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
    /// Provider timing fields.
    pub timing: UsageTiming,
    /// Downstream stream outcome fields.
    pub stream: UsageStreamDetails,
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
            timing: event.timing.clone(),
            stream: event.stream.clone(),
        }
    }
}

/// One compressed batch payload before compression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalUsageBatchV1 {
    /// Usage events in append order.
    pub events: Vec<JournalUsageEventV1>,
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };

    use super::{JournalUsageBatchV1, JournalUsageEventV1};

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
