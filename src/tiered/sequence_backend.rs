//! [`SequenceBackend<A>`] — the trait implemented by every tier in a
//! [`TieredSequence`][super::sequence::TieredSequence].
//!
//! A sequence backend is an indexed, mutable sequence of elements. It does not
//! need to be persistent or thread-safe on its own — thread safety comes from
//! the `Arc<Mutex<…>>` wrapping inside `TieredSequence`.

/// A mutable indexed sequence that can serve as one tier in a
/// [`TieredSequence`][super::sequence::TieredSequence].
///
/// Concrete backends in this module:
/// [`StdVecBackend`][super::sequence_backends::StdVecBackend],
/// [`PdsVectorBackend`][super::sequence_backends::PdsVectorBackend].
///
/// # Implementor notes
///
/// - `Send + 'static` is required so that a `TieredSequence` (which holds
///   backends behind an `Arc<Mutex<…>>`) can be shared across threads.
/// - `drain` must leave the backend empty and return all elements in order.
/// - `load_from` must append the iterator's elements to the backend.
///   It does **not** clear first — the append-log semantics of `TieredSequence`
///   rely on `load_from` extending cold rather than replacing it.
/// - There is no `push_front` or `pop_front`. `TieredSequence` uses cold as an
///   append-only committed log; prepend operations are semantically undefined
///   in this model.
pub trait SequenceBackend<A>: Send + 'static
where
    A: Clone,
{
    /// Returns the element at `index`, or `None` if out of bounds.
    ///
    /// Time: depends on backend; O(1) for `StdVecBackend`; O(log n) for
    /// `PdsVectorBackend`.
    fn get(&self, index: usize) -> Option<A>;

    /// Appends `value` to the back of the sequence.
    ///
    /// Time: O(1) amortised for `StdVecBackend`; O(1) amortised for
    /// `PdsVectorBackend`.
    fn push_back(&mut self, value: A);

    /// Removes and returns the last element, or `None` if empty.
    ///
    /// Time: O(1) amortised for `StdVecBackend`; O(1) amortised for
    /// `PdsVectorBackend`.
    fn pop_back(&mut self) -> Option<A>;

    /// Returns the number of elements currently stored.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the sequence contains no elements.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool;

    /// Appends all elements from `iter` to the back of the sequence.
    ///
    /// Used during propagation flush: the hot tier's elements are appended to the
    /// cold tier's committed log. Does **not** clear existing elements.
    ///
    /// Time: O(k) where k is the number of elements in `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = A>);

    /// Drains all elements, leaving the backend empty.
    ///
    /// Returns every element that was in the backend before the call, in order.
    /// After `drain`, the backend behaves as if newly constructed.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A>;
}
