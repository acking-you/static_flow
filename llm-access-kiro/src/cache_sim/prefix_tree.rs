//! Shared-prefix radix-tree operations: path insertion, TTL pruning, coldest
//! leaf eviction, edge splitting/lookup, and memory-footprint estimation.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn estimate_prefix_tree_memory_bytes(child_capacity: usize, page_count: usize) -> u64 {
    let root_bytes = std::mem::size_of::<PrefixNode>();
    let edge_bytes = child_capacity.saturating_mul(std::mem::size_of::<PrefixEdge>());
    let page_bytes = page_count.saturating_mul(std::mem::size_of::<CanonicalTokenPage>());
    root_bytes
        .saturating_add(edge_bytes)
        .saturating_add(page_bytes) as u64
}
pub(crate) fn estimate_anchor_index_memory_bytes(entries: usize) -> u64 {
    let entry_bytes = std::mem::size_of::<ConversationAnchorEntry>();
    let key_bytes = std::mem::size_of::<String>();
    entries.saturating_mul(entry_bytes.saturating_add(key_bytes)) as u64
}
pub(crate) fn insert_prefix_path(
    node: &mut PrefixNode,
    pages: &[CanonicalTokenPage],
    now: Instant,
) -> u64 {
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
pub(crate) fn prune_expired_children(node: &mut PrefixNode, now: Instant, ttl: Duration) -> u64 {
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
pub(crate) fn subtree_token_count_edge(edge: &PrefixEdge) -> u64 {
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
pub(crate) fn find_coldest_leaf_path(node: &PrefixNode) -> Option<Vec<usize>> {
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
pub(crate) fn remove_leaf_path(node: &mut PrefixNode, path: &[usize]) -> u64 {
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
pub(crate) fn push_child_edge(node: &mut PrefixNode, edge: PrefixEdge) {
    node.children.push(edge);
    node.children_sorted = false;
}
pub(crate) fn find_child_edge_index(node: &mut PrefixNode, first_page_key: u128) -> Option<usize> {
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
pub(crate) fn find_child_edge_index_linear(
    node: &PrefixNode,
    first_page_key: u128,
) -> Option<usize> {
    node.children
        .iter()
        .position(|edge| edge.first_page_key() == first_page_key)
}
pub(crate) fn common_prefix_len(
    left: &[CanonicalTokenPage],
    right: &[CanonicalTokenPage],
) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left.key == right.key)
        .count()
}
pub(crate) fn split_edge_at(
    edge: &mut PrefixEdge,
    split_at: usize,
    prefix_last_touched_at: Instant,
) {
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
pub(crate) fn prefix_pages_token_count(pages: &[CanonicalTokenPage]) -> u64 {
    pages.iter().map(|page| u64::from(page.token_count)).sum()
}
