//! Concrete [`BagBackend`] and [`OrderedBagBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsBagBackend`] | `pds::Bag` (HAMT-backed multiset) |
//! | [`PdsOrdBagBackend`] | `pds::OrdBag` (B+ tree-backed ordered multiset) |

use super::bag_backend::{BagBackend, OrderedBagBackend};

// --- PdsBagBackend ---

/// A [`BagBackend`] backed by `pds::Bag` (HAMT with structural sharing).
///
/// All operations use the pds functional API. `Clone` is O(1) via
/// reference-count increment.
///
/// Use this backend as either the hot or cold tier when structural sharing and
/// O(1) clones of the bag state are needed.
#[derive(Clone, Default)]
pub struct PdsBagBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Inner pds multiset.
    inner: crate::Bag<A>,
}

impl<A> PdsBagBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::Bag::new(),
        }
    }

    /// Returns a reference to the inner `pds::Bag`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::Bag<A> {
        &self.inner
    }
}

impl<A> BagBackend<A> for PdsBagBackend<A>
where
    A: Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static,
{
    /// Inserts one occurrence of `value`.
    ///
    /// Time: O(log n).
    fn insert(&mut self, value: A) {
        self.inner.insert(value);
    }

    /// Inserts `count` occurrences of `value` in a single functional update.
    ///
    /// Delegates to `pds::Bag::insert_many`, which increments the element's
    /// count in one HAMT path-copy rather than making `count` separate updates.
    ///
    /// A `count` of 0 is a no-op.
    ///
    /// Time: O(log n) regardless of `count`.
    fn insert_many(&mut self, value: A, count: usize) {
        if count > 0 {
            self.inner.insert_many(value, count);
        }
    }

    /// Removes one occurrence of `value`.
    ///
    /// Returns `false` if absent.
    ///
    /// Time: O(log n).
    fn remove(&mut self, value: &A) -> bool {
        let prev = self.inner.count(value);
        if prev == 0 {
            return false;
        }
        self.inner.remove(value);
        true
    }

    fn count(&self, value: &A) -> usize {
        self.inner.count(value)
    }

    fn len(&self) -> usize {
        self.inner.total_count()
    }

    fn is_empty(&self) -> bool {
        self.inner.total_count() == 0
    }

    /// Replaces the backend with the supplied `(element, count)` pairs.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (A, usize)>) {
        let mut bag = crate::Bag::new();
        for (elem, count) in iter {
            if count > 0 {
                bag.insert_many(elem, count);
            }
        }
        self.inner = bag;
    }

    /// Drains all `(element, count)` pairs.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(A, usize)> {
        let pairs: Vec<(A, usize)> = self.inner.iter().map(|(a, c)| (a.clone(), c)).collect();
        self.inner = crate::Bag::new();
        pairs
    }
}

// --- PdsOrdBagBackend ---

/// A [`BagBackend`] and [`OrderedBagBackend`] backed by `pds::OrdBag` (a
/// persistent B+ tree-backed ordered multiset with structural sharing).
///
/// All operations use the pds functional API. `Clone` is O(1).
///
/// Use this backend when O(1) snapshots, structural sharing, and ordered
/// range queries over the multiset are all required.
#[derive(Clone, Default)]
pub struct PdsOrdBagBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Inner pds ordered multiset.
    inner: crate::OrdBag<A>,
}

impl<A> PdsOrdBagBackend<A>
where
    A: Clone + Ord + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::OrdBag::new(),
        }
    }

    /// Returns a reference to the inner `pds::OrdBag`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::OrdBag<A> {
        &self.inner
    }
}

impl<A> BagBackend<A> for PdsOrdBagBackend<A>
where
    A: Clone + Ord + Send + Sync + 'static,
{
    /// Inserts one occurrence of `value`.
    ///
    /// Time: O(log n).
    fn insert(&mut self, value: A) {
        self.inner.insert(value);
    }

    /// Inserts `count` occurrences of `value` in a single functional update.
    ///
    /// Delegates to `pds::OrdBag::insert_many`, which increments the element's
    /// count in one B+ tree path-copy rather than making `count` separate updates.
    ///
    /// A `count` of 0 is a no-op.
    ///
    /// Time: O(log n) regardless of `count`.
    fn insert_many(&mut self, value: A, count: usize) {
        if count > 0 {
            self.inner.insert_many(value, count);
        }
    }

    /// Removes one occurrence of `value`.
    ///
    /// Returns `false` if absent.
    ///
    /// Time: O(log n).
    fn remove(&mut self, value: &A) -> bool {
        let prev = self.inner.count(value);
        if prev == 0 {
            return false;
        }
        self.inner.remove(value);
        true
    }

    fn count(&self, value: &A) -> usize {
        self.inner.count(value)
    }

    fn len(&self) -> usize {
        self.inner.total_count()
    }

    fn is_empty(&self) -> bool {
        self.inner.total_count() == 0
    }

    /// Replaces the backend with the supplied `(element, count)` pairs.
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = (A, usize)>) {
        let mut bag = crate::OrdBag::new();
        for (elem, count) in iter {
            if count > 0 {
                bag.insert_many(elem, count);
            }
        }
        self.inner = bag;
    }

    /// Drains all `(element, count)` pairs in ascending element order.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(A, usize)> {
        let pairs: Vec<(A, usize)> = self.inner.iter().map(|(a, c)| (a.clone(), c)).collect();
        self.inner = crate::OrdBag::new();
        pairs
    }
}

impl<A> OrderedBagBackend<A> for PdsOrdBagBackend<A>
where
    A: Clone + Ord + Send + Sync + 'static,
{
    /// Returns all `(element, count)` pairs in ascending element order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<(A, usize)> {
        self.inner.iter().map(|(a, c)| (a.clone(), c)).collect()
    }

    /// Returns `(element, count)` pairs whose element values lie within
    /// `range`, in ascending order.
    ///
    /// Time: O(log n + k).
    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<(A, usize)> {
        self.inner
            .range(range)
            .map(|(a, c)| (a.clone(), c))
            .collect()
    }
}
