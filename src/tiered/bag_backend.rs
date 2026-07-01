//! [`BagBackend<A>`] and [`OrderedBagBackend<A>`] — the traits implemented by
//! every tier in a [`TieredBag`][super::bag::TieredBag].
//!
//! A bag (multiset) allows duplicate elements and tracks the count of each
//! distinct element. The `BagBackend` trait models this as an abstract store
//! of `(element, count)` pairs.

/// A mutable multiset (bag) store that can serve as one tier in a
/// [`TieredBag`][super::bag::TieredBag].
///
/// Each element can appear multiple times; the backend tracks the count
/// per distinct element. `len` returns the **total count** across all elements
/// (with multiplicity), not the number of distinct elements.
///
/// # Implementor notes
///
/// - `Send + 'static` is required so that a `TieredBag` (which holds backends
///   behind an `Arc<Mutex<…>>`) can be shared across threads.
/// - `drain` must leave the backend empty and return `(element, count)` pairs.
/// - `load_from` takes `(element, count)` pairs and **replaces** prior state.
pub trait BagBackend<A>: Send + 'static
where
    A: Clone,
{
    /// Inserts one occurrence of `value`, incrementing its count.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn insert(&mut self, value: A);

    /// Removes one occurrence of `value`.
    ///
    /// Returns `false` if `value` was absent (no change made). Returns `true`
    /// if one occurrence was removed (the count decrements by 1; the element
    /// is removed entirely when count reaches 0).
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn remove(&mut self, value: &A) -> bool;

    /// Returns the current count of `value`.
    ///
    /// Returns 0 if `value` is absent.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn count(&self, value: &A) -> usize;

    /// Tests whether `value` has a non-zero count.
    ///
    /// Equivalent to `self.count(value) > 0`.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn contains(&self, value: &A) -> bool {
        self.count(value) > 0
    }

    /// Returns the total count of all elements (with multiplicity).
    ///
    /// This is the sum of all individual element counts — not the number of
    /// distinct elements.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the total count is zero.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Bulk-replaces the backend's contents from `(element, count)` pairs.
    ///
    /// Clears prior state, then sets each element's count as given. Elements
    /// with count 0 in `iter` are skipped.
    ///
    /// Time: O(n) where n is the length of `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = (A, usize)>);

    /// Drains all `(element, count)` pairs from the backend, leaving it empty.
    ///
    /// Each pair represents a distinct element and its count; no pair has
    /// count 0.
    ///
    /// Time: O(n) where n is the number of distinct elements.
    fn drain(&mut self) -> Vec<(A, usize)>;
}

/// An ordered bag backend that extends [`BagBackend`] with range queries and
/// ordered iteration.
///
/// Implement this on backends wrapping `pds::OrdBag` to enable range queries
/// on `TieredBag` when both tiers implement this trait.
pub trait OrderedBagBackend<A>: BagBackend<A>
where
    A: Clone + Ord,
{
    /// Returns all `(element, count)` pairs in ascending element order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<(A, usize)>;

    /// Returns `(element, count)` pairs whose element values lie within
    /// `range`, in ascending order.
    ///
    /// Time: O(log n + k).
    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<(A, usize)>;
}
