//! Traits for insertion-order-preserving backends.
//!
//! [`InsertionOrderMapBackend`] and [`InsertionOrderSetBackend`] extend the
//! standard collection/set backend protocols with an `iter_insertion_order`
//! method that returns elements in the order they were first inserted.

/// A mutable map backend that preserves insertion order.
///
/// Extends the standard key-value operations with
/// [`iter_insertion_order`][InsertionOrderMapBackend::iter_insertion_order],
/// which iterates pairs in the order they were first inserted (not updated).
///
/// Used as a tier in
/// [`TieredInsertionOrderMap`][super::insertion_order::TieredInsertionOrderMap].
pub trait InsertionOrderMapBackend<K, V>: Send + 'static
where
    K: Clone,
    V: Clone,
{
    /// Returns the value associated with `key`, if present.
    ///
    /// Time: O(1) amortised.
    fn get(&self, key: &K) -> Option<V>;

    /// Inserts `(key, value)`, returning the previous value if the key existed.
    ///
    /// If the key already exists, the value is updated but the insertion
    /// position is **not** changed — the key retains its original position in
    /// iteration order.
    ///
    /// Time: O(1) amortised.
    fn insert(&mut self, key: K, value: V) -> Option<V>;

    /// Removes `key`, returning the previous value.
    ///
    /// Time: O(1) amortised.
    fn remove(&mut self, key: &K) -> Option<V>;

    /// Tests whether `key` is present.
    ///
    /// Time: O(1) amortised.
    fn contains_key(&self, key: &K) -> bool;

    /// Returns the number of entries.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the backend contains no entries.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Replaces the backend's contents with the supplied pairs, in iterator
    /// order.
    ///
    /// Unlike many collection backends, this operation **appends** in iterator
    /// order — later duplicates update the value but do not shift position.
    ///
    /// Time: O(n).
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>);

    /// Drains all entries in insertion order, returning them as a `Vec`.
    ///
    /// The backend is empty after this call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)>;

    /// Returns all entries in insertion order.
    ///
    /// Time: O(n).
    fn iter_insertion_order(&self) -> Vec<(K, V)>;
}

/// A mutable set backend that preserves insertion order.
///
/// Extends the standard set operations with
/// [`iter_insertion_order`][InsertionOrderSetBackend::iter_insertion_order],
/// which iterates elements in the order they were first inserted.
///
/// Used as a tier in
/// [`TieredInsertionOrderSet`][super::insertion_order::TieredInsertionOrderSet].
pub trait InsertionOrderSetBackend<A>: Send + 'static
where
    A: Clone,
{
    /// Tests whether `elem` is present.
    ///
    /// Time: O(1) amortised.
    fn contains(&self, elem: &A) -> bool;

    /// Inserts `elem`, returning `true` if it was newly inserted.
    ///
    /// Time: O(1) amortised.
    fn insert(&mut self, elem: A) -> bool;

    /// Removes `elem`, returning `true` if it was present.
    ///
    /// Time: O(1) amortised.
    fn remove(&mut self, elem: &A) -> bool;

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the backend contains no elements.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Replaces the backend's contents with the supplied elements, in iterator
    /// order. Duplicate elements are silently ignored.
    ///
    /// Time: O(n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>);

    /// Drains all elements in insertion order, returning them as a `Vec`.
    ///
    /// The backend is empty after this call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A>;

    /// Returns all elements in insertion order.
    ///
    /// Time: O(n).
    fn iter_insertion_order(&self) -> Vec<A>;
}
