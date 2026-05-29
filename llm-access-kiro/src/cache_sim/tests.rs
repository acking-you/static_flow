use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::wire::{
    CurrentMessage, HistoryAssistantMessage, HistoryUserMessage, InputSchema, Tool, ToolResult,
    ToolSpecification, ToolUseEntry, UserInputMessage, UserInputMessageContext,
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
fn prompt_projection_excludes_current_turn_from_lookup_anchor() {
    let state = ConversationState::new("conv-1")
        .with_history(vec![history_user("previous user"), history_assistant("previous answer")])
        .with_current_message(CurrentMessage::new(UserInputMessage::new(
            "new current turn",
            "ignored-model",
        )));

    let projection = PromptProjection::from_conversation_state(&state);
    let resume_anchor =
        projection.build_resume_anchor_hash(&AssistantMessage::new("assistant next"));

    assert_eq!(projection.lookup_anchor_hash, hash_segments(&projection.history_anchor_segments));
    assert!(projection
        .history_anchor_segments
        .iter()
        .all(|segment| !segment.contains("new current turn")));
    assert_ne!(projection.lookup_anchor_hash, resume_anchor);
}

#[test]
fn prompt_projection_excludes_current_tool_results_from_stable_prefix() {
    let current = UserInputMessage::new("continue", "ignored-model").with_context(
        UserInputMessageContext::new()
            .with_tool_results(vec![ToolResult::success("current-tool", "current result")])
            .with_tools(vec![tool(
                "search_files",
                "Search files",
                json!({"type":"object","properties":{"query":{"type":"string"}}}),
            )]),
    );
    let state = ConversationState::new("conv-1")
        .with_history(vec![history_user("existing history")])
        .with_current_message(CurrentMessage::new(current));

    let projection = PromptProjection::from_conversation_state(&state);
    let stable_prefix = projection
        .stable_prefix_segment_keys
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!stable_prefix.contains("current-tool"));
    assert!(!stable_prefix.contains("current result"));
    assert!(stable_prefix.contains("search_files"));
}

#[test]
fn prompt_projection_is_stable_for_equivalent_history() {
    let left = ConversationState::new("left")
        .with_history(vec![history_user("  hello world\r\n"), history_assistant("done  ")])
        .with_current_message(CurrentMessage::new(
            UserInputMessage::new("current", "ignored-model").with_context(
                UserInputMessageContext::new().with_tools(vec![tool(
                    "inspect_project",
                    " Inspect project ",
                    json!({
                        "properties": {
                            "path": {"type":"string"},
                            "recursive": {"type":"boolean"}
                        },
                        "type":"object"
                    }),
                )]),
            ),
        ));
    let right = ConversationState::new("right")
        .with_history(vec![history_user("hello world"), history_assistant("done")])
        .with_current_message(CurrentMessage::new(
            UserInputMessage::new("different current", "ignored-model").with_context(
                UserInputMessageContext::new().with_tools(vec![tool(
                    "inspect_project",
                    "Inspect project",
                    json!({
                        "type":"object",
                        "properties": {
                            "recursive": {"type":"boolean"},
                            "path": {"type":"string"}
                        }
                    }),
                )]),
            ),
        ));

    let left_projection = PromptProjection::from_conversation_state(&left);
    let right_projection = PromptProjection::from_conversation_state(&right);

    assert_eq!(left_projection.lookup_anchor_hash, right_projection.lookup_anchor_hash);
    assert_eq!(left_projection.stable_prefix_pages, right_projection.stable_prefix_pages);
    assert_ne!(
        left_projection.projected_input_token_count,
        right_projection.projected_input_token_count
    );
}

#[test]
fn prompt_projection_resume_anchor_ignores_current_tool_definitions() {
    let base_history = vec![history_user("existing history")];
    let current_a = UserInputMessage::new("continue", "ignored-model").with_context(
        UserInputMessageContext::new().with_tools(vec![tool(
            "search_files",
            "Search files",
            json!({"type":"object","properties":{"query":{"type":"string"}}}),
        )]),
    );
    let current_b = UserInputMessage::new("continue", "ignored-model").with_context(
        UserInputMessageContext::new().with_tools(vec![tool(
            "read_file",
            "Read file",
            json!({"type":"object","properties":{"path":{"type":"string"}}}),
        )]),
    );
    let state_a = ConversationState::new("conv-a")
        .with_history(base_history.clone())
        .with_current_message(CurrentMessage::new(current_a));
    let state_b = ConversationState::new("conv-b")
        .with_history(base_history)
        .with_current_message(CurrentMessage::new(current_b));

    let projection_a = PromptProjection::from_conversation_state(&state_a);
    let projection_b = PromptProjection::from_conversation_state(&state_b);
    let assistant = AssistantMessage::new("assistant reply")
        .with_tool_uses(vec![ToolUseEntry::new("tool-1", "search_files")]);

    assert_eq!(
        projection_a.build_resume_anchor_hash(&assistant),
        projection_b.build_resume_anchor_hash(&assistant)
    );
    assert_ne!(projection_a.stable_prefix_segment_keys, projection_b.stable_prefix_segment_keys);
}

#[test]
fn runtime_prompt_projection_preserves_matching_and_resume_hashes() {
    let state = ConversationState::new("conv-runtime")
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
    let assistant = AssistantMessage::new("assistant reply")
        .with_tool_uses(vec![ToolUseEntry::new("tool-1", "search_files")]);
    let expected_resume_anchor = projection.build_resume_anchor_hash(&assistant);
    let expected_lookup_anchor = projection.lookup_anchor_hash.clone();
    let expected_pages = projection.stable_prefix_pages.clone();
    let expected_projected_tokens = projection.projected_input_token_count;

    let runtime_projection = RuntimePromptProjection::from_conversation_state(&state);

    assert_eq!(runtime_projection.lookup_anchor_hash(), expected_lookup_anchor);
    assert_eq!(runtime_projection.stable_prefix_pages(), expected_pages);
    assert_eq!(runtime_projection.projected_input_token_count(), expected_projected_tokens);
    assert_eq!(runtime_projection.build_resume_anchor_hash(&assistant), expected_resume_anchor);
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
    assert_eq!(snapshot.conversation_anchors.max_entries, config.conversation_anchor_max_entries);
}

#[test]
fn prefix_tree_compresses_long_single_branch() {
    let pages = numbered_pages(512, 10_000);
    let mut tree = PrefixTree::default();
    let now = Instant::now();
    let ttl = Duration::from_secs(300);

    tree.insert(&pages, now, ttl, u64::MAX);

    let snapshot = tree.snapshot_stats(u64::MAX);
    assert_eq!(snapshot.resident_tokens, pages_token_count(&pages));
    assert_eq!(snapshot.node_count, 2);
    assert_eq!(snapshot.edge_count, 1);
    assert_eq!(snapshot.leaf_count, 1);
    let matched = tree.match_prefix(&pages, now + Duration::from_secs(1), ttl);
    assert_eq!(matched.matched_pages, pages.len());
    assert_eq!(matched.matched_tokens, pages_token_count(&pages));
}

#[test]
fn prefix_tree_splits_compressed_edges_on_divergence() {
    let first = pages_from_keys(&[1, 2, 3, 4]);
    let second = pages_from_keys(&[1, 2, 9, 10]);
    let divergent = pages_from_keys(&[1, 2, 3, 99]);
    let mut tree = PrefixTree::default();
    let now = Instant::now();
    let ttl = Duration::from_secs(300);

    tree.insert(&first, now, ttl, u64::MAX);
    tree.insert(&second, now + Duration::from_secs(1), ttl, u64::MAX);

    let snapshot = tree.snapshot_stats(u64::MAX);
    assert_eq!(
        snapshot.resident_tokens,
        pages_token_count(&first) + pages_token_count(&second[2..])
    );
    assert_eq!(snapshot.node_count, 4);
    assert_eq!(snapshot.edge_count, 3);
    assert_eq!(snapshot.leaf_count, 2);

    let matched_first = tree.match_prefix(&first, now + Duration::from_secs(2), ttl);
    assert_eq!(matched_first.matched_pages, first.len());
    assert_eq!(matched_first.matched_tokens, pages_token_count(&first));

    let matched_second = tree.match_prefix(&second, now + Duration::from_secs(3), ttl);
    assert_eq!(matched_second.matched_pages, second.len());
    assert_eq!(matched_second.matched_tokens, pages_token_count(&second));

    let matched_divergent = tree.match_prefix(&divergent, now + Duration::from_secs(4), ttl);
    assert_eq!(matched_divergent.matched_pages, 3);
    assert_eq!(matched_divergent.matched_tokens, pages_token_count(&divergent[..3]));
}

#[test]
fn prefix_tree_partial_match_only_refreshes_touched_prefix() {
    let first = pages_from_keys(&[1, 2, 3, 4]);
    let second = pages_from_keys(&[1, 2, 9, 10]);
    let divergent = pages_from_keys(&[1, 2, 3, 99]);
    let mut tree = PrefixTree::default();
    let now = Instant::now();
    let ttl = Duration::from_secs(30);

    tree.insert(&first, now, ttl, u64::MAX);
    tree.insert(&second, now, ttl, u64::MAX);
    let matched = tree.match_prefix(&divergent, now + Duration::from_secs(10), ttl);
    assert_eq!(matched.matched_pages, 3);

    tree.prune_expired(now + Duration::from_secs(35), ttl);

    assert_eq!(tree.resident_tokens, pages_token_count(&divergent[..3]));
    let retained = tree.match_prefix(&divergent[..3], now + Duration::from_secs(36), ttl);
    assert_eq!(retained.matched_pages, 3);
    let expired_branch = tree.match_prefix(&second, now + Duration::from_secs(37), ttl);
    assert_eq!(expired_branch.matched_pages, 2);
}

#[test]
fn radix_prefix_tree_matches_plain_trie_hit_semantics() {
    let ttl = Duration::from_secs(30);
    let now = Instant::now();
    let first = pages_from_keys(&[1, 2, 3, 4]);
    let second = pages_from_keys(&[1, 2, 9, 10]);
    let divergent = pages_from_keys(&[1, 2, 3, 99]);
    let short_prefix = pages_from_keys(&[1, 2]);
    let missing = pages_from_keys(&[7, 8]);
    let mut radix = PrefixTree::default();
    let mut plain = PlainPrefixTree::default();

    compare_insert(&mut radix, &mut plain, &first, now, ttl);
    compare_match(&mut radix, &mut plain, &first, now + Duration::from_secs(1), ttl);
    compare_insert(&mut radix, &mut plain, &second, now + Duration::from_secs(2), ttl);
    compare_match(&mut radix, &mut plain, &divergent, now + Duration::from_secs(10), ttl);
    compare_match(&mut radix, &mut plain, &second, now + Duration::from_secs(11), ttl);
    compare_match(&mut radix, &mut plain, &short_prefix, now + Duration::from_secs(12), ttl);
    compare_match(&mut radix, &mut plain, &missing, now + Duration::from_secs(13), ttl);

    let prune_at = now + Duration::from_secs(45);
    radix.prune_expired(prune_at, ttl);
    plain.prune_expired(prune_at, ttl);
    assert_eq!(radix.resident_tokens, plain.resident_tokens);
    compare_match(&mut radix, &mut plain, &divergent, now + Duration::from_secs(46), ttl);
    compare_match(&mut radix, &mut plain, &second, now + Duration::from_secs(47), ttl);
}

#[test]
fn radix_prefix_tree_matches_plain_trie_budget_eviction_semantics() {
    let ttl = Duration::from_secs(300);
    let now = Instant::now();
    let shared_first = pages_from_keys(&[1, 2, 3]);
    let shared_second = pages_from_keys(&[1, 2, 9]);
    let independent = pages_from_keys(&[5, 6]);
    let newest = pages_from_keys(&[7, 8, 9]);
    let max_tokens = 50;
    let mut radix = PrefixTree::default();
    let mut plain = PlainPrefixTree::default();

    compare_insert_with_limit(&mut radix, &mut plain, &shared_first, now, ttl, max_tokens);
    compare_insert_with_limit(
        &mut radix,
        &mut plain,
        &shared_second,
        now + Duration::from_secs(1),
        ttl,
        max_tokens,
    );
    compare_insert_with_limit(
        &mut radix,
        &mut plain,
        &independent,
        now + Duration::from_secs(2),
        ttl,
        max_tokens,
    );
    compare_match(&mut radix, &mut plain, &shared_second, now + Duration::from_secs(3), ttl);
    compare_insert_with_limit(
        &mut radix,
        &mut plain,
        &newest,
        now + Duration::from_secs(4),
        ttl,
        max_tokens,
    );

    compare_match(&mut radix, &mut plain, &shared_first, now + Duration::from_secs(5), ttl);
    compare_match(&mut radix, &mut plain, &shared_second, now + Duration::from_secs(6), ttl);
    compare_match(&mut radix, &mut plain, &newest, now + Duration::from_secs(7), ttl);
}

#[test]
fn prefix_tree_sorts_high_fanout_node_lazily_without_changing_hits() {
    let ttl = Duration::from_secs(300);
    let now = Instant::now();
    let mut radix = PrefixTree::default();
    let mut plain = PlainPrefixTree::default();

    for key in (0..32).rev() {
        compare_insert(&mut radix, &mut plain, &pages_from_keys(&[key]), now, ttl);
    }

    assert_ne!(root_first_page_keys(&radix), (0..32).collect::<Vec<_>>());
    compare_match(
        &mut radix,
        &mut plain,
        &pages_from_keys(&[17]),
        now + Duration::from_secs(1),
        ttl,
    );
    assert_eq!(root_first_page_keys(&radix), (0..32).collect::<Vec<_>>());
}

fn numbered_pages(count: usize, start: u128) -> Vec<CanonicalTokenPage> {
    (0..count)
        .map(|index| CanonicalTokenPage {
            key: start + index as u128,
            token_count: 64,
        })
        .collect()
}

fn pages_from_keys(keys: &[u128]) -> Vec<CanonicalTokenPage> {
    keys.iter()
        .map(|key| CanonicalTokenPage {
            key: *key,
            token_count: 10,
        })
        .collect()
}

fn pages_token_count(pages: &[CanonicalTokenPage]) -> u64 {
    pages.iter().map(|page| u64::from(page.token_count)).sum()
}

fn compare_insert(
    radix: &mut PrefixTree,
    plain: &mut PlainPrefixTree,
    pages: &[CanonicalTokenPage],
    now: Instant,
    ttl: Duration,
) {
    compare_insert_with_limit(radix, plain, pages, now, ttl, u64::MAX);
}

fn compare_insert_with_limit(
    radix: &mut PrefixTree,
    plain: &mut PlainPrefixTree,
    pages: &[CanonicalTokenPage],
    now: Instant,
    ttl: Duration,
    max_tokens: u64,
) {
    radix.insert(pages, now, ttl, max_tokens);
    plain.insert(pages, now, ttl, max_tokens);
    assert_eq!(radix.resident_tokens, plain.resident_tokens);
}

fn compare_match(
    radix: &mut PrefixTree,
    plain: &mut PlainPrefixTree,
    pages: &[CanonicalTokenPage],
    now: Instant,
    ttl: Duration,
) {
    let radix_match = radix.match_prefix(pages, now, ttl);
    let plain_match = plain.match_prefix(pages, now, ttl);
    assert_eq!(radix_match, plain_match);
    assert_eq!(radix.resident_tokens, plain.resident_tokens);
}

fn root_first_page_keys(tree: &PrefixTree) -> Vec<u128> {
    tree.root
        .children
        .iter()
        .map(|edge| edge.pages[0].key)
        .collect()
}

#[derive(Default)]
struct PlainPrefixTree {
    root: PlainPrefixNode,
    resident_tokens: u64,
}

#[derive(Default)]
struct PlainPrefixNode {
    token_count: u64,
    last_touched_at: Option<Instant>,
    children: BTreeMap<u128, PlainPrefixNode>,
}

impl PlainPrefixTree {
    fn match_prefix(
        &mut self,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
    ) -> PrefixCacheMatch {
        self.prune_expired(now, ttl);
        let mut current = &mut self.root;
        let mut matched = PrefixCacheMatch::default();
        for page in pages {
            let Some(child) = current.children.get_mut(&page.key) else {
                break;
            };
            child.last_touched_at = Some(now);
            matched.matched_pages = matched.matched_pages.saturating_add(1);
            matched.matched_tokens = matched.matched_tokens.saturating_add(child.token_count);
            current = child;
        }
        matched
    }

    fn insert(
        &mut self,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
        max_tokens: u64,
    ) {
        self.prune_expired(now, ttl);
        let mut current = &mut self.root;
        for page in pages {
            let child = current.children.entry(page.key).or_insert_with(|| {
                self.resident_tokens = self
                    .resident_tokens
                    .saturating_add(u64::from(page.token_count));
                PlainPrefixNode {
                    token_count: u64::from(page.token_count),
                    last_touched_at: Some(now),
                    children: BTreeMap::new(),
                }
            });
            child.last_touched_at = Some(now);
            current = child;
        }
        while self.resident_tokens > max_tokens {
            let Some(path) = plain_coldest_leaf_path(&self.root) else {
                break;
            };
            let removed = plain_remove_leaf_path(&mut self.root, &path);
            if removed == 0 {
                break;
            }
            self.resident_tokens = self.resident_tokens.saturating_sub(removed);
        }
    }

    fn prune_expired(&mut self, now: Instant, ttl: Duration) {
        let removed = prune_expired_plain_children(&mut self.root, now, ttl);
        self.resident_tokens = self.resident_tokens.saturating_sub(removed);
    }
}

fn prune_expired_plain_children(node: &mut PlainPrefixNode, now: Instant, ttl: Duration) -> u64 {
    let mut removed_tokens = 0u64;
    for child in node.children.values_mut() {
        removed_tokens =
            removed_tokens.saturating_add(prune_expired_plain_children(child, now, ttl));
    }
    let expired_keys = node
        .children
        .iter()
        .filter(|(_, child)| {
            child
                .last_touched_at
                .is_some_and(|last_touched_at| now.duration_since(last_touched_at) > ttl)
        })
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    for key in expired_keys {
        if let Some(child) = node.children.remove(&key) {
            removed_tokens = removed_tokens.saturating_add(plain_subtree_token_count(&child));
        }
    }
    removed_tokens
}

fn plain_coldest_leaf_path(node: &PlainPrefixNode) -> Option<Vec<u128>> {
    fn walk(node: &PlainPrefixNode, path: &mut Vec<u128>, best: &mut Option<(Instant, Vec<u128>)>) {
        if node.children.is_empty() {
            if let Some(last_touched_at) = node.last_touched_at {
                match best {
                    Some((current_oldest, _)) if last_touched_at >= *current_oldest => {},
                    _ => *best = Some((last_touched_at, path.clone())),
                }
            }
            return;
        }
        for (key, child) in &node.children {
            path.push(*key);
            walk(child, path, best);
            path.pop();
        }
    }

    let mut best = None;
    walk(node, &mut Vec::new(), &mut best);
    best.map(|(_, path)| path)
}

fn plain_remove_leaf_path(node: &mut PlainPrefixNode, path: &[u128]) -> u64 {
    fn remove_at(node: &mut PlainPrefixNode, path: &[u128]) -> u64 {
        let Some((key, remaining)) = path.split_first() else {
            return 0;
        };
        if remaining.is_empty() {
            return node
                .children
                .remove(key)
                .map(|child| plain_subtree_token_count(&child))
                .unwrap_or(0);
        }
        let Some(child) = node.children.get_mut(key) else {
            return 0;
        };
        let mut removed = remove_at(child, remaining);
        if child.children.is_empty() {
            let token_count = child.token_count;
            if node.children.remove(key).is_some() {
                removed = removed.saturating_add(token_count);
            }
        }
        removed
    }

    remove_at(node, path)
}

fn plain_subtree_token_count(node: &PlainPrefixNode) -> u64 {
    let mut total = node.token_count;
    for child in node.children.values() {
        total = total.saturating_add(plain_subtree_token_count(child));
    }
    total
}

#[test]
fn prefix_tree_handles_deep_paths_without_recursive_helpers() {
    let depth = 20_000usize;
    let pages = (0..depth)
        .map(|index| CanonicalTokenPage {
            key: index as u128 + 1,
            token_count: 64,
        })
        .collect::<Vec<_>>();
    let mut tree = PrefixTree::default();
    let now = Instant::now();
    let ttl = Duration::from_secs(300);

    tree.insert(&pages, now, ttl, u64::MAX);
    let matched = tree.match_prefix(&pages, now + Duration::from_secs(1), ttl);
    assert_eq!(matched.matched_pages, depth);
    assert_eq!(matched.matched_tokens, depth as u64 * 64);

    tree.prune_expired(now + ttl + Duration::from_secs(2), ttl);
    assert_eq!(tree.resident_tokens, 0);
    assert!(tree.root.children.is_empty());
}
