// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent symmetric bidirectional map (bijection within a single type).
//!
//! A `SymMap<A>` maintains a one-to-one mapping between values of the same
//! type, backed by two [`HashMap`][crate::HashMap]s (forward and backward).
//! Because both sides share a type, lookups can be parameterised by
//! [`Direction`] and the map can be [`swap`][GenericSymMap::swap]ped in O(1).
//!
//! # Examples
//!
//! ```
//! use pds::{SymMap, Direction};
//!
//! let mut sm = SymMap::new();
//! sm.insert("hello", "hola");
//! sm.insert("goodbye", "adiós");
//!
//! assert_eq!(sm.get(Direction::Forward, &"hello"), Some(&"hola"));
//! assert_eq!(sm.get(Direction::Backward, &"hola"), Some(&"hello"));
//!
//! let sm = sm.swap();
//! assert_eq!(sm.get(Direction::Forward, &"hola"), Some(&"hello"));
//! ```

#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::{FromIterator, Sum};
use core::ops::{Add, Index};

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hash_width::HashWidth;
use crate::hashmap::GenericHashMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Direction for lookups and removals on a [`SymMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Key → value (as inserted).
    Forward,
    /// Value → key (reverse lookup).
    Backward,
}

/// Type alias for [`GenericSymMap`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type SymMap<A> = GenericSymMap<A, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericSymMap`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type SymMap<A> = GenericSymMap<A, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent symmetric bidirectional map backed by two [`GenericHashMap`]s.
///
/// Both sides of the mapping share the same type `A`. The map can be
/// [`swap`][Self::swap]ped in O(1) to reverse the primary direction.
/// Clone is O(1) via structural sharing.
pub struct GenericSymMap<
    A,
    S,
    P: SharedPointerKind = DefaultSharedPtr,
    H: HashWidth = u64,
> {
    pub(crate) forward: GenericHashMap<A, A, S, P, H>,
    pub(crate) backward: GenericHashMap<A, A, S, P, H>,
}

// Manual Clone — avoid derive's spurious `P: Clone` bound.
impl<A: Clone, S: Clone, P: SharedPointerKind, H: HashWidth> Clone
    for GenericSymMap<A, S, P, H>
{
    fn clone(&self) -> Self {
        GenericSymMap {
            forward: self.forward.clone(),
            backward: self.backward.clone(),
        }
    }
}

#[cfg(feature = "std")]
impl<A, P> GenericSymMap<A, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty SymMap.
    #[must_use]
    pub fn new() -> Self {
        GenericSymMap {
            forward: GenericHashMap::new(),
            backward: GenericHashMap::new(),
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<A, P> GenericSymMap<A, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty SymMap (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericSymMap {
            forward: GenericHashMap::new(),
            backward: GenericHashMap::new(),
        }
    }
}

impl<A, S, P, H: HashWidth> GenericSymMap<A, S, P, H>
where
    P: SharedPointerKind,
{
    /// Test whether the symmap is empty.
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
    /// After swapping, what was the forward direction becomes backward and
    /// vice versa. This is a zero-cost operation — it moves two pointers.
    #[must_use]
    pub fn swap(self) -> Self {
        GenericSymMap {
            forward: self.backward,
            backward: self.forward,
        }
    }
}

impl<A, S, P, H: HashWidth> GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    /// Insert a pair, maintaining the bijection invariant.
    ///
    /// Establishes `a` → `b` in the forward direction and `b` → `a` in the
    /// backward direction. Any existing mappings that conflict are removed.
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

    /// Look up a value in the given direction.
    #[must_use]
    pub fn get<Q>(&self, dir: Direction, key: &Q) -> Option<&A>
    where
        Q: Hash + Equivalent<A> + ?Sized,
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
        Q: Hash + Equivalent<A> + ?Sized,
    {
        match dir {
            Direction::Forward => self.forward.contains_key(key),
            Direction::Backward => self.backward.contains_key(key),
        }
    }

    /// Remove a pair by looking up the key in the given direction.
    ///
    /// Returns the other half of the pair, if it was present.
    pub fn remove<Q>(&mut self, dir: Direction, key: &Q) -> Option<A>
    where
        Q: Hash + Equivalent<A> + ?Sized,
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

    /// Iterate over all pairs (forward direction: left → right).
    pub fn iter(&self) -> impl Iterator<Item = (&A, &A)> {
        self.forward.iter()
    }

    /// Iterate over all pairs in the given direction.
    pub fn iter_direction(&self, dir: Direction) -> impl Iterator<Item = (&A, &A)> {
        match dir {
            Direction::Forward => IterDirection::Forward(self.forward.iter()),
            Direction::Backward => IterDirection::Backward(self.backward.iter()),
        }
    }
}

/// Iterator wrapper to unify forward/backward iteration without boxing.
enum IterDirection<F, B> {
    Forward(F),
    Backward(B),
}

impl<'a, A, F, B> Iterator for IterDirection<F, B>
where
    F: Iterator<Item = (&'a A, &'a A)>,
    B: Iterator<Item = (&'a A, &'a A)>,
    A: 'a,
{
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

impl<A, S, P, H: HashWidth> Default for GenericSymMap<A, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericSymMap {
            forward: GenericHashMap::default(),
            backward: GenericHashMap::default(),
        }
    }
}

impl<A, S, P, H: HashWidth> PartialEq for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.forward == other.forward
    }
}

impl<A, S, P, H: HashWidth> Eq for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P, H: HashWidth> Hash for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        self.len().hash(state);
        // Order-independent: wrapping_add of per-entry hashes.
        let mut combined: u64 = 0;
        for (a, b) in self.iter() {
            let mut h = crate::util::FnvHasher::new();
            a.hash(&mut h);
            b.hash(&mut h);
            combined = combined.wrapping_add(h.finish());
        }
        combined.hash(state);
    }
}

impl<A, S, P, H: HashWidth> Debug for GenericSymMap<A, S, P, H>
where
    A: Debug + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (a, b) in self.iter() {
            d.entry(a, b);
        }
        d.finish()
    }
}

impl<A, S, P, H: HashWidth> FromIterator<(A, A)> for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = (A, A)>>(iter: I) -> Self {
        let mut sm = GenericSymMap {
            forward: GenericHashMap::default(),
            backward: GenericHashMap::default(),
        };
        for (a, b) in iter {
            sm.insert(a, b);
        }
        sm
    }
}

impl<A, S, P, H: HashWidth> From<Vec<(A, A)>> for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: Vec<(A, A)>) -> Self {
        v.into_iter().collect()
    }
}

impl<A, S, const N: usize, P, H: HashWidth> From<[(A, A); N]>
    for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(arr: [(A, A); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A, S, P, H: HashWidth> From<&'a [(A, A)]> for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [(A, A)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, A, S, P, H: HashWidth> From<&'a Vec<(A, A)>> for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<(A, A)>) -> Self {
        v.iter().cloned().collect()
    }
}

/// Index by key in the forward direction, returning the mapped partner value.
///
/// Panics if the key is not present. Note: `IndexMut` is not implemented
/// because mutating the returned value via a mutable reference would silently
/// invalidate the reverse entry stored in the backward map.
impl<Q, A, S, P, H: HashWidth> Index<&Q> for GenericSymMap<A, S, P, H>
where
    Q: Hash + Equivalent<A> + ?Sized,
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Output = A;

    fn index(&self, key: &Q) -> &Self::Output {
        // Access forward map directly to avoid the S: Default bound on get().
        match self.forward.get(key) {
            Some(v) => v,
            None => panic!("SymMap::index: key not found"),
        }
    }
}

impl<A, S, P, H: HashWidth> Add for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    type Output = GenericSymMap<A, S, P, H>;

    fn add(mut self, other: Self) -> Self::Output {
        self.extend(other);
        self
    }
}

impl<A, S, P, H: HashWidth> Add for &GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    type Output = GenericSymMap<A, S, P, H>;

    fn add(self, other: Self) -> Self::Output {
        self.clone() + other.clone()
    }
}

impl<A, S, P: SharedPointerKind, H: HashWidth> Sum for GenericSymMap<A, S, P, H>
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

impl<A, S, P, H: HashWidth> Extend<(A, A)> for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = (A, A)>>(&mut self, iter: I) {
        for (a, b) in iter {
            self.insert(a, b);
        }
    }
}

/// A consuming iterator over the pairs of a [`GenericSymMap`].
pub struct ConsumingIter<A: Eq, P: SharedPointerKind, H: HashWidth = u64> {
    inner: crate::hashmap::ConsumingIter<(A, A), P, H>,
}

impl<A, P, H: HashWidth> Iterator for ConsumingIter<A, P, H>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
    type Item = (A, A);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<A, P, H: HashWidth> ExactSizeIterator for ConsumingIter<A, P, H>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
}

impl<A, S, P, H: HashWidth> IntoIterator for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Item = (A, A);
    type IntoIter = ConsumingIter<A, P, H>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.forward.into_iter(),
        }
    }
}

impl<'a, A, S, P, H: HashWidth> IntoIterator for &'a GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
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

    assert_impl_all!(crate::SymMap<i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let sm: SymMap<&str> = SymMap::new();
        assert!(sm.is_empty());
        assert_eq!(sm.len(), 0);
    }

    #[test]
    fn insert_and_lookup() {
        let mut sm = SymMap::new();
        sm.insert("hello", "hola");
        sm.insert("goodbye", "adiós");

        assert_eq!(sm.get(Direction::Forward, &"hello"), Some(&"hola"));
        assert_eq!(sm.get(Direction::Backward, &"hola"), Some(&"hello"));
        assert_eq!(sm.get(Direction::Forward, &"goodbye"), Some(&"adiós"));
        assert_eq!(sm.get(Direction::Backward, &"adiós"), Some(&"goodbye"));
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn insert_overwrites_forward() {
        let mut sm = SymMap::new();
        sm.insert("a", "x");
        sm.insert("a", "y");

        assert_eq!(sm.get(Direction::Forward, &"a"), Some(&"y"));
        assert_eq!(sm.get(Direction::Backward, &"x"), None);
        assert_eq!(sm.get(Direction::Backward, &"y"), Some(&"a"));
        assert_eq!(sm.len(), 1);
    }

    #[test]
    fn insert_overwrites_backward() {
        let mut sm = SymMap::new();
        sm.insert("a", "x");
        sm.insert("b", "x");

        assert_eq!(sm.get(Direction::Forward, &"a"), None);
        assert_eq!(sm.get(Direction::Forward, &"b"), Some(&"x"));
        assert_eq!(sm.get(Direction::Backward, &"x"), Some(&"b"));
        assert_eq!(sm.len(), 1);
    }

    #[test]
    fn swap_reverses_direction() {
        let mut sm = SymMap::new();
        sm.insert("en", "es");

        let sm = sm.swap();
        assert_eq!(sm.get(Direction::Forward, &"es"), Some(&"en"));
        assert_eq!(sm.get(Direction::Backward, &"en"), Some(&"es"));
    }

    #[test]
    fn swap_is_involution() {
        let mut sm = SymMap::new();
        sm.insert("a", "b");
        sm.insert("c", "d");

        let original = sm.clone();
        let swapped_twice = sm.swap().swap();
        assert_eq!(original, swapped_twice);
    }

    #[test]
    fn contains() {
        let mut sm = SymMap::new();
        sm.insert("a", "b");
        assert!(sm.contains(Direction::Forward, &"a"));
        assert!(!sm.contains(Direction::Forward, &"b"));
        assert!(sm.contains(Direction::Backward, &"b"));
        assert!(!sm.contains(Direction::Backward, &"a"));
    }

    #[test]
    fn remove_forward() {
        let mut sm = SymMap::new();
        sm.insert("a", "b");
        assert_eq!(sm.remove(Direction::Forward, &"a"), Some("b"));
        assert!(sm.is_empty());
    }

    #[test]
    fn remove_backward() {
        let mut sm = SymMap::new();
        sm.insert("a", "b");
        assert_eq!(sm.remove(Direction::Backward, &"b"), Some("a"));
        assert!(sm.is_empty());
    }

    #[test]
    fn remove_absent() {
        let mut sm: SymMap<&str> = SymMap::new();
        assert_eq!(sm.remove(Direction::Forward, &"x"), None);
    }

    #[test]
    fn from_iterator() {
        let sm: SymMap<&str> =
            vec![("a", "x"), ("b", "y")].into_iter().collect();
        assert_eq!(sm.len(), 2);
        assert_eq!(sm.get(Direction::Forward, &"a"), Some(&"x"));
    }

    #[test]
    fn from_array() {
        let sm: SymMap<&str> = SymMap::from([("a", "x"), ("b", "y")]);
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn clone_shares_structure() {
        let mut sm = SymMap::new();
        sm.insert("a", "b");
        let sm2 = sm.clone();
        assert_eq!(sm, sm2);
    }

    #[test]
    fn equality() {
        let mut a = SymMap::new();
        a.insert("x", "1");
        a.insert("y", "2");

        let mut b = SymMap::new();
        b.insert("y", "2");
        b.insert("x", "1");

        assert_eq!(a, b);
    }

    #[test]
    fn inequality() {
        let mut a = SymMap::new();
        a.insert("x", "1");

        let mut b = SymMap::new();
        b.insert("x", "2");

        assert_ne!(a, b);
    }

    #[test]
    fn into_iter_owned() {
        let mut sm = SymMap::new();
        sm.insert("a", "x");
        sm.insert("b", "y");

        let mut pairs: Vec<_> = sm.into_iter().collect();
        pairs.sort();
        assert_eq!(pairs, vec![("a", "x"), ("b", "y")]);
    }

    #[test]
    fn iter_direction() {
        let mut sm = SymMap::new();
        sm.insert("a", "x");

        let fwd: Vec<_> = sm.iter_direction(Direction::Forward).collect();
        assert_eq!(fwd, vec![(&"a", &"x")]);

        let bwd: Vec<_> = sm.iter_direction(Direction::Backward).collect();
        assert_eq!(bwd, vec![(&"x", &"a")]);
    }

    #[test]
    fn for_loop() {
        let mut sm: SymMap<i32> = SymMap::new();
        sm.insert(1, 10);
        sm.insert(2, 20);

        let mut sum = 0;
        for (&a, &b) in &sm {
            sum += a + b;
        }
        assert_eq!(sum, 33);
    }

    #[test]
    fn extend_trait() {
        let mut sm = SymMap::new();
        sm.insert("a", "x");
        sm.extend(vec![("b", "y"), ("c", "z")]);
        assert_eq!(sm.len(), 3);
    }

    #[test]
    fn add_trait() {
        let mut a: SymMap<&str> = SymMap::new();
        a.insert("a", "x");
        let mut b = SymMap::new();
        b.insert("b", "y");
        let c = a + b;
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn default_is_empty() {
        let sm: SymMap<String> = Default::default();
        assert!(sm.is_empty());
    }

    #[test]
    fn debug_output() {
        let mut sm = SymMap::new();
        sm.insert("a", "x");
        let s = format!("{:?}", sm);
        assert!(s.contains("\"a\""));
        assert!(s.contains("\"x\""));
    }

    #[test]
    fn hash_order_independent() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(m: &SymMap<i32>) -> u64 {
            let mut h = DefaultHasher::new();
            m.hash(&mut h);
            h.finish()
        }
        let mut a = SymMap::new();
        a.insert(1, 10); a.insert(2, 20);
        let mut b = SymMap::new();
        b.insert(2, 20); b.insert(1, 10); // different insertion order
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn index_forward() {
        let mut sm = SymMap::new();
        sm.insert(1i32, 10i32);
        sm.insert(2, 20);
        // Index uses the forward direction (key → value).
        assert_eq!(sm[&1], 10);
        assert_eq!(sm[&2], 20);
    }

    #[test]
    #[should_panic(expected = "key not found")]
    fn index_panics_on_missing() {
        let sm: SymMap<i32> = SymMap::new();
        let _ = sm[&99];
    }

    #[test]
    fn sum_symmaps() {
        let maps: Vec<SymMap<i32>> = vec![
            {let mut m = SymMap::new(); m.insert(1, 10); m},
            {let mut m = SymMap::new(); m.insert(2, 20); m},
        ];
        let total: SymMap<i32> = maps.into_iter().sum();
        assert_eq!(total.len(), 2);
        assert_eq!(total.get(Direction::Forward, &1), Some(&10));
    }

    #[test]
    fn from_vec() {
        let sm: SymMap<i32> = vec![(1i32, 10i32), (2, 20)].into();
        assert_eq!(sm.len(), 2);
    }

    #[test]
    fn from_slice() {
        let sm: SymMap<i32> = [(1i32, 10i32)][..].into();
        assert_eq!(sm.len(), 1);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let sm: SymMap<i32> = SymMap::from(&v);
        assert_eq!(sm.len(), 2);
        assert_eq!(sm.get(Direction::Backward, &10), Some(&1));
    }
}
