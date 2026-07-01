//! [`UniqueVecBackend`] — the trait that makes a unique-vector usable as a tier
//! in a [`TieredUniqueVector`][super::unique_vec::TieredUniqueVector].

/// A mutable unique-vector backend for use as a tier in
/// [`TieredUniqueVector`][super::unique_vec::TieredUniqueVector].
///
/// A unique vector is an ordered sequence in which every element is distinct.
/// Backends must enforce uniqueness: `push_back` and `push_front` return
/// `false` and leave the collection unchanged when the element is already
/// present.
pub trait UniqueVecBackend<A>: Send + 'static
where
    A: Clone,
{
    /// Appends `elem` to the back of the sequence.
    ///
    /// Returns `true` if the element was inserted, `false` if it was already
    /// present (and the collection is unchanged).
    ///
    /// Time: O(log n).
    fn push_back(&mut self, elem: A) -> bool;

    /// Prepends `elem` to the front of the sequence.
    ///
    /// Returns `true` if the element was inserted, `false` if it was already
    /// present.
    ///
    /// Time: O(log n).
    fn push_front(&mut self, elem: A) -> bool;

    /// Removes and returns the last element, or `None` if empty.
    ///
    /// Time: O(log n).
    fn pop_back(&mut self) -> Option<A>;

    /// Removes and returns the first element, or `None` if empty.
    ///
    /// Time: O(log n).
    fn pop_front(&mut self) -> Option<A>;

    /// Returns a clone of the element at `index`, or `None` if out of bounds.
    ///
    /// Time: O(log n).
    fn get(&self, index: usize) -> Option<A>;

    /// Tests whether `elem` is present in the sequence.
    ///
    /// Time: O(log n).
    fn contains(&self, elem: &A) -> bool;

    /// Removes `elem` from the sequence, returning `true` if it was present.
    ///
    /// Time: O(n).
    fn remove_by_value(&mut self, elem: &A) -> bool;

    /// Returns the number of elements in the sequence.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the sequence is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Returns all elements in sequence order.
    ///
    /// Time: O(n).
    fn iter_all(&self) -> Vec<A>;

    /// Replaces the backend's contents with the supplied elements, in order.
    ///
    /// Duplicate elements in `iter` are skipped (uniqueness is enforced).
    ///
    /// Time: O(n log n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>);

    /// Drains all elements, returning them in sequence order.
    ///
    /// The backend is empty after this call.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A>;
}
