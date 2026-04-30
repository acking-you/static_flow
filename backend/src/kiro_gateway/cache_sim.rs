//! Re-exported Kiro cache simulator from the standalone LLM access runtime.

use std::time::Duration;

pub(crate) use llm_access_kiro::cache_sim::*;

use crate::state::LlmGatewayRuntimeConfig;

impl From<&LlmGatewayRuntimeConfig> for KiroCacheSimulationConfig {
    fn from(value: &LlmGatewayRuntimeConfig) -> Self {
        Self {
            mode: KiroCacheSimulationMode::from_runtime_value(&value.kiro_prefix_cache_mode),
            prefix_cache_max_tokens: value.kiro_prefix_cache_max_tokens,
            prefix_cache_entry_ttl: Duration::from_secs(value.kiro_prefix_cache_entry_ttl_seconds),
            conversation_anchor_max_entries: usize::try_from(
                value.kiro_conversation_anchor_max_entries,
            )
            .unwrap_or(usize::MAX),
            conversation_anchor_ttl: Duration::from_secs(
                value.kiro_conversation_anchor_ttl_seconds,
            ),
        }
    }
}
