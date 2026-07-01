//! Concrete [`InsertionOrderMapBackend`] and [`InsertionOrderSetBackend`]
//! implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsInsertionOrderMapBackend`] | `pds::InsertionOrderMap` |
//! | [`PdsInsertionOrderSetBackend`] | `pds::InsertionOrderSet` |
//! | [`PdsOrdInsertionOrderMapBackend`] | `pds::OrdInsertionOrderMap` |
//! | [`PdsOrdInsertionOrderSetBackend`] | `pds::OrdInsertionOrderSet` |

use super::insertion_order_backend::{InsertionOrderMapBackend, InsertionOrderSetBackend};

// --- PdsInsertionOrderMapBackend ---

/// An [`InsertionOrderMapBackend`] backed by `pds::InsertionOrderMap`.
///
/// `Clone` is O(1) via structural sharing.
#[derive(Clone, Default)]
pub struct PdsInsertionOrderMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    inner: crate::InsertionOrderMap<K, V>,
}

impl<K, V> PdsInsertionOrderMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::InsertionOrderMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::InsertionOrderMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::InsertionOrderMap<K, V> {
        &self.inner
    }
}

impl<K, V> InsertionOrderMapBackend<K, V> for PdsInsertionOrderMapBackend<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
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

    fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        for (k, v) in iter {
            self.inner.insert(k, v);
        }
    }

    fn drain(&mut self) -> Vec<(K, V)> {
        let pairs: Vec<(K, V)> = self
            .inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.inner = crate::InsertionOrderMap::new();
        pairs
    }

    fn iter_insertion_order(&self) -> Vec<(K, V)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// --- PdsInsertionOrderSetBackend ---

/// An [`InsertionOrderSetBackend`] backed by `pds::InsertionOrderSet`.
///
/// `Clone` is O(1) via structural sharing.
#[derive(Clone, Default)]
pub struct PdsInsertionOrderSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    inner: crate::InsertionOrderSet<A>,
}

impl<A> PdsInsertionOrderSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::InsertionOrderSet::new(),
        }
    }

    /// Returns a reference to the inner `pds::InsertionOrderSet`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::InsertionOrderSet<A> {
        &self.inner
    }
}

impl<A> InsertionOrderSetBackend<A> for PdsInsertionOrderSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
{
    fn contains(&self, elem: &A) -> bool {
        self.inner.contains(elem)
    }

    fn insert(&mut self, elem: A) -> bool {
        self.inner.insert(elem)
    }

    fn remove(&mut self, elem: &A) -> bool {
        self.inner.remove(elem)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        for elem in iter {
            self.inner.insert(elem);
        }
    }

    fn drain(&mut self) -> Vec<A> {
        let elems: Vec<A> = self.inner.iter().cloned().collect();
        self.inner = crate::InsertionOrderSet::new();
        elems
    }

    fn iter_insertion_order(&self) -> Vec<A> {
        self.inner.iter().cloned().collect()
    }
}

// --- PdsOrdInsertionOrderMapBackend ---

/// An [`InsertionOrderMapBackend`] backed by `pds::OrdInsertionOrderMap`.
///
/// Elements are iterated in insertion order (not sorted order), but the
/// underlying storage uses a B+ tree so keys must be `Ord`. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsOrdInsertionOrderMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + 'static,
{
    inner: crate::OrdInsertionOrderMap<K, V>,
}

impl<K, V> PdsOrdInsertionOrderMapBackend<K, V>
where
    K: Clone + Ord + 'static,
    V: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdInsertionOrderMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdInsertionOrderMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdInsertionOrderMap<K, V> {
        &self.inner
    }
}

impl<K, V> InsertionOrderMapBackend<K, V> for PdsOrdInsertionOrderMapBackend<K, V>
where
    K: Clone + Ord + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
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

    fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        for (k, v) in iter {
            self.inner.insert(k, v);
        }
    }

    fn drain(&mut self) -> Vec<(K, V)> {
        let pairs: Vec<(K, V)> = self
            .inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.inner = crate::OrdInsertionOrderMap::new();
        pairs
    }

    fn iter_insertion_order(&self) -> Vec<(K, V)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// --- PdsOrdInsertionOrderSetBackend ---

/// An [`InsertionOrderSetBackend`] backed by `pds::OrdInsertionOrderSet`.
///
/// Elements are iterated in insertion order, but the underlying storage uses a
/// B+ tree so elements must be `Ord`. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsOrdInsertionOrderSetBackend<A>
where
    A: Clone + Ord + 'static,
{
    inner: crate::OrdInsertionOrderSet<A>,
}

impl<A> PdsOrdInsertionOrderSetBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdInsertionOrderSet::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdInsertionOrderSet`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdInsertionOrderSet<A> {
        &self.inner
    }
}

impl<A> InsertionOrderSetBackend<A> for PdsOrdInsertionOrderSetBackend<A>
where
    A: Clone + Ord + Send + Sync + 'static,
{
    fn contains(&self, elem: &A) -> bool {
        self.inner.contains(elem)
    }

    fn insert(&mut self, elem: A) -> bool {
        self.inner.insert(elem)
    }

    fn remove(&mut self, elem: &A) -> bool {
        self.inner.remove(elem)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        for elem in iter {
            self.inner.insert(elem);
        }
    }

    fn drain(&mut self) -> Vec<A> {
        let elems: Vec<A> = self.inner.iter().cloned().collect();
        self.inner = crate::OrdInsertionOrderSet::new();
        elems
    }

    fn iter_insertion_order(&self) -> Vec<A> {
        self.inner.iter().cloned().collect()
    }
}
