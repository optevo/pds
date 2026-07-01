//! [`MultiMapBackend<K, V>`] — the trait implemented by every tier in a
//! [`TieredMultiMap`][super::multimap::TieredMultiMap].

/// A mutable multimap store that can serve as one tier in a
/// [`TieredMultiMap`][super::multimap::TieredMultiMap].
///
/// A multimap maps each key to a **set** of values (no duplicate (key, value)
/// pairs, but a key may map to multiple distinct values). `len` returns the
/// total number of distinct (key, value) pairs.
///
/// # Implementor notes
///
/// - `Send + 'static` is required so that a `TieredMultiMap` (which holds
///   backends behind an `Arc<Mutex<…>>`) can be shared across threads.
/// - `drain` must leave the backend empty and return all (key, value) pairs.
/// - `load_from` takes (key, value) pairs and **replaces** prior state.
pub trait MultiMapBackend<K, V>: Send + 'static
where
    K: Clone,
    V: Clone,
{
    /// Inserts a (key, value) pair.
    ///
    /// Has no effect if the exact pair already exists (set semantics per key).
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn insert(&mut self, key: K, value: V);

    /// Removes a single (key, value) pair.
    ///
    /// Returns `true` if the pair was present and removed. If the key's value
    /// set becomes empty, the key is removed entirely.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn remove_entry(&mut self, key: &K, value: &V) -> bool;

    /// Removes all values associated with `key`.
    ///
    /// Returns `true` if the key was present (and its values were removed).
    ///
    /// Time: O(k) where k is the number of values for `key`.
    fn remove_key(&mut self, key: &K) -> bool;

    /// Returns all values associated with `key`, in an unspecified order.
    ///
    /// Returns an empty `Vec` if the key is absent.
    ///
    /// Time: O(k) where k is the number of values for `key`.
    fn get_all(&self, key: &K) -> Vec<V>;

    /// Tests whether the exact (key, value) pair is present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    fn contains(&self, key: &K, value: &V) -> bool;

    /// Returns the total number of (key, value) pairs across all keys.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the multimap contains no (key, value) pairs.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Bulk-replaces the backend's contents from (key, value) pairs.
    ///
    /// Clears prior state, then inserts each pair.
    ///
    /// Time: O(n) where n is the length of `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>);

    /// Drains all (key, value) pairs, leaving the backend empty.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)>;
}
