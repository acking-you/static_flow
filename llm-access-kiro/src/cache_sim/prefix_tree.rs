//! Radix prefix-cache trie over canonical token pages.
//!
//! The trie stores shared stable-prefix spans as fixed-size token pages and
//! reports conservative full-page cache matches. It is owned by the simulator
//! and pruned by TTL plus a global resident-token budget.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::projection::CanonicalTokenPage;

const PREFIX_CHILD_SORT_THRESHOLD: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrefixCacheMatch {
    pub matched_pages: usize,
    pub matched_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct PrefixTreeRuntimeStats {
    pub resident_tokens: u64,
    pub max_tokens: u64,
    pub node_count: usize,
    pub leaf_count: usize,
    pub edge_count: usize,
    pub child_capacity: usize,
    pub estimated_memory_bytes: u64,
}

/// Shared radix trie of stable-prefix token pages.
///
/// Owned by `KiroCacheSimulator` behind a mutex. Tracks total resident tokens
/// so insertion can evict the coldest leaf once a global budget is exceeded.
#[derive(Debug, Default)]
pub struct PrefixTree {
    root: PrefixNode,
    resident_tokens: u64,
}

#[derive(Debug, Default)]
struct PrefixNode {
    children: Vec<PrefixEdge>,
    children_sorted: bool,
}

impl Drop for PrefixNode {
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut self.children);
        while let Some(mut edge) = stack.pop() {
            stack.extend(std::mem::take(&mut edge.child.children));
        }
    }
}

#[derive(Debug)]
struct PrefixEdge {
    pages: Box<[CanonicalTokenPage]>,
    token_count: u64,
    last_touched_at: Instant,
    child: PrefixNode,
}

impl PrefixEdge {
    fn new(pages: &[CanonicalTokenPage], now: Instant) -> Self {
        debug_assert!(!pages.is_empty());
        Self {
            pages: pages.to_vec().into_boxed_slice(),
            token_count: prefix_pages_token_count(pages),
            last_touched_at: now,
            child: PrefixNode::default(),
        }
    }

    fn first_page_key(&self) -> u128 {
        self.pages[0].key
    }
}

impl PrefixTree {
    /// Count the longest shared full-page prefix already resident in the trie.
    ///
    /// Matching only counts full pages. Partial-page matches are ignored on
    /// purpose so the reported cache hit stays conservative.
    pub fn match_prefix(
        &mut self,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
    ) -> PrefixCacheMatch {
        self.prune_expired(now, ttl);
        let mut current = &mut self.root;
        let mut matched = PrefixCacheMatch::default();
        let mut offset = 0usize;
        while offset < pages.len() {
            let Some(edge_index) = find_child_edge_index(current, pages[offset].key) else {
                break;
            };
            let edge = &mut current.children[edge_index];
            let common = common_prefix_len(&edge.pages, &pages[offset..]);
            if common == 0 {
                break;
            }
            matched.matched_pages = matched.matched_pages.saturating_add(common);
            matched.matched_tokens = matched
                .matched_tokens
                .saturating_add(prefix_pages_token_count(&edge.pages[..common]));
            if common < edge.pages.len() {
                split_edge_at(edge, common, now);
                break;
            }
            edge.last_touched_at = now;
            offset += common;
            current = &mut edge.child;
        }
        matched
    }

    /// Insert a stable-prefix page path, evicting the coldest leaves while the
    /// total resident token count exceeds `max_tokens`.
    pub fn insert(
        &mut self,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
        max_tokens: u64,
    ) {
        self.prune_expired(now, ttl);
        let added_tokens = insert_prefix_path(&mut self.root, pages, now);
        self.resident_tokens = self.resident_tokens.saturating_add(added_tokens);
        while self.resident_tokens > max_tokens {
            let Some(path) = find_coldest_leaf_path(&self.root) else {
                break;
            };
            let removed = remove_leaf_path(&mut self.root, &path);
            if removed == 0 {
                break;
            }
            self.resident_tokens = self.resident_tokens.saturating_sub(removed);
        }
    }

    /// Drop every edge whose subtree has not been touched within `ttl`.
    pub fn prune_expired(&mut self, now: Instant, ttl: Duration) {
        let removed = prune_expired_children(&mut self.root, now, ttl);
        self.resident_tokens = self.resident_tokens.saturating_sub(removed);
    }

    /// Walk the trie to report resident-token usage and node/edge counts.
    pub fn snapshot_stats(&self, max_tokens: u64) -> PrefixTreeRuntimeStats {
        let mut node_count = 0usize;
        let mut leaf_count = 0usize;
        let mut edge_count = 0usize;
        let mut child_capacity = 0usize;
        let mut page_count = 0usize;
        let mut stack = vec![(&self.root, true)];

        while let Some((node, is_root)) = stack.pop() {
            node_count = node_count.saturating_add(1);
            edge_count = edge_count.saturating_add(node.children.len());
            child_capacity = child_capacity.saturating_add(node.children.capacity());
            page_count = page_count.saturating_add(
                node.children
                    .iter()
                    .map(|edge| edge.pages.len())
                    .sum::<usize>(),
            );
            if node.children.is_empty() && !is_root {
                leaf_count = leaf_count.saturating_add(1);
            }
            stack.extend(node.children.iter().map(|edge| (&edge.child, false)));
        }

        let estimated_memory_bytes = estimate_prefix_tree_memory_bytes(child_capacity, page_count);
        PrefixTreeRuntimeStats {
            resident_tokens: self.resident_tokens,
            max_tokens,
            node_count,
            leaf_count,
            edge_count,
            child_capacity,
            estimated_memory_bytes,
        }
    }
}

fn estimate_prefix_tree_memory_bytes(child_capacity: usize, page_count: usize) -> u64 {
    let root_bytes = std::mem::size_of::<PrefixNode>();
    let edge_bytes = child_capacity.saturating_mul(std::mem::size_of::<PrefixEdge>());
    let page_bytes = page_count.saturating_mul(std::mem::size_of::<CanonicalTokenPage>());
    root_bytes
        .saturating_add(edge_bytes)
        .saturating_add(page_bytes) as u64
}

fn insert_prefix_path(node: &mut PrefixNode, pages: &[CanonicalTokenPage], now: Instant) -> u64 {
    let mut added_tokens: u64 = 0;
    let mut current = node;
    let mut offset = 0usize;
    while offset < pages.len() {
        let Some(edge_index) = find_child_edge_index(current, pages[offset].key) else {
            let edge = PrefixEdge::new(&pages[offset..], now);
            added_tokens = added_tokens.saturating_add(edge.token_count);
            push_child_edge(current, edge);
            return added_tokens;
        };

        let edge = &mut current.children[edge_index];
        let common = common_prefix_len(&edge.pages, &pages[offset..]);
        if common == 0 {
            let edge = PrefixEdge::new(&pages[offset..], now);
            added_tokens = added_tokens.saturating_add(edge.token_count);
            push_child_edge(current, edge);
            return added_tokens;
        }
        if common < edge.pages.len() {
            split_edge_at(edge, common, now);
        } else {
            edge.last_touched_at = now;
        }
        offset += common;
        if offset == pages.len() {
            return added_tokens;
        }
        current = &mut current.children[edge_index].child;
    }
    added_tokens
}

fn prune_expired_children(node: &mut PrefixNode, now: Instant, ttl: Duration) -> u64 {
    let mut removed_tokens: u64 = 0;
    let mut stack = vec![(node as *mut PrefixNode, false)];

    // We use an explicit DFS stack so prefix paths with tens of thousands of
    // pages never recurse on the thread stack. The raw pointers all originate
    // from the unique mutable borrow of `node`, and a node is only removed
    // after its children have already been processed.
    // SAFETY: every pointer in `stack` comes from the unique mutable borrow of
    // `node`. A node is only detached from its parent after all of its
    // descendants have already been processed, so no queued pointer can dangle.
    unsafe {
        while let Some((node_ptr, visited_children)) = stack.pop() {
            let current = &mut *node_ptr;
            if !visited_children {
                stack.push((node_ptr, true));
                for edge in &mut current.children {
                    stack.push((&mut edge.child as *mut PrefixNode, false));
                }
                continue;
            }

            let mut index = 0usize;
            while index < current.children.len() {
                if now.duration_since(current.children[index].last_touched_at) > ttl {
                    let edge = current.children.remove(index);
                    removed_tokens = removed_tokens.saturating_add(subtree_token_count_edge(&edge));
                } else {
                    index += 1;
                }
            }
        }
    }

    removed_tokens
}

fn subtree_token_count_edge(edge: &PrefixEdge) -> u64 {
    let mut total = edge.token_count;
    let mut stack = vec![&edge.child];
    while let Some(current) = stack.pop() {
        for edge in &current.children {
            total = total.saturating_add(edge.token_count);
            stack.push(&edge.child);
        }
    }
    total
}

fn find_coldest_leaf_path(node: &PrefixNode) -> Option<Vec<usize>> {
    struct Frame<'a> {
        node: &'a PrefixNode,
        incoming_last_touched_at: Option<Instant>,
        next_child: usize,
    }

    let mut best: Option<(Instant, Vec<usize>)> = None;
    let mut path = Vec::<usize>::new();
    let mut stack = vec![Frame {
        node,
        incoming_last_touched_at: None,
        next_child: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        if frame.node.children.is_empty() {
            if let Some(last_touched_at) = frame.incoming_last_touched_at {
                match &best {
                    Some((current_oldest, _)) if last_touched_at >= *current_oldest => {},
                    _ => best = Some((last_touched_at, path.clone())),
                }
            }
            stack.pop();
            if !path.is_empty() {
                path.pop();
            }
            continue;
        }

        if frame.next_child >= frame.node.children.len() {
            stack.pop();
            if !path.is_empty() {
                path.pop();
            }
            continue;
        }

        let edge_index = frame.next_child;
        frame.next_child += 1;
        let edge = &frame.node.children[edge_index];
        path.push(edge_index);
        stack.push(Frame {
            node: &edge.child,
            incoming_last_touched_at: Some(edge.last_touched_at),
            next_child: 0,
        });
    }

    best.map(|(_, path)| path)
}

fn remove_leaf_path(node: &mut PrefixNode, path: &[usize]) -> u64 {
    if path.is_empty() {
        return 0;
    }

    let mut lineage = Vec::with_capacity(path.len());
    let mut current_ptr = node as *mut PrefixNode;

    // The lineage stores each parent pointer plus the child index used to descend
    // one level. This lets us prune empty ancestors iteratively on the way back
    // up without recursive calls.
    // SAFETY: `lineage` stores parent pointers discovered by walking the tree
    // from the exclusive mutable root borrow. We only remove descendants while
    // walking back up that exact lineage, so each pointer remains valid until
    // the moment its corresponding child entry is removed.
    unsafe {
        for key in path {
            let current = &mut *current_ptr;
            let Some(edge) = current.children.get_mut(*key) else {
                return 0;
            };
            lineage.push((current_ptr, *key));
            current_ptr = &mut edge.child as *mut PrefixNode;
        }

        let (leaf_parent_ptr, leaf_index) = *lineage
            .last()
            .expect("non-empty path should always record one lineage entry");
        let leaf_parent = &mut *leaf_parent_ptr;
        if leaf_index >= leaf_parent.children.len() {
            return 0;
        }
        let removed_edge = leaf_parent.children.remove(leaf_index);
        let removed_subtree_tokens = subtree_token_count_edge(&removed_edge);
        if removed_subtree_tokens == 0 {
            return 0;
        }

        let mut removed_tokens = removed_subtree_tokens;
        for &(parent_ptr, child_index) in lineage[..lineage.len().saturating_sub(1)].iter().rev() {
            let parent = &mut *parent_ptr;
            let Some(edge) = parent.children.get(child_index) else {
                break;
            };
            if !edge.child.children.is_empty() {
                break;
            }
            let edge = parent.children.remove(child_index);
            removed_tokens = removed_tokens.saturating_add(edge.token_count);
        }

        removed_tokens
    }
}

fn push_child_edge(node: &mut PrefixNode, edge: PrefixEdge) {
    node.children.push(edge);
    node.children_sorted = false;
}

fn find_child_edge_index(node: &mut PrefixNode, first_page_key: u128) -> Option<usize> {
    if node.children.len() < PREFIX_CHILD_SORT_THRESHOLD {
        return find_child_edge_index_linear(node, first_page_key);
    }
    if !node.children_sorted {
        node.children
            .sort_unstable_by_key(|edge| edge.first_page_key());
        node.children_sorted = true;
    }
    node.children
        .binary_search_by_key(&first_page_key, |edge| edge.first_page_key())
        .ok()
}

fn find_child_edge_index_linear(node: &PrefixNode, first_page_key: u128) -> Option<usize> {
    node.children
        .iter()
        .position(|edge| edge.first_page_key() == first_page_key)
}

fn common_prefix_len(left: &[CanonicalTokenPage], right: &[CanonicalTokenPage]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left.key == right.key)
        .count()
}

fn split_edge_at(edge: &mut PrefixEdge, split_at: usize, prefix_last_touched_at: Instant) {
    debug_assert!(split_at > 0);
    debug_assert!(split_at < edge.pages.len());

    let old_pages = std::mem::take(&mut edge.pages).into_vec();
    let old_last_touched_at = edge.last_touched_at;
    let old_child = std::mem::take(&mut edge.child);
    let mut prefix_pages = old_pages;
    let suffix_pages = prefix_pages.split_off(split_at);
    let prefix_token_count = prefix_pages_token_count(&prefix_pages);
    let suffix_token_count = prefix_pages_token_count(&suffix_pages);

    edge.pages = prefix_pages.into_boxed_slice();
    edge.token_count = prefix_token_count;
    edge.last_touched_at = prefix_last_touched_at;
    edge.child = PrefixNode {
        children: vec![PrefixEdge {
            pages: suffix_pages.into_boxed_slice(),
            token_count: suffix_token_count,
            last_touched_at: old_last_touched_at,
            child: old_child,
        }],
        children_sorted: false,
    };
}

fn prefix_pages_token_count(pages: &[CanonicalTokenPage]) -> u64 {
    pages.iter().map(|page| u64::from(page.token_count)).sum()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    use super::{CanonicalTokenPage, PrefixCacheMatch, PrefixTree};

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

    fn prune_expired_plain_children(
        node: &mut PlainPrefixNode,
        now: Instant,
        ttl: Duration,
    ) -> u64 {
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
        fn walk(
            node: &PlainPrefixNode,
            path: &mut Vec<u128>,
            best: &mut Option<(Instant, Vec<u128>)>,
        ) {
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
}
