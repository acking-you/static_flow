//! Kiro prefix-cache simulation.
//!
//! Projects a corrected Kiro `ConversationState` into cache-simulation views
//! and tracks shared prefix-cache state across requests. The work is split into
//! focused submodules:
//!
//! ```text
//!  ConversationState
//!     -> [projection]   canonical token pages + conversation anchor segments
//!     -> [prefix_tree]  radix trie of shared stable-prefix pages
//!     -> [anchor_index] LRU conversation-id recovery keyed by anchor hash
//!     -> [simulator]    KiroCacheSimulator ties the three together
//! ```

mod anchor_index;
mod prefix_tree;
mod projection;
mod simulator;

pub use anchor_index::{AnchorTokenCounts, ConversationAnchorRuntimeStats};
pub use prefix_tree::{PrefixCacheMatch, PrefixTreeRuntimeStats};
pub use projection::{CanonicalTokenPage, PromptProjection, RuntimePromptProjection};
pub use simulator::{
    KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulationMode, KiroCacheSimulator,
};

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};

    use super::{
        projection::PREFIX_CACHE_PAGE_SIZE, KiroCacheSimulationConfig, KiroCacheSimulationMode,
        KiroCacheSimulator, PrefixCacheMatch, PromptProjection,
    };
    use crate::wire::{
        AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
        HistoryUserMessage, InputSchema, Message, Tool, ToolSpecification, UserInputMessage,
        UserInputMessageContext,
    };

    fn tool(name: &str, description: &str, schema: Value) -> Tool {
        Tool {
            tool_specification: ToolSpecification {
                name: name.to_string(),
                description: description.to_string(),
                input_schema: InputSchema::from_json(schema),
            },
        }
    }

    fn history_user(content: &str) -> Message {
        Message::User(HistoryUserMessage::new(content, "ignored-model"))
    }

    fn history_assistant(content: &str) -> Message {
        Message::Assistant(HistoryAssistantMessage::new(content))
    }

    #[test]
    fn cache_simulator_matches_stable_prefix_after_success_is_recorded() {
        let state = ConversationState::new("conv-1")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();

        simulator.record_success(&projection, &assistant, "real-conv", true, config, now);
        let matched = simulator.match_prefix(&projection, config, now + Duration::from_secs(1));

        assert_eq!(matched.matched_pages, projection.stable_prefix_pages.len());
        assert!(matched.matched_tokens > 0);
    }

    #[test]
    fn cache_simulator_recovers_resume_anchor_from_post_turn_history() {
        let initial_state = ConversationState::new("fallback-conv")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&initial_state);
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();
        simulator.record_success(&projection, &assistant, "real-conv", true, config, now);

        let follow_up_state = ConversationState::new("new-fallback")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue analysis", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up_state);

        assert_eq!(
            simulator.recover_conversation_id(
                &follow_up_projection,
                config,
                now + Duration::from_secs(1)
            ),
            Some("real-conv".to_string())
        );
    }

    #[test]
    fn cache_simulator_can_record_anchor_without_warming_prefix_tree() {
        let initial_state = ConversationState::new("fallback-conv")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&initial_state);
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();

        simulator.record_success(&projection, &assistant, "real-conv", false, config, now);

        let matched = simulator.match_prefix(&projection, config, now + Duration::from_secs(1));
        assert_eq!(matched, PrefixCacheMatch::default());

        let follow_up_state = ConversationState::new("new-fallback")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue analysis", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up_state);
        assert_eq!(
            simulator.recover_conversation_id(
                &follow_up_projection,
                config,
                now + Duration::from_secs(1)
            ),
            Some("real-conv".to_string())
        );
    }

    #[test]
    fn cache_simulator_snapshot_reports_prefix_tree_and_anchor_usage() {
        let state = ConversationState::new("conv-1")
            .with_history(vec![history_user(&"stable prefix ".repeat(256))])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();

        simulator.record_success(&projection, &assistant, "real-conv", true, config, now);
        let snapshot = simulator.snapshot_stats(config, now + Duration::from_secs(1));

        assert_eq!(snapshot.mode, KiroCacheSimulationMode::PrefixTree);
        assert_eq!(snapshot.page_size_tokens, PREFIX_CACHE_PAGE_SIZE);
        assert_eq!(snapshot.prefix_tree.resident_tokens, projection.stable_prefix_token_count());
        assert_eq!(snapshot.prefix_tree.max_tokens, config.prefix_cache_max_tokens);
        assert!(snapshot.prefix_tree.node_count <= 2);
        assert_eq!(snapshot.prefix_tree.leaf_count, 1);
        assert!(snapshot.prefix_tree.estimated_memory_bytes > 0);
        assert_eq!(snapshot.conversation_anchors.entries, 1);
        assert_eq!(
            snapshot.conversation_anchors.max_entries,
            config.conversation_anchor_max_entries
        );
    }
}
