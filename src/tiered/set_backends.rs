//! Concrete [`SetBackend`] and [`OrderedSetBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`StdHashSetBackend`] | `std::collections::HashSet` |
//! | [`PdsHashSetBackend`] | `pds::HashSet` (HAMT, structural sharing) |
//! | [`StdBTreeSetBackend`] | `std::collections::BTreeSet` |
//! | [`PdsOrdSetBackend`] | `pds::OrdSet` (B+ tree, structural sharing) |

use super::set_backend::{OrderedSetBackend, SetBackend};

// --- StdHashSetBackend ---

/// A [`SetBackend`] backed by [`std::collections::HashSet`].
///
/// All operations are O(1) amortised. `Clone` is O(n) — a full deep copy.
///
/// Recommended as a hot-tier set backend when maximum write throughput matters
/// and structural sharing is not required.
#[derive(Clone, Default)]
pub struct StdHashSetBackend<A> {
    /// Inner standard-library hash set.
    inner: std::collections::HashSet<A>,
}

impl<A> StdHashSetBackend<A> {
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: std::collections::HashSet::new(),
        }
    }
}

impl<A> SetBackend<A> for StdHashSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
{
    fn contains(&self, value: &A) -> bool {
        self.inner.contains(value)
    }

    fn insert(&mut self, value: A) -> bool {
        self.inner.insert(value)
    }

    fn remove(&mut self, value: &A) -> bool {
        self.inner.remove(value)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clears the backend and loads the supplied elements.
    ///
    /// Time: O(n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        self.inner.clear();
        self.inner.extend(iter);
    }

    /// Drains all elements, leaving the backend empty.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        self.inner.drain().collect()
    }
}

// --- PdsHashSetBackend ---

/// A [`SetBackend`] backed by `pds::HashSet` (HAMT with structural sharing).
///
/// `insert` and `remove` use the functional API — each call produces a new set
/// stored in place. `Clone` is O(1) via reference-count increment.
///
/// Use this backend as a cold tier when O(1) snapshots or structural sharing matter.
#[derive(Clone, Default)]
pub struct PdsHashSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Inner pds HAMT set.
    inner: crate::HashSet<A>,
}

impl<A> PdsHashSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::HashSet::new(),
        }
    }

    /// Returns a reference to the inner `pds::HashSet`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::HashSet<A> {
        &self.inner
    }
}

impl<A> SetBackend<A> for PdsHashSetBackend<A>
where
    A: Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static,
{
    fn contains(&self, value: &A) -> bool {
        self.inner.contains(value)
    }

    /// Inserts using the pds mutable (CoW) API.
    ///
    /// A single HAMT traversal inserts the element and returns whether it was
    /// newly added. CoW semantics apply: the affected path is copied only when
    /// the inner set is shared (e.g. after `cold_snapshot`).
    ///
    /// Returns `true` if the element was newly inserted.
    ///
    /// Time: O(log n).
    fn insert(&mut self, value: A) -> bool {
        self.inner.insert(value).is_none()
    }

    /// Removes using the pds mutable (CoW) API.
    ///
    /// A single HAMT traversal removes the element. CoW semantics apply.
    ///
    /// Returns `true` if the element was present.
    ///
    /// Time: O(log n).
    fn remove(&mut self, value: &A) -> bool {
        self.inner.remove(value).is_some()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied elements via the functional API.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        let mut set = crate::HashSet::new();
        for a in iter {
            set = set.update(a);
        }
        self.inner = set;
    }

    /// Drains all elements, resetting the backend to an empty set.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        let mut elems = Vec::with_capacity(self.inner.len());
        elems.extend(self.inner.iter().cloned());
        self.inner = crate::HashSet::new();
        elems
    }
}

// --- StdBTreeSetBackend ---

/// A [`SetBackend`] and [`OrderedSetBackend`] backed by
/// [`std::collections::BTreeSet`].
///
/// All operations are O(log n). `Clone` is O(n). Iteration is in ascending order.
///
/// Use this backend when ordered queries (`range`, `iter_ordered`) are needed
/// and structural sharing is not required.
#[derive(Clone, Default)]
pub struct StdBTreeSetBackend<A> {
    /// Inner standard-library B-tree set.
    inner: std::collections::BTreeSet<A>,
}

impl<A> StdBTreeSetBackend<A> {
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: std::collections::BTreeSet::new(),
        }
    }
}

impl<A> SetBackend<A> for StdBTreeSetBackend<A>
where
    A: Clone + Ord + Send + 'static,
{
    fn contains(&self, value: &A) -> bool {
        self.inner.contains(value)
    }

    fn insert(&mut self, value: A) -> bool {
        self.inner.insert(value)
    }

    fn remove(&mut self, value: &A) -> bool {
        self.inner.remove(value)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clears the backend and loads the supplied elements.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        self.inner.clear();
        self.inner.extend(iter);
    }

    /// Drains all elements in ascending order.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        let old = std::mem::take(&mut self.inner);
        old.into_iter().collect()
    }
}

impl<A> OrderedSetBackend<A> for StdBTreeSetBackend<A>
where
    A: Clone + Ord + Send + 'static,
{
    /// Returns all elements in ascending order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<A> {
        self.inner.iter().cloned().collect()
    }

    /// Returns elements within `range` in ascending order.
    ///
    /// Time: O(log n + k).
    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<A> {
        self.inner.range(range).cloned().collect()
    }

    /// Returns the smallest element, or `None` if empty.
    ///
    /// Time: O(log n).
    fn first(&self) -> Option<A> {
        self.inner.iter().next().cloned()
    }

    /// Returns the largest element, or `None` if empty.
    ///
    /// Time: O(log n).
    fn last(&self) -> Option<A> {
        self.inner.iter().next_back().cloned()
    }
}

// --- PdsOrdSetBackend ---

/// A [`SetBackend`] and [`OrderedSetBackend`] backed by `pds::OrdSet` (a
/// persistent B+ tree with structural sharing).
///
/// All mutations use the functional API — each call produces a new set stored
/// in place. `Clone` is O(1) via reference-count increment.
///
/// Use this backend as a cold tier when O(1) snapshots, structural sharing,
/// and ordered queries are all required.
#[derive(Clone, Default)]
pub struct PdsOrdSetBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Inner pds B+ tree set.
    inner: crate::OrdSet<A>,
}

impl<A> PdsOrdSetBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdSet::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdSet`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdSet<A> {
        &self.inner
    }
}

impl<A> SetBackend<A> for PdsOrdSetBackend<A>
where
    A: Clone + Ord + Send + Sync + 'static,
{
    fn contains(&self, value: &A) -> bool {
        self.inner.contains(value)
    }

    /// Inserts using the pds mutable (CoW) API.
    ///
    /// A single B+ tree traversal inserts the element and returns whether it was
    /// newly added. CoW semantics apply: the affected path is copied only when
    /// the inner set is shared (e.g. after `cold_snapshot`).
    ///
    /// Time: O(log n).
    fn insert(&mut self, value: A) -> bool {
        self.inner.insert(value).is_none()
    }

    /// Removes using the pds mutable (CoW) API.
    ///
    /// A single B+ tree traversal removes the element. CoW semantics apply.
    ///
    /// Time: O(log n).
    fn remove(&mut self, value: &A) -> bool {
        self.inner.remove(value).is_some()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Replaces the backend with the supplied elements via the functional API.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        let mut set = crate::OrdSet::new();
        for a in iter {
            set = set.update(a);
        }
        self.inner = set;
    }

    /// Drains all elements, resetting the backend to an empty set.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        let mut elems = Vec::with_capacity(self.inner.len());
        elems.extend(self.inner.iter().cloned());
        self.inner = crate::OrdSet::new();
        elems
    }
}

impl<A> OrderedSetBackend<A> for PdsOrdSetBackend<A>
where
    A: Clone + Ord + Send + Sync + 'static,
{
    /// Returns all elements in ascending order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<A> {
        self.inner.iter().cloned().collect()
    }

    /// Returns elements within `range` in ascending order.
    ///
    /// Time: O(log n + k).
    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<A> {
        self.inner.range(range).cloned().collect()
    }

    /// Returns the smallest element, or `None` if empty.
    ///
    /// Time: O(log n).
    fn first(&self) -> Option<A> {
        self.inner.get_min().cloned()
    }

    /// Returns the largest element, or `None` if empty.
    ///
    /// Time: O(log n).
    fn last(&self) -> Option<A> {
        self.inner.get_max().cloned()
    }
}
