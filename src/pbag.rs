// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent multiset (bag).
//!
//! A `PBag` is an unordered collection that allows duplicate elements,
//! tracking the count of each distinct element. Backed by a
//! [`HashMap<A, usize>`][crate::HashMap], it provides O(log n) insert,
//! remove, and lookup operations with structural sharing.
//!
//! # Examples
//!
//! ```
//! use imbl::PBag;
//!
//! let mut bag = PBag::new();
//! bag.insert("apple");
//! bag.insert("apple");
//! bag.insert("banana");
//!
//! assert_eq!(bag.count(&"apple"), 2);
//! assert_eq!(bag.count(&"banana"), 1);
//! assert_eq!(bag.total_count(), 3);
//! ```

use std::collections::hash_map::RandomState;
use std::fmt::{Debug, Error, Formatter};
use std::hash::{BuildHasher, Hash};
use std::iter::FromIterator;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hashmap::GenericHashMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericPBag`] with default hasher and pointer type.
pub type PBag<A> = GenericPBag<A, RandomState, DefaultSharedPtr>;

/// A persistent multiset (bag) backed by [`GenericHashMap`].
///
/// Tracks the count of each distinct element. Clone is O(1) via
/// structural sharing.
pub struct GenericPBag<A, S = RandomState, P: SharedPointerKind = DefaultSharedPtr> {
    map: GenericHashMap<A, usize, S, P>,
    total: usize,
}

// Manual Clone to avoid derive's spurious `P: Clone` bound.
impl<A: Clone, S: Clone, P: SharedPointerKind> Clone for GenericPBag<A, S, P> {
    fn clone(&self) -> Self {
        GenericPBag {
            map: self.map.clone(),
            total: self.total,
        }
    }
}

impl<A, P> GenericPBag<A, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty bag.
    #[must_use]
    pub fn new() -> Self {
        GenericPBag {
            map: GenericHashMap::new(),
            total: 0,
        }
    }
}

impl<A, S, P> GenericPBag<A, S, P>
where
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    /// Create an empty bag with a custom hasher.
    #[must_use]
    fn new_default() -> Self {
        GenericPBag {
            map: GenericHashMap::default(),
            total: 0,
        }
    }
}

impl<A, S, P> GenericPBag<A, S, P>
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

impl<A, S, P> GenericPBag<A, S, P>
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

impl<A, S, P> GenericPBag<A, S, P>
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

impl<A, P> Default for GenericPBag<A, RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<A, S, P> PartialEq for GenericPBag<A, S, P>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.total == other.total && self.map == other.map
    }
}

impl<A, S, P> Eq for GenericPBag<A, S, P>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P> Debug for GenericPBag<A, S, P>
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

impl<A, S, P> FromIterator<A> for GenericPBag<A, S, P>
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

impl<A, S, P> Extend<A> for GenericPBag<A, S, P>
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn new_bag_is_empty() {
        let bag: PBag<i32> = PBag::new();
        assert!(bag.is_empty());
        assert_eq!(bag.len(), 0);
        assert_eq!(bag.total_count(), 0);
    }

    #[test]
    fn insert_and_count() {
        let mut bag = PBag::new();
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
        let mut bag = PBag::new();
        bag.insert_many("a", 5);
        assert_eq!(bag.count(&"a"), 5);
        assert_eq!(bag.total_count(), 5);
        bag.insert_many("a", 3);
        assert_eq!(bag.count(&"a"), 8);
        assert_eq!(bag.total_count(), 8);
    }

    #[test]
    fn insert_many_zero() {
        let mut bag = PBag::new();
        bag.insert_many("a", 0);
        assert!(bag.is_empty());
        assert_eq!(bag.total_count(), 0);
    }

    #[test]
    fn remove_single() {
        let mut bag = PBag::new();
        bag.insert("a");
        bag.insert("a");
        let prev = bag.remove(&"a");
        assert_eq!(prev, 2);
        assert_eq!(bag.count(&"a"), 1);
        assert_eq!(bag.total_count(), 1);
    }

    #[test]
    fn remove_last_occurrence() {
        let mut bag = PBag::new();
        bag.insert("a");
        bag.remove(&"a");
        assert!(!bag.contains(&"a"));
        assert!(bag.is_empty());
    }

    #[test]
    fn remove_absent() {
        let mut bag: PBag<&str> = PBag::new();
        let prev = bag.remove(&"x");
        assert_eq!(prev, 0);
        assert!(bag.is_empty());
    }

    #[test]
    fn remove_all() {
        let mut bag = PBag::new();
        bag.insert_many("a", 5);
        bag.insert("b");
        let prev = bag.remove_all(&"a");
        assert_eq!(prev, 5);
        assert!(!bag.contains(&"a"));
        assert_eq!(bag.total_count(), 1);
    }

    #[test]
    fn contains() {
        let mut bag = PBag::new();
        assert!(!bag.contains(&1));
        bag.insert(1);
        assert!(bag.contains(&1));
    }

    #[test]
    fn sum_bags() {
        let mut a = PBag::new();
        a.insert_many("x", 2);
        a.insert("y");

        let mut b = PBag::new();
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
        let mut a = PBag::new();
        a.insert_many("x", 3);
        a.insert_many("y", 1);

        let mut b = PBag::new();
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
        let mut a = PBag::new();
        a.insert_many("x", 5);
        a.insert_many("y", 2);

        let mut b = PBag::new();
        b.insert_many("x", 3);
        b.insert_many("y", 10);

        let c = a.difference(&b);
        assert_eq!(c.count(&"x"), 2);
        assert_eq!(c.count(&"y"), 0);
        assert_eq!(c.total_count(), 2);
    }

    #[test]
    fn from_iterator() {
        let bag: PBag<i32> = vec![1, 2, 2, 3, 3, 3].into_iter().collect();
        assert_eq!(bag.count(&1), 1);
        assert_eq!(bag.count(&2), 2);
        assert_eq!(bag.count(&3), 3);
        assert_eq!(bag.total_count(), 6);
        assert_eq!(bag.len(), 3);
    }

    #[test]
    fn clone_shares_structure() {
        let mut bag = PBag::new();
        bag.insert_many("a", 10);
        let bag2 = bag.clone();
        assert_eq!(bag, bag2);
    }

    #[test]
    fn equality() {
        let mut a = PBag::new();
        a.insert(1);
        a.insert(2);
        a.insert(2);

        let mut b = PBag::new();
        b.insert(2);
        b.insert(1);
        b.insert(2);

        assert_eq!(a, b);
    }

    #[test]
    fn inequality_different_counts() {
        let mut a = PBag::new();
        a.insert(1);
        a.insert(1);

        let mut b = PBag::new();
        b.insert(1);

        assert_ne!(a, b);
    }
}
