//! Concrete [`CollectionBackend`] and [`OrderedCollectionBackend`] implementations.
//!
//! The following backends are provided:
//!
//! | Backend | Underlying store | Feature gate |
//! |---------|-----------------|--------------|
//! | [`StdHashMapBackend`] | `std::collections::HashMap` | `tiered` (always) |
//! | [`PdsHashMapBackend`] | `pds::HashMap` (HAMT, structural sharing) | `tiered` (always) |
//! | [`StdBTreeMapBackend`] | `std::collections::BTreeMap` | `tiered` (always) |
//! | [`PdsOrdMapBackend`] | `pds::OrdMap` (B+ tree, structural sharing) | `tiered` (always) |
//! | [`MerkleWrapperBackend`] | `MerkleWrapper<pds::HashMap<K, V>>` | `tiered` + `traits` |
//!
//! All backends implement [`Clone`] and [`Default`].

use super::backend::{CollectionBackend, OrderedCollectionBackend};

// --- StdHashMapBackend ---

/// A [`CollectionBackend`] backed by [`std::collections::HashMap`].
///
/// All operations are O(1) amortised. `Clone` is O(n) — a full deep copy of
/// every entry.
///
/// This is the recommended hot-tier backend when maximum write throughput
/// matters and structural sharing is not required.
#[derive(Clone, Default)]
pub struct StdHashMapBackend<K, V> {
    /// Inner standard-library hash map.
    inner: std::collections::HashMap<K, V>,
}

impl<K, V> StdHashMapBackend<K, V> {
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: std::collections::HashMap::new(),
        }
    }
}

impl<K, V> CollectionBackend<K, V> for StdHashMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Send + 'static,
{
    fn get(&self, key: &K) -> Option<V> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clears the backend and loads the supplied entries.
    ///
    /// Time: O(n) where n is the length of `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        self.inner.clear();
        self.inner.extend(iter);
    }

    /// Drains all entries, leaving the backend empty.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        self.inner.drain().collect()
    }
}

// --- PdsHashMapBackend ---

/// A [`CollectionBackend`] backed by `pds::HashMap` (HAMT with structural sharing).
///
/// `insert` and `remove` use the functional API — each call produces a new map
/// and stores it in place. `Clone` is O(1) via reference-count increment.
///
/// Use this backend as a cold tier when O(1) snapshots or structural sharing
/// between tiers matter.
#[derive(Clone, Default)]
pub struct PdsHashMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    /// Inner pds HAMT map.
    inner: crate::HashMap<K, V>,
}

impl<K, V> PdsHashMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::HashMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::HashMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::HashMap<K, V> {
        &self.inner
    }
}

impl<K, V> CollectionBackend<K, V> for PdsHashMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static,
    V: Clone + std::hash::Hash + Send + Sync + 'static,
{
    fn get(&self, key: &K) -> Option<V> {
        self.inner.get(key).cloned()
    }

    /// Inserts using the pds mutable (CoW) API.
    ///
    /// Uses the mutable `insert` method which performs a single HAMT traversal
    /// and returns the previous value atomically. If the inner map is shared
    /// (e.g. after a `cold_snapshot`), the affected path is copied on write;
    /// if exclusively owned, the mutation happens in place.
    ///
    /// Time: O(log N).
    fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    /// Removes using the pds mutable (CoW) API.
    ///
    /// Uses the mutable `remove` method which performs a single HAMT traversal
    /// and returns the evicted value atomically. CoW semantics apply: the path
    /// is copied only if the map is shared.
    ///
    /// Time: O(log N).
    fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied entries via the functional API.
    ///
    /// Starts from an empty map and inserts each entry in turn.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        let mut map = crate::HashMap::new();
        for (k, v) in iter {
            map = map.update(k, v);
        }
        self.inner = map;
    }

    /// Drains all entries, resetting the backend to an empty map.
    ///
    /// Returns every `(key, value)` pair that was in the map.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        let mut pairs = Vec::with_capacity(self.inner.len());
        pairs.extend(self.inner.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.inner = crate::HashMap::new();
        pairs
    }
}

// --- MerkleWrapperBackend ---

/// A [`CollectionBackend`] backed by `MerkleWrapper<pds::HashMap<K, V>>`.
///
/// Provides the same functional semantics as [`PdsHashMapBackend`] but adds a
/// cached BLAKE3 Merkle root that changes with every mutation. Use this backend
/// when content-addressed identity (e.g. detecting that the cold tier has
/// changed) is needed.
///
/// Only available when both `tiered` and `traits` features are enabled.
#[cfg(feature = "traits")]
#[derive(Clone)]
pub struct MerkleWrapperBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    /// Inner Merkle-wrapped pds HashMap.
    inner: crate::MerkleWrapper<crate::HashMap<K, V>, K, V>,
}

#[cfg(feature = "traits")]
impl<K, V> Default for MerkleWrapperBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    fn default() -> Self {
        Self {
            inner: crate::MerkleWrapper::new(crate::HashMap::new()),
        }
    }
}

#[cfg(feature = "traits")]
impl<K, V> MerkleWrapperBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a reference to the inner `MerkleWrapper`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::MerkleWrapper<crate::HashMap<K, V>, K, V> {
        &self.inner
    }
}

#[cfg(feature = "traits")]
impl<K, V> CollectionBackend<K, V> for MerkleWrapperBackend<K, V>
where
    K: Clone
        + Eq
        + std::hash::Hash
        + std::fmt::Debug
        + serde_core::Serialize
        + Send
        + Sync
        + 'static,
    V: Clone + std::hash::Hash + serde_core::Serialize + Send + Sync + 'static,
{
    fn get(&self, key: &K) -> Option<V> {
        use crate::traits::PersistentMap;
        self.inner.get_cloned(key)
    }

    /// Inserts using the functional API on the wrapped map.
    ///
    /// Creates a new `MerkleWrapper` (clearing the cached root) and stores it in
    /// place. Returns the previous value if present.
    ///
    /// Time: O(log N) for the inner insert; O(1) for the wrapper.
    fn insert(&mut self, key: K, value: V) -> Option<V> {
        use crate::traits::PersistentMap;
        let prev = self.inner.get_cloned(&key);
        self.inner = self.inner.insert(key, value);
        prev
    }

    /// Removes using the functional API on the wrapped map.
    ///
    /// Creates a new `MerkleWrapper` and stores it in place.
    ///
    /// Time: O(log N) for the inner remove; O(1) for the wrapper.
    fn remove(&mut self, key: &K) -> Option<V> {
        use crate::traits::PersistentMap;
        let (new_inner, prev) = self.inner.remove(key);
        self.inner = new_inner;
        prev
    }

    fn len(&self) -> usize {
        use crate::traits::PersistentMap;
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        use crate::traits::PersistentMap;
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied entries.
    ///
    /// Builds a new `pds::HashMap` from the iterator, then wraps it.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        let mut map = crate::HashMap::new();
        for (k, v) in iter {
            map = map.update(k, v);
        }
        self.inner = crate::MerkleWrapper::new(map);
    }

    /// Drains all entries, resetting the backend to an empty wrapped map.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        let inner_map = self.inner.inner();
        let mut pairs = Vec::with_capacity(inner_map.len());
        pairs.extend(inner_map.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.inner = crate::MerkleWrapper::new(crate::HashMap::new());
        pairs
    }
}

// --- StdBTreeMapBackend ---

/// A [`CollectionBackend`] and [`OrderedCollectionBackend`] backed by
/// [`std::collections::BTreeMap`].
///
/// All keyed operations are O(log n). `Clone` is O(n) — a full deep copy.
/// Iteration is always in ascending key order.
///
/// Use this backend as a hot tier when ordered queries (`range`, `iter_ordered`)
/// are needed and structural sharing is not required.
#[derive(Clone, Default)]
pub struct StdBTreeMapBackend<K, V> {
    /// Inner standard-library B-tree map.
    inner: std::collections::BTreeMap<K, V>,
}

impl<K, V> StdBTreeMapBackend<K, V> {
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: std::collections::BTreeMap::new(),
        }
    }
}

impl<K, V> CollectionBackend<K, V> for StdBTreeMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Ord + Send + 'static,
    V: Clone + Send + 'static,
{
    fn get(&self, key: &K) -> Option<V> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clears the backend and loads the supplied entries.
    ///
    /// Time: O(n) where n is the length of `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        self.inner.clear();
        self.inner.extend(iter);
    }

    /// Drains all entries, leaving the backend empty.
    ///
    /// The standard `BTreeMap::into_iter` is used after replacing the inner map
    /// with an empty one.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        let old = std::mem::take(&mut self.inner);
        old.into_iter().collect()
    }
}

impl<K, V> OrderedCollectionBackend<K, V> for StdBTreeMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Ord + Send + 'static,
    V: Clone + Send + 'static,
{
    /// Returns entries whose keys lie within `range`, in ascending order.
    ///
    /// Time: O(log n + k) where k is the number of entries in the range.
    fn range(&self, range: impl std::ops::RangeBounds<K>) -> Vec<(K, V)> {
        self.inner
            .range(range)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Returns all entries in ascending key order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<(K, V)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Returns the smallest key, or `None` if empty.
    ///
    /// Time: O(log n).
    fn first_key(&self) -> Option<K> {
        self.inner.keys().next().cloned()
    }

    /// Returns the largest key, or `None` if empty.
    ///
    /// Time: O(log n).
    fn last_key(&self) -> Option<K> {
        self.inner.keys().next_back().cloned()
    }
}

// --- PdsOrdMapBackend ---

/// A [`CollectionBackend`] and [`OrderedCollectionBackend`] backed by
/// `pds::OrdMap` (a persistent B+ tree with structural sharing).
///
/// All mutations use the functional API — each call produces a new map stored in
/// place. `Clone` is O(1) via reference-count increment.
///
/// Use this backend as a cold tier when O(1) snapshots, structural sharing, and
/// ordered queries are all required.
#[derive(Clone, Default)]
pub struct PdsOrdMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + 'static,
{
    /// Inner pds B+ tree map.
    inner: crate::OrdMap<K, V>,
}

impl<K, V> PdsOrdMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdMap<K, V> {
        &self.inner
    }
}

impl<K, V> CollectionBackend<K, V> for PdsOrdMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Ord + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn get(&self, key: &K) -> Option<V> {
        self.inner.get(key).cloned()
    }

    /// Inserts using the pds mutable (CoW) API.
    ///
    /// A single B+ tree traversal inserts the key and returns the previous value
    /// atomically. CoW semantics apply: the affected path is copied only when
    /// the inner map is shared (e.g. after `cold_snapshot`).
    ///
    /// Time: O(log N).
    fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    /// Removes using the pds mutable (CoW) API.
    ///
    /// A single B+ tree traversal removes the key and returns its value
    /// atomically. CoW semantics apply: the affected path is copied only when
    /// the inner map is shared.
    ///
    /// Time: O(log N).
    fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied entries via the functional API.
    ///
    /// Starts from an empty map and inserts each entry in turn.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        let mut map = crate::OrdMap::new();
        for (k, v) in iter {
            map = map.update(k, v);
        }
        self.inner = map;
    }

    /// Drains all entries, resetting the backend to an empty map.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)> {
        let mut pairs = Vec::with_capacity(self.inner.len());
        pairs.extend(self.inner.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.inner = crate::OrdMap::new();
        pairs
    }
}

impl<K, V> OrderedCollectionBackend<K, V> for PdsOrdMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Ord + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Returns entries whose keys lie within `range`, in ascending order.
    ///
    /// Time: O(log n + k) where k is the number of entries in the range.
    fn range(&self, range: impl std::ops::RangeBounds<K>) -> Vec<(K, V)> {
        self.inner
            .range(range)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Returns all entries in ascending key order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<(K, V)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Returns the smallest key, or `None` if empty.
    ///
    /// Time: O(log N).
    fn first_key(&self) -> Option<K> {
        self.inner.get_min().map(|(k, _)| k.clone())
    }

    /// Returns the largest key, or `None` if empty.
    ///
    /// Time: O(log N).
    fn last_key(&self) -> Option<K> {
        self.inner.get_max().map(|(k, _)| k.clone())
    }
}
