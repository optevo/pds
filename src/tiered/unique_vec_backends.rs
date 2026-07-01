//! Concrete [`UniqueVecBackend`] implementations.
//!
//! | Backend | Underlying store |
//! |---------|-----------------|
//! | [`PdsUniqueVecBackend`] | `pds::UniqueVector` (HAMT-backed unique sequence) |

use super::unique_vec_backend::UniqueVecBackend;

// --- PdsUniqueVecBackend ---

/// A [`UniqueVecBackend`] backed by `pds::UniqueVector` (HAMT with structural sharing).
///
/// `Clone` is O(1) via structural sharing.
#[derive(Clone, Default)]
pub struct PdsUniqueVecBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    inner: crate::UniqueVector<A>,
}

impl<A> PdsUniqueVecBackend<A>
where
    A: Clone + Eq + std::hash::Hash + 'static,
{
    /// Creates an empty backend.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: crate::UniqueVector::new(),
        }
    }

    /// Returns a reference to the inner `pds::UniqueVector`.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &crate::UniqueVector<A> {
        &self.inner
    }
}

impl<A> UniqueVecBackend<A> for PdsUniqueVecBackend<A>
where
    A: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
{
    fn push_back(&mut self, elem: A) -> bool {
        self.inner.push_back(elem)
    }

    fn push_front(&mut self, elem: A) -> bool {
        self.inner.push_front(elem)
    }

    fn pop_back(&mut self) -> Option<A> {
        self.inner.pop_back()
    }

    fn pop_front(&mut self) -> Option<A> {
        self.inner.pop_front()
    }

    fn get(&self, index: usize) -> Option<A> {
        self.inner.get(index).cloned()
    }

    fn contains(&self, elem: &A) -> bool {
        self.inner.contains(elem)
    }

    fn remove_by_value(&mut self, elem: &A) -> bool {
        self.inner.remove_by_value(elem)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn iter_all(&self) -> Vec<A> {
        self.inner.iter().cloned().collect()
    }

    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        let mut v = crate::UniqueVector::new();
        for elem in iter {
            v.push_back(elem);
        }
        self.inner = v;
    }

    fn drain(&mut self) -> Vec<A> {
        let elems = self.iter_all();
        self.inner = crate::UniqueVector::new();
        elems
    }
}
