//! Model-identity handling: identity probes, Claude Code identity override,
//! and stripping volatile Claude Code billing headers from system text.

use super::{
    ResponseModelIdentity, CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE,
    CLAUDE_CODE_BILLING_HEADER_PREFIX, CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE,
    GENERIC_ANTHROPIC_IDENTITY_OVERRIDE,
};
use crate::anthropic::types::MessagesRequest;

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

fn response_model_identity(model: &str) -> Option<ResponseModelIdentity> {
    let model_name = requested_model_identity_name(model)?;
    Some(ResponseModelIdentity {
        model_name: format!("Claude {model_name}"),
        model_id: requested_model_identity_id(model).to_string(),
    })
}

pub fn response_identity_for_current_turn(
    req: &MessagesRequest,
    current_content: &str,
) -> Option<ResponseModelIdentity> {
    is_model_identity_probe(current_content).then(|| response_model_identity(&req.model))?
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

pub fn anthropic_identity_override(requested_model: &str) -> String {
    let Some(identity) = response_model_identity(requested_model) else {
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

pub fn normalize_claude_code_model_identity(content: String, requested_model: &str) -> String {
    let Some(model_name) = requested_model_identity_name(requested_model) else {
        return content;
    };
    let model_id = requested_model_identity_id(requested_model);
    let replacement = format!(
        "You are powered by the model named {model_name}. The exact model ID is {model_id}."
    );
    let has_existing_model_identity = content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.contains("You are powered by the model named")
            && trimmed.contains("The exact model ID is")
    });
    let mut replaced_existing = false;
    let mut inserted_after_identity = false;
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.contains("You are powered by the model named")
                && trimmed.contains("The exact model ID is")
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
