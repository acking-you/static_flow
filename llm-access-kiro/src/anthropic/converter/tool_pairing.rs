//! Tool-use/tool-result pairing validation and pruning of orphaned
//! tool_use / tool_result entries from conversation history.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

// Validates that every tool_result in the current message has a matching
// tool_use in history. Returns the validated results and the set of
// orphaned tool_use IDs that have no corresponding result anywhere.
pub(crate) fn validate_tool_pairing(
    history: &[Message],
    tool_results: &[ToolResult],
) -> (Vec<ToolResult>, HashSet<String>) {
    let mut all_tool_use_ids = HashSet::new();
    let mut history_tool_result_ids = HashSet::new();

    for message in history {
        match message {
            Message::Assistant(message) => {
                if let Some(tool_uses) = &message.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        all_tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
            },
            Message::User(message) => {
                for result in &message
                    .user_input_message
                    .user_input_message_context
                    .tool_results
                {
                    history_tool_result_ids.insert(result.tool_use_id.clone());
                }
            },
        }
    }

    let mut unpaired_tool_use_ids: HashSet<String> = all_tool_use_ids
        .difference(&history_tool_result_ids)
        .cloned()
        .collect();
    let mut filtered_results = Vec::new();
    for result in tool_results {
        if unpaired_tool_use_ids.contains(&result.tool_use_id) {
            filtered_results.push(result.clone());
            unpaired_tool_use_ids.remove(&result.tool_use_id);
        }
    }
    (filtered_results, unpaired_tool_use_ids)
}
// Removes tool_use entries from assistant messages in history whose IDs
// are in the orphaned set (no matching tool_result exists).
pub(crate) fn remove_orphaned_tool_uses(history: &mut [Message], orphaned_ids: &HashSet<String>) {
    if orphaned_ids.is_empty() {
        return;
    }
    for message in history.iter_mut() {
        if let Message::Assistant(message) = message {
            if let Some(tool_uses) = message.assistant_response_message.tool_uses.as_mut() {
                tool_uses.retain(|entry| !orphaned_ids.contains(&entry.tool_use_id));
                if tool_uses.is_empty() {
                    message.assistant_response_message.tool_uses = None;
                }
            }
        }
    }
}
// Drops history tool_results that do not correspond to an earlier assistant
// tool_use in the preserved history prefix. Kiro rejects history turns that
// contain tool results without a prior tool call, so we enforce that invariant
// before validating the current turn.
pub(crate) fn prune_orphaned_history_tool_results(history: &mut Vec<Message>) {
    let mut pending_tool_use_ids = HashSet::<String>::new();
    let mut retained = Vec::with_capacity(history.len());

    for message in history.drain(..) {
        match message {
            Message::Assistant(message) => {
                if let Some(tool_uses) = &message.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        pending_tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
                retained.push(Message::Assistant(message));
            },
            Message::User(mut message) => {
                let context = &mut message.user_input_message.user_input_message_context;
                if !context.tool_results.is_empty() {
                    context
                        .tool_results
                        .retain(|result| pending_tool_use_ids.remove(&result.tool_use_id));
                }

                let has_content = !message.user_input_message.content.trim().is_empty();
                let has_images = !message.user_input_message.images.is_empty();
                let has_documents = !message.user_input_message.documents.is_empty();
                let has_tool_results = !context.tool_results.is_empty();
                if has_content || has_images || has_documents || has_tool_results {
                    retained.push(Message::User(message));
                }
            },
        }
    }

    *history = retained;
}
