//! [`SymMapBackend`] — the trait that makes a symmetric map usable as a tier
//! in a [`TieredSymMap`][super::symmap::TieredSymMap].

/// Direction for lookups and removals on a [`SymMapBackend`].
///
/// Mirrors [`pds::Direction`][crate::Direction] so that `SymMapBackend`
/// implementations can use the same direction semantics without depending on
/// the outer API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymMapDirection {
    /// Forward: look up by the first element of the pair (`a → b`).
    Forward,
    /// Backward: look up by the second element of the pair (`b → a`).
    Backward,
}

/// A mutable symmetric-map backend for use as a tier in
/// [`TieredSymMap`][super::symmap::TieredSymMap].
///
/// A symmetric map stores ordered pairs `(a, b)` and supports lookup in both
/// directions. It is symmetric in the sense that `insert(a, b)` and
/// `insert(b, a)` are semantically equivalent — either form makes the pair
/// look-up-able from both sides.
pub trait SymMapBackend<A>: Send + 'static
where
    A: Clone,
{
    /// Inserts the pair `(a, b)`, enabling lookup in both directions.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn insert(&mut self, a: A, b: A);

    /// Returns a clone of the value associated with `key` in `dir`.
    ///
    /// `Forward` looks up `key` as the first element of pairs; `Backward` as
    /// the second.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn get(&self, dir: SymMapDirection, key: &A) -> Option<A>;

    /// Tests whether `key` is present in the given direction.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn contains(&self, dir: SymMapDirection, key: &A) -> bool;

    /// Removes the pair associated with `key` in `dir`, returning the partner.
    ///
    /// Returns `None` if the key is not present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    fn remove(&mut self, dir: SymMapDirection, key: &A) -> Option<A>;

    /// Returns the number of pairs.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the backend contains no pairs.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Replaces the backend's contents with the supplied pairs.
    ///
    /// Each `(a, b)` in the iterator is inserted as a forward pair.
    ///
    /// Time: O(n log n) for ordered backends; O(n) amortised for hash backends.
    fn load_from(&mut self, iter: impl Iterator<Item = (A, A)>);

    /// Drains all pairs from the backend, returning them as a `Vec`.
    ///
    /// Each returned `(a, b)` is a forward pair. The backend is empty after this
    /// call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<(A, A)>;
}
