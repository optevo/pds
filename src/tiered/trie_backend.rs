//! [`TrieBackend`] — the trait that makes a trie usable as a tier in a
//! [`TieredTrie`][super::trie::TieredTrie].

/// A mutable trie backend for use as a tier in [`TieredTrie`][super::trie::TieredTrie].
///
/// Keys are sequences of key segments — `Vec<K>` — rather than single scalar
/// values. This allows prefix queries: [`prefix_get`][TrieBackend::prefix_get]
/// returns all entries whose key starts with a given prefix.
pub trait TrieBackend<K, V>: Send + 'static
where
    K: Clone,
    V: Clone,
{
    /// Returns the value stored at `key`, if present.
    ///
    /// Time: O(d) where d is the depth of `key` in the trie.
    fn get(&self, key: &[K]) -> Option<V>;

    /// Inserts `(key, value)`, returning the previous value.
    ///
    /// Time: O(d).
    fn insert(&mut self, key: Vec<K>, value: V) -> Option<V>;

    /// Removes the entry at `key`, returning the previous value.
    ///
    /// Time: O(d).
    fn remove(&mut self, key: &[K]) -> Option<V>;

    /// Returns all `(path, value)` pairs whose path starts with `prefix`.
    ///
    /// Paths are returned as owned `Vec<K>` (full path, not relative to
    /// prefix). Returns an empty `Vec` if no entries exist under `prefix`.
    ///
    /// Time: O(d + m) where m is the number of entries under the prefix.
    fn prefix_get(&self, prefix: &[K]) -> Vec<(Vec<K>, V)>;

    /// Returns all `(path, value)` pairs in the trie.
    ///
    /// Time: O(n).
    fn iter_all(&self) -> Vec<(Vec<K>, V)>;

    /// Returns the number of values in the trie.
    ///
    /// Time: O(n).
    fn len(&self) -> usize;

    /// Tests whether the trie contains no values.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Replaces the backend's contents with the supplied (path, value) pairs.
    ///
    /// Time: O(n × d).
    fn load_from(&mut self, iter: impl Iterator<Item = (Vec<K>, V)>);

    /// Drains all entries, returning them as a `Vec`.
    ///
    /// The backend is empty after this call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(Vec<K>, V)>;
}
