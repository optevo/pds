// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent sorted bidirectional map (bijection between two types).
//!
//! An [`OrdBiMap`] maintains a one-to-one mapping between keys and values,
//! backed by two [`OrdMap`][crate::OrdMap]s (forward: K→V, backward: V→K).
//! Both directions support O(log n) lookup. Iteration is always in sorted
//! forward-key order.
//!
//! # Bijection invariant
//!
//! Every key maps to exactly one value, and every value maps to exactly one
//! key. Inserting a pair `(k, v)` will remove any existing mapping for `k`
//! *and* any existing mapping for `v` before establishing the new pair.
//!
//! Prefer [`OrdBiMap`] over [`BiMap`][crate::BiMap] when:
//! - Keys or values implement `Ord` but not `Hash + Eq`.
//! - You need sorted iteration without a separate sort step.
//! - You want `PartialOrd` / `Ord` on the bimap itself.
//!
//! # Examples
//!
//! ```
//! use pds::OrdBiMap;
//!
//! let mut bm = OrdBiMap::new();
//! bm.insert("alice", 1);
//! bm.insert("bob", 2);
//!
//! assert_eq!(bm.get_by_key(&"alice"), Some(&1));
//! assert_eq!(bm.get_by_value(&2), Some(&"bob"));
//!
//! // Iteration is always in sorted key order.
//! let pairs: Vec<_> = bm.iter().map(|(k, v)| (*k, *v)).collect();
//! assert_eq!(pairs, vec![("alice", 1), ("bob", 2)]);
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

/// Type alias for [`GenericOrdBiMap`] with the default pointer type.
pub type OrdBiMap<K, V> = GenericOrdBiMap<K, V, DefaultSharedPtr>;

/// A persistent sorted bidirectional map backed by two [`GenericOrdMap`]s.
///
/// Maintains a bijection: each key maps to exactly one value and vice versa.
/// Clone is O(1) via structural sharing.
///
/// Unlike [`BiMap`][crate::BiMap], this type requires only `K: Ord + Clone` and
/// `V: Ord + Clone` — no `Hash + Eq` constraint. Iteration is always in sorted key order.
pub struct GenericOrdBiMap<K, V, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) forward: GenericOrdMap<K, V, P>,
    pub(crate) backward: GenericOrdMap<V, K, P>,
}

// Manual Clone — avoid spurious `P: Clone` bound from derive.
impl<K: Clone, V: Clone, P: SharedPointerKind> Clone for GenericOrdBiMap<K, V, P> {
    fn clone(&self) -> Self {
        GenericOrdBiMap {
            forward: self.forward.clone(),
            backward: self.backward.clone(),
        }
    }
}

impl<K, V, P: SharedPointerKind> GenericOrdBiMap<K, V, P> {
    /// Create an empty OrdBiMap.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdBiMap {
            forward: GenericOrdMap::new(),
            backward: GenericOrdMap::new(),
        }
    }

    /// Test whether the bimap is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Return the number of key-value pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }
}

impl<K: Ord, V: Ord, P: SharedPointerKind> GenericOrdBiMap<K, V, P> {
    /// Iterate over all key-value pairs in sorted key order.
    pub fn iter(&self) -> MapIter<'_, K, V, P> {
        self.forward.iter()
    }

    /// Iterate over all keys in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.forward.keys()
    }

    /// Iterate over all values in sorted key order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.forward.values()
    }

    /// Look up a value by its key.
    #[must_use]
    pub fn get_by_key<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Comparable<K> + ?Sized,
    {
        self.forward.get(key)
    }

    /// Look up a key by its value.
    #[must_use]
    pub fn get_by_value<Q>(&self, value: &Q) -> Option<&K>
    where
        Q: Comparable<V> + ?Sized,
    {
        self.backward.get(value)
    }

    /// Test whether a key is present.
    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        self.forward.contains_key(key)
    }

    /// Test whether a value is present.
    #[must_use]
    pub fn contains_value<Q>(&self, value: &Q) -> bool
    where
        Q: Comparable<V> + ?Sized,
    {
        self.backward.contains_key(value)
    }
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> GenericOrdBiMap<K, V, P> {
    /// Insert a key-value pair, maintaining the bijection invariant.
    ///
    /// If `key` already maps to a value, the old value's backward entry is
    /// removed. If `value` already maps to a key, the old key's forward entry
    /// is removed. Then the new pair is established in both directions.
    ///
    /// Returns `None` if neither `key` nor `value` was previously present.
    /// Returns `Some((old_value, old_key))` if an existing mapping was displaced.
    pub fn insert(&mut self, key: K, value: V) -> Option<(Option<V>, Option<K>)> {
        let old_value = self.forward.remove(&key);
        let old_key = self.backward.remove(&value);

        // Clean up cross-references from displaced entries.
        if let Some(ref ov) = old_value {
            self.backward.remove(ov);
        }
        if let Some(ref ok) = old_key {
            self.forward.remove(ok);
        }

        self.forward.insert(key.clone(), value.clone());
        self.backward.insert(value, key);

        if old_value.is_some() || old_key.is_some() {
            Some((old_value, old_key))
        } else {
            None
        }
    }

    /// Remove a pair by key. Returns the removed value, if present.
    pub fn remove_by_key<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: Comparable<K> + ?Sized,
    {
        if let Some(value) = self.forward.remove(key) {
            self.backward.remove(&value);
            Some(value)
        } else {
            None
        }
    }

    /// Remove a pair by value. Returns the removed key, if present.
    pub fn remove_by_value<Q>(&mut self, value: &Q) -> Option<K>
    where
        Q: Comparable<V> + ?Sized,
    {
        if let Some(key) = self.backward.remove(value) {
            self.forward.remove(&key);
            Some(key)
        } else {
            None
        }
    }

    /// Return the union of two bimaps; entries from `other` overwrite entries in `self`.
    ///
    /// For conflicting keys or values, `other`'s mapping wins. The bijection
    /// invariant is maintained by the underlying [`insert`][Self::insert] logic.
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
    #[must_use]
    pub fn symmetric_difference(self, other: &Self) -> Self {
        // Clone self — O(1) via structural sharing — to check membership after consuming.
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

impl<K: Ord, V: Ord, P: SharedPointerKind> Default for GenericOrdBiMap<K, V, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord, V: Ord, P: SharedPointerKind> PartialEq for GenericOrdBiMap<K, V, P> {
    fn eq(&self, other: &Self) -> bool {
        self.forward == other.forward
    }
}

impl<K: Ord, V: Ord, P: SharedPointerKind> Eq for GenericOrdBiMap<K, V, P> {}

impl<K: Ord, V: Ord, P: SharedPointerKind> PartialOrd for GenericOrdBiMap<K, V, P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord, V: Ord, P: SharedPointerKind> Ord for GenericOrdBiMap<K, V, P> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare by sorted forward-direction iteration (canonical order).
        self.forward.iter().cmp(other.forward.iter())
    }
}

impl<K: Ord + Hash, V: Ord + Hash, P: SharedPointerKind> Hash for GenericOrdBiMap<K, V, P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Sequential hashing is valid: forward iteration is always in sorted key order,
        // so two equal bimaps hash identically without an order-independent combiner.
        self.len().hash(state);
        for (k, v) in self.iter() {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl<K: Ord + Clone + Debug, V: Ord + Clone + Debug, P: SharedPointerKind> Debug
    for GenericOrdBiMap<K, V, P>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, v) in self.iter() {
            d.entry(k, v);
        }
        d.finish()
    }
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> FromIterator<(K, V)>
    for GenericOrdBiMap<K, V, P>
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut bm = Self::new();
        for (k, v) in iter {
            bm.insert(k, v);
        }
        bm
    }
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> From<Vec<(K, V)>>
    for GenericOrdBiMap<K, V, P>
{
    fn from(v: Vec<(K, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K: Ord + Clone, V: Ord + Clone, const N: usize, P: SharedPointerKind> From<[(K, V); N]>
    for GenericOrdBiMap<K, V, P>
{
    fn from(arr: [(K, V); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> From<&'a [(K, V)]>
    for GenericOrdBiMap<K, V, P>
{
    fn from(slice: &'a [(K, V)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> From<&'a Vec<(K, V)>>
    for GenericOrdBiMap<K, V, P>
{
    fn from(v: &'a Vec<(K, V)>) -> Self {
        v.iter().cloned().collect()
    }
}

/// Index by key (forward direction), returning the mapped value.
///
/// Panics if the key is not present. `IndexMut` is not implemented because
/// mutating a value via a mutable reference would silently invalidate the reverse
/// lookup (`value → key`) stored in the backward map.
impl<Q, K: Ord, V: Ord, P: SharedPointerKind> Index<&Q> for GenericOrdBiMap<K, V, P>
where
    Q: Comparable<K> + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        match self.forward.get(key) {
            Some(v) => v,
            None => panic!("OrdBiMap::index: key not found"),
        }
    }
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> Extend<(K, V)>
    for GenericOrdBiMap<K, V, P>
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

/// Consuming iterator over the pairs of a [`GenericOrdBiMap`] in sorted key order.
pub struct ConsumingIter<K, V, P: SharedPointerKind> {
    inner: MapConsumingIter<K, V, P>,
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> Iterator for ConsumingIter<K, V, P> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> ExactSizeIterator
    for ConsumingIter<K, V, P>
{
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> IntoIterator
    for GenericOrdBiMap<K, V, P>
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<K, V, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.forward.into_iter(),
        }
    }
}

impl<'a, K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> IntoIterator
    for &'a GenericOrdBiMap<K, V, P>
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

    assert_impl_all!(OrdBiMap<i32, i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let bm: OrdBiMap<&str, i32> = OrdBiMap::new();
        assert!(bm.is_empty());
        assert_eq!(bm.len(), 0);
    }

    #[test]
    fn insert_and_lookup() {
        let mut bm = OrdBiMap::new();
        bm.insert("alice", 1);
        bm.insert("bob", 2);

        assert_eq!(bm.get_by_key(&"alice"), Some(&1));
        assert_eq!(bm.get_by_value(&1), Some(&"alice"));
        assert_eq!(bm.get_by_key(&"bob"), Some(&2));
        assert_eq!(bm.get_by_value(&2), Some(&"bob"));
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn insert_returns_displaced() {
        let mut bm = OrdBiMap::new();
        let r1 = bm.insert("alice", 1);
        assert_eq!(r1, None);

        // Overwrite key.
        let r2 = bm.insert("alice", 2);
        assert_eq!(r2, Some((Some(1), None)));
        assert_eq!(bm.get_by_key(&"alice"), Some(&2));
        assert_eq!(bm.get_by_value(&1), None);
    }

    #[test]
    fn insert_removes_conflicting_value() {
        let mut bm = OrdBiMap::new();
        bm.insert("alice", 1);
        bm.insert("bob", 1); // Steals value 1 from alice.

        assert_eq!(bm.get_by_key(&"alice"), None);
        assert_eq!(bm.get_by_key(&"bob"), Some(&1));
        assert_eq!(bm.get_by_value(&1), Some(&"bob"));
        assert_eq!(bm.len(), 1);
    }

    #[test]
    fn contains_key_and_value() {
        let mut bm = OrdBiMap::new();
        bm.insert("a", 1);

        assert!(bm.contains_key(&"a"));
        assert!(bm.contains_value(&1));
        assert!(!bm.contains_key(&"b"));
        assert!(!bm.contains_value(&2));
    }

    #[test]
    fn remove_by_key() {
        let mut bm = OrdBiMap::new();
        bm.insert("alice", 1);

        let removed = bm.remove_by_key(&"alice");
        assert_eq!(removed, Some(1));
        assert!(bm.is_empty());
    }

    #[test]
    fn remove_by_value() {
        let mut bm = OrdBiMap::new();
        bm.insert("alice", 1);

        let removed = bm.remove_by_value(&1);
        assert_eq!(removed, Some("alice"));
        assert!(bm.is_empty());
    }

    #[test]
    fn remove_absent_returns_none() {
        let mut bm: OrdBiMap<&str, i32> = OrdBiMap::new();
        assert_eq!(bm.remove_by_key(&"x"), None);
        assert_eq!(bm.remove_by_value(&99), None);
    }

    #[test]
    fn iter_is_sorted() {
        let mut bm = OrdBiMap::new();
        bm.insert("charlie", 3);
        bm.insert("alice", 1);
        bm.insert("bob", 2);

        let pairs: Vec<_> = bm.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    }

    #[test]
    fn keys_are_sorted() {
        let mut bm = OrdBiMap::new();
        bm.insert(3, 30);
        bm.insert(1, 10);
        bm.insert(2, 20);

        let keys: Vec<_> = bm.keys().copied().collect();
        assert_eq!(keys, vec![1, 2, 3]);
    }

    #[test]
    fn values_in_key_order() {
        let mut bm = OrdBiMap::new();
        bm.insert(3, 30);
        bm.insert(1, 10);
        bm.insert(2, 20);

        let values: Vec<_> = bm.values().copied().collect();
        assert_eq!(values, vec![10, 20, 30]);
    }

    #[test]
    fn equality() {
        let mut bm1 = OrdBiMap::new();
        bm1.insert("a", 1);

        let mut bm2 = OrdBiMap::new();
        bm2.insert("a", 1);

        assert_eq!(bm1, bm2);
    }

    #[test]
    fn ord_comparison() {
        let mut bm1: OrdBiMap<i32, i32> = OrdBiMap::new();
        bm1.insert(1, 10);

        let mut bm2: OrdBiMap<i32, i32> = OrdBiMap::new();
        bm2.insert(2, 20);

        assert!(bm1 < bm2);
    }

    #[test]
    fn hash_same_for_equal_maps() {
        use std::hash::DefaultHasher;

        let mut bm1 = OrdBiMap::new();
        bm1.insert(1i32, 10i32);
        bm1.insert(2, 20);

        let mut bm2 = OrdBiMap::new();
        bm2.insert(2i32, 20i32);
        bm2.insert(1, 10);

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        bm1.hash(&mut h1);
        bm2.hash(&mut h2);
        assert_eq!(bm1, bm2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn clone_shares_structure() {
        let mut bm: OrdBiMap<i32, i32> = OrdBiMap::new();
        bm.insert(1, 10);
        let clone = bm.clone();
        assert_eq!(bm, clone);
    }

    #[test]
    fn default_is_empty() {
        let bm: OrdBiMap<i32, i32> = OrdBiMap::default();
        assert!(bm.is_empty());
    }

    #[test]
    fn from_vec() {
        let bm: OrdBiMap<&str, i32> = OrdBiMap::from(vec![("a", 1), ("b", 2)]);
        assert_eq!(bm.len(), 2);
        assert_eq!(bm.get_by_key(&"a"), Some(&1));
    }

    #[test]
    fn from_array() {
        let bm: OrdBiMap<i32, i32> = OrdBiMap::from([(1, 10), (2, 20)]);
        assert_eq!(bm.len(), 2);
        assert_eq!(bm.get_by_key(&2), Some(&20));
    }

    #[test]
    fn from_slice() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let bm: OrdBiMap<i32, i32> = OrdBiMap::from(v.as_slice());
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let bm: OrdBiMap<i32, i32> = OrdBiMap::from(&v);
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn from_iterator() {
        let pairs = vec![("a", 1), ("b", 2), ("c", 3)];
        let bm: OrdBiMap<&str, i32> = pairs.into_iter().collect();
        assert_eq!(bm.len(), 3);
        assert_eq!(bm.get_by_key(&"c"), Some(&3));
    }

    #[test]
    fn extend_adds_pairs() {
        let mut bm: OrdBiMap<i32, i32> = OrdBiMap::new();
        bm.extend([(1, 10), (2, 20)]);
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn into_iter_owned_sorted() {
        let mut bm = OrdBiMap::new();
        bm.insert(3i32, 30i32);
        bm.insert(1, 10);
        bm.insert(2, 20);

        let pairs: Vec<_> = bm.into_iter().collect();
        assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    fn into_iter_ref() {
        let mut bm = OrdBiMap::new();
        bm.insert(1i32, 10i32);

        let pairs: Vec<_> = (&bm).into_iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![(1, 10)]);
    }

    #[test]
    fn union_merges_pairs() {
        let mut bm1 = OrdBiMap::new();
        bm1.insert(1i32, 10i32);

        let mut bm2 = OrdBiMap::new();
        bm2.insert(2i32, 20i32);

        let combined = bm1.union(bm2);
        assert_eq!(combined.len(), 2);
        assert_eq!(combined.get_by_key(&1), Some(&10));
        assert_eq!(combined.get_by_key(&2), Some(&20));
    }

    #[test]
    fn difference_by_key() {
        let mut bm1 = OrdBiMap::new();
        bm1.insert(1i32, 10i32);
        bm1.insert(2, 20);

        let mut bm2 = OrdBiMap::new();
        bm2.insert(2i32, 20i32);

        let diff = bm1.difference(&bm2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.get_by_key(&1), Some(&10));
    }

    #[test]
    fn intersection_by_key() {
        let mut bm1 = OrdBiMap::new();
        bm1.insert(1i32, 10i32);
        bm1.insert(2, 20);

        let mut bm2 = OrdBiMap::new();
        bm2.insert(2i32, 20i32);
        bm2.insert(3, 30);

        let inter = bm1.intersection(&bm2);
        assert_eq!(inter.len(), 1);
        assert_eq!(inter.get_by_key(&2), Some(&20));
    }

    #[test]
    fn symmetric_difference_by_key() {
        let mut bm1 = OrdBiMap::new();
        bm1.insert(1i32, 10i32);
        bm1.insert(2, 20);

        let mut bm2 = OrdBiMap::new();
        bm2.insert(2i32, 20i32);
        bm2.insert(3, 30);

        let sd = bm1.symmetric_difference(&bm2);
        assert_eq!(sd.len(), 2);
        assert!(sd.contains_key(&1));
        assert!(sd.contains_key(&3));
        assert!(!sd.contains_key(&2));
    }

    #[test]
    fn debug_format() {
        let mut bm = OrdBiMap::new();
        bm.insert(1i32, 10i32);
        let s = format!("{:?}", bm);
        assert!(s.contains("1"));
        assert!(s.contains("10"));
    }

    #[test]
    fn index_returns_value() {
        let mut bm = OrdBiMap::new();
        bm.insert(1i32, 10i32);
        assert_eq!(bm[&1i32], 10);
    }

    #[test]
    #[should_panic]
    fn index_panics_on_missing() {
        let bm: OrdBiMap<i32, i32> = OrdBiMap::new();
        let _ = bm[&99i32];
    }
}
