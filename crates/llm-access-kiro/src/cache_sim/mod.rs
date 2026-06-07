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
mod snapshot;

pub use anchor_index::{AnchorTokenCounts, ConversationAnchorRuntimeStats};
pub use prefix_tree::{PrefixCacheMatch, PrefixTreeRuntimeStats};
pub use projection::{CanonicalTokenPage, PromptProjection, RuntimePromptProjection};
pub use simulator::{
    KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulationMode, KiroCacheSimulator,
};
pub use snapshot::{
    peek_header, KiroSnapshotImportOutcome, SnapshotCaps, SnapshotError,
    MAX_COMPRESSED_SNAPSHOT_BYTES,
};

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};

    use super::{
        projection::PREFIX_CACHE_PAGE_SIZE, KiroCacheSimulationConfig, KiroCacheSimulationMode,
        KiroCacheSimulator, PrefixCacheMatch, PromptProjection, SnapshotCaps,
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

    fn prefix_tree_config() -> KiroCacheSimulationConfig {
        KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        }
    }

    fn warm_state() -> ConversationState {
        ConversationState::new("conv-1")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ))
    }

    #[test]
    fn snapshot_export_import_restores_prefix_and_anchor() {
        let state = warm_state();
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply");
        let config = prefix_tree_config();
        let caps = SnapshotCaps::default();
        let now = Instant::now();

        let warm = KiroCacheSimulator::default();
        warm.record_success(&projection, &assistant, "real-conv", true, config, now);
        let blob = warm
            .export_snapshot(config, caps, now)
            .expect("export produces a blob");

        // Simulate a restart: a fresh simulator restores from the blob.
        let restored = KiroCacheSimulator::default();
        let outcome = restored.import_snapshot(Some(&blob), &[], config, caps, now);
        assert!(outcome.prefix_from_own);
        assert!(outcome.prefix_resident_tokens > 0);
        assert_eq!(outcome.anchor_entries, 1);
        assert_eq!(outcome.decode_errors, 0);

        let matched = restored.match_prefix(&projection, config, now + Duration::from_secs(1));
        assert_eq!(matched.matched_pages, projection.stable_prefix_pages.len());
        assert!(matched.matched_tokens > 0);

        // Recover the conversation id from the resumed follow-up turn.
        let follow_up = ConversationState::new("new-conv")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up);
        assert_eq!(
            restored.recover_conversation_id(
                &follow_up_projection,
                config,
                now + Duration::from_secs(2)
            ),
            Some("real-conv".to_string())
        );
    }

    #[test]
    fn snapshot_peer_seeds_prefix_when_own_absent() {
        let state = warm_state();
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply");
        let config = prefix_tree_config();
        let caps = SnapshotCaps::default();
        let now = Instant::now();

        let peer = KiroCacheSimulator::default();
        peer.record_success(&projection, &assistant, "peer-conv", true, config, now);
        let peer_blob = peer.export_snapshot(config, caps, now).expect("peer blob");

        let restored = KiroCacheSimulator::default();
        let outcome = restored.import_snapshot(None, &[peer_blob], config, caps, now);
        assert!(!outcome.prefix_from_own);
        assert!(outcome.prefix_from_peer);
        assert!(outcome.prefix_resident_tokens > 0);

        let matched = restored.match_prefix(&projection, config, now + Duration::from_secs(1));
        assert_eq!(matched.matched_pages, projection.stable_prefix_pages.len());
    }

    #[test]
    fn snapshot_empty_own_prefix_does_not_shadow_warm_peer() {
        // Own snapshot has anchors but an empty prefix tree (anchor-only warm).
        // A warm peer prefix must still seed the tree rather than being shadowed
        // by the present-but-empty own snapshot.
        let state = warm_state();
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply");
        let config = prefix_tree_config();
        let caps = SnapshotCaps::default();
        let now = Instant::now();

        // Own: record anchor without warming the prefix tree -> empty prefix.
        let own = KiroCacheSimulator::default();
        own.record_success(&projection, &assistant, "own-conv", false, config, now);
        let own_blob = own.export_snapshot(config, caps, now).expect("own blob");

        // Peer: warm prefix tree.
        let peer = KiroCacheSimulator::default();
        peer.record_success(&projection, &assistant, "peer-conv", true, config, now);
        let peer_blob = peer.export_snapshot(config, caps, now).expect("peer blob");

        let restored = KiroCacheSimulator::default();
        let outcome = restored.import_snapshot(Some(&own_blob), &[peer_blob], config, caps, now);
        assert!(!outcome.prefix_from_own, "empty own prefix must not be the source");
        assert!(outcome.prefix_from_peer, "warm peer prefix must seed the tree");
        assert!(outcome.prefix_resident_tokens > 0);

        let matched = restored.match_prefix(&projection, config, now + Duration::from_secs(1));
        assert_eq!(matched.matched_pages, projection.stable_prefix_pages.len());
    }

    #[test]
    fn snapshot_formula_mode_round_trips_anchor_only() {
        let state = warm_state();
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply");
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::Formula,
            ..prefix_tree_config()
        };
        let caps = SnapshotCaps::default();
        let now = Instant::now();

        let warm = KiroCacheSimulator::default();
        // record_success in Formula mode records the anchor but not the tree.
        warm.record_success(&projection, &assistant, "real-conv", false, config, now);
        let blob = warm
            .export_snapshot(config, caps, now)
            .expect("formula export still persists anchors");

        let restored = KiroCacheSimulator::default();
        let outcome = restored.import_snapshot(Some(&blob), &[], config, caps, now);
        assert_eq!(outcome.anchor_entries, 1);
        assert_eq!(outcome.prefix_resident_tokens, 0);

        // No prefix matching in Formula mode, but the anchor recovers.
        let follow_up = ConversationState::new("new-conv")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up);
        assert_eq!(
            restored.recover_conversation_id(
                &follow_up_projection,
                config,
                now + Duration::from_secs(1)
            ),
            Some("real-conv".to_string())
        );
    }

    #[test]
    fn snapshot_import_counts_decode_errors_and_stays_empty() {
        let config = prefix_tree_config();
        let caps = SnapshotCaps::default();
        let now = Instant::now();
        let restored = KiroCacheSimulator::default();
        // Garbage that is neither valid gzip nor a valid frame.
        let outcome = restored.import_snapshot(Some(b"not-a-snapshot"), &[], config, caps, now);
        assert_eq!(outcome.decode_errors, 1);
        assert_eq!(outcome.anchor_entries, 0);
        assert_eq!(outcome.prefix_resident_tokens, 0);
    }
}
