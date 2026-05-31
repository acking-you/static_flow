//! Thinking-mode prefix generation and injection into the current turn.

use crate::{anthropic::types::MessagesRequest, wire::UserInputMessage};

// Generates the XML thinking-mode prefix from the request's thinking
// configuration. This is intentionally applied only to the current user turn:
// Kiro has no first-class thinking-level request field, and placing the dynamic
// level/budget in history would poison the stable cache prefix.
//
// For adaptive thinking, preserve the caller's exact effort label instead of
// normalizing it. Local verification against Kiro showed that `low`,
// `medium`, `high`, `xhigh`, and `max` are not interchangeable:
// `low`/`medium` can stay minimal, `high` produces short hidden rationale, and
// `xhigh`/`max` produce materially deeper hidden reasoning in both buffered and
// streaming paths.
fn generate_thinking_prefix(req: &MessagesRequest) -> Option<String> {
    if let Some(thinking) = &req.thinking {
        if thinking.thinking_type == "enabled" {
            return Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{}</\
                 max_thinking_length>",
                thinking.budget_tokens
            ));
        }
        if thinking.thinking_type == "adaptive" {
            let effort = req
                .output_config
                .as_ref()
                .map(|config| config.effective_effort())
                .unwrap_or("xhigh");
            return Some(format!(
                "<thinking_mode>adaptive</thinking_mode><thinking_effort>{effort}</\
                 thinking_effort>"
            ));
        }
    }
    None
}

fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>")
        || content.contains("<max_thinking_length>")
        || content.contains("<thinking_effort>")
}

pub fn apply_thinking_prefix_to_current_turn(
    req: &MessagesRequest,
    user_input: &mut UserInputMessage,
) {
    let Some(prefix) = generate_thinking_prefix(req) else {
        return;
    };
    if has_thinking_tags(&user_input.content) {
        return;
    }
    user_input.content = if user_input.content.is_empty() {
        prefix
    } else {
        format!("{prefix}\n{}", user_input.content)
    };
}
