//! Concrete [`SymMapBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsSymMapBackend`] | `pds::SymMap` (HAMT-backed symmetric map) |
//! | [`PdsOrdSymMapBackend`] | `pds::OrdSymMap` (B+ tree-backed symmetric map) |

use super::symmap_backend::{SymMapBackend, SymMapDirection};

// --- PdsSymMapBackend ---

/// A [`SymMapBackend`] backed by `pds::SymMap` (HAMT with structural sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsSymMapBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Inner pds hash symmetric map.
    inner: crate::SymMap<A>,
}

impl<A> PdsSymMapBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::SymMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::SymMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::SymMap<A> {
        &self.inner
    }
}

impl<A> SymMapBackend<A> for PdsSymMapBackend<A>
where
    A: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
{
    fn insert(&mut self, a: A, b: A) {
        self.inner.insert(a, b);
    }

    fn get(&self, dir: SymMapDirection, key: &A) -> Option<A> {
        let pds_dir = to_pds_direction(dir);
        self.inner.get(pds_dir, key).cloned()
    }

    fn contains(&self, dir: SymMapDirection, key: &A) -> bool {
        let pds_dir = to_pds_direction(dir);
        self.inner.contains(pds_dir, key)
    }

    fn remove(&mut self, dir: SymMapDirection, key: &A) -> Option<A> {
        let pds_dir = to_pds_direction(dir);
        self.inner.remove(pds_dir, key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied forward pairs.
    ///
    /// Time: O(n).
    fn load_from(&mut self, iter: impl Iterator<Item = (A, A)>) {
        let mut sm = crate::SymMap::new();
        for (a, b) in iter {
            sm.insert(a, b);
        }
        self.inner = sm;
    }

    /// Drains all forward pairs.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(A, A)> {
        let pairs: Vec<(A, A)> = self
            .inner
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        self.inner = crate::SymMap::new();
        pairs
    }
}

// --- PdsOrdSymMapBackend ---

/// A [`SymMapBackend`] backed by `pds::OrdSymMap` (B+ tree with structural sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
#[derive(Clone, Default)]
pub struct PdsOrdSymMapBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Inner pds ordered symmetric map.
    inner: crate::OrdSymMap<A>,
}

impl<A> PdsOrdSymMapBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdSymMap::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdSymMap`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdSymMap<A> {
        &self.inner
    }
}

impl<A> SymMapBackend<A> for PdsOrdSymMapBackend<A>
where
    A: Clone + Ord + Send + Sync + 'static,
{
    fn insert(&mut self, a: A, b: A) {
        self.inner.insert(a, b);
    }

    fn get(&self, dir: SymMapDirection, key: &A) -> Option<A> {
        let pds_dir = to_pds_direction(dir);
        self.inner.get(pds_dir, key).cloned()
    }

    fn contains(&self, dir: SymMapDirection, key: &A) -> bool {
        let pds_dir = to_pds_direction(dir);
        self.inner.contains(pds_dir, key)
    }

    fn remove(&mut self, dir: SymMapDirection, key: &A) -> Option<A> {
        let pds_dir = to_pds_direction(dir);
        self.inner.remove(pds_dir, key)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied forward pairs.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (A, A)>) {
        let mut sm = crate::OrdSymMap::new();
        for (a, b) in iter {
            sm.insert(a, b);
        }
        self.inner = sm;
    }

    /// Drains all forward pairs in key order.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(A, A)> {
        let pairs: Vec<(A, A)> = self
            .inner
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        self.inner = crate::OrdSymMap::new();
        pairs
    }
}

// --- Direction conversion helper ---

/// Converts a [`SymMapDirection`] to `pds::Direction`.
fn to_pds_direction(dir: SymMapDirection) -> crate::Direction {
    match dir {
        SymMapDirection::Forward => crate::Direction::Forward,
        SymMapDirection::Backward => crate::Direction::Backward,
    }
}
