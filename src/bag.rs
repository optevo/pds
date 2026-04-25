// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent multiset (bag).
//!
//! A [`Bag`] is an unordered collection that allows duplicate elements,
//! tracking the count of each distinct element. Backed by a
//! [`HashMap<A, usize>`][crate::HashMap], it provides O(log n) insert,
//! remove, and lookup operations with structural sharing.
//!
//! # Examples
//!
//! ```
//! use pds::Bag;
//!
//! let mut bag = Bag::new();
//! bag.insert("apple");
//! bag.insert("apple");
//! bag.insert("banana");
//!
//! assert_eq!(bag.count(&"apple"), 2);
//! assert_eq!(bag.count(&"banana"), 1);
//! assert_eq!(bag.total_count(), 3);
//! ```

#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::{FromIterator, Sum};
use core::ops::Add;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hashmap::GenericHashMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericBag`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type Bag<A> = GenericBag<A, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericBag`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type Bag<A> = GenericBag<A, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent multiset (bag) backed by [`GenericHashMap`].
///
/// Tracks the count of each distinct element. Clone is O(1) via
/// structural sharing.
pub struct GenericBag<A, S, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) map: GenericHashMap<A, usize, S, P>,
    pub(crate) total: usize,
}

// Manual Clone to avoid derive's spurious `P: Clone` bound.
impl<A: Clone, S: Clone, P: SharedPointerKind> Clone for GenericBag<A, S, P> {
    fn clone(&self) -> Self {
        GenericBag {
            map: self.map.clone(),
            total: self.total,
        }
    }
}

#[cfg(feature = "std")]
impl<A, P> GenericBag<A, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty bag.
    #[must_use]
    pub fn new() -> Self {
        GenericBag {
            map: GenericHashMap::new(),
            total: 0,
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<A, P> GenericBag<A, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty bag (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericBag {
            map: GenericHashMap::new(),
            total: 0,
        }
    }
}

impl<A, S, P> GenericBag<A, S, P>
where
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    /// Create an empty bag with a custom hasher.
    #[must_use]
    fn new_default() -> Self {
        GenericBag {
            map: GenericHashMap::default(),
            total: 0,
        }
    }
}

impl<A, S, P> GenericBag<A, S, P>
where
    P: SharedPointerKind,
{
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

impl<A, S, P> GenericBag<A, S, P>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Return the count of a specific element.
    #[must_use]
    pub fn count<Q>(&self, value: &Q) -> usize
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        self.map.get(value).copied().unwrap_or(0)
    }

    /// Test whether the bag contains at least one of the given element.
    #[must_use]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        self.map.contains_key(value)
    }
}

impl<A, S, P> GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
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
        Q: Hash + Equivalent<A> + ?Sized,
    {
        let prev = self.count(value);
        if prev == 0 {
            return 0;
        }
        if prev == 1 {
            self.map.remove(value);
        } else {
            // Re-insert with decremented count. extract_with_key returns
            // (key, value, new_map) — we need the owned key for re-insert.
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
        Q: Hash + Equivalent<A> + ?Sized,
    {
        match self.map.remove_with_key(value) {
            Some((_, count)) => {
                self.total -= count;
                count
            }
            None => 0,
        }
    }

    /// Return the multiset sum (union with added multiplicities).
    ///
    /// For each element, the result count is the sum of counts in
    /// both bags.
    #[must_use]
    pub fn sum(&self, other: &Self) -> Self {
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
    /// For each element, the result count is the minimum of the counts
    /// in both bags.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self
    where
        S: Default,
    {
        let mut result = Self::new_default();
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };
        for (k, &count) in smaller.map.iter() {
            let other_count = larger.count(k);
            let min = count.min(other_count);
            if min > 0 {
                result.map.insert(k.clone(), min);
                result.total += min;
            }
        }
        result
    }

    /// Return the multiset difference.
    ///
    /// For each element, the result count is `self.count - other.count`,
    /// clamped to zero.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self
    where
        S: Default,
    {
        let mut result = Self::new_default();
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

    /// Iterate over distinct elements and their counts.
    pub fn iter(&self) -> impl Iterator<Item = (&A, usize)> {
        self.map.iter().map(|(k, &v)| (k, v))
    }
}

impl<A, S, P> Default for GenericBag<A, S, P>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericBag {
            map: crate::hashmap::GenericHashMap::default(),
            total: 0,
        }
    }
}

impl<A, S, P> PartialEq for GenericBag<A, S, P>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.total == other.total && self.map == other.map
    }
}

impl<A, S, P> Eq for GenericBag<A, S, P>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P> Hash for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        self.len().hash(state);
        // Order-independent: wrapping_add of per-entry hashes.
        // Each (element, count) pair is hashed as a unit.
        let mut combined: u64 = 0;
        for (a, count) in self.iter() {
            let mut h = crate::util::FnvHasher::new();
            a.hash(&mut h);
            count.hash(&mut h);
            combined = combined.wrapping_add(h.finish());
        }
        combined.hash(state);
    }
}

impl<A, S, P> Debug for GenericBag<A, S, P>
where
    A: Debug + Hash + Eq + Clone,
    S: BuildHasher + Clone,
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

impl<A, S, P> FromIterator<A> for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        let mut bag = Self::new_default();
        for item in iter {
            bag.insert(item);
        }
        bag
    }
}

impl<A, S, P> Extend<A> for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = A>>(&mut self, iter: I) {
        for item in iter {
            self.insert(item);
        }
    }
}

impl<A, S, P> From<Vec<A>> for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: Vec<A>) -> Self {
        v.into_iter().collect()
    }
}

impl<A, S, const N: usize, P> From<[A; N]> for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A, S, P> From<&'a [A]> for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, A, S, P> From<&'a Vec<A>> for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<A>) -> Self {
        v.iter().cloned().collect()
    }
}

impl<A, S, P> Add for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = GenericBag<A, S, P>;

    fn add(self, other: Self) -> Self::Output {
        self.sum(&other)
    }
}

impl<A, S, P> Add for &GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = GenericBag<A, S, P>;

    fn add(self, other: Self) -> Self::Output {
        self.sum(other)
    }
}

impl<A, S, P: SharedPointerKind> Sum for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn sum<I>(it: I) -> Self
    where
        I: Iterator<Item = Self>,
    {
        it.fold(Self::default(), |a, b| a + b)
    }
}

/// A consuming iterator over the elements of a [`GenericBag`].
///
/// Each item is `(element, count)`.
pub struct ConsumingIter<A: Hash + Eq, S, P: SharedPointerKind> {
    inner: crate::hashmap::ConsumingIter<(A, usize), P>,
    _phantom: core::marker::PhantomData<S>,
}

impl<A, S, P> Iterator for ConsumingIter<A, S, P>
where
    A: Hash + Eq + Clone,
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

impl<A, S, P> ExactSizeIterator for ConsumingIter<A, S, P>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P> core::iter::FusedIterator for ConsumingIter<A, S, P>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P> IntoIterator for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = (A, usize);
    type IntoIter = ConsumingIter<A, S, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.map.into_iter(),
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<'a, A, S, P> IntoIterator for &'a GenericBag<A, S, P>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Item = (&'a A, usize);
    type IntoIter = core::iter::Map<
        crate::hashmap::Iter<'a, A, usize, P>,
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

    assert_impl_all!(crate::Bag<i32>: Send, Sync);

    #[test]
    fn new_bag_is_empty() {
        let bag: Bag<i32> = Bag::new();
        assert!(bag.is_empty());
        assert_eq!(bag.len(), 0);
        assert_eq!(bag.total_count(), 0);
    }

    #[test]
    fn insert_and_count() {
        let mut bag = Bag::new();
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
        let mut bag = Bag::new();
        bag.insert_many("a", 5);
        assert_eq!(bag.count(&"a"), 5);
        assert_eq!(bag.total_count(), 5);
        bag.insert_many("a", 3);
        assert_eq!(bag.count(&"a"), 8);
        assert_eq!(bag.total_count(), 8);
    }

    #[test]
    fn insert_many_zero() {
        let mut bag = Bag::new();
        bag.insert_many("a", 0);
        assert!(bag.is_empty());
        assert_eq!(bag.total_count(), 0);
    }

    #[test]
    fn remove_single() {
        let mut bag = Bag::new();
        bag.insert("a");
        bag.insert("a");
        let prev = bag.remove(&"a");
        assert_eq!(prev, 2);
        assert_eq!(bag.count(&"a"), 1);
        assert_eq!(bag.total_count(), 1);
    }

    #[test]
    fn remove_last_occurrence() {
        let mut bag = Bag::new();
        bag.insert("a");
        bag.remove(&"a");
        assert!(!bag.contains(&"a"));
        assert!(bag.is_empty());
    }

    #[test]
    fn remove_absent() {
        let mut bag: Bag<&str> = Bag::new();
        let prev = bag.remove(&"x");
        assert_eq!(prev, 0);
        assert!(bag.is_empty());
    }

    #[test]
    fn remove_all() {
        let mut bag = Bag::new();
        bag.insert_many("a", 5);
        bag.insert("b");
        let prev = bag.remove_all(&"a");
        assert_eq!(prev, 5);
        assert!(!bag.contains(&"a"));
        assert_eq!(bag.total_count(), 1);
    }

    #[test]
    fn contains() {
        let mut bag = Bag::new();
        assert!(!bag.contains(&1));
        bag.insert(1);
        assert!(bag.contains(&1));
    }

    #[test]
    fn sum_bags() {
        let mut a = Bag::new();
        a.insert_many("x", 2);
        a.insert("y");

        let mut b = Bag::new();
        b.insert_many("x", 3);
        b.insert("z");

        let c = a.sum(&b);
        assert_eq!(c.count(&"x"), 5);
        assert_eq!(c.count(&"y"), 1);
        assert_eq!(c.count(&"z"), 1);
        assert_eq!(c.total_count(), 7);
    }

    #[test]
    fn intersection_bags() {
        let mut a = Bag::new();
        a.insert_many("x", 3);
        a.insert_many("y", 1);

        let mut b = Bag::new();
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
        let mut a = Bag::new();
        a.insert_many("x", 5);
        a.insert_many("y", 2);

        let mut b = Bag::new();
        b.insert_many("x", 3);
        b.insert_many("y", 10);

        let c = a.difference(&b);
        assert_eq!(c.count(&"x"), 2);
        assert_eq!(c.count(&"y"), 0);
        assert_eq!(c.total_count(), 2);
    }

    #[test]
    fn from_iterator() {
        let bag: Bag<i32> = vec![1, 2, 2, 3, 3, 3].into_iter().collect();
        assert_eq!(bag.count(&1), 1);
        assert_eq!(bag.count(&2), 2);
        assert_eq!(bag.count(&3), 3);
        assert_eq!(bag.total_count(), 6);
        assert_eq!(bag.len(), 3);
    }

    #[test]
    fn clone_shares_structure() {
        let mut bag = Bag::new();
        bag.insert_many("a", 10);
        let bag2 = bag.clone();
        assert_eq!(bag, bag2);
    }

    #[test]
    fn equality() {
        let mut a = Bag::new();
        a.insert(1);
        a.insert(2);
        a.insert(2);

        let mut b = Bag::new();
        b.insert(2);
        b.insert(1);
        b.insert(2);

        assert_eq!(a, b);
    }

    #[test]
    fn into_iter_owned() {
        let mut bag = Bag::new();
        bag.insert("a");
        bag.insert("a");
        bag.insert("b");

        let mut items: Vec<_> = bag.into_iter().collect();
        items.sort_by_key(|(k, _)| *k);
        assert_eq!(items, vec![("a", 2), ("b", 1)]);
    }

    #[test]
    fn into_iter_ref() {
        let mut bag = Bag::new();
        bag.insert("a");
        bag.insert("b");

        let mut items: Vec<_> = (&bag).into_iter().collect();
        items.sort_by_key(|(k, _)| *k);
        assert_eq!(items, vec![(&"a", 1), (&"b", 1)]);
    }

    #[test]
    fn for_loop() {
        let mut bag = Bag::new();
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
    fn inequality_different_counts() {
        let mut a = Bag::new();
        a.insert(1);
        a.insert(1);

        let mut b = Bag::new();
        b.insert(1);

        assert_ne!(a, b);
    }

    #[test]
    fn debug_format() {
        let mut b = Bag::new();
        b.insert(1i32);
        let s = format!("{:?}", b);
        assert!(!s.is_empty());
    }

    #[test]
    fn default_is_empty() {
        let b: Bag<i32> = Bag::default();
        assert!(b.is_empty());
    }

    #[test]
    fn hash_order_independent() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(b: &Bag<i32>) -> u64 {
            let mut h = DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        }
        let mut a = Bag::new();
        a.insert(1); a.insert(2);
        let mut b = Bag::new();
        b.insert(2); b.insert(1); // different insertion order
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn add_union_owned() {
        let mut a = Bag::new();
        a.insert(1i32); a.insert(2);
        let mut b = Bag::new();
        b.insert(2i32); b.insert(3);
        let c = a + b;
        assert_eq!(c.count(&1), 1);
        assert_eq!(c.count(&2), 2); // appears in both
        assert_eq!(c.count(&3), 1);
    }

    #[test]
    fn add_union_ref() {
        let mut a = Bag::new();
        a.insert(1i32);
        let mut b = Bag::new();
        b.insert(2i32);
        let c = &a + &b;
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn extend_adds_elements() {
        let mut b: Bag<i32> = Bag::new();
        b.extend(vec![1, 1, 2]);
        assert_eq!(b.count(&1), 2);
        assert_eq!(b.count(&2), 1);
    }

    #[test]
    fn from_vec() {
        let b: Bag<i32> = vec![1, 1, 2].into();
        assert_eq!(b.count(&1), 2);
    }

    #[test]
    fn from_array() {
        let b: Bag<i32> = [1i32, 2, 2].into();
        assert_eq!(b.count(&2), 2);
    }

    #[test]
    fn from_slice() {
        let b: Bag<i32> = [1i32, 2][..].into();
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![1i32, 1, 2];
        let b: Bag<i32> = Bag::from(&v);
        assert_eq!(b.count(&1), 2);
        assert_eq!(b.count(&2), 1);
    }

    #[test]
    fn sum_via_iterator() {
        // Tests the Sum trait via Iterator::sum(), distinct from the sum() method.
        let bags: Vec<Bag<i32>> = vec![
            {let mut b = Bag::new(); b.insert(1); b.insert(1); b},
            {let mut b = Bag::new(); b.insert(2); b},
        ];
        let total: Bag<i32> = bags.into_iter().sum();
        assert_eq!(total.count(&1), 2);
        assert_eq!(total.count(&2), 1);
    }
}
