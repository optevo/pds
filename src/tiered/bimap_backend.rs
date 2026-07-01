//! [`BiMapBackend`] — the trait that makes a bijection map usable as a tier in
//! a [`TieredBiMap`][super::bimap::TieredBiMap].

/// A mutable bijection-map backend for use as a tier in
/// [`TieredBiMap`][super::bimap::TieredBiMap].
///
/// A bijection maps each key to exactly one value **and** each value to exactly
/// one key. Implementors must uphold this invariant — inserting a pair that
/// would violate it must displace the conflicting pair(s) rather than create
/// duplicate associations.
///
/// # Bijection invariant (important note for tiered use)
///
/// When a [`TieredBiMap`][super::bimap::TieredBiMap] is used with a
/// [`Manual`][super::policy::PropagationPolicy::Manual] or
/// [`Batched`][super::policy::PropagationPolicy::Batched] policy, the hot tier
/// may transiently violate the bijection with respect to cold tier state. The
/// bijection is guaranteed globally only after a flush.
pub trait BiMapBackend<K, V>: Send + 'static
where
    K: Clone,
    V: Clone,
{
    /// Inserts a `(key, value)` pair, displacing any existing pair for `key`.
    ///
    /// Returns the previous value associated with `key`, if any.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn insert(&mut self, key: K, value: V) -> Option<V>;

    /// Returns a clone of the value associated with `key`, if present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn get_by_key(&self, key: &K) -> Option<V>;

    /// Returns a clone of the key associated with `value`, if present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn get_by_value(&self, value: &V) -> Option<K>;

    /// Tests whether `key` is present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn contains_key(&self, key: &K) -> bool;

    /// Tests whether `value` is present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn contains_value(&self, value: &V) -> bool;

    /// Removes the pair associated with `key`, returning the displaced value.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn remove_by_key(&mut self, key: &K) -> Option<V>;

    /// Removes the pair associated with `value`, returning the displaced key.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn remove_by_value(&mut self, value: &V) -> Option<K>;

    /// Returns the number of (key, value) pairs.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the backend contains no pairs.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Replaces the backend's contents with the supplied pairs.
    ///
    /// Callers are responsible for ensuring the input is a valid bijection
    /// (no duplicate keys, no duplicate values). If duplicates are present the
    /// last writer wins, consistent with the backend's `insert` semantics.
    ///
    /// Time: O(n log n) for ordered backends; O(n) amortised for hash backends.
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>);

    /// Drains all pairs from the backend, returning them as a `Vec`.
    ///
    /// The backend is empty after this call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(K, V)>;
}
