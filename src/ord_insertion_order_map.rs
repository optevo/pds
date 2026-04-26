// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent map preserving insertion order, backed entirely by `OrdMap`.
//!
//! An [`OrdInsertionOrderMap`] iterates entries in the order they were first
//! inserted. Backed by two [`OrdMap`][crate::OrdMap]s:
//!
//! - `OrdMap<K, usize>` — key → insertion counter (O(log n) lookup)
//! - `OrdMap<usize, (K, V)>` — counter → entry (O(log n) ordered iteration)
//!
//! All operations are O(log n) with structural sharing. Unlike
//! [`InsertionOrderMap`][crate::InsertionOrderMap], this type requires only
//! `K: Ord + Clone` — no `Hash + Eq` constraint — and works in `no_std`
//! without the `foldhash` feature.
//!
//! # Examples
//!
//! ```
//! use pds::OrdInsertionOrderMap;
//!
//! let mut map = OrdInsertionOrderMap::new();
//! map.insert("c", 3);
//! map.insert("a", 1);
//! map.insert("b", 2);
//!
//! // Iteration is in insertion order.
//! let keys: Vec<_> = map.keys().collect();
//! assert_eq!(keys, vec![&"c", &"a", &"b"]);
//!
//! // Lookup is O(log n) regardless of insertion order.
//! assert_eq!(map.get(&"a"), Some(&1));
//! ```

use alloc::vec::Vec;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;
use core::ops::{Index, IndexMut};

use archery::SharedPointerKind;
use equivalent::Comparable;

use crate::ord::map::ConsumingIter as OrdMapConsumingIter;
use crate::ordmap::GenericOrdMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericOrdInsertionOrderMap`] with the default pointer type.
pub type OrdInsertionOrderMap<K, V> = GenericOrdInsertionOrderMap<K, V, DefaultSharedPtr>;

/// A persistent map that iterates in insertion order, backed by two [`GenericOrdMap`]s.
///
/// - `key_index: OrdMap<K, usize>` maps keys to their insertion counters.
/// - `entries: OrdMap<usize, (K, V)>` maps counters to entries in insertion order.
///
/// Deletion is O(log n) with no tombstones — the counter-indexed OrdMap and the
/// key OrdMap are both cleaned up in a single remove operation each. Clone is O(1).
pub struct GenericOrdInsertionOrderMap<K, V, P: SharedPointerKind = DefaultSharedPtr> {
    /// Key → insertion counter.
    key_index: GenericOrdMap<K, usize, P>,
    /// Counter → (key, value), iterated in insertion order.
    pub(crate) entries: GenericOrdMap<usize, (K, V), P>,
    /// Next counter to assign (monotonically increasing).
    next_idx: usize,
}

// Manual Clone — avoid spurious `P: Clone` bound from derive.
impl<K: Clone, V: Clone, P: SharedPointerKind> Clone for GenericOrdInsertionOrderMap<K, V, P> {
    fn clone(&self) -> Self {
        GenericOrdInsertionOrderMap {
            key_index: self.key_index.clone(),
            entries: self.entries.clone(),
            next_idx: self.next_idx,
        }
    }
}

impl<K, V, P: SharedPointerKind> GenericOrdInsertionOrderMap<K, V, P> {
    /// Create an empty OrdInsertionOrderMap.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdInsertionOrderMap {
            key_index: GenericOrdMap::new(),
            entries: GenericOrdMap::new(),
            next_idx: 0,
        }
    }

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
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> GenericOrdInsertionOrderMap<K, V, P> {
    /// Get a reference to the value for a key.
    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Comparable<K> + ?Sized,
    {
        let idx = self.key_index.get(key)?;
        self.entries.get(idx).map(|(_, v)| v)
    }

    /// Get a mutable reference to the value for a key.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Comparable<K> + ?Sized,
    {
        let idx = *self.key_index.get(key)?;
        self.entries.get_mut(&idx).map(|(_, v)| v)
    }

    /// Test whether a key is present.
    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        self.key_index.contains_key(key)
    }

    /// Insert a key-value pair.
    ///
    /// If the key already exists, its value is updated but its position in
    /// the insertion order is preserved. Returns the previous value.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(&idx) = self.key_index.get(&key) {
            // Key exists — update value, keep position.
            let old = self.entries.get(&idx).map(|(_, v)| v.clone());
            self.entries.insert(idx, (key, value));
            old
        } else {
            // New key — assign next counter.
            let idx = self.next_idx;
            self.next_idx += 1;
            self.key_index.insert(key.clone(), idx);
            self.entries.insert(idx, (key, value));
            None
        }
    }

    /// Remove a key-value pair. Returns the removed value if present.
    ///
    /// O(log n) — no tombstones. Both OrdMaps are cleaned up immediately.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: Comparable<K> + ?Sized,
    {
        let idx = self.key_index.remove_with_key(key)?.1;
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
        self.key_index.remove(&k);
        Some((k, v))
    }

    /// Remove and return the last key-value pair in insertion order (LIFO dequeue).
    ///
    /// Returns `None` if the map is empty.
    pub fn pop_back(&mut self) -> Option<(K, V)> {
        let counter = self.entries.get_max()?.0;
        let (k, v) = self.entries.remove(&counter)?;
        self.key_index.remove(&k);
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
    /// New keys from `other` are appended in `other`'s insertion order after
    /// all of `self`'s keys.
    #[must_use]
    pub fn union(mut self, other: Self) -> Self {
        self.extend(other);
        self
    }

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
    /// `self`'s unique entries come first (in their original insertion order),
    /// followed by `other`'s unique entries.
    #[must_use]
    pub fn symmetric_difference(self, other: &Self) -> Self {
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

impl<K: Ord, V, P: SharedPointerKind> Default for GenericOrdInsertionOrderMap<K, V, P> {
    fn default() -> Self {
        GenericOrdInsertionOrderMap {
            key_index: GenericOrdMap::new(),
            entries: GenericOrdMap::new(),
            next_idx: 0,
        }
    }
}

impl<K: Ord + Clone, V: PartialEq + Clone, P: SharedPointerKind> PartialEq
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        // Two insertion-ordered maps are equal only when they have the same
        // entries in the same insertion order.
        self.iter()
            .zip(other.iter())
            .all(|((k1, v1), (k2, v2))| k1 == k2 && v1 == v2)
    }
}

impl<K: Ord + Clone, V: Eq + Clone, P: SharedPointerKind> Eq
    for GenericOrdInsertionOrderMap<K, V, P>
{
}

impl<K: Ord + Clone + Hash, V: Hash + Clone, P: SharedPointerKind> Hash
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Ordered: insertion order is part of identity, so sequential hashing is correct.
        for (k, v) in self.iter() {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl<K: Ord + Clone + Debug, V: Debug + Clone, P: SharedPointerKind> Debug
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, v) in self.iter() {
            d.entry(k, v);
        }
        d.finish()
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> FromIterator<(K, V)>
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> From<Vec<(K, V)>>
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn from(v: Vec<(K, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K: Ord + Clone, V: Clone, const N: usize, P: SharedPointerKind> From<[(K, V); N]>
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn from(arr: [(K, V); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, K: Ord + Clone, V: Clone, P: SharedPointerKind> From<&'a [(K, V)]>
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn from(slice: &'a [(K, V)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, K: Ord + Clone, V: Clone, P: SharedPointerKind> From<&'a Vec<(K, V)>>
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn from(v: &'a Vec<(K, V)>) -> Self {
        v.iter().cloned().collect()
    }
}

impl<Q, K: Ord + Clone, V: Clone, P: SharedPointerKind> Index<&Q>
    for GenericOrdInsertionOrderMap<K, V, P>
where
    Q: Comparable<K> + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        match self.get(key) {
            Some(v) => v,
            None => panic!("OrdInsertionOrderMap::index: key not found"),
        }
    }
}

impl<Q, K: Ord + Clone, V: Clone, P: SharedPointerKind> IndexMut<&Q>
    for GenericOrdInsertionOrderMap<K, V, P>
where
    Q: Comparable<K> + ?Sized,
{
    fn index_mut(&mut self, key: &Q) -> &mut Self::Output {
        match self.get_mut(key) {
            Some(v) => v,
            None => panic!("OrdInsertionOrderMap::index_mut: key not found"),
        }
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Extend<(K, V)>
    for GenericOrdInsertionOrderMap<K, V, P>
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

/// A consuming iterator over the entries of a [`GenericOrdInsertionOrderMap`].
///
/// Yields `(K, V)` pairs in insertion order.
pub struct ConsumingIter<K, V, P: SharedPointerKind> {
    inner: OrdMapConsumingIter<usize, (K, V), P>,
}

impl<K: Clone, V: Clone, P: SharedPointerKind> Iterator for ConsumingIter<K, V, P> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(_, (k, v))| (k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K: Clone, V: Clone, P: SharedPointerKind> ExactSizeIterator for ConsumingIter<K, V, P> {}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> IntoIterator
    for GenericOrdInsertionOrderMap<K, V, P>
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<K, V, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.entries.into_iter(),
        }
    }
}

impl<'a, K: Ord + Clone, V: Clone, P: SharedPointerKind> IntoIterator
    for &'a GenericOrdInsertionOrderMap<K, V, P>
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

    assert_impl_all!(OrdInsertionOrderMap<i32, i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let map: OrdInsertionOrderMap<&str, i32> = OrdInsertionOrderMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(map.get(&"a"), Some(&1));
        assert_eq!(map.get(&"b"), Some(&2));
        assert_eq!(map.get(&"c"), None);
    }

    #[test]
    fn insertion_order_preserved() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("c", 3);
        map.insert("a", 1);
        map.insert("b", 2);

        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"c", &"a", &"b"]);
    }

    #[test]
    fn update_preserves_order() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("b", 20);
        map.insert("a", 10);
        map.insert("b", 99); // Update "b", should stay first.

        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"b", &"a"]);
        assert_eq!(map.get(&"b"), Some(&99));
    }

    #[test]
    fn update_returns_old_value() {
        let mut map = OrdInsertionOrderMap::new();
        assert_eq!(map.insert("a", 1), None);
        assert_eq!(map.insert("a", 2), Some(1));
    }

    #[test]
    fn remove_cleans_up() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);

        let removed = map.remove(&"a");
        assert_eq!(removed, Some(1));
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key(&"a"));
        assert_eq!(map.get(&"b"), Some(&2));
    }

    #[test]
    fn remove_absent_returns_none() {
        let mut map: OrdInsertionOrderMap<&str, i32> = OrdInsertionOrderMap::new();
        assert_eq!(map.remove(&"x"), None);
    }

    #[test]
    fn remove_then_reinsert_appends() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        map.remove(&"a");
        map.insert("a", 10); // Reinserted at end.

        let keys: Vec<_> = map.keys().collect();
        assert_eq!(keys, vec![&"b", &"a"]);
    }

    #[test]
    fn contains_key() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("x", 42);
        assert!(map.contains_key(&"x"));
        assert!(!map.contains_key(&"y"));
    }

    #[test]
    fn iter_insertion_order() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert(3, 30);
        map.insert(1, 10);
        map.insert(2, 20);

        let pairs: Vec<_> = map.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![(3, 30), (1, 10), (2, 20)]);
    }

    #[test]
    fn values_insertion_order() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("b", 2);
        map.insert("a", 1);

        let vals: Vec<_> = map.values().copied().collect();
        assert_eq!(vals, vec![2, 1]);
    }

    #[test]
    fn equality_same_order() {
        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert("a", 1);
        m1.insert("b", 2);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert("a", 1);
        m2.insert("b", 2);

        assert_eq!(m1, m2);
    }

    #[test]
    fn inequality_different_order() {
        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert("a", 1);
        m1.insert("b", 2);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert("b", 2);
        m2.insert("a", 1);

        assert_ne!(m1, m2);
    }

    #[test]
    fn hash_same_for_equal_maps() {
        use std::hash::DefaultHasher;

        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert(1i32, 10i32);
        m1.insert(2, 20);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert(1i32, 10i32);
        m2.insert(2, 20);

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        m1.hash(&mut h1);
        m2.hash(&mut h2);
        assert_eq!(m1, m2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn clone_shares_structure() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("a", 1);
        let clone = map.clone();
        assert_eq!(map, clone);
    }

    #[test]
    fn default_is_empty() {
        let map: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::default();
        assert!(map.is_empty());
    }

    #[test]
    fn from_vec() {
        let map: OrdInsertionOrderMap<&str, i32> =
            OrdInsertionOrderMap::from(vec![("a", 1), ("b", 2)]);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&"a"), Some(&1));
    }

    #[test]
    fn from_array() {
        let map: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::from([(1, 10), (2, 20)]);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn from_slice() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let map: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::from(v.as_slice());
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let map: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::from(&v);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn from_iterator() {
        let map: OrdInsertionOrderMap<&str, i32> =
            vec![("a", 1), ("b", 2)].into_iter().collect();
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn extend() {
        let mut map: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::new();
        map.extend([(1, 10), (2, 20)]);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn into_iter_insertion_order() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert(3i32, 30i32);
        map.insert(1, 10);
        map.insert(2, 20);

        let pairs: Vec<_> = map.into_iter().collect();
        assert_eq!(pairs, vec![(3, 30), (1, 10), (2, 20)]);
    }

    #[test]
    fn into_iter_ref() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert(1i32, 10i32);

        let pairs: Vec<_> = (&map).into_iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![(1, 10)]);
    }

    #[test]
    fn index_returns_value() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert(1i32, 10i32);
        assert_eq!(map[&1i32], 10);
    }

    #[test]
    fn index_mut_updates_value() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert(1i32, 10i32);
        map[&1i32] = 99;
        assert_eq!(map.get(&1), Some(&99));
    }

    #[test]
    #[should_panic]
    fn index_panics_on_missing() {
        let map: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::new();
        let _ = map[&99i32];
    }

    #[test]
    fn union_appends_new_keys() {
        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert(1i32, 10i32);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert(2i32, 20i32);

        let combined = m1.union(m2);
        assert_eq!(combined.len(), 2);
        let keys: Vec<_> = combined.keys().copied().collect();
        assert_eq!(keys, vec![1, 2]);
    }

    #[test]
    fn difference() {
        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert(1i32, 10i32);
        m1.insert(2, 20);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert(2i32, 20i32);

        let diff = m1.difference(&m2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.get(&1), Some(&10));
    }

    #[test]
    fn intersection() {
        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert(1i32, 10i32);
        m1.insert(2, 20);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert(2i32, 99i32);
        m2.insert(3, 30);

        let inter = m1.intersection(&m2);
        assert_eq!(inter.len(), 1);
        assert_eq!(inter.get(&2), Some(&20));
    }

    #[test]
    fn symmetric_difference() {
        let mut m1 = OrdInsertionOrderMap::new();
        m1.insert(1i32, 10i32);
        m1.insert(2, 20);

        let mut m2 = OrdInsertionOrderMap::new();
        m2.insert(2i32, 99i32);
        m2.insert(3, 30);

        let sd = m1.symmetric_difference(&m2);
        assert_eq!(sd.len(), 2);
        assert!(sd.contains_key(&1));
        assert!(sd.contains_key(&3));
        assert!(!sd.contains_key(&2));
    }

    #[test]
    fn get_mut() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert("a", 1i32);
        *map.get_mut(&"a").unwrap() = 42;
        assert_eq!(map.get(&"a"), Some(&42));
    }

    #[test]
    fn debug_format() {
        let mut map = OrdInsertionOrderMap::new();
        map.insert(1i32, 10i32);
        let s = format!("{:?}", map);
        assert!(s.contains("1"));
        assert!(s.contains("10"));
    }

    #[test]
    fn front_and_back_empty() {
        let m: OrdInsertionOrderMap<i32, i32> = OrdInsertionOrderMap::new();
        assert_eq!(m.front(), None);
        assert_eq!(m.back(), None);
    }

    #[test]
    fn front_and_back_multiple() {
        let mut m = OrdInsertionOrderMap::new();
        m.insert(1i32, 10i32);
        m.insert(2i32, 20i32);
        m.insert(3i32, 30i32);
        assert_eq!(m.front(), Some((&1, &10)));
        assert_eq!(m.back(), Some((&3, &30)));
    }

    #[test]
    fn pop_front_fifo_order() {
        let mut m = OrdInsertionOrderMap::new();
        m.insert(1i32, 10i32);
        m.insert(2i32, 20i32);
        m.insert(3i32, 30i32);
        assert_eq!(m.pop_front(), Some((1, 10)));
        assert_eq!(m.pop_front(), Some((2, 20)));
        assert_eq!(m.pop_front(), Some((3, 30)));
        assert_eq!(m.pop_front(), None);
        assert!(m.is_empty());
    }

    #[test]
    fn pop_back_lifo_order() {
        let mut m = OrdInsertionOrderMap::new();
        m.insert(1i32, 10i32);
        m.insert(2i32, 20i32);
        m.insert(3i32, 30i32);
        assert_eq!(m.pop_back(), Some((3, 30)));
        assert_eq!(m.pop_back(), Some((2, 20)));
        assert_eq!(m.pop_back(), Some((1, 10)));
        assert_eq!(m.pop_back(), None);
    }

    #[test]
    fn pop_front_removes_from_key_index() {
        let mut m = OrdInsertionOrderMap::new();
        m.insert(10i32, "a");
        m.insert(20i32, "b");
        m.pop_front();
        assert_eq!(m.get(&10), None);
        assert!(m.contains_key(&20));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn pop_front_unique_queue_dedup() {
        let mut queue = OrdInsertionOrderMap::<i32, ()>::new();
        queue.insert(1, ());
        queue.insert(2, ());
        queue.insert(1, ()); // duplicate — no-op
        queue.insert(3, ());
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop_front().map(|(k, _)| k), Some(1));
        assert_eq!(queue.pop_front().map(|(k, _)| k), Some(2));
        assert_eq!(queue.pop_front().map(|(k, _)| k), Some(3));
        assert_eq!(queue.pop_front(), None);
    }
}
