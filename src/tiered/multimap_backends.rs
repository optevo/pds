//! Concrete [`MultiMapBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsHashMultiMapBackend`] | `pds::HashMultiMap` (HAMT-backed) |
//! | [`PdsOrdMultiMapBackend`] | `pds::OrdMultiMap` (B+ tree-backed) |

use super::multimap_backend::MultiMapBackend;

// --- PdsHashMultiMapBackend ---

/// A [`MultiMapBackend`] backed by `pds::HashMultiMap` (HAMT with structural
/// sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsHashMultiMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + Eq + std::hash::Hash + 'static,
{
    /// Inner pds hash multimap.
    inner: crate::HashMultiMap<K, V>,
}

impl<K, V> PdsHashMultiMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::HashMultiMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::HashMultiMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::HashMultiMap<K, V> {
        &self.inner
    }
}

impl<K, V> MultiMapBackend<K, V> for PdsHashMultiMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static,
    V: Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static,
{
    fn insert(&mut self, key: K, value: V) {
        self.inner.insert(key, value);
    }

    fn remove_entry(&mut self, key: &K, value: &V) -> bool {
        self.inner.remove(key, value)
    }

    fn remove_key(&mut self, key: &K) -> bool {
        let had_key = self.inner.contains_key(key);
        if had_key {
            self.inner.remove_all(key);
        }
        had_key
    }

    fn get_all(&self, key: &K) -> Vec<V> {
        self.inner.get(key).iter().cloned().collect()
    }

    fn contains(&self, key: &K, value: &V) -> bool {
        self.inner.contains(key, value)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied (key, value) pairs.
    ///
    /// Time: O(n).
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        let mut mm = crate::HashMultiMap::new();
        for (k, v) in iter {
            mm.insert(k, v);
        }
        self.inner = mm;
    }

    /// Drains all (key, value) pairs.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        let pairs: Vec<(K, V)> = self
            .inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.inner = crate::HashMultiMap::new();
        pairs
    }
}

// --- PdsOrdMultiMapBackend ---

/// A [`MultiMapBackend`] backed by `pds::OrdMultiMap` (B+ tree with structural
/// sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsOrdMultiMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + Ord + 'static,
{
    /// Inner pds ordered multimap.
    inner: crate::OrdMultiMap<K, V>,
}

impl<K, V> PdsOrdMultiMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + Ord + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdMultiMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdMultiMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdMultiMap<K, V> {
        &self.inner
    }
}

impl<K, V> MultiMapBackend<K, V> for PdsOrdMultiMapBackend<K, V>
where
    K: Clone + Ord + Send + Sync + 'static,
    V: Clone + Ord + Send + Sync + 'static,
{
    fn insert(&mut self, key: K, value: V) {
        self.inner.insert(key, value);
    }

    fn remove_entry(&mut self, key: &K, value: &V) -> bool {
        self.inner.remove(key, value)
    }

    fn remove_key(&mut self, key: &K) -> bool {
        let had_key = self.inner.contains_key(key);
        if had_key {
            self.inner.remove_all(key);
        }
        had_key
    }

    fn get_all(&self, key: &K) -> Vec<V> {
        self.inner.get(key).iter().cloned().collect()
    }

    fn contains(&self, key: &K, value: &V) -> bool {
        self.inner.contains(key, value)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied (key, value) pairs.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        let mut mm = crate::OrdMultiMap::new();
        for (k, v) in iter {
            mm.insert(k, v);
        }
        self.inner = mm;
    }

    /// Drains all (key, value) pairs in key order.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        let pairs: Vec<(K, V)> = self
            .inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.inner = crate::OrdMultiMap::new();
        pairs
    }
}
