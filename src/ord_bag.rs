// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent sorted multiset (bag).
//!
//! An [`OrdBag`] is a sorted collection that allows duplicate elements,
//! tracking the count of each distinct element. Backed by an
//! [`OrdMap<A, usize>`][crate::OrdMap], it provides O(log n) insert,
//! remove, and lookup operations with structural sharing and deterministic
//! sorted iteration order. Because iteration order is canonical (sorted by
//! `A`), `OrdBag` implements `PartialOrd`, `Ord`, and `Hash` with no
//! additional requirements beyond `A: Ord`.
//!
//! Prefer [`OrdBag`] over [`Bag`][crate::Bag] when you need:
//! - Sorted iteration without a separate sort step
//! - `PartialOrd` / `Ord` on the bag itself
//! - Range queries over elements
//! - No `Hash + Eq` bound on `A`
//!
//! # Examples
//!
//! ```
//! use pds::OrdBag;
//!
//! let mut bag = OrdBag::new();
//! bag.insert("apple");
//! bag.insert("apple");
//! bag.insert("banana");
//!
//! assert_eq!(bag.count(&"apple"), 2);
//! assert_eq!(bag.count(&"banana"), 1);
//! assert_eq!(bag.total_count(), 3);
//!
//! // Iteration is always in sorted order.
//! let items: Vec<_> = bag.iter().collect();
//! assert_eq!(items, vec![(&"apple", 2), (&"banana", 1)]);
//! ```
//!
//! ## Range queries
//!
//! ```
//! use pds::OrdBag;
//!
//! let mut bag = OrdBag::new();
//! for x in [3, 1, 4, 1, 5, 9, 2, 6] {
//!     bag.insert(x);
//! }
//! let mid: Vec<_> = bag.range(2..=5).collect();
//! assert_eq!(mid, vec![(&2, 1), (&3, 1), (&4, 1), (&5, 1)]);
//! ```
//!
//! ## Parallel iteration (`rayon` feature)
//!
//! With the `rayon` feature, `OrdBag` provides a parallel iterator via
//! [`par_iter()`][rayon::iter::IntoParallelRefIterator::par_iter] — yields
//! `(&A, usize)` pairs in parallel (one per distinct element).

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;
use core::ops::RangeBounds;

use archery::SharedPointerKind;
use equivalent::Comparable;

use crate::ordmap::{ConsumingIter as MapConsumingIter, GenericOrdMap};
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericOrdBag`] with the default pointer type.
///
/// Unlike [`Bag`][crate::Bag], `OrdBag` requires no hasher and works in
/// `no_std` environments without the `foldhash` feature.
pub type OrdBag<A> = GenericOrdBag<A, DefaultSharedPtr>;

/// A persistent sorted multiset (bag) backed by [`GenericOrdMap`].
///
/// Tracks the count of each distinct element. Clone is O(1) via structural
/// sharing. Iteration is always in ascending order of `A`.
pub struct GenericOrdBag<A, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) map: GenericOrdMap<A, usize, P>,
    pub(crate) total: usize,
}

// Manual Clone to avoid spurious `P: Clone` bound from derive.
impl<A: Clone, P: SharedPointerKind> Clone for GenericOrdBag<A, P> {
    fn clone(&self) -> Self {
        GenericOrdBag {
            map: self.map.clone(),
            total: self.total,
        }
    }
}

impl<A, P: SharedPointerKind> GenericOrdBag<A, P> {
    /// Create an empty bag.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdBag {
            map: GenericOrdMap::default(),
            total: 0,
        }
    }

    /// Test whether a bag is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Return the number of distinct elements in the bag.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Return the total count of all elements (sum of all multiplicities).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.total
    }
}

impl<A, P> GenericOrdBag<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
    /// Return the count of a specific element.
    #[must_use]
    pub fn count<Q>(&self, value: &Q) -> usize
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.get(value).copied().unwrap_or(0)
    }

    /// Test whether the bag contains at least one occurrence of the given element.
    #[must_use]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.contains_key(value)
    }
}

impl<A, P> GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    /// Insert one occurrence of a value, returning the previous count.
    pub fn insert(&mut self, value: A) -> usize {
        let prev = self.count(&value);
        self.map.insert(value, prev + 1);
        self.total += 1;
        prev
    }

    /// Insert `n` occurrences of a value, returning the previous count.
    pub fn insert_many(&mut self, value: A, n: usize) -> usize {
        if n == 0 {
            return self.count(&value);
        }
        let prev = self.count(&value);
        self.map.insert(value, prev + n);
        self.total += n;
        prev
    }

    /// Remove one occurrence of a value, returning the previous count.
    ///
    /// If the element is not present, returns 0 and makes no changes.
    pub fn remove<Q>(&mut self, value: &Q) -> usize
    where
        Q: Comparable<A> + ?Sized,
    {
        let prev = self.count(value);
        if prev == 0 {
            return 0;
        }
        if prev == 1 {
            self.map.remove(value);
        } else {
            // `extract_with_key` is the canonical way to recover the owned key
            // from a `&Q` query. The returned new map is discarded — we only
            // use `k` to re-insert with a decremented count.
            if let Some((k, _, _)) = self.map.extract_with_key(value) {
                self.map.insert(k, prev - 1);
            }
        }
        self.total -= 1;
        prev
    }

    /// Remove all occurrences of a value, returning the previous count.
    pub fn remove_all<Q>(&mut self, value: &Q) -> usize
    where
        Q: Comparable<A> + ?Sized,
    {
        match self.map.remove_with_key(value) {
            Some((_, count)) => {
                self.total -= count;
                count
            }
            None => 0,
        }
    }

    /// Return the multiset union (sum of multiplicities).
    ///
    /// For each element, the result count is the sum of counts in both bags.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut result = self.clone();
        for (k, &count) in other.map.iter() {
            let prev = result.count(k);
            result.map.insert(k.clone(), prev + count);
            result.total += count;
        }
        result
    }

    /// Return the multiset intersection (minimum multiplicities).
    ///
    /// For each element, the result count is the minimum of the counts in
    /// both bags.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut result = Self::new();
        for (k, &count) in self.map.iter() {
            let other_count = other.count(k);
            let min = count.min(other_count);
            if min > 0 {
                result.map.insert(k.clone(), min);
                result.total += min;
            }
        }
        result
    }

    /// Return the multiset relative complement (`self` minus `other`).
    ///
    /// For each element, the result count is `self.count − other.count`,
    /// clamped to zero.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        let mut result = Self::new();
        for (k, &count) in self.map.iter() {
            let other_count = other.count(k);
            let diff = count.saturating_sub(other_count);
            if diff > 0 {
                result.map.insert(k.clone(), diff);
                result.total += diff;
            }
        }
        result
    }

    /// Return the multiset symmetric difference (absolute difference of multiplicities).
    ///
    /// For each element, the result count is `|self.count − other.count|`.
    /// Elements whose counts are equal in both bags are excluded.
    #[must_use]
    pub fn symmetric_difference(&self, other: &Self) -> Self {
        let mut result = Self::new();
        // Elements where self_count > other_count.
        for (k, &self_count) in self.map.iter() {
            let other_count = other.count(k);
            if self_count > other_count {
                let diff = self_count - other_count;
                result.map.insert(k.clone(), diff);
                result.total += diff;
            }
        }
        // Elements where other_count > self_count (only in other or higher count in other).
        for (k, &other_count) in other.map.iter() {
            let self_count = self.count(k);
            if other_count > self_count {
                let diff = other_count - self_count;
                result.map.insert(k.clone(), diff);
                result.total += diff;
            }
        }
        result
    }
}

impl<A, P> GenericOrdBag<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
    /// Iterate over distinct elements and their counts in ascending element order.
    pub fn iter(&self) -> impl Iterator<Item = (&A, usize)> {
        self.map.iter().map(|(k, &v)| (k, v))
    }

    /// Iterate over a range of elements and their counts in ascending order.
    ///
    /// The range is bounded by element value, not by index or count.
    pub fn range<R, Q>(&self, range: R) -> impl Iterator<Item = (&A, usize)> + '_
    where
        R: RangeBounds<Q>,
        Q: Comparable<A> + ?Sized,
    {
        self.map.range(range).map(|(k, &v)| (k, v))
    }
}

impl<A, P> Default for GenericOrdBag<A, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericOrdBag {
            map: GenericOrdMap::default(),
            total: 0,
        }
    }
}

impl<A, P> PartialEq for GenericOrdBag<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.total == other.total && self.map == other.map
    }
}

impl<A, P> Eq for GenericOrdBag<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
}

impl<A, P> PartialOrd for GenericOrdBag<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<A, P> Ord for GenericOrdBag<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
    fn cmp(&self, other: &Self) -> Ordering {
        // Lexicographic comparison over (element, count) pairs in sorted order.
        self.map.iter().cmp(other.map.iter())
    }
}

impl<A, P> Hash for GenericOrdBag<A, P>
where
    A: Ord + Hash,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        // Iteration is in canonical sorted order, so sequential hashing is
        // deterministic and well-defined (unlike unordered Bag's XOR combiner).
        self.len().hash(state);
        for (k, count) in self.iter() {
            k.hash(state);
            count.hash(state);
        }
    }
}

impl<A, P> Debug for GenericOrdBag<A, P>
where
    A: Ord + Debug,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, count) in self.iter() {
            d.entry(k, &count);
        }
        d.finish()
    }
}

impl<A, P> FromIterator<A> for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        let mut bag = Self::new();
        for item in iter {
            bag.insert(item);
        }
        bag
    }
}

impl<A, P> Extend<A> for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = A>>(&mut self, iter: I) {
        for item in iter {
            self.insert(item);
        }
    }
}

impl<A, P> From<Vec<A>> for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(v: Vec<A>) -> Self {
        v.into_iter().collect()
    }
}

impl<A, const N: usize, P> From<[A; N]> for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A, P> From<&'a [A]> for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(slice: &'a [A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, A, P> From<&'a Vec<A>> for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<A>) -> Self {
        v.iter().cloned().collect()
    }
}

/// A consuming iterator over the elements of a [`GenericOrdBag`].
///
/// Each item is `(element, count)` in ascending element order.
pub struct ConsumingIter<A, P: SharedPointerKind> {
    inner: MapConsumingIter<A, usize, P>,
}

impl<A, P> Iterator for ConsumingIter<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    type Item = (A, usize);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<A, P> ExactSizeIterator for ConsumingIter<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
}

impl<A, P> core::iter::FusedIterator for ConsumingIter<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
}

impl<A, P> IntoIterator for GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    type Item = (A, usize);
    type IntoIter = ConsumingIter<A, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.map.into_iter(),
        }
    }
}

impl<'a, A, P> IntoIterator for &'a GenericOrdBag<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    type Item = (&'a A, usize);
    type IntoIter = core::iter::Map<
        crate::ordmap::Iter<'a, A, usize, P>,
        fn((&'a A, &'a usize)) -> (&'a A, usize),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter().map(|(k, &v)| (k, v))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(OrdBag<i32>: Send, Sync);

    #[test]
    fn new_bag_is_empty() {
        let bag: OrdBag<i32> = OrdBag::new();
        assert!(bag.is_empty());
        assert_eq!(bag.len(), 0);
        assert_eq!(bag.total_count(), 0);
    }

    #[test]
    fn insert_and_count() {
        let mut bag = OrdBag::new();
        bag.insert("a");
        bag.insert("a");
        bag.insert("b");
        assert_eq!(bag.count(&"a"), 2);
        assert_eq!(bag.count(&"b"), 1);
        assert_eq!(bag.count(&"c"), 0);
        assert_eq!(bag.len(), 2);
        assert_eq!(bag.total_count(), 3);
    }

    #[test]
    fn insert_many() {
        let mut bag = OrdBag::new();
        bag.insert_many("a", 5);
        assert_eq!(bag.count(&"a"), 5);
        assert_eq!(bag.total_count(), 5);
        bag.insert_many("a", 3);
        assert_eq!(bag.count(&"a"), 8);
        assert_eq!(bag.total_count(), 8);
    }

    #[test]
    fn insert_many_zero() {
        let mut bag = OrdBag::new();
        bag.insert_many("a", 0);
        assert!(bag.is_empty());
        assert_eq!(bag.total_count(), 0);
    }

    #[test]
    fn remove_single() {
        let mut bag = OrdBag::new();
        bag.insert("a");
        bag.insert("a");
        let prev = bag.remove(&"a");
        assert_eq!(prev, 2);
        assert_eq!(bag.count(&"a"), 1);
        assert_eq!(bag.total_count(), 1);
    }

    #[test]
    fn remove_last_occurrence() {
        let mut bag = OrdBag::new();
        bag.insert("a");
        bag.remove(&"a");
        assert!(!bag.contains(&"a"));
        assert!(bag.is_empty());
    }

    #[test]
    fn remove_absent() {
        let mut bag: OrdBag<&str> = OrdBag::new();
        let prev = bag.remove(&"x");
        assert_eq!(prev, 0);
        assert!(bag.is_empty());
    }

    #[test]
    fn remove_all() {
        let mut bag = OrdBag::new();
        bag.insert_many("a", 5);
        bag.insert("b");
        let prev = bag.remove_all(&"a");
        assert_eq!(prev, 5);
        assert!(!bag.contains(&"a"));
        assert_eq!(bag.total_count(), 1);
    }

    #[test]
    fn contains() {
        let mut bag = OrdBag::new();
        assert!(!bag.contains(&1));
        bag.insert(1);
        assert!(bag.contains(&1));
    }

    #[test]
    fn iter_is_sorted() {
        let mut bag = OrdBag::new();
        bag.insert(3);
        bag.insert(1);
        bag.insert(2);
        bag.insert(1);
        let items: Vec<_> = bag.iter().map(|(k, c)| (*k, c)).collect();
        assert_eq!(items, vec![(1, 2), (2, 1), (3, 1)]);
    }

    #[test]
    fn range_query() {
        let bag: OrdBag<i32> = vec![1, 2, 2, 3, 4, 5].into_iter().collect();
        let items: Vec<_> = bag.range(2..=4).map(|(k, c)| (*k, c)).collect();
        assert_eq!(items, vec![(2, 2), (3, 1), (4, 1)]);
    }

    #[test]
    fn union_bags() {
        let mut a = OrdBag::new();
        a.insert_many("x", 2);
        a.insert("y");

        let mut b = OrdBag::new();
        b.insert_many("x", 3);
        b.insert("z");

        let c = a.union(&b);
        assert_eq!(c.count(&"x"), 5);
        assert_eq!(c.count(&"y"), 1);
        assert_eq!(c.count(&"z"), 1);
        assert_eq!(c.total_count(), 7);
    }

    #[test]
    fn intersection_bags() {
        let mut a = OrdBag::new();
        a.insert_many("x", 3);
        a.insert_many("y", 1);

        let mut b = OrdBag::new();
        b.insert_many("x", 2);
        b.insert_many("z", 5);

        let c = a.intersection(&b);
        assert_eq!(c.count(&"x"), 2);
        assert_eq!(c.count(&"y"), 0);
        assert_eq!(c.count(&"z"), 0);
        assert_eq!(c.total_count(), 2);
    }

    #[test]
    fn difference_bags() {
        let mut a = OrdBag::new();
        a.insert_many("x", 5);
        a.insert_many("y", 2);

        let mut b = OrdBag::new();
        b.insert_many("x", 3);
        b.insert_many("y", 10);

        let c = a.difference(&b);
        assert_eq!(c.count(&"x"), 2);
        assert_eq!(c.count(&"y"), 0);
        assert_eq!(c.total_count(), 2);
    }

    #[test]
    fn symmetric_difference_bags() {
        let mut a = OrdBag::new();
        a.insert_many("x", 5);
        a.insert_many("y", 2);
        a.insert_many("z", 3);

        let mut b = OrdBag::new();
        b.insert_many("x", 3);
        b.insert_many("y", 2); // equal — excluded
        b.insert_many("w", 1);

        let c = a.symmetric_difference(&b);
        assert_eq!(c.count(&"x"), 2); // |5-3|
        assert_eq!(c.count(&"y"), 0); // equal, excluded
        assert_eq!(c.count(&"z"), 3); // only in a
        assert_eq!(c.count(&"w"), 1); // only in b
        assert_eq!(c.total_count(), 6);
    }

    #[test]
    fn symmetric_difference_disjoint() {
        let mut a = OrdBag::new();
        a.insert_many("a", 2);

        let mut b = OrdBag::new();
        b.insert_many("b", 3);

        let c = a.symmetric_difference(&b);
        assert_eq!(c.count(&"a"), 2);
        assert_eq!(c.count(&"b"), 3);
        assert_eq!(c.total_count(), 5);
    }

    #[test]
    fn partial_eq() {
        let mut a = OrdBag::new();
        a.insert(1);
        a.insert(2);
        a.insert(2);

        let mut b = OrdBag::new();
        b.insert(2);
        b.insert(1);
        b.insert(2);

        // Same elements and counts regardless of insertion order.
        assert_eq!(a, b);
    }

    #[test]
    fn inequality_different_counts() {
        let mut a = OrdBag::new();
        a.insert(1);
        a.insert(1);

        let mut b = OrdBag::new();
        b.insert(1);

        assert_ne!(a, b);
    }

    #[test]
    fn ord_comparison() {
        let mut a: OrdBag<i32> = OrdBag::new();
        a.insert(1);
        a.insert(2);

        let mut b: OrdBag<i32> = OrdBag::new();
        b.insert(1);
        b.insert(3); // 3 > 2

        assert!(a < b);
        assert!(b > a);
    }

    #[test]
    fn hash_is_order_independent() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(b: &OrdBag<i32>) -> u64 {
            let mut h = DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        }
        // Two bags with the same elements in different insertion order must
        // hash identically because both sort to the same canonical sequence.
        let mut a = OrdBag::new();
        a.insert(1);
        a.insert(2);
        let mut b = OrdBag::new();
        b.insert(2);
        b.insert(1);
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn from_iterator() {
        let bag: OrdBag<i32> = vec![1, 2, 2, 3, 3, 3].into_iter().collect();
        assert_eq!(bag.count(&1), 1);
        assert_eq!(bag.count(&2), 2);
        assert_eq!(bag.count(&3), 3);
        assert_eq!(bag.total_count(), 6);
        assert_eq!(bag.len(), 3);
    }

    #[test]
    fn clone_shares_structure() {
        let mut bag = OrdBag::new();
        bag.insert_many("a", 10);
        let bag2 = bag.clone();
        assert_eq!(bag, bag2);
    }

    #[test]
    fn into_iter_owned() {
        let mut bag = OrdBag::new();
        bag.insert("a");
        bag.insert("a");
        bag.insert("b");
        let items: Vec<_> = bag.into_iter().collect();
        // ConsumingIter preserves sorted order.
        assert_eq!(items, vec![("a", 2), ("b", 1)]);
    }

    #[test]
    fn into_iter_ref() {
        let mut bag = OrdBag::new();
        bag.insert("a");
        bag.insert("b");
        let items: Vec<_> = (&bag).into_iter().collect();
        assert_eq!(items, vec![(&"a", 1), (&"b", 1)]);
    }

    #[test]
    fn for_loop() {
        let mut bag = OrdBag::new();
        bag.insert(1);
        bag.insert(2);
        bag.insert(2);
        let mut total = 0;
        for (_, count) in &bag {
            total += count;
        }
        assert_eq!(total, 3);
    }

    #[test]
    fn debug_format() {
        let mut b = OrdBag::new();
        b.insert(1i32);
        let s = format!("{:?}", b);
        assert!(!s.is_empty());
    }

    #[test]
    fn default_is_empty() {
        let b: OrdBag<i32> = OrdBag::default();
        assert!(b.is_empty());
    }

    #[test]
    fn extend_adds_elements() {
        let mut b: OrdBag<i32> = OrdBag::new();
        b.extend(vec![1, 1, 2]);
        assert_eq!(b.count(&1), 2);
        assert_eq!(b.count(&2), 1);
    }

    #[test]
    fn from_vec() {
        let b: OrdBag<i32> = vec![1, 1, 2].into();
        assert_eq!(b.count(&1), 2);
    }

    #[test]
    fn from_array() {
        let b: OrdBag<i32> = [1i32, 2, 2].into();
        assert_eq!(b.count(&2), 2);
    }

    #[test]
    fn from_slice() {
        let b: OrdBag<i32> = [1i32, 2][..].into();
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![1i32, 1, 2];
        let b: OrdBag<i32> = OrdBag::from(&v);
        assert_eq!(b.count(&1), 2);
        assert_eq!(b.count(&2), 1);
    }
}
