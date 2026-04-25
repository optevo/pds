// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! An ordered set.
//!
//! An immutable ordered set implemented as a [B+tree] [1].
//!
//! Most operations on this type of set are O(log n). A
//! [`GenericHashSet`] is usually a better choice for
//! performance, but the `OrdSet` has the advantage of only requiring
//! an [`Ord`][std::cmp::Ord] constraint on its values, and of being
//! ordered, so values always come out from lowest to highest, where a
//! [`GenericHashSet`] has no guaranteed ordering.
//!
//! [1]: https://en.wikipedia.org/wiki/B%2B_tree

use alloc::borrow::ToOwned;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::{FromIterator, FusedIterator};
use core::ops::RangeBounds;

use archery::SharedPointerKind;
use equivalent::Comparable;

use super::map;
use crate::hashset::GenericHashSet;
use crate::shared_ptr::DefaultSharedPtr;
use crate::GenericOrdMap;

/// Construct a set from a sequence of values.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use pds::ordset::OrdSet;
/// # fn main() {
/// assert_eq!(
///   ordset![1, 2, 3],
///   OrdSet::from(vec![1, 2, 3])
/// );
/// # }
/// ```
#[macro_export]
macro_rules! ordset {
    () => { $crate::ordset::OrdSet::new() };

    ( $($x:expr),* ) => {{
        let mut l = $crate::ordset::OrdSet::new();
        $(
            l.insert($x);
        )*
            l
    }};
}

/// Type alias for [`GenericOrdSet`] that uses [`DefaultSharedPtr`] as the pointer type.
///
/// [GenericOrdSet]: ./struct.GenericOrdSet.html
/// [DefaultSharedPtr]: ../shared_ptr/type.DefaultSharedPtr.html
pub type OrdSet<A> = GenericOrdSet<A, DefaultSharedPtr>;

/// An ordered set.
///
/// An immutable ordered set implemented as a [B+ tree][1].
///
/// ## Complexity vs Standard Library
///
/// | Operation | `OrdSet` | [`BTreeSet`] |
/// |---|---|---|
/// | `clone` | **O(1)** | O(n) |
/// | `eq` | O(n) | O(n) |
/// | `contains` | O(log n) | O(log n) |
/// | `insert` | O(log n) | O(log n) |
/// | `remove` | O(log n) | O(log n) |
/// | `split_at` | **O(log n)** | O(n) |
/// | `union` / `intersection` | O(n + m) | O(n + m) |
/// | `range` | O(log n + k) | O(log n + k) |
/// | `from_iter` | O(n log n) | O(n log n) |
///
/// **Bold** = asymptotically better than the std alternative.
///
/// The key advantage is `clone` in O(1) via structural sharing. Two
/// sets from a common ancestor share all unmodified nodes in memory.
/// `split_at` is also O(log n) vs O(n).
///
/// [`HashSet`][hashset::HashSet] is usually a better choice when
/// ordering isn't required, but `OrdSet` only needs
/// [`Ord`][std::cmp::Ord] (not `Hash + Eq`) and keeps values sorted.
///
/// [`BTreeSet`]: https://doc.rust-lang.org/std/collections/struct.BTreeSet.html
/// [hashset::HashSet]: ../hashset/type.HashSet.html
/// [1]: https://en.wikipedia.org/wiki/B%2B_tree
pub struct GenericOrdSet<A, P: SharedPointerKind> {
    pub(crate) map: GenericOrdMap<A, (), P>,
}

impl<A, P: SharedPointerKind> GenericOrdSet<A, P> {
    /// Construct an empty set.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        GenericOrdSet {
            map: GenericOrdMap::new(),
        }
    }

    /// Construct a set with a single value.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # type OrdSet<T> = pds::ordset::OrdSet<T>;
    /// let set = OrdSet::unit(123);
    /// assert!(set.contains(&123));
    /// ```
    #[inline]
    #[must_use]
    pub fn unit(a: A) -> Self {
        GenericOrdSet {
            map: GenericOrdMap::unit(a, ()),
        }
    }

    /// Test whether a set is empty.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// assert!(
    ///   !ordset![1, 2, 3].is_empty()
    /// );
    /// assert!(
    ///   OrdSet::<i32>::new().is_empty()
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the size of a set.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// assert_eq!(3, ordset![1, 2, 3].len());
    /// ```
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Test whether two sets refer to the same content in memory.
    ///
    /// This is true if the two sides are references to the same set,
    /// or if the two sets refer to the same root node.
    ///
    /// This would return true if you're comparing a set to itself, or
    /// if you're comparing a set to a fresh clone of itself.
    ///
    /// Time: O(1)
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.map.ptr_eq(&other.map)
    }

    /// Discard all elements from the set.
    ///
    /// This leaves you with an empty set, and all elements that
    /// were previously inside it are dropped.
    ///
    /// Time: O(n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::OrdSet;
    /// let mut set = ordset![1, 2, 3];
    /// set.clear();
    /// assert!(set.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

impl<A, P> GenericOrdSet<A, P>
where
    A: Ord,
    P: SharedPointerKind,
{
    /// Get the smallest value in a set.
    ///
    /// If the set is empty, returns `None`.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn get_min(&self) -> Option<&A> {
        self.map.get_min().map(|v| &v.0)
    }

    /// Get the largest value in a set.
    ///
    /// If the set is empty, returns `None`.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn get_max(&self) -> Option<&A> {
        self.map.get_max().map(|v| &v.0)
    }

    /// Create an iterator over the contents of the set.
    #[must_use]
    pub fn iter(&self) -> Iter<'_, A, P> {
        Iter {
            it: self.map.iter(),
        }
    }

    /// Create an iterator over a range inside the set.
    #[must_use]
    pub fn range<R, Q>(&self, range: R) -> RangedIter<'_, A, P>
    where
        R: RangeBounds<Q>,
        Q: Comparable<A> + ?Sized,
    {
        RangedIter {
            it: self.map.range(range),
        }
    }

    /// Get an iterator over the differences between this set and
    /// another, i.e. the set of entries to add or remove to this set
    /// in order to make it equal to the other set.
    ///
    /// This function will avoid visiting nodes which are shared
    /// between the two sets, meaning that even very large sets can be
    /// compared quickly if most of their structure is shared.
    ///
    /// Time: O(n) (where n is the number of unique elements across
    /// the two sets, minus the number of elements belonging to nodes
    /// shared between them)
    #[must_use]
    pub fn diff<'a, 'b>(&'a self, other: &'b Self) -> DiffIter<'a, 'b, A, P> {
        DiffIter {
            it: self.map.diff(&other.map),
        }
    }

    /// Test if a value is part of a set.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let mut set = ordset!{1, 2, 3};
    /// assert!(set.contains(&1));
    /// assert!(!set.contains(&4));
    /// ```
    #[inline]
    #[must_use]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.contains_key(value)
    }

    /// Returns a reference to the element in the set, if any, that is equal to the value.
    /// The value may be any borrowed form of the set’s element type, but the ordering on
    /// the borrowed form must match the ordering on the element type.
    ///
    /// This is useful when the elements in the set are unique by for example an id,
    /// and you want to get the element out of the set by using the id.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use core::borrow::Borrow;
    /// # use core::cmp::Ordering;
    /// # use pds::ordset::OrdSet;
    /// # #[derive(Clone)]
    /// // Implements Eq and ord by delegating to id
    /// struct FancyItem {
    ///     id: u32,
    ///     data: String,
    /// }
    /// # impl Eq for FancyItem {}
    /// # impl PartialEq<Self> for FancyItem {fn eq(&self, other: &Self) -> bool { self.id.eq(&other.id)}}
    /// # impl PartialOrd<Self> for FancyItem {fn partial_cmp(&self, other: &Self) -> Option<Ordering> {self.id.partial_cmp(&other.id)}}
    /// # impl Ord for FancyItem {fn cmp(&self, other: &Self) -> Ordering {self.id.cmp(&other.id)}}
    /// # impl Borrow<u32> for FancyItem {fn borrow(&self) -> &u32 {&self.id}}
    /// let mut set = ordset!{
    ///     FancyItem {id: 0, data: String::from("Hello")},
    ///     FancyItem {id: 1, data: String::from("Test")}
    /// };
    /// assert_eq!(set.get(&1).unwrap().data, "Test");
    /// assert_eq!(set.get(&0).unwrap().data, "Hello");
    ///
    /// ```
    pub fn get<Q>(&self, value: &Q) -> Option<&A>
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.get_key_value(value).map(|(k, _)| k)
    }

    /// Get the closest smaller value in a set to a given value.
    ///
    /// If the set contains the given value, this is returned.
    /// Otherwise, the closest value in the set smaller than the
    /// given value is returned. If the smallest value in the set
    /// is larger than the given value, `None` is returned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate pds;
    /// # use pds::OrdSet;
    /// let set = ordset![1, 3, 5, 7, 9];
    /// assert_eq!(Some(&5), set.get_prev(&6));
    /// ```
    #[must_use]
    pub fn get_prev<Q>(&self, value: &Q) -> Option<&A>
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.get_prev(value).map(|(k, _)| k)
    }

    /// Get the closest larger value in a set to a given value.
    ///
    /// If the set contains the given value, this is returned.
    /// Otherwise, the closest value in the set larger than the
    /// given value is returned. If the largest value in the set
    /// is smaller than the given value, `None` is returned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate pds;
    /// # use pds::OrdSet;
    /// let set = ordset![1, 3, 5, 7, 9];
    /// assert_eq!(Some(&5), set.get_next(&4));
    /// ```
    #[must_use]
    pub fn get_next<Q>(&self, value: &Q) -> Option<&A>
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.get_next(value).map(|(k, _)| k)
    }

    /// Get the closest strictly smaller value in a set to a given value.
    ///
    /// Unlike [`get_prev`][Self::get_prev], this never returns the given
    /// value itself — it uses `Bound::Excluded`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate pds;
    /// # use pds::OrdSet;
    /// let set = ordset![1, 3, 5, 7, 9];
    /// assert_eq!(Some(&1), set.get_prev_exclusive(&3));
    /// assert_eq!(Some(&3), set.get_prev_exclusive(&4));
    /// ```
    #[must_use]
    pub fn get_prev_exclusive<Q>(&self, value: &Q) -> Option<&A>
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.get_prev_exclusive(value).map(|(k, _)| k)
    }

    /// Get the closest strictly larger value in a set to a given value.
    ///
    /// Unlike [`get_next`][Self::get_next], this never returns the given
    /// value itself — it uses `Bound::Excluded`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate pds;
    /// # use pds::OrdSet;
    /// let set = ordset![1, 3, 5, 7, 9];
    /// assert_eq!(Some(&5), set.get_next_exclusive(&3));
    /// assert_eq!(Some(&5), set.get_next_exclusive(&4));
    /// ```
    #[must_use]
    pub fn get_next_exclusive<Q>(&self, value: &Q) -> Option<&A>
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.get_next_exclusive(value).map(|(k, _)| k)
    }

    /// Test whether a set is a subset of another set, meaning that
    /// all values in our set must also be in the other set.
    ///
    /// Time: O(n log m) where m is the size of the other set
    #[must_use]
    pub fn is_subset<RS>(&self, other: RS) -> bool
    where
        RS: Borrow<Self>,
    {
        let other = other.borrow();
        if other.len() < self.len() {
            return false;
        }
        self.iter().all(|a| other.contains(a))
    }

    /// Test whether a set is a proper subset of another set, meaning
    /// that all values in our set must also be in the other set. A
    /// proper subset must also be smaller than the other set.
    ///
    /// Time: O(n log m) where m is the size of the other set
    #[must_use]
    pub fn is_proper_subset<RS>(&self, other: RS) -> bool
    where
        RS: Borrow<Self>,
    {
        self.len() != other.borrow().len() && self.is_subset(other)
    }

    /// Check whether two sets share no elements.
    ///
    /// Uses a simultaneous traversal of both sets in sorted order,
    /// returning `false` at the first shared element. O(n + m) time.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let a = ordset!{1, 2, 3};
    /// let b = ordset!{4, 5, 6};
    /// let c = ordset!{3, 4, 5};
    /// assert!(a.disjoint(&b));
    /// assert!(!a.disjoint(&c));
    /// ```
    #[must_use]
    pub fn disjoint(&self, other: &Self) -> bool {
        self.map.disjoint(&other.map)
    }

    /// Check invariants
    #[cfg(any(test, fuzzing))]
    #[allow(unreachable_pub)] // `pub` so fuzz targets can call it; only compiled under test/fuzzing, hence unreachable in normal builds.
    pub fn check_sane(&self)
    where
        A: core::fmt::Debug,
    {
        self.map.check_sane();
    }
}

impl<A, P> GenericOrdSet<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    /// Insert a value into a set.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let mut set = ordset!{};
    /// set.insert(123);
    /// set.insert(456);
    /// assert_eq!(
    ///   set,
    ///   ordset![123, 456]
    /// );
    /// ```
    #[inline]
    pub fn insert(&mut self, a: A) -> Option<A> {
        self.map.insert_key_value(a, ()).map(|(k, _)| k)
    }

    /// Remove a value from a set.
    ///
    /// Time: O(log n)
    #[inline]
    pub fn remove<Q>(&mut self, value: &Q) -> Option<A>
    where
        Q: Comparable<A> + ?Sized,
    {
        self.map.remove_with_key(value).map(|(k, _)| k)
    }

    /// Apply a diff to produce a new set.
    ///
    /// Takes any iterator of [`DiffItem`] values (such as from
    /// [`diff`][GenericOrdSet::diff]) and applies each change —
    /// `Add` inserts values, `Remove` removes values.
    ///
    /// Time: O(d log n) where d is the number of diff items
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let base = ordset!{1, 2, 3};
    /// let modified = ordset!{2, 3, 4};
    /// let diff: Vec<_> = base.diff(&modified).collect();
    /// let patched = base.apply_diff(diff);
    /// assert_eq!(patched, modified);
    /// ```
    #[must_use]
    pub fn apply_diff<'a, 'b, I>(&self, diff: I) -> Self
    where
        I: IntoIterator<Item = DiffItem<'a, 'b, A>>,
        A: 'a + 'b,
    {
        let mut out = self.clone();
        for item in diff {
            match item {
                DiffItem::Add(a) => {
                    out.insert(a.clone());
                }
                DiffItem::Remove(a) => {
                    out.remove(a);
                }
            }
        }
        out
    }

    /// Remove all values from a set that do not satisfy the given
    /// predicate.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let mut set = ordset!{1, 2, 3, 4, 5};
    /// set.retain(|v| v % 2 != 0);
    /// assert_eq!(set, ordset!{1, 3, 5});
    /// ```
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&A) -> bool,
    {
        let to_remove: Vec<A> = self.iter().filter(|a| !f(a)).cloned().collect();
        for a in &to_remove {
            self.remove(a);
        }
    }

    /// Split a set into two sets, where the first contains values
    /// that satisfy the predicate and the second contains values
    /// that do not.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set = ordset!{1, 2, 3, 4, 5};
    /// let (evens, odds) = set.partition(|v| v % 2 == 0);
    /// assert_eq!(evens, ordset!{2, 4});
    /// assert_eq!(odds, ordset!{1, 3, 5});
    /// ```
    #[must_use]
    pub fn partition<F>(&self, mut f: F) -> (Self, Self)
    where
        F: FnMut(&A) -> bool,
    {
        let mut left = Self::new();
        let mut right = Self::new();
        for a in self.iter() {
            if f(a) {
                left.insert(a.clone());
            } else {
                right.insert(a.clone());
            }
        }
        (left, right)
    }

    /// Keep only values that are in the given set.
    ///
    /// Time: O(n log m) where n = self.len(), m = other.len()
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set = ordset!{1, 2, 3, 4, 5};
    /// let keep = ordset!{2, 4, 6};
    /// assert_eq!(set.restrict(&keep), ordset!{2, 4});
    /// ```
    #[must_use]
    pub fn restrict(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.retain(|a| other.contains(a));
        out
    }

    /// Remove the smallest value from a set.
    ///
    /// Time: O(log n)
    pub fn remove_min(&mut self) -> Option<A> {
        // FIXME implement this at the node level for better efficiency
        let key = self.get_min()?.clone();
        self.remove(&key)
    }

    /// Remove the largest value from a set.
    ///
    /// Time: O(log n)
    pub fn remove_max(&mut self) -> Option<A> {
        // FIXME implement this at the node level for better efficiency
        let key = self.get_max()?.clone();
        self.remove(&key)
    }

    /// Construct a new set from the current set with the given value
    /// added.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set = ordset![456];
    /// assert_eq!(
    ///   set.update(123),
    ///   ordset![123, 456]
    /// );
    /// ```
    #[must_use]
    pub fn update(&self, a: A) -> Self {
        let mut out = self.clone();
        out.insert(a);
        out
    }

    /// Construct a new set with the given value removed if it's in
    /// the set.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn without<Q>(&self, value: &Q) -> Self
    where
        Q: Comparable<A> + ?Sized,
    {
        let mut out = self.clone();
        out.remove(value);
        out
    }

    /// Remove the smallest value from a set, and return that value as
    /// well as the updated set.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn without_min(&self) -> (Option<A>, Self) {
        match self.get_min() {
            Some(v) => (Some(v.clone()), self.without(v)),
            None => (None, self.clone()),
        }
    }

    /// Remove the largest value from a set, and return that value as
    /// well as the updated set.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn without_max(&self) -> (Option<A>, Self) {
        match self.get_max() {
            Some(v) => (Some(v.clone()), self.without(v)),
            None => (None, self.clone()),
        }
    }

    /// Construct the union of two sets.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set1 = ordset!{1, 2};
    /// let set2 = ordset!{2, 3};
    /// let expected = ordset!{1, 2, 3};
    /// assert_eq!(expected, set1.union(set2));
    /// ```
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let (mut to_mutate, to_consume) = if self.len() >= other.len() {
            (self, other)
        } else {
            (other, self)
        };
        for value in to_consume {
            to_mutate.insert(value);
        }
        to_mutate
    }

    /// Construct the union of multiple sets.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn unions<I>(i: I) -> Self
    where
        I: IntoIterator<Item = Self>,
    {
        i.into_iter().fold(Self::default(), Self::union)
    }

    /// Construct the symmetric difference between two sets.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set1 = ordset!{1, 2};
    /// let set2 = ordset!{2, 3};
    /// let expected = ordset!{1, 3};
    /// assert_eq!(expected, set1.symmetric_difference(set2));
    /// ```
    #[must_use]
    pub fn symmetric_difference(mut self, other: Self) -> Self {
        for value in other {
            if self.remove(&value).is_none() {
                self.insert(value);
            }
        }
        self
    }

    /// Construct the relative complement between two sets, that is the set
    /// of values in `self` that do not occur in `other`.
    ///
    /// Time: O(m log n) where m is the size of the other set
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set1 = ordset!{1, 2};
    /// let set2 = ordset!{2, 3};
    /// let expected = ordset!{1};
    /// assert_eq!(expected, set1.difference(set2));
    /// ```
    #[must_use]
    pub fn difference(mut self, other: Self) -> Self {
        for value in other {
            let _ = self.remove(&value);
        }
        self
    }

    /// Construct the intersection of two sets.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::ordset::OrdSet;
    /// let set1 = ordset!{1, 2};
    /// let set2 = ordset!{2, 3};
    /// let expected = ordset!{2};
    /// assert_eq!(expected, set1.intersection(set2));
    /// ```
    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        let mut out = Self::default();
        for value in other {
            if self.contains(&value) {
                out.insert(value);
            }
        }
        out
    }

    /// Split a set into two, with the left hand set containing values
    /// which are smaller than `split`, and the right hand set
    /// containing values which are larger than `split`.
    ///
    /// The `split` value itself is discarded.
    ///
    /// Time: O(n)
    #[must_use]
    pub fn split<Q>(self, split: &Q) -> (Self, Self)
    where
        Q: Comparable<A> + ?Sized,
    {
        let (left, _, right) = self.split_member(split);
        (left, right)
    }

    /// Split a set into two, with the left hand set containing values
    /// which are smaller than `split`, and the right hand set
    /// containing values which are larger than `split`.
    ///
    /// Returns a tuple of the two sets and a boolean which is true if
    /// the `split` value existed in the original set, and false
    /// otherwise.
    ///
    /// Time: O(n)
    #[must_use]
    pub fn split_member<Q>(self, split: &Q) -> (Self, bool, Self)
    where
        Q: Comparable<A> + ?Sized,
    {
        let mut left = Self::default();
        let mut right = Self::default();
        let mut present = false;
        for value in self {
            match split.compare(&value).reverse() {
                Ordering::Less => {
                    left.insert(value);
                }
                Ordering::Equal => {
                    present = true;
                }
                Ordering::Greater => {
                    right.insert(value);
                }
            }
        }
        (left, present, right)
    }

    /// Construct a set with only the `n` smallest values from a given
    /// set.
    ///
    /// Time: O(n)
    #[must_use]
    pub fn take(&self, n: usize) -> Self {
        self.iter().take(n).cloned().collect()
    }

    /// Construct a set with the `n` smallest values removed from a
    /// given set.
    ///
    /// Time: O(n)
    #[must_use]
    pub fn skip(&self, n: usize) -> Self {
        self.iter().skip(n).cloned().collect()
    }
}

// Core traits

impl<A, P: SharedPointerKind> Clone for GenericOrdSet<A, P> {
    /// Clone a set.
    ///
    /// Time: O(1)
    #[inline]
    fn clone(&self) -> Self {
        GenericOrdSet {
            map: self.map.clone(),
        }
    }
}

// TODO: Support PartialEq for OrdSet that have different P
impl<A: Ord, P: SharedPointerKind> PartialEq for GenericOrdSet<A, P> {
    fn eq(&self, other: &Self) -> bool {
        self.map.eq(&other.map)
    }
}

impl<A: Ord, P: SharedPointerKind> Eq for GenericOrdSet<A, P> {}

impl<A: Ord, P: SharedPointerKind> PartialOrd for GenericOrdSet<A, P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Ord, P: SharedPointerKind> Ord for GenericOrdSet<A, P> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<A: Ord + Hash, P: SharedPointerKind> Hash for GenericOrdSet<A, P> {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        for i in self.iter() {
            i.hash(state);
        }
    }
}

impl<A, P: SharedPointerKind> Default for GenericOrdSet<A, P> {
    fn default() -> Self {
        GenericOrdSet::new()
    }
}

impl<A, R, P> Extend<R> for GenericOrdSet<A, P>
where
    A: Ord + Clone + From<R>,
    P: SharedPointerKind,
{
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = R>,
    {
        for value in iter {
            self.insert(From::from(value));
        }
    }
}

impl<A: Ord + Debug, P: SharedPointerKind> Debug for GenericOrdSet<A, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_set().entries(self.iter()).finish()
    }
}

// Iterators

/// An iterator over the elements of a set.
pub struct Iter<'a, A, P: SharedPointerKind> {
    it: map::Iter<'a, A, (), P>,
}

// We impl Clone instead of deriving it, because we want Clone even if K and V aren't.
impl<'a, A, P: SharedPointerKind> Clone for Iter<'a, A, P> {
    fn clone(&self) -> Self {
        Iter {
            it: self.it.clone(),
        }
    }
}

impl<'a, A, P: SharedPointerKind> Iterator for Iter<'a, A, P>
where
    A: 'a + Ord,
{
    type Item = &'a A;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|(k, _)| k)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, A, P> DoubleEndedIterator for Iter<'a, A, P>
where
    A: 'a + Ord,
    P: SharedPointerKind,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.it.next_back().map(|(k, _)| k)
    }
}

impl<'a, A, P> ExactSizeIterator for Iter<'a, A, P>
where
    A: 'a + Ord,
    P: SharedPointerKind,
{
}

impl<'a, A, P> FusedIterator for Iter<'a, A, P>
where
    A: 'a + Ord,
    P: SharedPointerKind,
{
}

/// A ranged iterator over the elements of a set.
///
/// The only difference from `Iter` is that this one doesn't implement
/// `ExactSizeIterator` because we can't know the size of the range without first
/// iterating over it to count.
pub struct RangedIter<'a, A, P: SharedPointerKind> {
    it: map::RangedIter<'a, A, (), P>,
}

impl<'a, A, P> Iterator for RangedIter<'a, A, P>
where
    A: 'a + Ord,
    P: SharedPointerKind,
{
    type Item = &'a A;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|(k, _)| k)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, A, P> DoubleEndedIterator for RangedIter<'a, A, P>
where
    A: 'a + Ord,
    P: SharedPointerKind,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.it.next_back().map(|(k, _)| k)
    }
}

/// A consuming iterator over the elements of a set.
pub struct ConsumingIter<A, P: SharedPointerKind> {
    it: map::ConsumingIter<A, (), P>,
}

impl<A, P> Iterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
    type Item = A;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|v| v.0)
    }
}

impl<A, P> DoubleEndedIterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.it.next_back().map(|v| v.0)
    }
}

impl<A, P> ExactSizeIterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
}

impl<A, P> FusedIterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
}

/// An iterator over the difference between two sets.
pub struct DiffIter<'a, 'b, A, P: SharedPointerKind> {
    it: map::DiffIter<'a, 'b, A, (), P>,
}

/// A description of a difference between two ordered sets.
#[derive(PartialEq, Eq, Debug)]
pub enum DiffItem<'a, 'b, A> {
    /// This value has been added to the new set.
    Add(&'b A),
    /// This value has been removed from the new set.
    Remove(&'a A),
}

impl<'a, 'b, A, P> Iterator for DiffIter<'a, 'b, A, P>
where
    A: Ord + PartialEq,
    P: SharedPointerKind,
{
    type Item = DiffItem<'a, 'b, A>;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|item| match item {
            map::DiffItem::Add(k, _) => DiffItem::Add(k),
            map::DiffItem::Remove(k, _) => DiffItem::Remove(k),
            // Note that since the underlying map keys are unique and the values
            // are fixed `()`, we can never have an update.
            map::DiffItem::Update { .. } => unreachable!(),
        })
    }
}

impl<'a, 'b, A, P> FusedIterator for DiffIter<'a, 'b, A, P>
where
    A: Ord + PartialEq,
    P: SharedPointerKind,
{
}

impl<A, R, P> FromIterator<R> for GenericOrdSet<A, P>
where
    A: Ord + Clone + From<R>,
    P: SharedPointerKind,
{
    fn from_iter<T>(i: T) -> Self
    where
        T: IntoIterator<Item = R>,
    {
        let mut out = Self::new();
        for item in i {
            out.insert(From::from(item));
        }
        out
    }
}

impl<'a, A, P> IntoIterator for &'a GenericOrdSet<A, P>
where
    A: 'a + Ord,
    P: SharedPointerKind,
{
    type Item = &'a A;
    type IntoIter = Iter<'a, A, P>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<A, P> IntoIterator for GenericOrdSet<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    type Item = A;
    type IntoIter = ConsumingIter<A, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            it: self.map.into_iter(),
        }
    }
}

// Conversions

impl<A, OA, P1, P2> From<&GenericOrdSet<&A, P2>> for GenericOrdSet<OA, P1>
where
    A: ToOwned<Owned = OA> + Ord + ?Sized,
    OA: Ord + Clone,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(set: &GenericOrdSet<&A, P2>) -> Self {
        set.iter().map(|a| (*a).to_owned()).collect()
    }
}

impl<'a, A, P> From<&'a [A]> for GenericOrdSet<A, P>
where
    A: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(slice: &'a [A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> From<Vec<A>> for GenericOrdSet<A, P> {
    fn from(vec: Vec<A>) -> Self {
        vec.into_iter().collect()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> From<&Vec<A>> for GenericOrdSet<A, P> {
    fn from(vec: &Vec<A>) -> Self {
        vec.iter().cloned().collect()
    }
}

impl<A: Ord + Clone, const N: usize, P: SharedPointerKind> From<[A; N]> for GenericOrdSet<A, P> {
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

#[cfg(feature = "std")]
impl<A: Eq + Hash + Ord + Clone, P: SharedPointerKind> From<std::collections::HashSet<A>>
    for GenericOrdSet<A, P>
{
    fn from(hash_set: std::collections::HashSet<A>) -> Self {
        hash_set.into_iter().collect()
    }
}

#[cfg(feature = "std")]
impl<A: Eq + Hash + Ord + Clone, P: SharedPointerKind> From<&std::collections::HashSet<A>>
    for GenericOrdSet<A, P>
{
    fn from(hash_set: &std::collections::HashSet<A>) -> Self {
        hash_set.iter().cloned().collect()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> From<BTreeSet<A>> for GenericOrdSet<A, P> {
    fn from(btree_set: BTreeSet<A>) -> Self {
        btree_set.into_iter().collect()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> From<&BTreeSet<A>> for GenericOrdSet<A, P> {
    fn from(btree_set: &BTreeSet<A>) -> Self {
        btree_set.iter().cloned().collect()
    }
}

impl<A: Hash + Eq + Ord + Clone, S: BuildHasher, P1: SharedPointerKind, P2: SharedPointerKind>
    From<GenericHashSet<A, S, P2>> for GenericOrdSet<A, P1>
{
    fn from(hashset: GenericHashSet<A, S, P2>) -> Self {
        hashset.into_iter().collect()
    }
}

impl<A: Hash + Eq + Ord + Clone, S: BuildHasher, P1: SharedPointerKind, P2: SharedPointerKind>
    From<&GenericHashSet<A, S, P2>> for GenericOrdSet<A, P1>
{
    fn from(hashset: &GenericHashSet<A, S, P2>) -> Self {
        hashset.into_iter().cloned().collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::proptest::*;
    use proptest::proptest;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    assert_impl_all!(OrdSet<i32>: Send, Sync);
    assert_not_impl_any!(OrdSet<*const i32>: Send, Sync);
    assert_covariant!(OrdSet<T> in T);

    #[test]
    fn match_strings_with_string_slices() {
        let mut set: OrdSet<String> = From::from(&ordset!["foo", "bar"]);
        set = set.without("bar");
        assert!(!set.contains("bar"));
        set.remove("foo");
        assert!(!set.contains("foo"));
    }

    #[test]
    fn ranged_iter() {
        let set = ordset![1, 2, 3, 4, 5];
        let range: Vec<i32> = set.range::<_, i32>(..).cloned().collect();
        assert_eq!(vec![1, 2, 3, 4, 5], range);
        let range: Vec<i32> = set.range::<_, i32>(..).rev().cloned().collect();
        assert_eq!(vec![5, 4, 3, 2, 1], range);
        let range: Vec<i32> = set.range(2..5).cloned().collect();
        assert_eq!(vec![2, 3, 4], range);
        let range: Vec<i32> = set.range(2..5).rev().cloned().collect();
        assert_eq!(vec![4, 3, 2], range);
        let range: Vec<i32> = set.range(3..).cloned().collect();
        assert_eq!(vec![3, 4, 5], range);
        let range: Vec<i32> = set.range(3..).rev().cloned().collect();
        assert_eq!(vec![5, 4, 3], range);
        let range: Vec<i32> = set.range(..4).cloned().collect();
        assert_eq!(vec![1, 2, 3], range);
        let range: Vec<i32> = set.range(..4).rev().cloned().collect();
        assert_eq!(vec![3, 2, 1], range);
        let range: Vec<i32> = set.range(..=3).cloned().collect();
        assert_eq!(vec![1, 2, 3], range);
        let range: Vec<i32> = set.range(..=3).rev().cloned().collect();
        assert_eq!(vec![3, 2, 1], range);
    }

    proptest! {
        #[test]
        fn proptest_a_set(ref s in ord_set(".*", 10..100)) {
            assert!(s.len() < 100);
            assert!(s.len() >= 10);
        }

        #[test]
        fn long_ranged_iter(max in 1..1000) {
            let range = 0..max;
            let expected: Vec<i32> = range.clone().collect();
            let set: OrdSet<i32> = OrdSet::from_iter(range.clone());
            let result: Vec<i32> = set.range::<_, i32>(..).cloned().collect();
            assert_eq!(expected, result);

            let expected: Vec<i32> = range.clone().rev().collect();
            let set: OrdSet<i32> = OrdSet::from_iter(range);
            let result: Vec<i32> = set.range::<_, i32>(..).rev().cloned().collect();
            assert_eq!(expected, result);
        }
    }

    #[test]
    fn get_prev_exclusive_and_get_next_exclusive() {
        let set = ordset![1, 3, 5, 7, 9];

        // Value present — exclusive skips the value itself
        assert_eq!(set.get_prev_exclusive(&5), Some(&3));
        assert_eq!(set.get_next_exclusive(&5), Some(&7));

        // Value absent — same as inclusive variants
        assert_eq!(set.get_prev_exclusive(&6), Some(&5));
        assert_eq!(set.get_next_exclusive(&6), Some(&7));

        // Boundaries
        assert_eq!(set.get_prev_exclusive(&1), None);
        assert_eq!(set.get_next_exclusive(&9), None);

        // Empty set
        let empty: OrdSet<i32> = OrdSet::new();
        assert_eq!(empty.get_prev_exclusive(&5), None);
        assert_eq!(empty.get_next_exclusive(&5), None);
    }

    #[test]
    fn apply_diff_roundtrip() {
        let base = ordset![1, 2, 3];
        let modified = ordset![2, 3, 4];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_empty_diff() {
        let set = ordset![1, 2, 3];
        let patched = set.apply_diff(vec![]);
        assert_eq!(patched, set);
    }

    #[test]
    fn apply_diff_from_empty() {
        let base: OrdSet<i32> = OrdSet::new();
        let modified = ordset![1, 2, 3];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_to_empty() {
        let base = ordset![1, 2, 3];
        let modified: OrdSet<i32> = OrdSet::new();
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn retain_keeps_matching() {
        let mut set = ordset![1, 2, 3, 4, 5];
        set.retain(|v| v % 2 != 0);
        assert_eq!(set, ordset![1, 3, 5]);
    }

    #[test]
    fn retain_empty_set() {
        let mut set: OrdSet<i32> = OrdSet::new();
        set.retain(|_| false);
        assert!(set.is_empty());
    }

    #[test]
    fn retain_remove_all() {
        let mut set = ordset![1, 2, 3];
        set.retain(|_| false);
        assert!(set.is_empty());
    }

    #[test]
    fn retain_keep_all() {
        let mut set = ordset![1, 2, 3];
        set.retain(|_| true);
        assert_eq!(set, ordset![1, 2, 3]);
    }

    #[test]
    fn partition_basic() {
        let set = ordset![1, 2, 3, 4, 5];
        let (evens, odds) = set.partition(|v| v % 2 == 0);
        assert_eq!(evens, ordset![2, 4]);
        assert_eq!(odds, ordset![1, 3, 5]);
    }

    #[test]
    fn disjoint_basic() {
        let a = ordset![1, 2, 3];
        let b = ordset![4, 5, 6];
        let c = ordset![3, 4, 5];
        assert!(a.disjoint(&b));
        assert!(!a.disjoint(&c));
    }

    #[test]
    fn disjoint_empty() {
        let a = ordset![1, 2];
        let b: OrdSet<i32> = OrdSet::new();
        assert!(a.disjoint(&b));
        assert!(b.disjoint(&a));
    }

    #[test]
    fn restrict_basic() {
        let set = ordset![1, 2, 3, 4, 5];
        let keep = ordset![2, 4, 6];
        assert_eq!(set.restrict(&keep), ordset![2, 4]);
    }

    // --- Coverage gap tests ---

    #[test]
    fn new_unit_is_empty_len() {
        let empty: OrdSet<i32> = OrdSet::new();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let single = OrdSet::unit(42);
        assert!(!single.is_empty());
        assert_eq!(single.len(), 1);
        assert!(single.contains(&42));
    }

    #[test]
    fn ptr_eq_and_clear() {
        let a = ordset![1, 2, 3];
        let b = a.clone();
        assert!(a.ptr_eq(&b));

        let c = ordset![1, 2, 3]; // independently constructed
        assert!(!a.ptr_eq(&c));
        assert_eq!(a, c); // structurally equal though

        let mut d = a.clone();
        d.clear();
        assert!(d.is_empty());
        assert_eq!(d.len(), 0);
        assert!(!a.is_empty()); // original unaffected
    }

    #[test]
    fn get_min_get_max() {
        let set = ordset![3, 1, 5, 2, 4];
        assert_eq!(set.get_min(), Some(&1));
        assert_eq!(set.get_max(), Some(&5));

        let empty: OrdSet<i32> = OrdSet::new();
        assert_eq!(empty.get_min(), None);
        assert_eq!(empty.get_max(), None);

        let single = OrdSet::unit(99);
        assert_eq!(single.get_min(), Some(&99));
        assert_eq!(single.get_max(), Some(&99));
    }

    #[test]
    fn get_exact() {
        let set = ordset![10, 20, 30];
        assert_eq!(set.get(&20), Some(&20));
        assert_eq!(set.get(&25), None);
    }

    #[test]
    fn get_prev_get_next() {
        let set = ordset![10, 20, 30, 40, 50];
        // Inclusive: value present → returns itself
        assert_eq!(set.get_prev(&30), Some(&30));
        assert_eq!(set.get_next(&30), Some(&30));
        // Value absent → nearest neighbour
        assert_eq!(set.get_prev(&25), Some(&20));
        assert_eq!(set.get_next(&25), Some(&30));
        // Boundaries
        assert_eq!(set.get_prev(&5), None);
        assert_eq!(set.get_next(&55), None);
    }

    #[test]
    fn insert_returns_replaced() {
        let mut set = ordset![1, 2, 3];
        assert_eq!(set.insert(4), None); // new element
        assert_eq!(set.insert(2), Some(2)); // existing element
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn remove_returns_removed() {
        let mut set = ordset![1, 2, 3];
        assert_eq!(set.remove(&2), Some(2));
        assert_eq!(set.remove(&5), None);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn remove_min_remove_max() {
        let mut set = ordset![1, 2, 3, 4, 5];
        assert_eq!(set.remove_min(), Some(1));
        assert_eq!(set, ordset![2, 3, 4, 5]);
        assert_eq!(set.remove_max(), Some(5));
        assert_eq!(set, ordset![2, 3, 4]);

        let mut empty: OrdSet<i32> = OrdSet::new();
        assert_eq!(empty.remove_min(), None);
        assert_eq!(empty.remove_max(), None);
    }

    #[test]
    fn update_persistent() {
        let set = ordset![1, 2, 3];
        let updated = set.update(4);
        assert_eq!(updated, ordset![1, 2, 3, 4]);
        assert_eq!(set, ordset![1, 2, 3]); // original unchanged
    }

    #[test]
    fn without_min_without_max() {
        let set = ordset![10, 20, 30];
        let (min, rest) = set.without_min();
        assert_eq!(min, Some(10));
        assert_eq!(rest, ordset![20, 30]);

        let (max, rest) = set.without_max();
        assert_eq!(max, Some(30));
        assert_eq!(rest, ordset![10, 20]);

        let empty: OrdSet<i32> = OrdSet::new();
        let (min, rest) = empty.without_min();
        assert_eq!(min, None);
        assert!(rest.is_empty());
        let (max, rest) = empty.without_max();
        assert_eq!(max, None);
        assert!(rest.is_empty());
    }

    #[test]
    fn is_subset_is_proper_subset() {
        let a = ordset![1, 2, 3];
        let b = ordset![1, 2, 3, 4, 5];
        let c = ordset![1, 2, 3];

        assert!(a.is_subset(&b));
        assert!(a.is_subset(&c));
        assert!(a.is_proper_subset(&b));
        assert!(!a.is_proper_subset(&c)); // equal sets are not proper subsets
        assert!(!b.is_subset(&a));
    }

    #[test]
    fn union_basic() {
        let a = ordset![1, 2, 3];
        let b = ordset![3, 4, 5];
        assert_eq!(a.union(b), ordset![1, 2, 3, 4, 5]);
    }

    #[test]
    fn unions_multiple() {
        let sets = vec![ordset![1, 2], ordset![2, 3], ordset![3, 4]];
        assert_eq!(OrdSet::unions(sets), ordset![1, 2, 3, 4]);

        // Empty iterator
        let empty: Vec<OrdSet<i32>> = vec![];
        assert_eq!(OrdSet::unions(empty), OrdSet::new());
    }

    #[test]
    fn symmetric_difference_basic() {
        let a = ordset![1, 2, 3];
        let b = ordset![2, 3, 4];
        let result = a.symmetric_difference(b);
        assert_eq!(result, ordset![1, 4]);
    }

    #[test]
    fn difference_basic() {
        let a = ordset![1, 2, 3, 4, 5];
        let b = ordset![2, 4];
        assert_eq!(a.difference(b), ordset![1, 3, 5]);
    }

    #[test]
    fn intersection_basic() {
        let a = ordset![1, 2, 3, 4];
        let b = ordset![2, 4, 6];
        assert_eq!(a.intersection(b), ordset![2, 4]);
    }

    #[test]
    fn split_basic() {
        let set = ordset![1, 2, 3, 4, 5];
        let (left, right) = set.split(&3);
        assert_eq!(left, ordset![1, 2]);
        assert_eq!(right, ordset![4, 5]);
    }

    #[test]
    fn split_member_basic() {
        let set = ordset![1, 2, 3, 4, 5];
        let (left, present, right) = set.clone().split_member(&3);
        assert_eq!(left, ordset![1, 2]);
        assert!(present);
        assert_eq!(right, ordset![4, 5]);

        let (left, present, right) = set.split_member(&6);
        assert_eq!(left, ordset![1, 2, 3, 4, 5]);
        assert!(!present);
        assert!(right.is_empty());
    }

    #[test]
    fn take_skip() {
        let set = ordset![10, 20, 30, 40, 50];
        assert_eq!(set.take(3), ordset![10, 20, 30]);
        assert_eq!(set.skip(2), ordset![30, 40, 50]);
        assert_eq!(set.take(0), OrdSet::new());
        assert_eq!(set.skip(5), OrdSet::new());
        assert_eq!(set.take(10), set); // take more than len
    }

    #[test]
    fn from_conversions() {
        // From<Vec>
        let set: OrdSet<i32> = OrdSet::from(vec![3, 1, 2, 1]);
        assert_eq!(set, ordset![1, 2, 3]);

        // From<BTreeSet>
        let btree: BTreeSet<i32> = [1, 2, 3].into_iter().collect();
        let set: OrdSet<i32> = OrdSet::from(btree);
        assert_eq!(set, ordset![1, 2, 3]);

        // OrdSet → Vec via into_iter
        let set = ordset![1, 2, 3];
        let v: Vec<i32> = set.into_iter().collect();
        assert_eq!(v, vec![1, 2, 3]);
    }

    #[test]
    fn extend_trait() {
        let mut set = ordset![1, 2];
        set.extend(vec![3, 4, 5]);
        assert_eq!(set, ordset![1, 2, 3, 4, 5]);

        // Extend with another set (From<OrdSet> path)
        let mut set2 = ordset![1, 2];
        set2.extend(ordset![3, 4, 5]);
        assert_eq!(set2, ordset![1, 2, 3, 4, 5]);
    }

    #[test]
    fn partial_ord_ord() {
        let a = ordset![1, 2, 3];
        let b = ordset![1, 2, 4];
        assert!(a < b);
        assert!(b > a);

        let c = ordset![1, 2, 3];
        assert!(a <= c);
        assert!(a >= c);
    }

    #[test]
    fn hash_trait() {
        use crate::test::MetroHashBuilder;
        let a = ordset![1, 2, 3];
        let b = ordset![1, 2, 3];
        let bh = MetroHashBuilder::new(0);
        let ha = bh.hash_one(&a);
        let hb = bh.hash_one(&b);
        assert_eq!(ha, hb);
    }

    #[test]
    fn debug_display() {
        let set = ordset![1, 2, 3];
        let debug = format!("{:?}", set);
        assert!(debug.contains('1'));
        assert!(debug.contains('3'));
    }

    #[test]
    fn into_iterator() {
        let set = ordset![1, 2, 3];
        let items: Vec<i32> = set.into_iter().collect();
        assert_eq!(items, vec![1, 2, 3]);
    }

    #[test]
    fn from_hashset() {
        use crate::HashSet;
        let hs: HashSet<i32> = vec![3, 1, 2].into_iter().collect();
        let os: OrdSet<i32> = OrdSet::from(hs);
        assert_eq!(os.len(), 3);
        assert!(os.contains(&1));
        assert!(os.contains(&2));
        assert!(os.contains(&3));
    }

    #[test]
    fn iter_fused_and_exact_size() {
        let set = ordset![1, 2, 3, 4, 5];
        let mut it = set.iter();
        assert_eq!(it.len(), 5);
        it.next();
        assert_eq!(it.len(), 4);
        // Exhaust
        while it.next().is_some() {}
        assert_eq!(it.len(), 0);
        assert_eq!(it.next(), None); // fused: stays None
        assert_eq!(it.next(), None);
    }

    #[test]
    fn large_set_operations() {
        // Exercise deeper B+ tree paths (node splits, merges)
        let n = 1000;
        let a: OrdSet<i32> = (0..n).collect();
        let b: OrdSet<i32> = (n / 2..n + n / 2).collect();

        let u = a.clone().union(b.clone());
        assert_eq!(u.len() as i32, n + n / 2);

        let i = a.clone().intersection(b.clone());
        assert_eq!(i.len() as i32, n / 2);

        let d = a.clone().difference(b.clone());
        assert_eq!(d.len() as i32, n / 2); // elements in a but not b

        let sd = a.clone().symmetric_difference(b.clone());
        assert_eq!(sd.len() as i32, n);

        // Verify ordering preserved
        let items: Vec<i32> = u.iter().cloned().collect();
        for w in items.windows(2) {
            assert!(w[0] < w[1]);
        }
    }

    #[test]
    fn check_sane_valid_set() {
        let set: OrdSet<i32> = (0..100).collect();
        set.check_sane(); // should not panic
    }
}
