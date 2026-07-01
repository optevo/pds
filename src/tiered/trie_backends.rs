//! Concrete [`TrieBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsTrieBackend`] | `pds::Trie` (HAMT-backed prefix tree) |
//! | [`PdsOrdTrieBackend`] | `pds::OrdTrie` (B+ tree-backed prefix tree) |

use super::trie_backend::TrieBackend;

// --- PdsTrieBackend ---

/// A [`TrieBackend`] backed by `pds::Trie` (HAMT with structural sharing).
///
/// `Clone` is O(1) via structural sharing.
#[derive(Clone, Default)]
pub struct PdsTrieBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    inner: crate::Trie<K, V>,
}

impl<K, V> PdsTrieBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::Trie::new(),
        }
    }

    /// Returns a reference to the inner `pds::Trie`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::Trie<K, V> {
        &self.inner
    }
}

impl<K, V> TrieBackend<K, V> for PdsTrieBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn get(&self, key: &[K]) -> Option<V> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: Vec<K>, value: V) -> Option<V> {
        self.inner.insert(&key, value)
    }

    fn remove(&mut self, key: &[K]) -> Option<V> {
        self.inner.remove(key)
    }

    fn prefix_get(&self, prefix: &[K]) -> Vec<(Vec<K>, V)> {
        match self.inner.iter_prefix(prefix) {
            None => Vec::new(),
            Some(iter) => iter
                .map(|(path_refs, v)| {
                    let full_path: Vec<K> = prefix
                        .iter()
                        .chain(path_refs.iter().copied())
                        .cloned()
                        .collect();
                    (full_path, v.clone())
                })
                .collect(),
        }
    }

    fn iter_all(&self) -> Vec<(Vec<K>, V)> {
        self.inner
            .iter()
            .map(|(path_refs, v)| {
                let path: Vec<K> = path_refs.into_iter().cloned().collect();
                (path, v.clone())
            })
            .collect()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = (Vec<K>, V)>) {
        let mut t = crate::Trie::new();
        for (path, v) in iter {
            t.insert(&path, v);
        }
        self.inner = t;
    }

    fn drain(&mut self) -> Vec<(Vec<K>, V)> {
        let pairs = self.iter_all();
        self.inner = crate::Trie::new();
        pairs
    }
}

// --- PdsOrdTrieBackend ---

/// A [`TrieBackend`] backed by `pds::OrdTrie` (B+ tree with structural sharing).
///
/// Keys at each trie node are stored in sorted order. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsOrdTrieBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + 'static,
{
    inner: crate::OrdTrie<K, V>,
}

impl<K, V> PdsOrdTrieBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdTrie::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdTrie`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdTrie<K, V> {
        &self.inner
    }
}

impl<K, V> TrieBackend<K, V> for PdsOrdTrieBackend<K, V>
where
    K: Clone + Ord + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn get(&self, key: &[K]) -> Option<V> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: Vec<K>, value: V) -> Option<V> {
        self.inner.insert(&key, value)
    }

    fn remove(&mut self, key: &[K]) -> Option<V> {
        self.inner.remove(key)
    }

    fn prefix_get(&self, prefix: &[K]) -> Vec<(Vec<K>, V)> {
        match self.inner.iter_prefix(prefix) {
            None => Vec::new(),
            Some(iter) => iter
                .map(|(path_refs, v)| {
                    let full_path: Vec<K> = prefix
                        .iter()
                        .chain(path_refs.iter().copied())
                        .cloned()
                        .collect();
                    (full_path, v.clone())
                })
                .collect(),
        }
    }

    fn iter_all(&self) -> Vec<(Vec<K>, V)> {
        self.inner
            .iter()
            .map(|(path_refs, v)| {
                let path: Vec<K> = path_refs.into_iter().cloned().collect();
                (path, v.clone())
            })
            .collect()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = (Vec<K>, V)>) {
        let mut t = crate::OrdTrie::new();
        for (path, v) in iter {
            t.insert(&path, v);
        }
        self.inner = t;
    }

    fn drain(&mut self) -> Vec<(Vec<K>, V)> {
        let pairs = self.iter_all();
        self.inner = crate::OrdTrie::new();
        pairs
    }
}
