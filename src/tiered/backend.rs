//! [`CollectionBackend<K, V>`] — the trait implemented by every tier in a
//! [`TieredCollection`][super::TieredCollection].
//!
//! A backend is a mutable, self-contained store for key-value pairs. It does
//! not need to be persistent or thread-safe on its own — thread safety comes
//! from the `Arc<Mutex<…>>` wrapping inside `TieredCollection`.

/// A mutable key-value store that can serve as one tier in a
/// [`TieredCollection`][super::TieredCollection].
///
/// All concrete backends in this crate implement this trait:
/// [`StdHashMapBackend`][super::backends::StdHashMapBackend],
/// [`PdsHashMapBackend`][super::backends::PdsHashMapBackend], and
/// (behind the `traits` feature)
/// [`MerkleWrapperBackend`][super::backends::MerkleWrapperBackend].
///
/// # Implementor notes
///
/// - `send + 'static` is required so that a `TieredCollection` (which holds
///   backends behind an `Arc<Mutex<…>>`) can be shared across threads.
/// - `drain` must leave the backend empty and return all entries that were
///   present before the call. `load_from` must replace the backend's contents
///   with the supplied iterator (clearing prior state).
pub trait CollectionBackend<K, V>: Send + 'static
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
{
    /// Returns a clone of the value for `key`, or `None` if absent.
    ///
    /// Time: depends on backend; O(1) amortised for hash-based backends.
    fn get(&self, key: &K) -> Option<V>;

    /// Inserts `key` → `value`, returning the previous value if `key` was
    /// already present.
    ///
    /// Time: depends on backend; O(1) amortised for hash-based backends,
    /// O(log N) for HAMT-based backends.
    fn insert(&mut self, key: K, value: V) -> Option<V>;

    /// Removes `key` and returns the associated value, or `None` if absent.
    ///
    /// Time: depends on backend; O(1) amortised for hash-based backends.
    fn remove(&mut self, key: &K) -> Option<V>;

    /// Returns the number of key-value pairs currently stored.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the backend contains no key-value pairs.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Bulk-replaces the backend's contents from an iterator.
    ///
    /// Called during propagation to update the cold tier from the hot tier's
    /// accumulated state. Any entries that were previously in the backend but
    /// are not in `iter` are replaced (i.e., this is **not** a merge — it is a
    /// load that first clears the backend).
    ///
    /// Implementations must clear prior state before inserting iterator items.
    ///
    /// Time: O(n) where n is the length of `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>);

    /// Drains all entries from the backend, leaving it empty.
    ///
    /// Returns every `(key, value)` pair that was in the backend before the
    /// call. After `drain`, the backend behaves as if newly constructed.
    ///
    /// Time: O(n) where n is the number of entries drained.
    fn drain(&mut self) -> Vec<(K, V)>;

    /// Returns a clone of this backend as an owned value.
    ///
    /// Used to snapshot the cold tier without flushing. Provided as a default
    /// that delegates to [`Clone`]; backends that are not `Clone` must not call
    /// this method.
    ///
    /// Time: O(1) for HAMT-backed pds types (structural sharing); O(n) for
    /// `StdHashMapBackend` (deep copy).
    fn snapshot(&self) -> Self
    where
        Self: Sized + Clone,
    {
        self.clone()
    }
}
