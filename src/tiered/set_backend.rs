//! [`SetBackend<A>`] and [`OrderedSetBackend<A>`] — the traits implemented by
//! every tier in a [`TieredSet`][super::set::TieredSet].

/// A mutable set store that can serve as one tier in a
/// [`TieredSet`][super::set::TieredSet].
///
/// Mirrors [`CollectionBackend`][super::backend::CollectionBackend] for
/// key-value maps, but stores only values (no associated value per element).
///
/// # Implementor notes
///
/// - `Send + 'static` is required so that a `TieredSet` (which holds backends
///   behind an `Arc<Mutex<…>>`) can be shared across threads.
/// - `drain` must leave the backend empty.
/// - `load_from` must **replace** the backend's contents (clear then insert).
pub trait SetBackend<A>: Send + 'static
where
    A: Clone,
{
    /// Tests whether `value` is present in the backend.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn contains(&self, value: &A) -> bool;

    /// Inserts `value` into the backend.
    ///
    /// Returns `true` if the element was newly inserted, `false` if it was
    /// already present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn insert(&mut self, value: A) -> bool;

    /// Removes `value` from the backend.
    ///
    /// Returns `true` if the element was present (and thus removed), `false`
    /// if it was absent.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn remove(&mut self, value: &A) -> bool;

    /// Returns the number of elements currently stored.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the backend contains no elements.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Bulk-replaces the backend's contents from an iterator.
    ///
    /// Clears prior state, then inserts every element from `iter`. Duplicates
    /// in `iter` are silently deduplicated (set semantics).
    ///
    /// Time: O(n) where n is the length of `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = A>);

    /// Drains all elements from the backend, leaving it empty.
    ///
    /// Returns every element that was present before the call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A>;
}

/// An ordered set backend that extends [`SetBackend`] with range queries and
/// ordered iteration.
///
/// Implement this trait on backends that wrap ordered sets (`BTreeSet`,
/// `pds::OrdSet`) to enable [`TieredSetOrdExt`][super::set::TieredSetOrdExt]
/// methods on a `TieredSet` whose both tiers implement it.
///
/// # Implementor notes
///
/// All returned `Vec`s are in **ascending element order**. `A` must additionally
/// implement [`Ord`] because ordered iteration and range queries require a total
/// order.
pub trait OrderedSetBackend<A>: SetBackend<A>
where
    A: Clone + Ord,
{
    /// Returns all elements in ascending order.
    ///
    /// Time: O(n).
    fn iter_ordered(&self) -> Vec<A>;

    /// Returns all elements whose values lie within `range`, in ascending order.
    ///
    /// Time: O(log n + k) where k is the number of elements in the range.
    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<A>;

    /// Returns the smallest element, or `None` if the backend is empty.
    ///
    /// Time: O(log n).
    fn first(&self) -> Option<A>;

    /// Returns the largest element, or `None` if the backend is empty.
    ///
    /// Time: O(log n).
    fn last(&self) -> Option<A>;
}
