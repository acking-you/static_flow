//! Model-identity handling: identity probes, Claude Code identity override,
//! and stripping volatile Claude Code billing headers from system text.

use super::{
    ResponseIdentityKind, ResponseIdentityLanguage, ResponseIdentityPlatform,
    ResponseModelIdentity, CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE,
    CLAUDE_CODE_BILLING_HEADER_PREFIX, CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE,
    GENERIC_ANTHROPIC_IDENTITY_OVERRIDE,
};
use crate::anthropic::types::MessagesRequest;

const MODEL_IDENTITY_PREFIX: &str = "You are powered by the model named ";
const MODEL_IDENTITY_DELIMITER: &str = ". The exact model ID is ";
const CLAUDE_CODE_MEMORY_PATH_MARKER: &str = "/.claude/projects/";

fn requested_model_identity_id(model: &str) -> &str {
    model.strip_suffix("-thinking").unwrap_or(model)
}

fn requested_model_identity_name(model: &str) -> Option<&'static str> {
    match requested_model_identity_id(model) {
        "claude-opus-4-8" => Some("Opus 4.8"),
        "claude-opus-4-7" => Some("Opus 4.7"),
        "claude-opus-4-6" => Some("Opus 4.6"),
        "claude-sonnet-4-6" => Some("Sonnet 4.6"),
        "claude-sonnet-4-5" => Some("Sonnet 4.5"),
        "claude-haiku-4-5" => Some("Haiku 4.5"),
        _ => None,
    }
}

fn build_response_identity(
    model_short_name: impl Into<String>,
    model_id: impl Into<String>,
) -> ResponseModelIdentity {
    let model_short_name = model_short_name.into();
    let model_name = if model_short_name.starts_with("Claude ") {
        model_short_name.clone()
    } else {
        format!("Claude {model_short_name}")
    };
    ResponseModelIdentity {
        model_name,
        model_short_name: model_short_name
            .strip_prefix("Claude ")
            .unwrap_or(&model_short_name)
            .to_string(),
        model_id: model_id.into(),
        kind: ResponseIdentityKind::ModelOnly,
        platform: ResponseIdentityPlatform::ClaudeCode,
        thinking_language: ResponseIdentityLanguage::Chinese,
        repo_name_hint: None,
    }
}

fn requested_response_identity(model: &str) -> Option<ResponseModelIdentity> {
    let model_name = requested_model_identity_name(model)?;
    Some(build_response_identity(model_name, requested_model_identity_id(model)))
}

fn cleaned_request_system_text(req: &MessagesRequest) -> Option<String> {
    req.system
        .as_ref()
        .map(|system| {
            system
                .iter()
                .map(|message| strip_volatile_claude_code_billing_header(message.text.clone()))
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|content| !content.is_empty())
}

fn prompt_identity_platform(system_text: &str) -> Option<ResponseIdentityPlatform> {
    system_text
        .lines()
        .find_map(|line| match line.trim_start() {
            CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE => Some(ResponseIdentityPlatform::ClaudeCode),
            CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE => Some(ResponseIdentityPlatform::ClaudeAgentSdk),
            _ => None,
        })
}

fn parse_prompt_model_identity(system_text: &str) -> Option<ResponseModelIdentity> {
    let line = system_text.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .contains(MODEL_IDENTITY_PREFIX)
            .then_some(trimmed)
            .filter(|candidate| candidate.contains(MODEL_IDENTITY_DELIMITER))
    })?;
    let prefix_start = line.find(MODEL_IDENTITY_PREFIX)? + MODEL_IDENTITY_PREFIX.len();
    let remainder = &line[prefix_start..];
    let delimiter_index = remainder.find(MODEL_IDENTITY_DELIMITER)?;
    let short_name = remainder[..delimiter_index].trim();
    let model_id = remainder[delimiter_index + MODEL_IDENTITY_DELIMITER.len()..]
        .trim()
        .trim_end_matches('.');
    if model_id.is_empty() || short_name.is_empty() {
        return None;
    }
    let normalized_short_name = requested_model_identity_name(model_id).unwrap_or(short_name);
    Some(build_response_identity(normalized_short_name, model_id))
}

fn primary_workdir_basename(system_text: &str) -> Option<&str> {
    let line = system_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("- Primary working directory: "))?;
    line.rsplit('/').find(|segment| !segment.is_empty())
}

fn extract_repo_name_hint(system_text: &str) -> Option<String> {
    let slug_start = system_text.find(CLAUDE_CODE_MEMORY_PATH_MARKER)?;
    let slug_remainder = &system_text[slug_start + CLAUDE_CODE_MEMORY_PATH_MARKER.len()..];
    let slug_end = slug_remainder.find("/memory/")?;
    let slug = slug_remainder[..slug_end].trim_matches('`');
    let workdir_basename = primary_workdir_basename(system_text)?;
    let marker = format!("{workdir_basename}-");
    let suffix = slug
        .rfind(&marker)
        .map(|index| &slug[index + marker.len()..])?;
    (!suffix.is_empty()).then_some(suffix.to_string())
}

fn is_multi_identity_probe_zh(content: &str) -> bool {
    content.contains("多重身份")
        && (content.contains("你是谁")
            || content.contains("不要隐瞒")
            || content.contains("不要骗我"))
}

fn is_multi_identity_probe_en(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("multiple identities")
        && (lower.contains("who are you")
            || lower.contains("do not hide anything")
            || lower.contains("don't hide anything"))
}

fn is_conflict_probe_zh(content: &str) -> bool {
    let lower = content.to_lowercase();
    let mentions_products = mentions_conflict_product(&lower);
    lower.contains("身份冲突")
        || lower.contains("包含你的thinking")
        || (lower.contains("多重身份") && lower.contains("thinking"))
        || (mentions_products && (lower.contains("那个平台") || lower.contains("平台中")))
}

fn is_conflict_probe_en(content: &str) -> bool {
    let lower = content.to_lowercase();
    let mentions_products = mentions_conflict_product(&lower);
    lower.contains("identity conflict")
        || lower.contains("include your thinking")
        || (lower.contains("multiple identities") && lower.contains("thinking"))
        || (mentions_products && lower.contains("platform"))
}

fn mentions_conflict_product(lower_content: &str) -> bool {
    ["kiro", "warp", "windsurf", "0z", "sn", "antigravity"]
        .iter()
        .any(|name| contains_ascii_token(lower_content, name))
}

fn contains_ascii_token(content: &str, token: &str) -> bool {
    content
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part == token)
}

fn is_model_identity_probe(content: &str) -> bool {
    let lower = content.to_lowercase();
    let compact = lower
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_' && *ch != '`')
        .collect::<String>();
    let asks_identity = lower.contains("who are you")
        || lower.contains("what are you")
        || lower.contains("your identity")
        || lower.contains("are you claude")
        || lower.contains("are you kiro")
        || content.contains("你是谁")
        || content.contains("你是什么")
        || content.contains("你的身份")
        || content.contains("你是Claude")
        || content.contains("你是 Claude")
        || content.contains("你是Kiro")
        || content.contains("你是 Kiro");
    let asks_model_identity = lower.contains("what model are you")
        || lower.contains("which model are you")
        || lower.contains("your model")
        || (compact.contains("modelid") && (lower.contains("you") || lower.contains("your")))
        || content.contains("你的模型")
        || content.contains("你是什么模型")
        || content.contains("你是哪种模型")
        || ((content.contains("模型ID") || content.contains("模型 ID"))
            && (content.contains("你") || content.contains("你的")));

    asks_identity || asks_model_identity
}

fn model_identity_probe_language(content: &str) -> ResponseIdentityLanguage {
    if content
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
    {
        ResponseIdentityLanguage::Chinese
    } else {
        ResponseIdentityLanguage::English
    }
}

pub fn effective_response_identity_for_request(
    req: &MessagesRequest,
) -> Option<ResponseModelIdentity> {
    let system_text = cleaned_request_system_text(req);
    let prompt_platform = system_text.as_deref().and_then(prompt_identity_platform);
    let mut identity = match system_text.as_deref().and_then(|text| {
        prompt_platform.and_then(|platform| {
            parse_prompt_model_identity(text).map(|mut identity| {
                identity.platform = platform;
                identity
            })
        })
    }) {
        Some(identity) => identity,
        None => requested_response_identity(&req.model)?,
    };
    if let Some(platform) = prompt_platform {
        identity.platform = platform;
    }
    identity.repo_name_hint = system_text
        .as_deref()
        .and_then(extract_repo_name_hint)
        .or(identity.repo_name_hint);
    Some(identity)
}

pub fn response_identity_for_current_turn(
    req: &MessagesRequest,
    current_content: &str,
) -> Option<ResponseModelIdentity> {
    let system_text = cleaned_request_system_text(req);
    let has_claude_identity_prompt = system_text
        .as_deref()
        .and_then(prompt_identity_platform)
        .is_some();
    let kind = if has_claude_identity_prompt && is_conflict_probe_zh(current_content) {
        Some(ResponseIdentityKind::ConflictJsonZh)
    } else if has_claude_identity_prompt && is_conflict_probe_en(current_content) {
        Some(ResponseIdentityKind::ConflictJsonEn)
    } else if has_claude_identity_prompt && is_multi_identity_probe_zh(current_content) {
        Some(ResponseIdentityKind::MultiIdentityZh)
    } else if has_claude_identity_prompt && is_multi_identity_probe_en(current_content) {
        Some(ResponseIdentityKind::MultiIdentityEn)
    } else if is_model_identity_probe(current_content) {
        Some(ResponseIdentityKind::ModelOnly)
    } else {
        None
    }?;

    let mut identity = effective_response_identity_for_request(req)?;
    identity.kind = kind;
    if kind == ResponseIdentityKind::ModelOnly {
        identity.thinking_language = model_identity_probe_language(current_content);
    }
    Some(identity)
}

pub fn anthropic_identity_override(identity: Option<&ResponseModelIdentity>) -> String {
    let Some(identity) = identity else {
        return GENERIC_ANTHROPIC_IDENTITY_OVERRIDE.to_string();
    };
    format!(
        "<identity_override>\nYou are Claude, made by Anthropic. For this request, your model \
         name is {model_name} and your public API model ID is {model_id}. When asked about your \
         identity, model name, or model ID, answer with this Claude identity. Never claim to be \
         Kiro, Warp, or any other product. You are Claude, running on the Anthropic API \
         platform.\n</identity_override>",
        model_name = identity.model_name,
        model_id = identity.model_id
    )
}

pub fn normalize_claude_code_model_identity(
    content: String,
    identity: Option<&ResponseModelIdentity>,
) -> String {
    let Some(identity) = identity else {
        return content;
    };
    let replacement = format!(
        "You are powered by the model named {}. The exact model ID is {}.",
        identity.model_short_name, identity.model_id
    );
    let has_existing_model_identity = content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.contains(MODEL_IDENTITY_PREFIX) && trimmed.contains(MODEL_IDENTITY_DELIMITER)
    });
    let mut replaced_existing = false;
    let mut inserted_after_identity = false;
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.contains(MODEL_IDENTITY_PREFIX) && trimmed.contains(MODEL_IDENTITY_DELIMITER)
            {
                replaced_existing = true;
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}{replacement}")
            } else if !has_existing_model_identity
                && !replaced_existing
                && !inserted_after_identity
                && (trimmed == CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE
                    || trimmed == CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE)
            {
                inserted_after_identity = true;
                format!("{line}\n{replacement}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn strip_volatile_claude_code_billing_header(content: String) -> String {
    content
        .lines()
        .filter(|line| !is_claude_code_billing_header_text(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_claude_code_billing_header_text(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with(CLAUDE_CODE_BILLING_HEADER_PREFIX)
        && (trimmed.contains("cc_version=")
            || trimmed.contains("cc_entrypoint=")
            || trimmed.contains("cch="))
}
