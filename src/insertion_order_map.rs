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

#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash};
use core::iter::FromIterator;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hash_width::HashWidth;
use crate::hashmap::GenericHashMap;
use crate::ordmap::GenericOrdMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericInsertionOrderMap`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type InsertionOrderMap<K, V> =
    GenericInsertionOrderMap<K, V, RandomState, DefaultSharedPtr>;

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
    entries: GenericOrdMap<usize, (K, V), P>,
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
}

#[cfg(feature = "std")]
impl<K, V, P> Default for GenericInsertionOrderMap<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> Default for GenericInsertionOrderMap<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
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
}
