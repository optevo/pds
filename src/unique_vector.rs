// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent sequence with O(log n) indexed access and uniqueness guarantees.
//!
//! [`UniqueVector<A>`] combines the RRB-tree sequence of [`Vector`][crate::Vector]
//! with the O(log n) membership testing of [`HashSet`][crate::HashSet]. Every
//! element appears at most once; inserting a duplicate is a no-op that returns
//! `false`. Elements remain in the order they were first inserted.
//!
//! Use `UniqueVector` when you need **both**:
//! - Positional access (`get(i)`, `Index`, `pop_front`, `pop_back`) — only possible
//!   with a sequence type.
//! - Deduplication — guaranteed by the internal `HashSet`.
//!
//! If you do not need `get(i)`, prefer [`InsertionOrderSet`][crate::InsertionOrderSet],
//! which uses less memory (no Vector overhead) and also supports
//! `pop_front` / `pop_back`.
//!
//! # Performance
//!
//! All primary operations are O(log n):
//!
//! | Operation | Complexity | Notes |
//! |-----------|-----------|-------|
//! | `push_back` / `push_front` | O(log n) | HashSet check + Vector push |
//! | `pop_front` | O(log n) | Vector pop + HashSet remove |
//! | `pop_back` | O(1)\* | Vector pop + HashSet remove |
//! | `get(i)` | O(log n) | Vector index |
//! | `contains` | O(log n) | HashSet lookup |
//! | `remove_by_value` | O(n) | Linear scan to find position |
//!
//! Clone is O(1) via structural sharing — both internal trees share nodes with
//! the original.
//!
//! # Example
//!
//! ```
//! use pds::UniqueVector;
//!
//! // Deduplicating work queue with positional access.
//! let mut queue = UniqueVector::new();
//! assert!(queue.push_back("task-a"));
//! assert!(queue.push_back("task-b"));
//! assert!(!queue.push_back("task-a")); // duplicate — ignored
//! assert!(queue.push_back("task-c"));
//!
//! assert_eq!(queue.len(), 3);
//! assert_eq!(queue.get(1), Some(&"task-b"));
//!
//! assert_eq!(queue.pop_front(), Some("task-a"));
//! assert_eq!(queue.pop_front(), Some("task-b"));
//! assert_eq!(queue.pop_front(), Some("task-c"));
//! assert_eq!(queue.pop_front(), None);
//! ```

use alloc::vec::Vec;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::FromIterator;
use core::ops::Index;
#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;

use archery::SharedPointerKind;

use crate::hash_width::HashWidth;
use crate::hashset::GenericHashSet;
use crate::shared_ptr::DefaultSharedPtr;
use crate::vector::{ConsumingIter as VecConsumingIter, GenericVector, Iter as VecIter};

// ─── Type aliases ─────────────────────────────────────────────────────────────

/// Type alias for [`GenericUniqueVector`] with the default hasher and pointer type.
#[cfg(feature = "std")]
pub type UniqueVector<A> = GenericUniqueVector<A, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericUniqueVector`] using [`foldhash::fast::RandomState`] —
/// available in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type UniqueVector<A> =
    GenericUniqueVector<A, foldhash::fast::RandomState, DefaultSharedPtr>;

// ─── Struct ───────────────────────────────────────────────────────────────────

/// A persistent sequence with O(log n) indexed access and uniqueness guarantees.
///
/// See the [module documentation][self] for usage and performance notes.
pub struct GenericUniqueVector<A, S, P: SharedPointerKind = DefaultSharedPtr, H: HashWidth = u64> {
    /// Element sequence in insertion order.
    vec: GenericVector<A, P>,
    /// Membership index for O(log n) uniqueness checks.
    set: GenericHashSet<A, S, P, H>,
}

// ─── Manual Clone ─────────────────────────────────────────────────────────────

impl<A: Clone, S: Clone, P: SharedPointerKind, H: HashWidth> Clone
    for GenericUniqueVector<A, S, P, H>
{
    fn clone(&self) -> Self {
        GenericUniqueVector {
            vec: self.vec.clone(),
            set: self.set.clone(),
        }
    }
}

// ─── Constructors ─────────────────────────────────────────────────────────────

#[cfg(feature = "std")]
impl<A, P: SharedPointerKind> GenericUniqueVector<A, RandomState, P> {
    /// Create an empty `UniqueVector`.
    #[must_use]
    pub fn new() -> Self {
        GenericUniqueVector {
            vec: GenericVector::new(),
            set: GenericHashSet::new(),
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<A, P: SharedPointerKind> GenericUniqueVector<A, foldhash::fast::RandomState, P> {
    /// Create an empty `UniqueVector`.
    #[must_use]
    pub fn new() -> Self {
        GenericUniqueVector {
            vec: GenericVector::new(),
            set: GenericHashSet::new(),
        }
    }
}

impl<A, S, P: SharedPointerKind, H: HashWidth> GenericUniqueVector<A, S, P, H>
where
    S: BuildHasher,
{
    /// Create an empty `UniqueVector` with the given hasher.
    #[must_use]
    pub fn with_hasher(hasher: S) -> Self
    where
        S: Clone,
    {
        GenericUniqueVector {
            vec: GenericVector::new(),
            set: GenericHashSet::with_hasher(hasher),
        }
    }
}

// ─── Core operations ──────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Return the number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Return `true` if the vector contains no elements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    /// Return `true` if `elem` is present.
    #[must_use]
    pub fn contains(&self, elem: &A) -> bool {
        self.set.contains(elem)
    }

    /// Return a reference to the element at index `i`, or `None` if out of bounds.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&A> {
        self.vec.get(index)
    }

    /// Return a reference to the first element, or `None` if empty.
    #[must_use]
    pub fn front(&self) -> Option<&A> {
        self.vec.get(0)
    }

    /// Return a reference to the last element, or `None` if empty.
    #[must_use]
    pub fn back(&self) -> Option<&A> {
        let n = self.vec.len();
        if n == 0 { None } else { self.vec.get(n - 1) }
    }

    /// Append `elem` to the back. Returns `true` if newly inserted, `false` if
    /// already present (the vector is unchanged).
    pub fn push_back(&mut self, elem: A) -> bool {
        if self.set.insert(elem.clone()).is_none() {
            self.vec.push_back(elem);
            true
        } else {
            false
        }
    }

    /// Prepend `elem` to the front. Returns `true` if newly inserted, `false` if
    /// already present (the vector is unchanged).
    pub fn push_front(&mut self, elem: A) -> bool {
        if self.set.insert(elem.clone()).is_none() {
            self.vec.push_front(elem);
            true
        } else {
            false
        }
    }

    /// Remove and return the first element, or `None` if empty.
    pub fn pop_front(&mut self) -> Option<A> {
        let elem = self.vec.pop_front()?;
        self.set.remove(&elem);
        Some(elem)
    }

    /// Remove and return the last element, or `None` if empty.
    pub fn pop_back(&mut self) -> Option<A> {
        let elem = self.vec.pop_back()?;
        self.set.remove(&elem);
        Some(elem)
    }

    /// Remove the element at index `i` and return it.
    ///
    /// This is O(log n) for the structural split/concat; the index must be in bounds.
    ///
    /// # Panics
    ///
    /// Panics if `index >= len()`.
    pub fn remove(&mut self, index: usize) -> A {
        let elem = self.vec.remove(index);
        self.set.remove(&elem);
        elem
    }

    /// Find `elem` by value and remove it. Returns `true` if it was present.
    ///
    /// This is O(n) — a linear scan of the vector to find the position — followed
    /// by O(log n) removal. For hot paths prefer `pop_front` / `pop_back`.
    pub fn remove_by_value(&mut self, elem: &A) -> bool {
        if !self.set.contains(elem) {
            return false;
        }
        // Linear scan to find position.
        let pos = self.vec.iter().position(|e| e == elem).expect("set/vec invariant");
        self.vec.remove(pos);
        self.set.remove(elem);
        true
    }

    /// Iterate over elements in insertion order.
    pub fn iter(&self) -> VecIter<'_, A, P> {
        self.vec.iter()
    }

    /// Return the union: all elements from `self`, then any elements from `other`
    /// not already in `self`, in `other`'s order.
    #[must_use]
    pub fn union(mut self, other: Self) -> Self {
        for elem in other.vec {
            self.push_back(elem);
        }
        self
    }

    /// Return elements in `self` that are not in `other`, preserving `self`'s order.
    #[must_use]
    pub fn difference(self, other: &Self) -> Self
    where
        S: Default,
    {
        self.into_iter().filter(|e| !other.contains(e)).collect()
    }

    /// Return elements present in both `self` and `other`, preserving `self`'s order.
    #[must_use]
    pub fn intersection(self, other: &Self) -> Self
    where
        S: Default,
    {
        self.into_iter().filter(|e| other.contains(e)).collect()
    }

    /// Return elements in exactly one of `self` or `other`.
    ///
    /// `self`'s unique elements come first (in their original order), followed
    /// by `other`'s unique elements.
    #[must_use]
    pub fn symmetric_difference(self, other: Self) -> Self
    where
        S: Default,
    {
        // Borrow both before collecting so each filter checks the original.
        let self_only: Self = self.iter().filter(|e| !other.contains(e)).cloned().collect();
        let other_only: Self = other.iter().filter(|e| !self.contains(e)).cloned().collect();
        self_only.union(other_only)
    }
}

// ─── Default ──────────────────────────────────────────────────────────────────

impl<A, S, P: SharedPointerKind, H: HashWidth> Default for GenericUniqueVector<A, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericUniqueVector {
            vec: GenericVector::new(),
            set: GenericHashSet::default(),
        }
    }
}

// ─── Debug ────────────────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> Debug for GenericUniqueVector<A, S, P, H>
where
    A: Debug + Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_list().entries(self.vec.iter()).finish()
    }
}

// ─── PartialEq / Eq ───────────────────────────────────────────────────────────

impl<A, S, S2, P, P2, H: HashWidth> PartialEq<GenericUniqueVector<A, S2, P2, H>>
    for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    S2: BuildHasher + Clone,
    P: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn eq(&self, other: &GenericUniqueVector<A, S2, P2, H>) -> bool {
        // UniqueVector is a sequence — equality is order-sensitive.
        self.vec.len() == other.vec.len() && self.vec.iter().zip(other.vec.iter()).all(|(a, b)| a == b)
    }
}

impl<A, S, P, H: HashWidth> Eq for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

// ─── PartialOrd / Ord ─────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> PartialOrd for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone + Ord,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<A, S, P, H: HashWidth> Ord for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone + Ord,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.vec.iter().cmp(other.vec.iter())
    }
}

// ─── Hash ─────────────────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> core::hash::Hash for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn hash<Hs: Hasher>(&self, state: &mut Hs) {
        // Hash in sequence order — order-sensitive, matching PartialEq.
        for elem in self.vec.iter() {
            elem.hash(state);
        }
    }
}

// ─── FromIterator ─────────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> FromIterator<A> for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        let mut uv = Self::default();
        for elem in iter {
            uv.push_back(elem);
        }
        uv
    }
}

// ─── Extend ───────────────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> Extend<A> for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = A>>(&mut self, iter: I) {
        for elem in iter {
            self.push_back(elem);
        }
    }
}

// ─── IntoIterator ─────────────────────────────────────────────────────────────

/// Consuming iterator for [`GenericUniqueVector`].
pub struct ConsumingIter<A, P: SharedPointerKind>(VecConsumingIter<A, P>);

impl<A: Clone, P: SharedPointerKind> Iterator for ConsumingIter<A, P> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<A: Clone, P: SharedPointerKind> DoubleEndedIterator for ConsumingIter<A, P> {
    fn next_back(&mut self) -> Option<A> {
        self.0.next_back()
    }
}

impl<A: Clone, P: SharedPointerKind> ExactSizeIterator for ConsumingIter<A, P> {}

impl<A, S, P, H: HashWidth> IntoIterator for GenericUniqueVector<A, S, P, H>
where
    A: Clone,
    P: SharedPointerKind,
{
    type Item = A;
    type IntoIter = ConsumingIter<A, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter(self.vec.into_iter())
    }
}

impl<'a, A, S, P, H: HashWidth> IntoIterator for &'a GenericUniqueVector<A, S, P, H>
where
    A: Clone,
    P: SharedPointerKind,
{
    type Item = &'a A;
    type IntoIter = VecIter<'a, A, P>;

    fn into_iter(self) -> Self::IntoIter {
        self.vec.iter()
    }
}

// ─── Index ────────────────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> Index<usize> for GenericUniqueVector<A, S, P, H>
where
    A: Clone,
    P: SharedPointerKind,
{
    type Output = A;

    fn index(&self, index: usize) -> &A {
        &self.vec[index]
    }
}

// ─── From conversions ─────────────────────────────────────────────────────────

impl<A, S, P, H: HashWidth> From<Vec<A>> for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn from(v: Vec<A>) -> Self {
        v.into_iter().collect()
    }
}

impl<A, S, P, H: HashWidth> From<&Vec<A>> for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn from(v: &Vec<A>) -> Self {
        v.iter().cloned().collect()
    }
}

impl<A, S, P, H: HashWidth, const N: usize> From<[A; N]> for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn from(arr: [A; N]) -> Self {
        arr.into_iter().collect()
    }
}

impl<A, S, P, H: HashWidth> From<&[A]> for GenericUniqueVector<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn from(slice: &[A]) -> Self {
        slice.iter().cloned().collect()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(UniqueVector<i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let uv: UniqueVector<i32> = UniqueVector::new();
        assert!(uv.is_empty());
        assert_eq!(uv.len(), 0);
    }

    #[test]
    fn push_back_dedup() {
        let mut uv = UniqueVector::new();
        assert!(uv.push_back(1i32));
        assert!(uv.push_back(2i32));
        assert!(!uv.push_back(1i32)); // duplicate
        assert!(uv.push_back(3i32));
        assert_eq!(uv.len(), 3);
        assert_eq!(uv[0], 1);
        assert_eq!(uv[1], 2);
        assert_eq!(uv[2], 3);
    }

    #[test]
    fn push_front_dedup() {
        let mut uv = UniqueVector::new();
        assert!(uv.push_back(1i32));
        assert!(uv.push_front(0i32));
        assert!(!uv.push_front(1i32)); // duplicate — ignored
        assert_eq!(uv.len(), 2);
        assert_eq!(uv[0], 0);
        assert_eq!(uv[1], 1);
    }

    #[test]
    fn pop_front_fifo() {
        let mut uv = UniqueVector::new();
        uv.push_back("a");
        uv.push_back("b");
        uv.push_back("c");
        assert_eq!(uv.pop_front(), Some("a"));
        assert_eq!(uv.pop_front(), Some("b"));
        assert_eq!(uv.pop_front(), Some("c"));
        assert_eq!(uv.pop_front(), None);
    }

    #[test]
    fn pop_back_lifo() {
        let mut uv = UniqueVector::new();
        uv.push_back(1i32);
        uv.push_back(2i32);
        uv.push_back(3i32);
        assert_eq!(uv.pop_back(), Some(3));
        assert_eq!(uv.pop_back(), Some(2));
        assert_eq!(uv.pop_back(), Some(1));
        assert_eq!(uv.pop_back(), None);
    }

    #[test]
    fn pop_front_allows_reinsert() {
        // After popping, the same value can be re-inserted.
        let mut uv = UniqueVector::new();
        uv.push_back(42i32);
        uv.push_back(99i32);
        assert_eq!(uv.pop_front(), Some(42));
        assert!(uv.push_back(42)); // was popped — may be re-inserted
        assert_eq!(uv.len(), 2);
        assert_eq!(uv[0], 99);
        assert_eq!(uv[1], 42);
    }

    #[test]
    fn get_indexed_access() {
        let uv: UniqueVector<i32> = vec![10, 20, 30, 20, 10].into(); // deduplicates
        assert_eq!(uv.len(), 3);
        assert_eq!(uv.get(0), Some(&10));
        assert_eq!(uv.get(1), Some(&20));
        assert_eq!(uv.get(2), Some(&30));
        assert_eq!(uv.get(3), None);
    }

    #[test]
    fn front_back() {
        let mut uv = UniqueVector::new();
        assert_eq!(uv.front(), None);
        assert_eq!(uv.back(), None);
        uv.push_back(1i32);
        uv.push_back(2i32);
        uv.push_back(3i32);
        assert_eq!(uv.front(), Some(&1));
        assert_eq!(uv.back(), Some(&3));
    }

    #[test]
    fn contains() {
        let uv: UniqueVector<i32> = [1, 2, 3].into();
        assert!(uv.contains(&1));
        assert!(uv.contains(&3));
        assert!(!uv.contains(&99));
    }

    #[test]
    fn remove_by_index() {
        let mut uv: UniqueVector<i32> = [1, 2, 3, 4].into();
        let removed = uv.remove(1); // removes 2
        assert_eq!(removed, 2);
        assert_eq!(uv.len(), 3);
        assert!(!uv.contains(&2));
        assert!(uv.push_back(2)); // can re-insert after removal
    }

    #[test]
    fn remove_by_value() {
        let mut uv: UniqueVector<i32> = [1, 2, 3].into();
        assert!(uv.remove_by_value(&2));
        assert_eq!(uv.len(), 2);
        assert!(!uv.contains(&2));
        assert!(!uv.remove_by_value(&99)); // not present
    }

    #[test]
    fn iter_insertion_order() {
        let uv: UniqueVector<i32> = vec![5, 3, 1, 3, 5, 7].into();
        let elems: Vec<_> = uv.iter().copied().collect();
        assert_eq!(elems, vec![5, 3, 1, 7]);
    }

    #[test]
    fn into_iter_consuming() {
        let uv: UniqueVector<i32> = [1, 2, 3].into();
        let elems: Vec<_> = uv.into_iter().collect();
        assert_eq!(elems, vec![1, 2, 3]);
    }

    #[test]
    fn equality_order_sensitive() {
        let a: UniqueVector<i32> = [1, 2, 3].into();
        let b: UniqueVector<i32> = [1, 2, 3].into();
        let c: UniqueVector<i32> = [3, 2, 1].into();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_order_sensitive() {
        use std::hash::{DefaultHasher, Hash, Hasher};
        fn hash_of<T: Hash>(v: &T) -> u64 {
            let mut h = DefaultHasher::new();
            v.hash(&mut h);
            h.finish()
        }
        let a: UniqueVector<i32> = [1, 2, 3].into();
        let b: UniqueVector<i32> = [1, 2, 3].into();
        let c: UniqueVector<i32> = [3, 2, 1].into();
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn ord() {
        let a: UniqueVector<i32> = [1, 2].into();
        let b: UniqueVector<i32> = [1, 3].into();
        assert!(a < b);
    }

    #[test]
    fn from_vec_deduplicates() {
        let uv: UniqueVector<i32> = vec![1, 2, 1, 3, 2].into();
        assert_eq!(uv.len(), 3);
        let elems: Vec<_> = uv.iter().copied().collect();
        assert_eq!(elems, vec![1, 2, 3]);
    }

    #[test]
    fn from_array() {
        let uv: UniqueVector<i32> = [1i32, 2, 3, 2, 1].into();
        assert_eq!(uv.len(), 3);
    }

    #[test]
    fn from_slice() {
        let uv: UniqueVector<i32> = [1i32, 2, 3][..].into();
        assert_eq!(uv.len(), 3);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![1i32, 2, 3];
        let uv: UniqueVector<i32> = (&v).into();
        assert_eq!(uv.len(), 3);
    }

    #[test]
    fn extend_deduplicates() {
        let mut uv: UniqueVector<i32> = [1, 2].into();
        uv.extend([2, 3, 4, 1]);
        assert_eq!(uv.len(), 4);
        let elems: Vec<_> = uv.iter().copied().collect();
        assert_eq!(elems, vec![1, 2, 3, 4]);
    }

    #[test]
    fn default_is_empty() {
        let uv: UniqueVector<i32> = UniqueVector::default();
        assert!(uv.is_empty());
    }

    #[test]
    fn debug_format() {
        let uv: UniqueVector<i32> = [1, 2, 3].into();
        let s = format!("{:?}", uv);
        assert!(s.contains("1"));
        assert!(s.contains("2"));
        assert!(s.contains("3"));
    }

    #[test]
    fn union() {
        let a: UniqueVector<i32> = [1, 2, 3].into();
        let b: UniqueVector<i32> = [3, 4, 5].into();
        let u = a.union(b);
        let elems: Vec<_> = u.iter().copied().collect();
        // 1, 2, 3 from a; then 4, 5 from b (3 already present)
        assert_eq!(elems, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn difference() {
        let a: UniqueVector<i32> = [1, 2, 3, 4].into();
        let b: UniqueVector<i32> = [2, 4].into();
        let d = a.difference(&b);
        let elems: Vec<_> = d.iter().copied().collect();
        assert_eq!(elems, vec![1, 3]);
    }

    #[test]
    fn intersection() {
        let a: UniqueVector<i32> = [1, 2, 3, 4].into();
        let b: UniqueVector<i32> = [2, 4, 5].into();
        let i = a.intersection(&b);
        let elems: Vec<_> = i.iter().copied().collect();
        assert_eq!(elems, vec![2, 4]);
    }

    #[test]
    fn symmetric_difference() {
        let a: UniqueVector<i32> = [1, 2, 3].into();
        let b: UniqueVector<i32> = [2, 3, 4].into();
        let sd = a.symmetric_difference(b);
        let elems: Vec<_> = sd.iter().copied().collect();
        assert_eq!(elems, vec![1, 4]);
    }

    #[test]
    fn dedup_queue_with_indexed_access() {
        // The key use case: FIFO dedup queue where position matters.
        let mut queue: UniqueVector<&str> = UniqueVector::new();
        queue.push_back("task-a");
        queue.push_back("task-b");
        queue.push_back("task-a"); // duplicate
        queue.push_back("task-c");

        assert_eq!(queue.len(), 3);
        assert_eq!(queue[1], "task-b"); // O(log n) indexed access
        assert_eq!(queue.pop_front(), Some("task-a"));
        assert_eq!(queue.pop_front(), Some("task-b"));
        assert_eq!(queue.pop_front(), Some("task-c"));
    }
}
