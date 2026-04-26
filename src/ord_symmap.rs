// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent sorted symmetric bidirectional map (bijection within a single type).
//!
//! An [`OrdSymMap`] maintains a one-to-one mapping between values of the same
//! type, backed by two [`OrdMap<A, A>`][crate::OrdMap]s (forward and backward).
//! Because both sides share a type, lookups can be parameterised by
//! [`Direction`] and the map can be
//! [`swap`][GenericOrdSymMap::swap]ped in O(1).
//!
//! Prefer [`OrdSymMap`] over [`SymMap`][crate::SymMap] when:
//! - Values implement `Ord` but not `Hash + Eq`.
//! - You need sorted iteration without a separate sort step.
//! - You want `PartialOrd` / `Ord` on the symmap itself.
//!
//! # Examples
//!
//! ```
//! use pds::{OrdSymMap};
//! use pds::symmap::Direction;
//!
//! let mut sm = OrdSymMap::new();
//! sm.insert("hello", "hola");
//! sm.insert("goodbye", "adiós");
//!
//! assert_eq!(sm.get(Direction::Forward, &"hello"), Some(&"hola"));
//! assert_eq!(sm.get(Direction::Backward, &"hola"), Some(&"hello"));
//!
//! // Iteration is always in sorted (forward) order.
//! let sm = sm.swap();
//! assert_eq!(sm.get(Direction::Forward, &"hola"), Some(&"hello"));
//! ```

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;
use core::ops::Index;

use archery::SharedPointerKind;
use equivalent::Comparable;

use crate::ord::map::{ConsumingIter as MapConsumingIter, Iter as MapIter};
use crate::ordmap::GenericOrdMap;
use crate::shared_ptr::DefaultSharedPtr;
use crate::symmap::Direction;

/// Type alias for [`GenericOrdSymMap`] with the default pointer type.
pub type OrdSymMap<A> = GenericOrdSymMap<A, DefaultSharedPtr>;

/// A persistent sorted symmetric bidirectional map backed by two [`GenericOrdMap`]s.
///
/// Both sides of the mapping share the same type `A`. The map can be
/// [`swap`][Self::swap]ped in O(1) to reverse the primary direction.
/// Clone is O(1) via structural sharing.
///
/// Unlike [`SymMap`][crate::SymMap], this type requires only `A: Ord + Clone` —
/// no `Hash + Eq` constraint. Iteration is always in sorted key order.
pub struct GenericOrdSymMap<A, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) forward: GenericOrdMap<A, A, P>,
    pub(crate) backward: GenericOrdMap<A, A, P>,
}

// Manual Clone — avoid spurious `P: Clone` bound from derive.
impl<A: Clone, P: SharedPointerKind> Clone for GenericOrdSymMap<A, P> {
    fn clone(&self) -> Self {
        GenericOrdSymMap {
            forward: self.forward.clone(),
            backward: self.backward.clone(),
        }
    }
}

impl<A, P: SharedPointerKind> GenericOrdSymMap<A, P> {
    /// Create an empty OrdSymMap.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdSymMap {
            forward: GenericOrdMap::new(),
            backward: GenericOrdMap::new(),
        }
    }

    /// Test whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Return the number of pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    /// Swap the forward and backward maps in O(1).
    ///
    /// After swapping, what was the forward direction becomes backward and vice versa.
    #[must_use]
    pub fn swap(self) -> Self {
        GenericOrdSymMap {
            forward: self.backward,
            backward: self.forward,
        }
    }
}

impl<A: Ord, P: SharedPointerKind> GenericOrdSymMap<A, P> {
    /// Iterate over all pairs in sorted (forward) key order.
    pub fn iter(&self) -> MapIter<'_, A, A, P> {
        self.forward.iter()
    }

    /// Iterate over all pairs in the given direction.
    pub fn iter_direction(&self, dir: Direction) -> IterDirection<'_, A, P> {
        match dir {
            Direction::Forward => IterDirection::Forward(self.forward.iter()),
            Direction::Backward => IterDirection::Backward(self.backward.iter()),
        }
    }

    /// Look up a value in the given direction.
    #[must_use]
    pub fn get<Q>(&self, dir: Direction, key: &Q) -> Option<&A>
    where
        Q: Comparable<A> + ?Sized,
    {
        match dir {
            Direction::Forward => self.forward.get(key),
            Direction::Backward => self.backward.get(key),
        }
    }

    /// Test whether a key exists in the given direction.
    #[must_use]
    pub fn contains<Q>(&self, dir: Direction, key: &Q) -> bool
    where
        Q: Comparable<A> + ?Sized,
    {
        match dir {
            Direction::Forward => self.forward.contains_key(key),
            Direction::Backward => self.backward.contains_key(key),
        }
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> GenericOrdSymMap<A, P> {
    /// Insert a pair, maintaining the bijection invariant.
    ///
    /// Establishes `a` → `b` in the forward direction and `b` → `a` in the
    /// backward direction. Any existing mappings that conflict are removed first.
    pub fn insert(&mut self, a: A, b: A) {
        // Remove conflicting cross-references.
        if let Some(old_b) = self.forward.remove(&a) {
            self.backward.remove(&old_b);
        }
        if let Some(old_a) = self.backward.remove(&b) {
            self.forward.remove(&old_a);
        }

        self.forward.insert(a.clone(), b.clone());
        self.backward.insert(b, a);
    }

    /// Remove a pair by looking up the key in the given direction.
    ///
    /// Returns the other half of the pair, if it was present.
    pub fn remove<Q>(&mut self, dir: Direction, key: &Q) -> Option<A>
    where
        Q: Comparable<A> + ?Sized,
    {
        match dir {
            Direction::Forward => {
                if let Some(value) = self.forward.remove(key) {
                    self.backward.remove(&value);
                    Some(value)
                } else {
                    None
                }
            }
            Direction::Backward => {
                if let Some(key_val) = self.backward.remove(key) {
                    self.forward.remove(&key_val);
                    Some(key_val)
                } else {
                    None
                }
            }
        }
    }

    /// Return the union of two OrdSymMaps; entries from `other` overwrite entries in `self`.
    ///
    /// For conflicting pairs, `other`'s mapping wins. The bijection invariant is
    /// maintained by the underlying [`insert`][Self::insert] logic.
    #[must_use]
    pub fn union(mut self, other: Self) -> Self {
        self.extend(other);
        self
    }

    /// Return entries whose forward keys are in `self` but not in `other`.
    #[must_use]
    pub fn difference(self, other: &Self) -> Self {
        self.into_iter()
            .filter(|(a, _)| !other.contains(Direction::Forward, a))
            .collect()
    }

    /// Return entries whose forward keys are in both `self` and `other`; `self`'s values are kept.
    #[must_use]
    pub fn intersection(self, other: &Self) -> Self {
        self.into_iter()
            .filter(|(a, _)| other.contains(Direction::Forward, a))
            .collect()
    }

    /// Return entries whose forward keys are in exactly one of `self` or `other`.
    #[must_use]
    pub fn symmetric_difference(self, other: &Self) -> Self {
        // Clone self — O(1) via structural sharing — to check membership after consuming.
        let self_clone = self.clone();
        let self_diff: Self = self
            .into_iter()
            .filter(|(a, _)| !other.contains(Direction::Forward, a))
            .collect();
        let other_diff: Self = other
            .clone()
            .into_iter()
            .filter(|(a, _)| !self_clone.contains(Direction::Forward, a))
            .collect();
        self_diff.union(other_diff)
    }
}

/// Iterator wrapper for `iter_direction` — avoids boxing by unifying forward and backward
/// iterators as an enum.
pub enum IterDirection<'a, A, P: SharedPointerKind> {
    /// Iterating in the forward (insertion) direction.
    Forward(MapIter<'a, A, A, P>),
    /// Iterating in the backward (reverse) direction.
    Backward(MapIter<'a, A, A, P>),
}

impl<'a, A: Ord, P: SharedPointerKind> Iterator for IterDirection<'a, A, P> {
    type Item = (&'a A, &'a A);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IterDirection::Forward(it) => it.next(),
            IterDirection::Backward(it) => it.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            IterDirection::Forward(it) => it.size_hint(),
            IterDirection::Backward(it) => it.size_hint(),
        }
    }
}

impl<A: Ord, P: SharedPointerKind> Default for GenericOrdSymMap<A, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Ord, P: SharedPointerKind> PartialEq for GenericOrdSymMap<A, P> {
    fn eq(&self, other: &Self) -> bool {
        self.forward == other.forward
    }
}

impl<A: Ord, P: SharedPointerKind> Eq for GenericOrdSymMap<A, P> {}

impl<A: Ord, P: SharedPointerKind> PartialOrd for GenericOrdSymMap<A, P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Ord, P: SharedPointerKind> Ord for GenericOrdSymMap<A, P> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare by sorted forward-direction iteration (canonical order).
        self.forward.iter().cmp(other.forward.iter())
    }
}

impl<A: Ord + Hash, P: SharedPointerKind> Hash for GenericOrdSymMap<A, P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Sequential hashing is valid here: forward iteration is always in sorted order,
        // so two equal maps hash identically without an order-independent combiner.
        self.len().hash(state);
        for (a, b) in self.iter() {
            a.hash(state);
            b.hash(state);
        }
    }
}

impl<A: Ord + Clone + Debug, P: SharedPointerKind> Debug for GenericOrdSymMap<A, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (a, b) in self.iter() {
            d.entry(a, b);
        }
        d.finish()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> FromIterator<(A, A)> for GenericOrdSymMap<A, P> {
    fn from_iter<I: IntoIterator<Item = (A, A)>>(iter: I) -> Self {
        let mut sm = Self::new();
        for (a, b) in iter {
            sm.insert(a, b);
        }
        sm
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> From<Vec<(A, A)>> for GenericOrdSymMap<A, P> {
    fn from(v: Vec<(A, A)>) -> Self {
        v.into_iter().collect()
    }
}

impl<A: Ord + Clone, const N: usize, P: SharedPointerKind> From<[(A, A); N]>
    for GenericOrdSymMap<A, P>
{
    fn from(arr: [(A, A); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A: Ord + Clone, P: SharedPointerKind> From<&'a [(A, A)]> for GenericOrdSymMap<A, P> {
    fn from(slice: &'a [(A, A)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, A: Ord + Clone, P: SharedPointerKind> From<&'a Vec<(A, A)>> for GenericOrdSymMap<A, P> {
    fn from(v: &'a Vec<(A, A)>) -> Self {
        v.iter().cloned().collect()
    }
}

/// Index by forward key, returning the mapped partner value.
///
/// Panics if the key is not present. `IndexMut` is not implemented because mutating
/// the returned value would silently invalidate the reverse entry in the backward map.
impl<Q, A: Ord, P: SharedPointerKind> Index<&Q> for GenericOrdSymMap<A, P>
where
    Q: Comparable<A> + ?Sized,
{
    type Output = A;

    fn index(&self, key: &Q) -> &Self::Output {
        match self.forward.get(key) {
            Some(v) => v,
            None => panic!("OrdSymMap::index: key not found"),
        }
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> Extend<(A, A)> for GenericOrdSymMap<A, P> {
    fn extend<I: IntoIterator<Item = (A, A)>>(&mut self, iter: I) {
        for (a, b) in iter {
            self.insert(a, b);
        }
    }
}

/// Consuming iterator over the pairs of a [`GenericOrdSymMap`] in forward sorted order.
pub struct ConsumingIter<A, P: SharedPointerKind> {
    inner: MapConsumingIter<A, A, P>,
}

impl<A: Ord + Clone, P: SharedPointerKind> Iterator for ConsumingIter<A, P> {
    type Item = (A, A);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<A: Ord + Clone, P: SharedPointerKind> ExactSizeIterator for ConsumingIter<A, P> {}

impl<A: Ord + Clone, P: SharedPointerKind> IntoIterator for GenericOrdSymMap<A, P> {
    type Item = (A, A);
    type IntoIter = ConsumingIter<A, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.forward.into_iter(),
        }
    }
}

impl<'a, A: Ord + Clone, P: SharedPointerKind> IntoIterator for &'a GenericOrdSymMap<A, P> {
    type Item = (&'a A, &'a A);
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = (&'a A, &'a A)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.iter())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(OrdSymMap<i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let sm: OrdSymMap<&str> = OrdSymMap::new();
        assert!(sm.is_empty());
        assert_eq!(sm.len(), 0);
    }

    #[test]
    fn insert_and_lookup() {
        let mut sm = OrdSymMap::new();
        sm.insert("hello", "hola");
        sm.insert("goodbye", "adios");

        assert_eq!(sm.get(Direction::Forward, &"hello"), Some(&"hola"));
        assert_eq!(sm.get(Direction::Backward, &"hola"), Some(&"hello"));
        assert_eq!(sm.get(Direction::Forward, &"goodbye"), Some(&"adios"));
        assert_eq!(sm.get(Direction::Backward, &"adios"), Some(&"goodbye"));
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn insert_overwrites_forward() {
        let mut sm = OrdSymMap::new();
        sm.insert("a", "x");
        sm.insert("a", "y");

        assert_eq!(sm.get(Direction::Forward, &"a"), Some(&"y"));
        assert_eq!(sm.get(Direction::Backward, &"x"), None);
        assert_eq!(sm.get(Direction::Backward, &"y"), Some(&"a"));
        assert_eq!(sm.len(), 1);
    }

    #[test]
    fn insert_overwrites_backward() {
        let mut sm = OrdSymMap::new();
        sm.insert("a", "x");
        sm.insert("b", "x");

        assert_eq!(sm.get(Direction::Forward, &"a"), None);
        assert_eq!(sm.get(Direction::Forward, &"b"), Some(&"x"));
        assert_eq!(sm.get(Direction::Backward, &"x"), Some(&"b"));
        assert_eq!(sm.len(), 1);
    }

    #[test]
    fn swap_reverses_direction() {
        let mut sm = OrdSymMap::new();
        sm.insert("en", "es");

        let sm = sm.swap();
        assert_eq!(sm.get(Direction::Forward, &"es"), Some(&"en"));
        assert_eq!(sm.get(Direction::Backward, &"en"), Some(&"es"));
    }

    #[test]
    fn swap_is_involution() {
        let mut sm: OrdSymMap<&str> = OrdSymMap::new();
        sm.insert("a", "b");
        sm.insert("c", "d");

        let original = sm.clone();
        let swapped_twice = sm.swap().swap();
        assert_eq!(original, swapped_twice);
    }

    #[test]
    fn contains() {
        let mut sm = OrdSymMap::new();
        sm.insert("a", "b");

        assert!(sm.contains(Direction::Forward, &"a"));
        assert!(sm.contains(Direction::Backward, &"b"));
        assert!(!sm.contains(Direction::Forward, &"b"));
        assert!(!sm.contains(Direction::Backward, &"a"));
    }

    #[test]
    fn remove_forward() {
        let mut sm = OrdSymMap::new();
        sm.insert("a", "b");

        let removed = sm.remove(Direction::Forward, &"a");
        assert_eq!(removed, Some("b"));
        assert!(sm.is_empty());
    }

    #[test]
    fn remove_backward() {
        let mut sm = OrdSymMap::new();
        sm.insert("a", "b");

        let removed = sm.remove(Direction::Backward, &"b");
        assert_eq!(removed, Some("a"));
        assert!(sm.is_empty());
    }

    #[test]
    fn remove_absent_returns_none() {
        let mut sm: OrdSymMap<&str> = OrdSymMap::new();
        assert_eq!(sm.remove(Direction::Forward, &"x"), None);
    }

    #[test]
    fn iter_is_sorted() {
        let mut sm = OrdSymMap::new();
        sm.insert(3, 30);
        sm.insert(1, 10);
        sm.insert(2, 20);

        let pairs: Vec<_> = sm.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    fn iter_direction_forward_sorted() {
        let mut sm = OrdSymMap::new();
        sm.insert(3, 30);
        sm.insert(1, 10);
        sm.insert(2, 20);

        let pairs: Vec<_> = sm
            .iter_direction(Direction::Forward)
            .map(|(k, v)| (*k, *v))
            .collect();
        assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    fn iter_direction_backward_sorted() {
        let mut sm = OrdSymMap::new();
        sm.insert(1, 10);
        sm.insert(2, 20);

        // Backward map is OrdMap<value, key>, so iteration is sorted by value.
        let pairs: Vec<_> = sm
            .iter_direction(Direction::Backward)
            .map(|(v, k)| (*v, *k))
            .collect();
        assert_eq!(pairs, vec![(10, 1), (20, 2)]);
    }

    #[test]
    fn equality() {
        let mut sm1 = OrdSymMap::new();
        sm1.insert("a", "b");

        let mut sm2 = OrdSymMap::new();
        sm2.insert("a", "b");

        assert_eq!(sm1, sm2);
    }

    #[test]
    fn inequality() {
        let mut sm1 = OrdSymMap::new();
        sm1.insert("a", "b");

        let mut sm2 = OrdSymMap::new();
        sm2.insert("a", "c");

        assert_ne!(sm1, sm2);
    }

    #[test]
    fn ord_comparison() {
        let mut sm1: OrdSymMap<i32> = OrdSymMap::new();
        sm1.insert(1, 10);

        let mut sm2: OrdSymMap<i32> = OrdSymMap::new();
        sm2.insert(2, 20);

        assert!(sm1 < sm2);
    }

    #[test]
    fn hash_same_for_equal_maps() {
        use std::hash::DefaultHasher;

        let mut sm1 = OrdSymMap::new();
        sm1.insert(1i32, 10i32);
        sm1.insert(2, 20);

        let mut sm2 = OrdSymMap::new();
        sm2.insert(2i32, 20i32);
        sm2.insert(1, 10);

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        sm1.hash(&mut h1);
        sm2.hash(&mut h2);
        // Insertion order differs, but both maps are equal and must hash identically.
        assert_eq!(sm1, sm2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn clone_shares_structure() {
        let mut sm: OrdSymMap<i32> = OrdSymMap::new();
        sm.insert(1, 10);
        let clone = sm.clone();
        assert_eq!(sm, clone);
    }

    #[test]
    fn default_is_empty() {
        let sm: OrdSymMap<i32> = OrdSymMap::default();
        assert!(sm.is_empty());
    }

    #[test]
    fn from_vec() {
        let sm: OrdSymMap<i32> = OrdSymMap::from(vec![(1, 10), (2, 20)]);
        assert_eq!(sm.len(), 2);
        assert_eq!(sm.get(Direction::Forward, &1), Some(&10));
    }

    #[test]
    fn from_array() {
        let sm: OrdSymMap<i32> = OrdSymMap::from([(1, 10), (2, 20)]);
        assert_eq!(sm.len(), 2);
        assert_eq!(sm.get(Direction::Forward, &2), Some(&20));
    }

    #[test]
    fn from_slice() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let sm: OrdSymMap<i32> = OrdSymMap::from(v.as_slice());
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let sm: OrdSymMap<i32> = OrdSymMap::from(&v);
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn from_iterator() {
        let pairs = vec![(1i32, 10i32), (2, 20), (3, 30)];
        let sm: OrdSymMap<i32> = pairs.into_iter().collect();
        assert_eq!(sm.len(), 3);
        assert_eq!(sm.get(Direction::Forward, &3), Some(&30));
    }

    #[test]
    fn extend_adds_pairs() {
        let mut sm: OrdSymMap<i32> = OrdSymMap::new();
        sm.extend([(1, 10), (2, 20)]);
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn into_iter_owned_sorted() {
        let mut sm = OrdSymMap::new();
        sm.insert(3i32, 30i32);
        sm.insert(1, 10);
        sm.insert(2, 20);

        let pairs: Vec<_> = sm.into_iter().collect();
        assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    fn into_iter_ref() {
        let mut sm = OrdSymMap::new();
        sm.insert(1i32, 10i32);

        let pairs: Vec<_> = (&sm).into_iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![(1, 10)]);
    }

    #[test]
    fn union_merges_pairs() {
        let mut sm1 = OrdSymMap::new();
        sm1.insert(1i32, 10i32);

        let mut sm2 = OrdSymMap::new();
        sm2.insert(2i32, 20i32);

        let combined = sm1.union(sm2);
        assert_eq!(combined.len(), 2);
        assert_eq!(combined.get(Direction::Forward, &1), Some(&10));
        assert_eq!(combined.get(Direction::Forward, &2), Some(&20));
    }

    #[test]
    fn difference_by_key() {
        let mut sm1 = OrdSymMap::new();
        sm1.insert(1i32, 10i32);
        sm1.insert(2, 20);

        let mut sm2 = OrdSymMap::new();
        sm2.insert(2i32, 20i32);

        let diff = sm1.difference(&sm2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.get(Direction::Forward, &1), Some(&10));
    }

    #[test]
    fn intersection_by_key() {
        let mut sm1 = OrdSymMap::new();
        sm1.insert(1i32, 10i32);
        sm1.insert(2, 20);

        let mut sm2 = OrdSymMap::new();
        sm2.insert(2i32, 20i32);
        sm2.insert(3, 30);

        let inter = sm1.intersection(&sm2);
        assert_eq!(inter.len(), 1);
        assert_eq!(inter.get(Direction::Forward, &2), Some(&20));
    }

    #[test]
    fn symmetric_difference_by_key() {
        let mut sm1 = OrdSymMap::new();
        sm1.insert(1i32, 10i32);
        sm1.insert(2, 20);

        let mut sm2 = OrdSymMap::new();
        sm2.insert(2i32, 20i32);
        sm2.insert(3, 30);

        let sd = sm1.symmetric_difference(&sm2);
        assert_eq!(sd.len(), 2);
        assert!(sd.contains(Direction::Forward, &1));
        assert!(sd.contains(Direction::Forward, &3));
        assert!(!sd.contains(Direction::Forward, &2));
    }

    #[test]
    fn debug_format() {
        let mut sm = OrdSymMap::new();
        sm.insert(1i32, 10i32);
        let s = format!("{:?}", sm);
        assert!(s.contains("1"));
        assert!(s.contains("10"));
    }

    #[test]
    fn index_returns_value() {
        let mut sm = OrdSymMap::new();
        sm.insert(1i32, 10i32);
        assert_eq!(sm[&1i32], 10);
    }

    #[test]
    #[should_panic]
    fn index_panics_on_missing() {
        let sm: OrdSymMap<i32> = OrdSymMap::new();
        let _ = sm[&99i32];
    }
}
