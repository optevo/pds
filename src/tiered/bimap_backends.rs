//! Concrete [`BiMapBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsBiMapBackend`] | `pds::BiMap` (HAMT-backed bijection) |
//! | [`PdsOrdBiMapBackend`] | `pds::OrdBiMap` (B+ tree-backed bijection) |

use super::bimap_backend::BiMapBackend;

// --- PdsBiMapBackend ---

/// A [`BiMapBackend`] backed by `pds::BiMap` (HAMT with structural sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsBiMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + Eq + std::hash::Hash + 'static,
{
    /// Inner pds hash bijection map.
    inner: crate::BiMap<K, V>,
}

impl<K, V> PdsBiMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::BiMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::BiMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::BiMap<K, V> {
        &self.inner
    }
}

impl<K, V> BiMapBackend<K, V> for PdsBiMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
{
    fn insert(&mut self, key: K, value: V) -> Option<V> {
        // BiMap::insert returns Option<(Option<V>, Option<K>)> — the displaced
        // value (if the key already existed) and the displaced key (if the value
        // already existed). We return only the displaced value for this key,
        // consistent with a standard map insert semantics.
        let displaced = self.inner.insert(key, value);
        displaced.and_then(|(old_v, _old_k)| old_v)
    }

    fn get_by_key(&self, key: &K) -> Option<V> {
        self.inner.get_by_key(key).cloned()
    }

    fn get_by_value(&self, value: &V) -> Option<K> {
        self.inner.get_by_value(value).cloned()
    }

    fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    fn contains_value(&self, value: &V) -> bool {
        self.inner.contains_value(value)
    }

    fn remove_by_key(&mut self, key: &K) -> Option<V> {
        self.inner.remove_by_key(key)
    }

    fn remove_by_value(&mut self, value: &V) -> Option<K> {
        self.inner.remove_by_value(value)
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
        let mut bm = crate::BiMap::new();
        for (k, v) in iter {
            bm.insert(k, v);
        }
        self.inner = bm;
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
        self.inner = crate::BiMap::new();
        pairs
    }
}

// --- PdsOrdBiMapBackend ---

/// A [`BiMapBackend`] backed by `pds::OrdBiMap` (B+ tree with structural sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsOrdBiMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + Ord + 'static,
{
    /// Inner pds ordered bijection map.
    inner: crate::OrdBiMap<K, V>,
}

impl<K, V> PdsOrdBiMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + Ord + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdBiMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdBiMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdBiMap<K, V> {
        &self.inner
    }
}

impl<K, V> BiMapBackend<K, V> for PdsOrdBiMapBackend<K, V>
where
    K: Clone + Ord + Send + Sync + 'static,
    V: Clone + Ord + Send + Sync + 'static,
{
    fn insert(&mut self, key: K, value: V) -> Option<V> {
        let displaced = self.inner.insert(key, value);
        displaced.and_then(|(old_v, _old_k)| old_v)
    }

    fn get_by_key(&self, key: &K) -> Option<V> {
        self.inner.get_by_key(key).cloned()
    }

    fn get_by_value(&self, value: &V) -> Option<K> {
        self.inner.get_by_value(value).cloned()
    }

    fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    fn contains_value(&self, value: &V) -> bool {
        self.inner.contains_value(value)
    }

    fn remove_by_key(&mut self, key: &K) -> Option<V> {
        self.inner.remove_by_key(key)
    }

    fn remove_by_value(&mut self, value: &V) -> Option<K> {
        self.inner.remove_by_value(value)
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
        let mut bm = crate::OrdBiMap::new();
        for (k, v) in iter {
            bm.insert(k, v);
        }
        self.inner = bm;
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
        self.inner = crate::OrdBiMap::new();
        pairs
    }
}
