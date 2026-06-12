#[cfg(not(target_arch = "wasm32"))]
use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct SmallModelCache<K, V> {
    max_entries: usize,
    order: VecDeque<K>,
    values: HashMap<K, V>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<K: Copy + Eq + Hash, V> SmallModelCache<K, V> {
    pub(super) fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            order: VecDeque::new(),
            values: HashMap::new(),
        }
    }

    pub(super) fn get_or_try_insert_mut<E, F>(&mut self, key: K, init: F) -> Result<&mut V, E>
    where
        F: FnOnce() -> Result<V, E>,
    {
        if self.values.contains_key(&key) {
            self.touch(key);
            return Ok(self
                .values
                .get_mut(&key)
                .expect("cache key must exist after touch"));
        }

        if self.values.len() >= self.max_entries {
            self.evict_lru();
        }

        let value = init()?;
        self.values.insert(key, value);
        self.order.push_back(key);
        Ok(self
            .values
            .get_mut(&key)
            .expect("cache key must exist after insert"))
    }

    fn touch(&mut self, key: K) {
        if let Some(pos) = self.order.iter().position(|existing| *existing == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key);
    }

    fn evict_lru(&mut self) {
        while let Some(oldest) = self.order.pop_front() {
            if self.values.remove(&oldest).is_some() {
                break;
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::SmallModelCache;

    #[test]
    fn evicts_least_recently_used_entry() {
        let mut cache = SmallModelCache::new(2);

        let _ = cache
            .get_or_try_insert_mut(1_u8, || Ok::<_, ()>("one".to_string()))
            .expect("insert 1");
        let _ = cache
            .get_or_try_insert_mut(2_u8, || Ok::<_, ()>("two".to_string()))
            .expect("insert 2");

        let value = cache
            .get_or_try_insert_mut(1_u8, || Ok::<_, ()>("one-new".to_string()))
            .expect("touch 1");
        assert_eq!(value.as_str(), "one");

        let _ = cache
            .get_or_try_insert_mut(3_u8, || Ok::<_, ()>("three".to_string()))
            .expect("insert 3");

        let value = cache
            .get_or_try_insert_mut(2_u8, || Ok::<_, ()>("two-new".to_string()))
            .expect("reinsert 2");
        assert_eq!(value.as_str(), "two-new");
    }
}
