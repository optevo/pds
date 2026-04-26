// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent map preserving insertion order.
//!
//! An `InsertionOrderMap` provides the same key-value API as
//! [`HashMap`][crate::HashMap] but iterates entries in the order
//! they were first inserted. Backed by a `HashMap<K, usize>` (key
//! to insertion index) and an `OrdMap<usize, (K, V)>` (index to
//! entry). All operations are O(log n) with structural sharing.
//!
//! # Examples
//!
//! ```
//! use pds::InsertionOrderMap;
//!
//! let mut map = InsertionOrderMap::new();
//! map.insert("c", 3);
//! map.insert("a", 1);
//! map.insert("b", 2);
//!
//! let keys: Vec<_> = map.keys().collect();
//! assert_eq!(keys, vec![&"c", &"a", &"b"]);
//! ```
//!
//! ## Parallel iteration (`rayon` feature)
//!
//! With the `rayon` feature, `InsertionOrderMap` implements
//! [`IntoParallelRefIterator`][rayon::iter::IntoParallelRefIterator], yielding `(&K, &V)` pairs.
//! Note that parallel iteration does not preserve insertion order — worker threads process
//! subsets of the underlying B+ tree non-sequentially.
//!
//! [`FromParallelIterator`][rayon::iter::FromParallelIterator] and
//! [`ParallelExtend`][rayon::iter::ParallelExtend] are intentionally absent: parallel
//! collection fans entries out across threads with no ordering guarantee, so the
//! resulting map would have an arbitrary insertion order. Use the sequential
//! `FromIterator` / `Extend` impls when insertion order must be preserved.

use alloc::vec::Vec;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::FromIterator;
use core::ops::{Index, IndexMut};
#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hash_width::HashWidth;
use crate::hashmap::GenericHashMap;
use crate::ordmap::GenericOrdMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericInsertionOrderMap`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type InsertionOrderMap<K, V> = GenericInsertionOrderMap<K, V, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericInsertionOrderMap`] using [`foldhash::fast::RandomState`] —
/// available in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type InsertionOrderMap<K, V> =
    GenericInsertionOrderMap<K, V, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent map that iterates in insertion order.
///
/// Backed by a hash map for O(log n) key lookup and an ordered map
/// for insertion-ordered iteration. Clone is O(1) via structural
/// sharing.
pub struct GenericInsertionOrderMap<
    K,
    V,
    S,
    P: SharedPointerKind = DefaultSharedPtr,
    H: HashWidth = u64,
> {
    /// Key → insertion index.
    index: GenericHashMap<K, usize, S, P, H>,
    /// Insertion index → (key, value), ordered by index.
    pub(crate) entries: GenericOrdMap<usize, (K, V), P>,
    /// Next index to assign (monotonically increasing).
    next_idx: usize,
}

// Manual Clone.
impl<K: Clone, V: Clone, S: Clone, P: SharedPointerKind, H: HashWidth> Clone
    for GenericInsertionOrderMap<K, V, S, P, H>
{
    fn clone(&self) -> Self {
        GenericInsertionOrderMap {
            index: self.index.clone(),
            entries: self.entries.clone(),
            next_idx: self.next_idx,
        }
    }
}

#[cfg(feature = "std")]
impl<K, V, P> GenericInsertionOrderMap<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty insertion-ordered map.
    #[must_use]
    pub fn new() -> Self {
        GenericInsertionOrderMap {
            index: GenericHashMap::new(),
            entries: GenericOrdMap::new(),
            next_idx: 0,
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> GenericInsertionOrderMap<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty insertion-ordered map (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericInsertionOrderMap {
            index: GenericHashMap::new(),
            entries: GenericOrdMap::new(),
            next_idx: 0,
        }
    }
}

impl<K, V, S, P, H: HashWidth> GenericInsertionOrderMap<K, V, S, P, H>
where
    P: SharedPointerKind,
{
    /// Test whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return the number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Test whether two maps share the same underlying allocation.
    ///
    /// Returns `true` if `self` and `other` are the same version of the
    /// map — i.e. one is a clone of the other with no intervening
    /// mutations. This is a cheap pointer comparison, not a structural
    /// equality check.
    ///
    /// Time: O(1)
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.index.ptr_eq(&other.index) && self.entries.ptr_eq(&other.entries)
    }
}

impl<K, V, S, P, H: HashWidth> GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Get a reference to the value for a key.
    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        let idx = self.index.get(key)?;
        self.entries.get(idx).map(|(_, v)| v)
    }

    /// Get a mutable reference to the value for a key.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        let idx = *self.index.get(key)?;
        self.entries.get_mut(&idx).map(|(_, v)| v)
    }

    /// Test whether a key is present.
    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.index.contains_key(key)
    }

    /// Insert a key-value pair. If the key already exists, its value
    /// is updated but its position in the insertion order is preserved.
    ///
    /// Returns the previous value if the key was already present.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(&idx) = self.index.get(&key) {
            // Key exists — update value, keep position.
            let old = self.entries.get(&idx).map(|(_, v)| v.clone());
            self.entries.insert(idx, (key, value));
            old
        } else {
            // New key — assign next index.
            let idx = self.next_idx;
            self.next_idx += 1;
            self.index.insert(key.clone(), idx);
            self.entries.insert(idx, (key, value));
            None
        }
    }

    /// Remove a key-value pair.
    ///
    /// Returns the removed value if the key was present.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        let idx = self.index.remove_with_key(key)?.1;
        self.entries.remove(&idx).map(|(_, v)| v)
    }

    /// Return a reference to the first key-value pair in insertion order, or `None` if empty.
    pub fn front(&self) -> Option<(&K, &V)> {
        self.entries.get_min().map(|(_, (k, v))| (k, v))
    }

    /// Return a reference to the last key-value pair in insertion order, or `None` if empty.
    pub fn back(&self) -> Option<(&K, &V)> {
        self.entries.get_max().map(|(_, (k, v))| (k, v))
    }

    /// Remove and return the first key-value pair in insertion order (FIFO dequeue).
    ///
    /// Returns `None` if the map is empty.
    pub fn pop_front(&mut self) -> Option<(K, V)> {
        let counter = self.entries.get_min()?.0;
        let (k, v) = self.entries.remove(&counter)?;
        self.index.remove(&k);
        Some((k, v))
    }

    /// Remove and return the last key-value pair in insertion order (LIFO dequeue).
    ///
    /// Returns `None` if the map is empty.
    pub fn pop_back(&mut self) -> Option<(K, V)> {
        let counter = self.entries.get_max()?.0;
        let (k, v) = self.entries.remove(&counter)?;
        self.index.remove(&k);
        Some((k, v))
    }

    /// Iterate over key-value pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(_, (k, v))| (k, v))
    }

    /// Iterate over keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|(_, (k, _))| k)
    }

    /// Iterate over values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, (_, v))| v)
    }

    /// Return the union of two maps; entries from `other` overwrite entries in `self`.
    ///
    /// For conflicting keys, `other`'s value wins. New keys from `other` are
    /// appended in `other`'s insertion order after all of `self`'s keys.
    #[must_use]
    pub fn union(mut self, other: Self) -> Self {
        self.extend(other);
        self
    }
}

impl<K, V, S, P, H: HashWidth> Default for GenericInsertionOrderMap<K, V, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericInsertionOrderMap {
            index: GenericHashMap::default(),
            entries: GenericOrdMap::new(),
            next_idx: 0,
        }
    }
}

impl<K, V, S, P, H: HashWidth> PartialEq for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: PartialEq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        // Two insertion-ordered maps are equal if they have the same
        // entries in the same insertion order.
        self.iter()
            .zip(other.iter())
            .all(|((k1, v1), (k2, v2))| k1 == k2 && v1 == v2)
    }
}

impl<K, V, S, P, H: HashWidth> Eq for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<K, V, S, P, H: HashWidth> Hash for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        // Ordered: insertion order is part of identity.
        for (k, v) in self.iter() {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl<K, V, S, P, H: HashWidth> Debug for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Debug + Hash + Eq + Clone,
    V: Debug + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, v) in self.iter() {
            d.entry(k, v);
        }
        d.finish()
    }
}

impl<K, V, S, P, H: HashWidth> FromIterator<(K, V)> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = GenericInsertionOrderMap {
            index: GenericHashMap::default(),
            entries: GenericOrdMap::new(),
            next_idx: 0,
        };
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<K, V, S, P, H: HashWidth> From<Vec<(K, V)>> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: Vec<(K, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K, V, S, const N: usize, P, H: HashWidth> From<[(K, V); N]>
    for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(arr: [(K, V); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a [(K, V)]> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [(K, V)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a Vec<(K, V)>> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<(K, V)>) -> Self {
        v.iter().cloned().collect()
    }
}

impl<Q, K, V, S, P, H: HashWidth> Index<&Q> for GenericInsertionOrderMap<K, V, S, P, H>
where
    Q: Hash + Equivalent<K> + ?Sized,
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        match self.get(key) {
            Some(v) => v,
            None => panic!("InsertionOrderMap::index: key not found"),
        }
    }
}

impl<Q, K, V, S, P, H: HashWidth> IndexMut<&Q> for GenericInsertionOrderMap<K, V, S, P, H>
where
    Q: Hash + Equivalent<K> + ?Sized,
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn index_mut(&mut self, key: &Q) -> &mut Self::Output {
        match self.get_mut(key) {
            Some(v) => v,
            None => panic!("InsertionOrderMap::index_mut: key not found"),
        }
    }
}

impl<K, V, S, P, H: HashWidth> GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    /// Return entries whose keys are in `self` but not in `other`.
    #[must_use]
    pub fn difference(self, other: &Self) -> Self {
        self.into_iter()
            .filter(|(k, _)| !other.contains_key(k))
            .collect()
    }

    /// Return entries whose keys are in both `self` and `other`; `self`'s values are kept.
    #[must_use]
    pub fn intersection(self, other: &Self) -> Self {
        self.into_iter()
            .filter(|(k, _)| other.contains_key(k))
            .collect()
    }

    /// Return entries whose keys are in exactly one of `self` or `other`.
    ///
    /// The result preserves insertion order: self's unique entries first
    /// (in their original order), followed by other's unique entries.
    #[must_use]
    pub fn symmetric_difference(self, other: &Self) -> Self {
        // Clone self before consuming it — O(1) via structural sharing — so we can
        // check key membership for other's entries after self is consumed.
        let self_clone = self.clone();
        let self_diff: Self = self
            .into_iter()
            .filter(|(k, _)| !other.contains_key(k))
            .collect();
        let other_diff: Self = other
            .clone()
            .into_iter()
            .filter(|(k, _)| !self_clone.contains_key(k))
            .collect();
        self_diff.union(other_diff)
    }
}

impl<K, V, S, P, H: HashWidth> Extend<(K, V)> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

/// A consuming iterator over the entries of a [`GenericInsertionOrderMap`].
///
/// Yields `(K, V)` pairs in insertion order.
pub struct ConsumingIter<K, V, P: SharedPointerKind> {
    inner: crate::ordmap::ConsumingIter<usize, (K, V), P>,
}

impl<K, V, P> Iterator for ConsumingIter<K, V, P>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(_, (k, v))| (k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V, P> ExactSizeIterator for ConsumingIter<K, V, P>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
}

impl<K, V, P> core::iter::FusedIterator for ConsumingIter<K, V, P>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
}

impl<K, V, S, P, H: HashWidth> IntoIterator for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<K, V, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.entries.into_iter(),
        }
    }
}

impl<'a, K, V, S, P, H: HashWidth> IntoIterator for &'a GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Item = (&'a K, &'a V);
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = (&'a K, &'a V)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.iter())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(crate::InsertionOrderMap<i32, i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let map: InsertionOrderMap<&str, i32> = InsertionOrderMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut map = InsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get("b"), Some(&2));
        assert_eq!(map.get("c"), None);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn insertion_order_preserved() {
        let mut map = InsertionOrderMap::new();
        map.insert("c", 3);
        map.insert("a", 1);
        map.insert("b", 2);

        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"c", &"a", &"b"]);

        let values: Vec<_> = map.values().collect();
        assert_eq!(values, vec![&3, &1, &2]);
    }

    #[test]
    fn update_preserves_position() {
        let mut map = InsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);

        // Update "b" — should keep its position
        let old = map.insert("b", 20);
        assert_eq!(old, Some(2));

        let pairs: Vec<_> = map.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(pairs, vec![("a", 1), ("b", 20), ("c", 3)]);
    }

    #[test]
    fn remove() {
        let mut map = InsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);

        let removed = map.remove("b");
        assert_eq!(removed, Some(2));
        assert!(!map.contains_key("b"));
        assert_eq!(map.len(), 2);

        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"a", &"c"]);
    }

    #[test]
    fn remove_absent() {
        let mut map: InsertionOrderMap<&str, i32> = InsertionOrderMap::new();
        assert_eq!(map.remove("x"), None);
    }

    #[test]
    fn contains_key() {
        let mut map = InsertionOrderMap::new();
        assert!(!map.contains_key("a"));
        map.insert("a", 1);
        assert!(map.contains_key("a"));
    }

    #[test]
    fn from_iterator() {
        let map: InsertionOrderMap<&str, i32> =
            vec![("c", 3), ("a", 1), ("b", 2)].into_iter().collect();
        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"c", &"a", &"b"]);
    }

    #[test]
    fn clone_shares_structure() {
        let mut map = InsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        let map2 = map.clone();
        assert_eq!(map, map2);
    }

    #[test]
    fn equality_order_matters() {
        let mut a = InsertionOrderMap::new();
        a.insert("x", 1);
        a.insert("y", 2);

        let mut b = InsertionOrderMap::new();
        b.insert("y", 2);
        b.insert("x", 1);

        // Different insertion order → not equal
        assert_ne!(a, b);
    }

    #[test]
    fn equality_same_order() {
        let mut a = InsertionOrderMap::new();
        a.insert("x", 1);
        a.insert("y", 2);

        let mut b = InsertionOrderMap::new();
        b.insert("x", 1);
        b.insert("y", 2);

        assert_eq!(a, b);
    }

    #[test]
    fn into_iter_owned() {
        let mut map = InsertionOrderMap::new();
        map.insert("c", 3);
        map.insert("a", 1);
        map.insert("b", 2);

        let pairs: Vec<_> = map.into_iter().collect();
        assert_eq!(pairs, vec![("c", 3), ("a", 1), ("b", 2)]);
    }

    #[test]
    fn into_iter_ref() {
        let mut map = InsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);

        let pairs: Vec<_> = (&map).into_iter().collect();
        assert_eq!(pairs, vec![(&"a", &1), (&"b", &2)]);
    }

    #[test]
    fn for_loop() {
        let mut map = InsertionOrderMap::new();
        map.insert("x", 10);
        map.insert("y", 20);

        let mut sum = 0;
        for (_, &v) in &map {
            sum += v;
        }
        assert_eq!(sum, 30);
    }

    #[test]
    fn into_iter_preserves_insertion_order() {
        let mut map = InsertionOrderMap::new();
        map.insert(3, "c");
        map.insert(1, "a");
        map.insert(2, "b");

        let keys: Vec<_> = map.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![3, 1, 2]);
    }

    #[test]
    fn remove_then_reinsert_changes_order() {
        let mut map = InsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);
        map.remove("a");
        map.insert("a", 10);

        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"b", &"c", &"a"]);
    }

    #[test]
    fn debug_format() {
        let mut m = InsertionOrderMap::new();
        m.insert("k", 1i32);
        let s = format!("{:?}", m);
        assert!(!s.is_empty());
    }

    #[test]
    fn default_is_empty() {
        let m: InsertionOrderMap<i32, i32> = InsertionOrderMap::default();
        assert!(m.is_empty());
    }

    #[test]
    fn hash_same_for_equal_maps() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(m: &InsertionOrderMap<i32, i32>) -> u64 {
            let mut h = DefaultHasher::new();
            m.hash(&mut h);
            h.finish()
        }
        // Insertion order is part of identity, so same entries same order → equal hash.
        let mut a = InsertionOrderMap::new();
        a.insert(1, 10);
        a.insert(2, 20);
        let mut b = InsertionOrderMap::new();
        b.insert(1, 10);
        b.insert(2, 20);
        assert_eq!(hash_of(&a), hash_of(&b));
        // Different order → different hash (with high probability).
        let mut c = InsertionOrderMap::new();
        c.insert(2, 20);
        c.insert(1, 10);
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn union_method() {
        let mut a = InsertionOrderMap::new();
        a.insert("x", 1i32);
        a.insert("y", 2);
        let mut b = InsertionOrderMap::new();
        b.insert("y", 99); // conflict: b wins
        b.insert("z", 3);
        let c = a.union(b);
        assert_eq!(c.get("x"), Some(&1));
        assert_eq!(c.get("y"), Some(&99));
        assert_eq!(c.get("z"), Some(&3));
    }

    #[test]
    fn difference() {
        let mut a = InsertionOrderMap::new();
        a.insert("x", 1i32);
        a.insert("y", 2);
        a.insert("z", 3);
        let mut b = InsertionOrderMap::new();
        b.insert("y", 99);
        let c = a.difference(&b);
        assert_eq!(c.len(), 2);
        assert!(c.contains_key("x"));
        assert!(!c.contains_key("y"));
        assert!(c.contains_key("z"));
    }

    #[test]
    fn intersection_method() {
        let mut a = InsertionOrderMap::new();
        a.insert("x", 1i32);
        a.insert("y", 2);
        let mut b = InsertionOrderMap::new();
        b.insert("y", 99);
        b.insert("z", 3);
        let c = a.intersection(&b);
        assert_eq!(c.len(), 1);
        assert!(c.contains_key("y"));
        assert_eq!(c.get("y"), Some(&2)); // self's value is kept
        assert!(!c.contains_key("x"));
    }

    #[test]
    fn symmetric_difference_method() {
        let mut a = InsertionOrderMap::new();
        a.insert("x", 1i32);
        a.insert("y", 2);
        a.insert("z", 3);
        let mut b = InsertionOrderMap::new();
        b.insert("y", 99); // shared — excluded
        b.insert("w", 4); // only in b
        let c = a.symmetric_difference(&b);
        assert_eq!(c.len(), 3); // x, z (from a), w (from b)
        assert!(c.contains_key("x"));
        assert!(!c.contains_key("y"));
        assert!(c.contains_key("z"));
        assert!(c.contains_key("w"));
    }

    #[test]
    fn symmetric_difference_disjoint() {
        let mut a = InsertionOrderMap::new();
        a.insert(1i32, "a");
        let mut b = InsertionOrderMap::new();
        b.insert(2i32, "b");
        let c = a.symmetric_difference(&b);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn extend_adds_pairs() {
        let mut m: InsertionOrderMap<i32, i32> = InsertionOrderMap::new();
        m.extend(vec![(1, 10), (2, 20)]);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get(&1), Some(&10));
    }

    #[test]
    fn from_vec() {
        let m: InsertionOrderMap<i32, i32> = vec![(1, 10), (2, 20)].into();
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn from_array() {
        let m: InsertionOrderMap<i32, i32> = [(1i32, 10i32), (2, 20)].into();
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn from_slice() {
        let m: InsertionOrderMap<i32, i32> = [(1i32, 10i32)][..].into();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let m: InsertionOrderMap<i32, i32> = InsertionOrderMap::from(&v);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn index_and_index_mut() {
        let mut m = InsertionOrderMap::new();
        m.insert("a", 1i32);
        assert_eq!(m["a"], 1);
        m["a"] = 99;
        assert_eq!(m.get("a"), Some(&99));
    }

    #[test]
    #[should_panic(expected = "key not found")]
    fn index_panics_on_missing() {
        let m: InsertionOrderMap<&str, i32> = InsertionOrderMap::new();
        let _ = m["missing"];
    }

    #[test]
    fn get_mut_updates_value() {
        let mut m = InsertionOrderMap::new();
        m.insert("a", 1i32);
        *m.get_mut("a").unwrap() = 42;
        assert_eq!(m.get("a"), Some(&42));
        assert_eq!(m.get_mut("z"), None);
    }

    #[test]
    fn front_and_back_empty() {
        let m: InsertionOrderMap<&str, i32> = InsertionOrderMap::new();
        assert_eq!(m.front(), None);
        assert_eq!(m.back(), None);
    }

    #[test]
    fn front_and_back_single() {
        let mut m = InsertionOrderMap::new();
        m.insert("only", 1i32);
        assert_eq!(m.front(), Some((&"only", &1)));
        assert_eq!(m.back(), Some((&"only", &1)));
    }

    #[test]
    fn front_and_back_multiple() {
        let mut m = InsertionOrderMap::new();
        m.insert("a", 1i32);
        m.insert("b", 2i32);
        m.insert("c", 3i32);
        assert_eq!(m.front(), Some((&"a", &1)));
        assert_eq!(m.back(), Some((&"c", &3)));
    }

    #[test]
    fn pop_front_fifo_order() {
        let mut m = InsertionOrderMap::new();
        m.insert("a", 1i32);
        m.insert("b", 2i32);
        m.insert("c", 3i32);
        assert_eq!(m.pop_front(), Some(("a", 1)));
        assert_eq!(m.pop_front(), Some(("b", 2)));
        assert_eq!(m.pop_front(), Some(("c", 3)));
        assert_eq!(m.pop_front(), None);
        assert!(m.is_empty());
    }

    #[test]
    fn pop_back_lifo_order() {
        let mut m = InsertionOrderMap::new();
        m.insert("a", 1i32);
        m.insert("b", 2i32);
        m.insert("c", 3i32);
        assert_eq!(m.pop_back(), Some(("c", 3)));
        assert_eq!(m.pop_back(), Some(("b", 2)));
        assert_eq!(m.pop_back(), Some(("a", 1)));
        assert_eq!(m.pop_back(), None);
    }

    #[test]
    fn pop_front_removes_from_lookup() {
        // After pop_front, the key must no longer be findable via get().
        let mut m = InsertionOrderMap::new();
        m.insert("x", 42i32);
        m.insert("y", 99i32);
        m.pop_front();
        assert_eq!(m.get("x"), None);
        assert!(m.contains_key("y"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn pop_front_unique_queue_dedup() {
        // Simulate a deduplicating FIFO work queue.
        let mut queue = InsertionOrderMap::<&str, ()>::new();
        queue.insert("task-a", ());
        queue.insert("task-b", ());
        queue.insert("task-a", ()); // duplicate — no-op
        queue.insert("task-c", ());
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop_front().map(|(k, _)| k), Some("task-a"));
        assert_eq!(queue.pop_front().map(|(k, _)| k), Some("task-b"));
        assert_eq!(queue.pop_front().map(|(k, _)| k), Some("task-c"));
        assert_eq!(queue.pop_front(), None);
    }
}
