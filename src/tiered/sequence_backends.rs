//! Concrete [`SequenceBackend`] implementations.
//!
//! | Backend | Underlying store | Feature gate |
//! |---------|-----------------|--------------|
//! | [`StdVecBackend`] | `std::vec::Vec` | `tiered` (always) |
//! | [`PdsVectorBackend`] | `pds::Vector` (RRB tree, structural sharing) | `tiered` (always) |
//!
//! Both backends implement [`Clone`] and [`Default`].

use super::sequence_backend::SequenceBackend;

// --- StdVecBackend ---

/// A [`SequenceBackend`] backed by [`std::vec::Vec`].
///
/// Most operations are O(1) amortised. `Clone` is O(n) — a full deep copy.
#[derive(Clone, Default)]
pub struct StdVecBackend<A> {
    /// Inner standard-library vector.
    inner: Vec<A>,
}

impl<A> StdVecBackend<A> {
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }
}

impl<A> SequenceBackend<A> for StdVecBackend<A>
where
    A: Clone + Send + 'static,
{
    fn get(&self, index: usize) -> Option<A> {
        self.inner.get(index).cloned()
    }

    fn push_back(&mut self, value: A) {
        self.inner.push(value);
    }

    fn pop_back(&mut self) -> Option<A> {
        self.inner.pop()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Appends all elements from `iter` to the back.
    ///
    /// Time: O(k) where k is the number of elements in `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        self.inner.extend(iter);
    }

    /// Drains all elements, leaving the backend empty.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        self.inner.drain(..).collect()
    }
}

// --- PdsVectorBackend ---

/// A [`SequenceBackend`] backed by `pds::Vector` (an RRB tree with structural
/// sharing).
///
/// All mutations use the functional API — each call produces a new vector stored
/// in place. `Clone` is O(1) via reference-count increment.
///
/// Use this backend as a cold tier when O(1) snapshots or structural sharing
/// between tiers matter.
#[derive(Clone, Default)]
pub struct PdsVectorBackend<A>
where
    A: Clone + 'static,
{
    /// Inner pds RRB-tree vector.
    inner: crate::Vector<A>,
}

impl<A> PdsVectorBackend<A>
where
    A: Clone + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::Vector::new(),
        }
    }

    /// Returns a reference to the inner `pds::Vector`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::Vector<A> {
        &self.inner
    }
}

impl<A> SequenceBackend<A> for PdsVectorBackend<A>
where
    A: Clone + Send + Sync + 'static,
{
    /// Returns the element at `index`, or `None` if out of bounds.
    ///
    /// Time: O(log n).
    fn get(&self, index: usize) -> Option<A> {
        self.inner.get(index).cloned()
    }

    /// Appends `value` to the back using the pds functional API.
    ///
    /// Time: O(1) amortised.
    fn push_back(&mut self, value: A) {
        self.inner.push_back(value);
    }

    /// Removes and returns the last element.
    ///
    /// Time: O(1) amortised.
    fn pop_back(&mut self) -> Option<A> {
        self.inner.pop_back()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Appends all elements from `iter` to the back.
    ///
    /// Time: O(k log n) where k is the number of elements in `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        for item in iter {
            self.inner.push_back(item);
        }
    }

    /// Drains all elements, resetting the backend to an empty vector.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        let elems: Vec<A> = self.inner.iter().cloned().collect();
        self.inner = crate::Vector::new();
        elems
    }
}
