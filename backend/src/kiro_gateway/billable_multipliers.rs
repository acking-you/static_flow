//! Re-exported Kiro billable multiplier helpers from the standalone LLM access
//! runtime.

use std::collections::BTreeMap;

use anyhow::Result;
pub(crate) use llm_access_kiro::billable_multipliers::canonicalize_kiro_billable_model_multipliers_override_json;
use static_flow_shared::llm_gateway_store::LlmGatewayKeyRecord;

use crate::state::LlmGatewayRuntimeConfig;

pub(crate) fn resolve_effective_kiro_billable_model_multipliers(
    runtime: &LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
) -> Result<BTreeMap<String, f64>> {
    llm_access_kiro::billable_multipliers::resolve_effective_kiro_billable_model_multipliers(
        &runtime.kiro_billable_model_multipliers,
        key.kiro_billable_model_multipliers_override_json.as_deref(),
    )
}

pub(crate) fn uses_global_kiro_billable_model_multipliers(key: &LlmGatewayKeyRecord) -> bool {
    llm_access_kiro::billable_multipliers::uses_global_kiro_billable_model_multipliers(
        key.kiro_billable_model_multipliers_override_json.as_deref(),
    )
}
