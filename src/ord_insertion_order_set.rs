// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent set preserving insertion order, backed entirely by `OrdMap`.
//!
//! An [`OrdInsertionOrderSet`] stores unique elements in the order they were
//! first inserted. Backed by a
//! [`GenericOrdInsertionOrderMap<A, ()>`][crate::GenericOrdInsertionOrderMap].
//! Membership testing and removal are both O(log n). Clone is O(1) via
//! structural sharing.
//!
//! Unlike [`InsertionOrderSet`][crate::InsertionOrderSet], this type requires
//! only `A: Ord + Clone` — no `Hash + Eq` constraint — and works in `no_std`
//! without the `foldhash` feature.
//!
//! # Examples
//!
//! ```
//! use pds::OrdInsertionOrderSet;
//!
//! let mut set = OrdInsertionOrderSet::new();
//! set.insert("c");
//! set.insert("a");
//! set.insert("b");
//!
//! let elems: Vec<_> = set.iter().collect();
//! assert_eq!(elems, vec![&"c", &"a", &"b"]);
//! ```

use alloc::vec::Vec;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;

use archery::SharedPointerKind;
use equivalent::Comparable;

use crate::ord_insertion_order_map::{ConsumingIter as MapConsumingIter, GenericOrdInsertionOrderMap};
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericOrdInsertionOrderSet`] with the default pointer type.
pub type OrdInsertionOrderSet<A> = GenericOrdInsertionOrderSet<A, DefaultSharedPtr>;

/// A persistent set that iterates in insertion order.
///
/// Backed by a [`GenericOrdInsertionOrderMap<A, ()>`][crate::GenericOrdInsertionOrderMap]:
/// membership is O(log n) and iteration is in insertion order. Clone is O(1)
/// via structural sharing.
///
/// Unlike [`InsertionOrderSet`][crate::InsertionOrderSet], requires only
/// `A: Ord + Clone` — no `Hash + Eq` constraint.
pub struct GenericOrdInsertionOrderSet<A, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) map: GenericOrdInsertionOrderMap<A, (), P>,
}

// Manual Clone — avoid spurious `P: Clone` bound from derive.
impl<A: Clone, P: SharedPointerKind> Clone for GenericOrdInsertionOrderSet<A, P> {
    fn clone(&self) -> Self {
        GenericOrdInsertionOrderSet {
            map: self.map.clone(),
        }
    }
}

impl<A, P: SharedPointerKind> GenericOrdInsertionOrderSet<A, P> {
    /// Create an empty OrdInsertionOrderSet.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdInsertionOrderSet {
            map: GenericOrdInsertionOrderMap::new(),
        }
    }

    /// Test whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Return the number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> GenericOrdInsertionOrderSet<A, P> {
    /// Insert an element. Does nothing if already present.
    ///
    /// Returns `true` if the element was newly inserted, `false` if it already existed.
    pub fn insert(&mut self, value: A) -> bool {
        self.map.insert(value, ()).is_none()
    }

    /// Test whether an element is present.
    #[must_use]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.contains_key(value)
    }

    /// Remove an element. Returns `true` if it was present.
    pub fn remove<Q>(&mut self, value: &Q) -> bool
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.remove(value).is_some()
    }

    /// Return a reference to the first element in insertion order, or `None` if empty.
    pub fn front(&self) -> Option<&A> {
        self.map.front().map(|(a, _)| a)
    }

    /// Return a reference to the last element in insertion order, or `None` if empty.
    pub fn back(&self) -> Option<&A> {
        self.map.back().map(|(a, _)| a)
    }

    /// Remove and return the first element in insertion order (FIFO dequeue).
    ///
    /// Returns `None` if the set is empty.
    pub fn pop_front(&mut self) -> Option<A> {
        self.map.pop_front().map(|(a, _)| a)
    }

    /// Remove and return the last element in insertion order (LIFO dequeue).
    ///
    /// Returns `None` if the set is empty.
    pub fn pop_back(&mut self) -> Option<A> {
        self.map.pop_back().map(|(a, _)| a)
    }

    /// Iterate over elements in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &A> {
        self.map.keys()
    }

    /// Return the union of two sets.
    ///
    /// New elements from `other` are appended in `other`'s insertion order
    /// after all of `self`'s elements.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        GenericOrdInsertionOrderSet {
            map: self.map.union(other.map),
        }
    }

    /// Return elements in `self` but not in `other`.
    #[must_use]
    pub fn difference(self, other: &Self) -> Self {
        GenericOrdInsertionOrderSet {
            map: self.map.difference(&other.map),
        }
    }

    /// Return elements in both `self` and `other`.
    #[must_use]
    pub fn intersection(self, other: &Self) -> Self {
        GenericOrdInsertionOrderSet {
            map: self.map.intersection(&other.map),
        }
    }

    /// Return elements in exactly one of `self` or `other`.
    #[must_use]
    pub fn symmetric_difference(self, other: &Self) -> Self {
        GenericOrdInsertionOrderSet {
            map: self.map.symmetric_difference(&other.map),
        }
    }
}

impl<A: Ord, P: SharedPointerKind> Default for GenericOrdInsertionOrderSet<A, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> PartialEq for GenericOrdInsertionOrderSet<A, P> {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> Eq for GenericOrdInsertionOrderSet<A, P> {}

impl<A: Ord + Clone + Hash, P: SharedPointerKind> Hash for GenericOrdInsertionOrderSet<A, P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Ordered: insertion order is part of identity.
        for a in self.iter() {
            a.hash(state);
        }
    }
}

impl<A: Ord + Clone + Debug, P: SharedPointerKind> Debug for GenericOrdInsertionOrderSet<A, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_set();
        for a in self.iter() {
            d.entry(a);
        }
        d.finish()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> FromIterator<A> for GenericOrdInsertionOrderSet<A, P> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        let mut set = Self::new();
        for a in iter {
            set.insert(a);
        }
        set
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> From<Vec<A>> for GenericOrdInsertionOrderSet<A, P> {
    fn from(v: Vec<A>) -> Self {
        v.into_iter().collect()
    }
}

impl<A: Ord + Clone, const N: usize, P: SharedPointerKind> From<[A; N]>
    for GenericOrdInsertionOrderSet<A, P>
{
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A: Ord + Clone, P: SharedPointerKind> From<&'a [A]>
    for GenericOrdInsertionOrderSet<A, P>
{
    fn from(slice: &'a [A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, A: Ord + Clone, P: SharedPointerKind> From<&'a Vec<A>>
    for GenericOrdInsertionOrderSet<A, P>
{
    fn from(v: &'a Vec<A>) -> Self {
        v.iter().cloned().collect()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> Extend<A> for GenericOrdInsertionOrderSet<A, P> {
    fn extend<I: IntoIterator<Item = A>>(&mut self, iter: I) {
        for a in iter {
            self.insert(a);
        }
    }
}

/// A consuming iterator over the elements of a [`GenericOrdInsertionOrderSet`].
///
/// Yields elements in insertion order.
pub struct ConsumingIter<A, P: SharedPointerKind> {
    inner: MapConsumingIter<A, (), P>,
}

impl<A: Clone, P: SharedPointerKind> Iterator for ConsumingIter<A, P> {
    type Item = A;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(a, _)| a)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<A: Clone, P: SharedPointerKind> ExactSizeIterator for ConsumingIter<A, P> {}

impl<A: Ord + Clone, P: SharedPointerKind> IntoIterator for GenericOrdInsertionOrderSet<A, P> {
    type Item = A;
    type IntoIter = ConsumingIter<A, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.map.into_iter(),
        }
    }
}

impl<'a, A: Ord + Clone, P: SharedPointerKind> IntoIterator
    for &'a GenericOrdInsertionOrderSet<A, P>
{
    type Item = &'a A;
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = &'a A> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.iter())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(OrdInsertionOrderSet<i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn insert_and_contains() {
        let mut set = OrdInsertionOrderSet::new();
        assert!(set.insert("a"));
        assert!(set.insert("b"));
        assert!(!set.insert("a")); // Duplicate — returns false.

        assert!(set.contains(&"a"));
        assert!(set.contains(&"b"));
        assert!(!set.contains(&"c"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn insertion_order_preserved() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert("c");
        set.insert("a");
        set.insert("b");

        let elems: Vec<_> = set.iter().collect();
        assert_eq!(elems, vec![&"c", &"a", &"b"]);
    }

    #[test]
    fn remove() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert("a");
        set.insert("b");

        assert!(set.remove(&"a"));
        assert!(!set.contains(&"a"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn remove_absent_returns_false() {
        let mut set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::new();
        assert!(!set.remove(&99));
    }

    #[test]
    fn remove_then_reinsert_appends() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert("a");
        set.insert("b");
        set.remove(&"a");
        set.insert("a");

        let elems: Vec<_> = set.iter().collect();
        assert_eq!(elems, vec![&"b", &"a"]);
    }

    #[test]
    fn equality_same_order() {
        let mut s1 = OrdInsertionOrderSet::new();
        s1.insert("a");
        s1.insert("b");

        let mut s2 = OrdInsertionOrderSet::new();
        s2.insert("a");
        s2.insert("b");

        assert_eq!(s1, s2);
    }

    #[test]
    fn inequality_different_order() {
        let mut s1 = OrdInsertionOrderSet::new();
        s1.insert("a");
        s1.insert("b");

        let mut s2 = OrdInsertionOrderSet::new();
        s2.insert("b");
        s2.insert("a");

        assert_ne!(s1, s2);
    }

    #[test]
    fn hash_same_for_equal_sets() {
        use std::hash::DefaultHasher;

        let mut s1 = OrdInsertionOrderSet::new();
        s1.insert(1i32);
        s1.insert(2);

        let mut s2 = OrdInsertionOrderSet::new();
        s2.insert(1i32);
        s2.insert(2);

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        s1.hash(&mut h1);
        s2.hash(&mut h2);
        assert_eq!(s1, s2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn clone_shares_structure() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert(1i32);
        let clone = set.clone();
        assert_eq!(set, clone);
    }

    #[test]
    fn default_is_empty() {
        let set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::default();
        assert!(set.is_empty());
    }

    #[test]
    fn from_vec() {
        let set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::from(vec![1, 2, 3]);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn from_array() {
        let set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::from([1, 2, 3]);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn from_slice() {
        let v = vec![1i32, 2, 3];
        let set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::from(v.as_slice());
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![1i32, 2, 3];
        let set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::from(&v);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn from_iterator() {
        let set: OrdInsertionOrderSet<i32> = vec![1, 2, 3].into_iter().collect();
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn extend() {
        let mut set: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::new();
        set.extend([1, 2, 3]);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn into_iter_insertion_order() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert(3i32);
        set.insert(1);
        set.insert(2);

        let elems: Vec<_> = set.into_iter().collect();
        assert_eq!(elems, vec![3, 1, 2]);
    }

    #[test]
    fn into_iter_ref() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert(1i32);
        set.insert(2);

        let elems: Vec<_> = (&set).into_iter().copied().collect();
        assert_eq!(elems, vec![1, 2]);
    }

    #[test]
    fn union_appends_new_elements() {
        let mut s1 = OrdInsertionOrderSet::new();
        s1.insert(1i32);

        let mut s2 = OrdInsertionOrderSet::new();
        s2.insert(2i32);

        let combined = s1.union(s2);
        assert_eq!(combined.len(), 2);
        let elems: Vec<_> = combined.iter().copied().collect();
        assert_eq!(elems, vec![1, 2]);
    }

    #[test]
    fn difference() {
        let s1: OrdInsertionOrderSet<i32> = [1, 2, 3].iter().copied().collect();
        let s2: OrdInsertionOrderSet<i32> = [2, 3].iter().copied().collect();

        let diff = s1.difference(&s2);
        assert_eq!(diff.len(), 1);
        assert!(diff.contains(&1));
    }

    #[test]
    fn intersection() {
        let s1: OrdInsertionOrderSet<i32> = [1, 2, 3].iter().copied().collect();
        let s2: OrdInsertionOrderSet<i32> = [2, 3, 4].iter().copied().collect();

        let inter = s1.intersection(&s2);
        assert_eq!(inter.len(), 2);
        assert!(inter.contains(&2));
        assert!(inter.contains(&3));
    }

    #[test]
    fn symmetric_difference() {
        let s1: OrdInsertionOrderSet<i32> = [1, 2].iter().copied().collect();
        let s2: OrdInsertionOrderSet<i32> = [2, 3].iter().copied().collect();

        let sd = s1.symmetric_difference(&s2);
        assert_eq!(sd.len(), 2);
        assert!(sd.contains(&1));
        assert!(sd.contains(&3));
        assert!(!sd.contains(&2));
    }

    #[test]
    fn debug_format() {
        let mut set = OrdInsertionOrderSet::new();
        set.insert(1i32);
        let s = format!("{:?}", set);
        assert!(s.contains("1"));
    }

    #[test]
    fn front_back_empty() {
        let s: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::new();
        assert_eq!(s.front(), None);
        assert_eq!(s.back(), None);
    }

    #[test]
    fn front_back_order() {
        let mut s = OrdInsertionOrderSet::new();
        s.insert(10i32);
        s.insert(20i32);
        s.insert(30i32);
        assert_eq!(s.front(), Some(&10));
        assert_eq!(s.back(), Some(&30));
    }

    #[test]
    fn pop_front_fifo() {
        let mut s = OrdInsertionOrderSet::new();
        s.insert(1i32);
        s.insert(2i32);
        s.insert(3i32);
        assert_eq!(s.pop_front(), Some(1));
        assert_eq!(s.pop_front(), Some(2));
        assert_eq!(s.pop_front(), Some(3));
        assert_eq!(s.pop_front(), None);
    }

    #[test]
    fn pop_back_lifo() {
        let mut s = OrdInsertionOrderSet::new();
        s.insert(1i32);
        s.insert(2i32);
        s.insert(3i32);
        assert_eq!(s.pop_back(), Some(3));
        assert_eq!(s.pop_back(), Some(2));
        assert_eq!(s.pop_back(), Some(1));
        assert_eq!(s.pop_back(), None);
    }

    #[test]
    fn dedup_queue_simulation() {
        let mut queue: OrdInsertionOrderSet<i32> = OrdInsertionOrderSet::new();
        queue.insert(1);
        queue.insert(2);
        queue.insert(1); // duplicate — ignored
        queue.insert(3);
        assert_eq!(queue.len(), 3);
        let drained: Vec<_> = core::iter::from_fn(|| queue.pop_front()).collect();
        assert_eq!(drained, vec![1, 2, 3]);
    }
}
