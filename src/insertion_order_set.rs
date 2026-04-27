// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent set preserving insertion order.
//!
//! An `InsertionOrderSet` stores unique elements in the order they were first
//! inserted. Backed by a [`GenericInsertionOrderMap<A, ()>`][crate::GenericInsertionOrderMap],
//! which itself uses a hash map for O(log n) membership testing and an ordered
//! map for insertion-ordered iteration. All operations are O(log n) with
//! structural sharing.
//!
//! # Examples
//!
//! ```
//! use pds::InsertionOrderSet;
//!
//! let mut set = InsertionOrderSet::new();
//! set.insert("c");
//! set.insert("a");
//! set.insert("b");
//!
//! let elems: Vec<_> = set.iter().collect();
//! assert_eq!(elems, vec![&"c", &"a", &"b"]);
//! ```
//!
//! ## Parallel iteration (`rayon` feature)
//!
//! With the `rayon` feature, `InsertionOrderSet` implements
//! [`IntoParallelRefIterator`][rayon::iter::IntoParallelRefIterator], yielding `&A` references.
//! Note that parallel iteration does not preserve insertion order.
//!
//! [`FromParallelIterator`][rayon::iter::FromParallelIterator] and
//! [`ParallelExtend`][rayon::iter::ParallelExtend] are intentionally absent — parallel
//! collection does not preserve insertion order. Use the sequential
//! `FromIterator` / `Extend` impls instead.

use alloc::vec::Vec;
use core::fmt::{Debug, Display, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::FromIterator;
#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hash_width::HashWidth;
use crate::insertion_order_map::{ConsumingIter as MapConsumingIter, GenericInsertionOrderMap};
use crate::shared_ptr::DefaultSharedPtr;

/// Constructs an [`InsertionOrderSet`] from a sequence of elements.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use pds::InsertionOrderSet;
/// # fn main() {
/// let s = insertion_order_set!["c", "a", "b"];
/// let elems: Vec<_> = s.iter().collect();
/// assert_eq!(elems, vec![&"c", &"a", &"b"]);
/// # }
/// ```
#[macro_export]
macro_rules! insertion_order_set {
    () => { $crate::insertion_order_set::InsertionOrderSet::new() };

    ( $($x:expr),* ) => {{
        let mut l = $crate::insertion_order_set::InsertionOrderSet::new();
        $(
            l.insert($x);
        )*
        l
    }};

    ( $($x:expr ,)* ) => {{
        let mut l = $crate::insertion_order_set::InsertionOrderSet::new();
        $(
            l.insert($x);
        )*
        l
    }};
}

/// Type alias for [`GenericInsertionOrderSet`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type InsertionOrderSet<A> = GenericInsertionOrderSet<A, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericInsertionOrderSet`] using [`foldhash::fast::RandomState`] —
/// available in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type InsertionOrderSet<A> =
    GenericInsertionOrderSet<A, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent set that iterates in insertion order.
///
/// Backed by a [`GenericInsertionOrderMap<A, ()>`][crate::GenericInsertionOrderMap]:
/// membership is O(log n) and iteration is in insertion order. Clone is O(1)
/// via structural sharing.
pub struct GenericInsertionOrderSet<
    A,
    S,
    P: SharedPointerKind = DefaultSharedPtr,
    H: HashWidth = u64,
> {
    pub(crate) map: GenericInsertionOrderMap<A, (), S, P, H>,
}

// --- Manual Clone ---

impl<A: Clone, S: Clone, P: SharedPointerKind, H: HashWidth> Clone
    for GenericInsertionOrderSet<A, S, P, H>
{
    fn clone(&self) -> Self {
        GenericInsertionOrderSet {
            map: self.map.clone(),
        }
    }
}

// --- Constructors ---

#[cfg(feature = "std")]
impl<A, P> GenericInsertionOrderSet<A, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Creates an empty insertion-ordered set.
    #[must_use]
    pub fn new() -> Self {
        GenericInsertionOrderSet {
            map: GenericInsertionOrderMap::new(),
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<A, P> GenericInsertionOrderSet<A, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Creates an empty insertion-ordered set (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericInsertionOrderSet {
            map: GenericInsertionOrderMap::new(),
        }
    }
}

// --- Size ---

impl<A, S, P, H: HashWidth> GenericInsertionOrderSet<A, S, P, H>
where
    P: SharedPointerKind,
{
    /// Tests whether the set is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// assert!(set.is_empty());
    /// set.insert(1);
    /// assert!(!set.is_empty());
    /// ```
    ///
    /// Time: O(1)
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Returns the number of elements.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// assert_eq!(set.len(), 0);
    /// set.insert("a");
    /// set.insert("b");
    /// assert_eq!(set.len(), 2);
    /// ```
    ///
    /// Time: O(1)
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Tests whether two sets share the same underlying allocation.
    ///
    /// Returns `true` if `self` and `other` are the same version of the
    /// set — i.e. one is a clone of the other with no intervening
    /// mutations. This is a cheap pointer comparison, not a structural
    /// equality check.
    ///
    /// Time: O(1)
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.map.ptr_eq(&other.map)
    }
}

// --- Core operations and set ops ---

impl<A, S, P, H: HashWidth> GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Tests whether an element is present.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert(42);
    /// assert!(set.contains(&42));
    /// assert!(!set.contains(&99));
    /// ```
    ///
    /// Time: O(1) avg
    #[must_use]
    pub fn contains<Q>(&self, elem: &Q) -> bool
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        self.map.contains_key(elem)
    }

    /// Inserts an element.
    ///
    /// Returns `true` if the element was newly inserted, `false` if it was
    /// already present (the set is unchanged).
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// assert!(set.insert("a"));  // newly inserted
    /// assert!(!set.insert("a")); // already present — no change
    /// assert_eq!(set.len(), 1);
    /// ```
    ///
    /// Time: O(log n)
    pub fn insert(&mut self, elem: A) -> bool {
        self.map.insert(elem, ()).is_none()
    }

    /// Removes an element.
    ///
    /// Returns `true` if the element was present and has been removed,
    /// `false` if it was not present.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert("a");
    /// set.insert("b");
    /// assert!(set.remove("a"));
    /// assert!(!set.contains("a"));
    /// assert!(!set.remove("z")); // not present
    /// ```
    ///
    /// Time: O(log n)
    pub fn remove<Q>(&mut self, elem: &Q) -> bool
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        self.map.remove(elem).is_some()
    }

    /// Returns a reference to the first element in insertion order, or `None` if empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert("b");
    /// set.insert("a");
    /// assert_eq!(set.front(), Some(&"b")); // first inserted
    /// ```
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn front(&self) -> Option<&A> {
        self.map.front().map(|(a, _)| a)
    }

    /// Returns a reference to the last element in insertion order, or `None` if empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert("b");
    /// set.insert("a");
    /// assert_eq!(set.back(), Some(&"a")); // last inserted
    /// ```
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn back(&self) -> Option<&A> {
        self.map.back().map(|(a, _)| a)
    }

    /// Removes and return the first element in insertion order (FIFO dequeue).
    ///
    /// Returns `None` if the set is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert(1i32);
    /// set.insert(2);
    /// assert_eq!(set.pop_front(), Some(1));
    /// assert_eq!(set.pop_front(), Some(2));
    /// assert_eq!(set.pop_front(), None);
    /// ```
    ///
    /// Time: O(log n)
    pub fn pop_front(&mut self) -> Option<A> {
        self.map.pop_front().map(|(a, _)| a)
    }

    /// Removes and return the last element in insertion order (LIFO dequeue).
    ///
    /// Returns `None` if the set is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert(1i32);
    /// set.insert(2);
    /// assert_eq!(set.pop_back(), Some(2));
    /// assert_eq!(set.pop_back(), Some(1));
    /// assert_eq!(set.pop_back(), None);
    /// ```
    ///
    /// Time: O(log n)
    pub fn pop_back(&mut self) -> Option<A> {
        self.map.pop_back().map(|(a, _)| a)
    }

    /// Iterates over elements in insertion order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let mut set = InsertionOrderSet::new();
    /// set.insert("c");
    /// set.insert("a");
    /// set.insert("b");
    /// let elems: Vec<_> = set.iter().collect();
    /// assert_eq!(elems, vec![&"c", &"a", &"b"]);
    /// ```
    ///
    /// Time: O(1) to create; O(n) to consume
    pub fn iter(&self) -> impl Iterator<Item = &A> {
        self.map.iter().map(|(a, _)| a)
    }

    /// Returns the union of two sets.
    ///
    /// All elements from both sets are included. Elements already present in
    /// `self` keep their position; new elements from `other` are appended in
    /// `other`'s insertion order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let a: InsertionOrderSet<i32> = [1, 2].into();
    /// let b: InsertionOrderSet<i32> = [2, 3].into();
    /// let u = a.union(b);
    /// let elems: Vec<_> = u.iter().copied().collect();
    /// assert_eq!(elems, vec![1, 2, 3]); // 2 not duplicated
    /// ```
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn union(mut self, other: Self) -> Self {
        self.extend(other);
        self
    }

    /// Returns the symmetric difference of two sets — elements that are in
    /// exactly one of the two sets.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let a: InsertionOrderSet<i32> = [1, 2, 3].into();
    /// let b: InsertionOrderSet<i32> = [2, 4].into();
    /// let sd = a.symmetric_difference(b);
    /// assert!(sd.contains(&1));
    /// assert!(!sd.contains(&2));
    /// assert!(sd.contains(&3));
    /// assert!(sd.contains(&4));
    /// ```
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn symmetric_difference(mut self, other: Self) -> Self {
        for elem in other {
            if !self.remove(&elem) {
                self.insert(elem);
            }
        }
        self
    }
}

impl<A, S, P, H: HashWidth> GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    /// Returns elements in `self` that are not in `other` (set difference A \ B).
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let a: InsertionOrderSet<i32> = [1, 2, 3].into();
    /// let b: InsertionOrderSet<i32> = [2].into();
    /// let d = a.difference(&b);
    /// let elems: Vec<_> = d.iter().copied().collect();
    /// assert_eq!(elems, vec![1, 3]);
    /// ```
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn difference(self, other: &Self) -> Self {
        self.into_iter().filter(|a| !other.contains(a)).collect()
    }

    /// Returns elements present in both sets; elements keep their position from `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pds::InsertionOrderSet;
    /// let a: InsertionOrderSet<i32> = [1, 2, 3].into();
    /// let b: InsertionOrderSet<i32> = [2, 3, 4].into();
    /// let i = a.intersection(&b);
    /// let elems: Vec<_> = i.iter().copied().collect();
    /// assert_eq!(elems, vec![2, 3]);
    /// ```
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn intersection(self, other: &Self) -> Self {
        self.into_iter().filter(|a| other.contains(a)).collect()
    }
}

// --- Default ---

impl<A, S, P, H: HashWidth> Default for GenericInsertionOrderSet<A, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericInsertionOrderSet {
            map: GenericInsertionOrderMap::default(),
        }
    }
}

// --- PartialEq / Eq ---

impl<A, S, P, H: HashWidth> PartialEq for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        // Insertion order is part of the identity of this collection.
        self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<A, S, P, H: HashWidth> Eq for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

// --- Hash ---

impl<A, S, P, H: HashWidth> Hash for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        // Ordered: insertion order is part of identity.
        for a in self.iter() {
            a.hash(state);
        }
    }
}

// --- Debug ---

impl<A, S, P, H: HashWidth> Debug for GenericInsertionOrderSet<A, S, P, H>
where
    A: Debug + Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_set();
        for a in self.iter() {
            d.entry(a);
        }
        d.finish()
    }
}

// --- FromIterator ---
impl<A, S, P, H: HashWidth> Display for GenericInsertionOrderSet<A, S, P, H>
where
    A: Display + Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{{")?;
        let mut sep = "";
        for a in self.iter() {
            write!(f, "{sep}{a}")?;
            sep = ", ";
        }
        write!(f, "}}")
    }
}

impl<A, S, P, H: HashWidth> FromIterator<A> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        let mut set = GenericInsertionOrderSet {
            map: GenericInsertionOrderMap::default(),
        };
        for a in iter {
            set.insert(a);
        }
        set
    }
}

// --- From conversions ---

impl<A, S, P, H: HashWidth> From<Vec<A>> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: Vec<A>) -> Self {
        v.into_iter().collect()
    }
}

impl<A, S, const N: usize, P, H: HashWidth> From<[A; N]> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A, S, P, H: HashWidth> From<&'a [A]> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, A, S, P, H: HashWidth> From<&'a Vec<A>> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<A>) -> Self {
        v.iter().cloned().collect()
    }
}

// --- Extend ---

impl<A, S, P, H: HashWidth> Extend<A> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = A>>(&mut self, iter: I) {
        for a in iter {
            self.insert(a);
        }
    }
}

// --- Iterators ---

/// A consuming iterator over the elements of a [`GenericInsertionOrderSet`].
///
/// Yields elements in insertion order.
pub struct ConsumingIter<A, P: SharedPointerKind> {
    inner: MapConsumingIter<A, (), P>,
}

impl<A, P> Iterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
    type Item = A;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(a, ())| a)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<A, P> ExactSizeIterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
}

impl<A, P> core::iter::FusedIterator for ConsumingIter<A, P>
where
    A: Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P, H: HashWidth> IntoIterator for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = A;
    type IntoIter = ConsumingIter<A, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.map.into_iter(),
        }
    }
}

impl<'a, A, S, P, H: HashWidth> IntoIterator for &'a GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Item = &'a A;
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = &'a A> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.iter())
    }
}

// --- Tests ---

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(crate::InsertionOrderSet<i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let set: InsertionOrderSet<i32> = InsertionOrderSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn insert_and_contains() {
        let mut set = InsertionOrderSet::new();
        assert!(set.insert(1));
        assert!(set.insert(2));
        // Duplicate — not inserted.
        assert!(!set.insert(1));
        assert!(set.contains(&1));
        assert!(set.contains(&2));
        assert!(!set.contains(&3));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn insertion_order_preserved() {
        let mut set = InsertionOrderSet::new();
        set.insert("c");
        set.insert("a");
        set.insert("b");

        let elems: Vec<_> = set.iter().collect();
        assert_eq!(elems, vec![&"c", &"a", &"b"]);
    }

    #[test]
    fn insert_duplicate_preserves_order() {
        let mut set = InsertionOrderSet::new();
        set.insert("a");
        set.insert("b");
        set.insert("c");
        // Reinserting "a" must not change its position.
        assert!(!set.insert("a"));
        let elems: Vec<_> = set.iter().collect();
        assert_eq!(elems, vec![&"a", &"b", &"c"]);
    }

    #[test]
    fn remove() {
        let mut set = InsertionOrderSet::new();
        set.insert("a");
        set.insert("b");
        set.insert("c");

        assert!(set.remove("b"));
        assert!(!set.contains("b"));
        assert_eq!(set.len(), 2);

        let elems: Vec<_> = set.iter().collect();
        assert_eq!(elems, vec![&"a", &"c"]);
    }

    #[test]
    fn remove_absent() {
        let mut set: InsertionOrderSet<i32> = InsertionOrderSet::new();
        assert!(!set.remove(&42));
    }

    #[test]
    fn remove_then_reinsert_changes_order() {
        let mut set = InsertionOrderSet::new();
        set.insert("a");
        set.insert("b");
        set.insert("c");
        set.remove("a");
        set.insert("a");

        let elems: Vec<_> = set.iter().collect();
        assert_eq!(elems, vec![&"b", &"c", &"a"]);
    }

    #[test]
    fn clone_shares_structure() {
        let mut set = InsertionOrderSet::new();
        set.insert(1);
        set.insert(2);
        let set2 = set.clone();
        assert_eq!(set, set2);
    }

    #[test]
    fn equality_order_matters() {
        let mut a = InsertionOrderSet::new();
        a.insert(1);
        a.insert(2);

        let mut b = InsertionOrderSet::new();
        b.insert(2);
        b.insert(1);

        // Different insertion order → not equal.
        assert_ne!(a, b);
    }

    #[test]
    fn equality_same_order() {
        let mut a = InsertionOrderSet::new();
        a.insert(1);
        a.insert(2);

        let mut b = InsertionOrderSet::new();
        b.insert(1);
        b.insert(2);

        assert_eq!(a, b);
    }

    #[test]
    fn into_iter_owned() {
        let mut set = InsertionOrderSet::new();
        set.insert("c");
        set.insert("a");
        set.insert("b");

        let elems: Vec<_> = set.into_iter().collect();
        assert_eq!(elems, vec!["c", "a", "b"]);
    }

    #[test]
    fn into_iter_ref() {
        let mut set = InsertionOrderSet::new();
        set.insert(1);
        set.insert(2);

        let elems: Vec<_> = (&set).into_iter().collect();
        assert_eq!(elems, vec![&1, &2]);
    }

    #[test]
    fn for_loop() {
        let mut set = InsertionOrderSet::new();
        set.insert(10);
        set.insert(20);

        let mut sum = 0;
        for &v in &set {
            sum += v;
        }
        assert_eq!(sum, 30);
    }

    #[test]
    fn default_is_empty() {
        let set: InsertionOrderSet<i32> = InsertionOrderSet::default();
        assert!(set.is_empty());
    }

    #[test]
    fn from_iterator() {
        let set: InsertionOrderSet<i32> = vec![3, 1, 2, 1].into_iter().collect();
        // Duplicate 1 is dropped; order of first appearances is preserved.
        let elems: Vec<_> = set.iter().copied().collect();
        assert_eq!(elems, vec![3, 1, 2]);
    }

    #[test]
    fn debug_format() {
        let mut set = InsertionOrderSet::new();
        set.insert(1i32);
        let s = format!("{:?}", set);
        assert!(!s.is_empty());
    }

    #[test]
    fn hash_same_for_equal_sets() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(s: &InsertionOrderSet<i32>) -> u64 {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        }

        let mut a = InsertionOrderSet::new();
        a.insert(1);
        a.insert(2);
        let mut b = InsertionOrderSet::new();
        b.insert(1);
        b.insert(2);
        assert_eq!(hash_of(&a), hash_of(&b));

        // Different order → different hash (with high probability).
        let mut c = InsertionOrderSet::new();
        c.insert(2);
        c.insert(1);
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn union_method() {
        let mut a = InsertionOrderSet::new();
        a.insert(1i32);
        a.insert(2);
        let mut b = InsertionOrderSet::new();
        b.insert(2);
        b.insert(3); // 2 already in a — position kept from a
        let c = a.union(b);
        assert!(c.contains(&1));
        assert!(c.contains(&2));
        assert!(c.contains(&3));
        assert_eq!(c.len(), 3);
        // 1 and 2 come from a in insertion order; 3 appended from b.
        let elems: Vec<_> = c.iter().copied().collect();
        assert_eq!(elems, vec![1, 2, 3]);
    }

    #[test]
    fn difference_method() {
        let mut a = InsertionOrderSet::new();
        a.insert(1i32);
        a.insert(2);
        a.insert(3);
        let mut b = InsertionOrderSet::new();
        b.insert(2);
        let c = a.difference(&b);
        assert_eq!(c.len(), 2);
        assert!(c.contains(&1));
        assert!(!c.contains(&2));
        assert!(c.contains(&3));
        let elems: Vec<_> = c.iter().copied().collect();
        assert_eq!(elems, vec![1, 3]);
    }

    #[test]
    fn intersection_method() {
        let mut a = InsertionOrderSet::new();
        a.insert(1i32);
        a.insert(2);
        a.insert(3);
        let mut b = InsertionOrderSet::new();
        b.insert(2);
        b.insert(3);
        b.insert(4);
        let c = a.intersection(&b);
        assert_eq!(c.len(), 2);
        assert!(c.contains(&2));
        assert!(c.contains(&3));
        assert!(!c.contains(&1));
        assert!(!c.contains(&4));
        // Insertion order from a preserved.
        let elems: Vec<_> = c.iter().copied().collect();
        assert_eq!(elems, vec![2, 3]);
    }

    #[test]
    fn symmetric_difference_method() {
        let mut a = InsertionOrderSet::new();
        a.insert(1i32);
        a.insert(2);
        a.insert(3);
        let mut b = InsertionOrderSet::new();
        b.insert(2);
        b.insert(4);
        let c = a.symmetric_difference(b);
        // 1 and 3 are in a only; 4 is in b only; 2 is in both (removed).
        assert_eq!(c.len(), 3);
        assert!(c.contains(&1));
        assert!(!c.contains(&2));
        assert!(c.contains(&3));
        assert!(c.contains(&4));
    }

    #[test]
    fn extend_adds_elements() {
        let mut set: InsertionOrderSet<i32> = InsertionOrderSet::new();
        set.extend(vec![1, 2, 3, 2]); // duplicate 2 is ignored
        assert_eq!(set.len(), 3);
        assert!(set.contains(&1));
        assert!(set.contains(&3));
    }

    #[test]
    fn from_vec() {
        let s: InsertionOrderSet<i32> = vec![3, 1, 2].into();
        assert_eq!(s.len(), 3);
        let elems: Vec<_> = s.iter().copied().collect();
        assert_eq!(elems, vec![3, 1, 2]);
    }

    #[test]
    fn from_array() {
        let s: InsertionOrderSet<i32> = [3i32, 1, 2].into();
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn from_slice() {
        let s: InsertionOrderSet<i32> = [3i32, 1, 2][..].into();
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![1i32, 2, 3];
        let s: InsertionOrderSet<i32> = InsertionOrderSet::from(&v);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn front_back_empty() {
        let s: InsertionOrderSet<i32> = InsertionOrderSet::new();
        assert_eq!(s.front(), None);
        assert_eq!(s.back(), None);
    }

    #[test]
    fn front_back_order() {
        let mut s = InsertionOrderSet::new();
        s.insert(10i32);
        s.insert(20i32);
        s.insert(30i32);
        assert_eq!(s.front(), Some(&10));
        assert_eq!(s.back(), Some(&30));
    }

    #[test]
    fn pop_front_fifo() {
        let mut s = InsertionOrderSet::new();
        s.insert("a");
        s.insert("b");
        s.insert("c");
        assert_eq!(s.pop_front(), Some("a"));
        assert_eq!(s.pop_front(), Some("b"));
        assert_eq!(s.pop_front(), Some("c"));
        assert_eq!(s.pop_front(), None);
    }

    #[test]
    fn pop_back_lifo() {
        let mut s = InsertionOrderSet::new();
        s.insert("a");
        s.insert("b");
        s.insert("c");
        assert_eq!(s.pop_back(), Some("c"));
        assert_eq!(s.pop_back(), Some("b"));
        assert_eq!(s.pop_back(), Some("a"));
        assert_eq!(s.pop_back(), None);
    }

    #[test]
    fn dedup_queue_simulation() {
        // Classic BFS / work-queue pattern: enqueue only if not already pending.
        let mut queue: InsertionOrderSet<&str> = InsertionOrderSet::new();
        queue.insert("node-1");
        queue.insert("node-2");
        queue.insert("node-1"); // already queued — ignored
        queue.insert("node-3");
        assert_eq!(queue.len(), 3);
        let drained: Vec<_> = core::iter::from_fn(|| queue.pop_front()).collect();
        assert_eq!(drained, vec!["node-1", "node-2", "node-3"]);
    }

    #[test]
    fn macro_empty() {
        let s: InsertionOrderSet<&str> = insertion_order_set![];
        assert!(s.is_empty());
    }

    #[test]
    fn macro_with_elements() {
        let s = insertion_order_set!["c", "a", "b"];
        assert_eq!(s.len(), 3);
        assert!(s.contains(&"a"));
        // Elements must iterate in insertion order.
        let elems: Vec<_> = s.iter().collect();
        assert_eq!(elems, vec![&"c", &"a", &"b"]);
    }

    #[test]
    fn macro_trailing_comma() {
        let s = insertion_order_set!["x", "y", "z",];
        assert_eq!(s.len(), 3);
    }
}
