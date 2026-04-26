// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent sorted multimap (key to sorted set of values).
//!
//! An [`OrdMultiMap`] maps each key to an [`OrdSet<V>`][crate::OrdSet] of
//! distinct values, backed by an [`OrdMap<K, OrdSet<V>>`][crate::OrdMap].
//! All operations are O(log n) with structural sharing. Keys and values
//! require only `Ord` — no `Hash + Eq` constraint. Iteration is always in
//! sorted key order, and within each key the values are sorted too.
//!
//! Prefer [`OrdMultiMap`] over [`HashMultiMap`][crate::HashMultiMap] when:
//! - You need sorted iteration without a separate sort step.
//! - Keys or values implement `Ord` but not `Hash + Eq`.
//! - You want `PartialOrd` / `Ord` on the multimap itself.
//! - You want range queries over keys or per-key value sets.
//!
//! # Examples
//!
//! ```
//! use pds::OrdMultiMap;
//!
//! let mut mm = OrdMultiMap::new();
//! mm.insert("fruit", "apple");
//! mm.insert("fruit", "banana");
//! mm.insert("veggie", "carrot");
//!
//! assert_eq!(mm.key_count(&"fruit"), 2);
//! assert!(mm.contains(&"fruit", &"apple"));
//!
//! // Iteration is always in sorted (key, value) order.
//! let pairs: Vec<_> = mm.iter().collect();
//! assert_eq!(pairs, vec![
//!     (&"fruit",  &"apple"),
//!     (&"fruit",  &"banana"),
//!     (&"veggie", &"carrot"),
//! ]);
//! ```

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;
use core::ops::Index;

use archery::SharedPointerKind;
use equivalent::Comparable;

use crate::ordmap::{ConsumingIter as MapConsumingIter, GenericOrdMap};
use crate::ordset::{ConsumingIter as SetConsumingIter, GenericOrdSet};
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericOrdMultiMap`] with the default pointer type.
pub type OrdMultiMap<K, V> = GenericOrdMultiMap<K, V, DefaultSharedPtr>;

/// A persistent sorted multimap backed by [`GenericOrdMap`] and [`GenericOrdSet`].
///
/// Each key maps to a sorted set of distinct values. Clone is O(1) via
/// structural sharing. Iteration is always in ascending key order; within
/// each key, values are yielded in ascending value order.
pub struct GenericOrdMultiMap<K, V, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) map: GenericOrdMap<K, GenericOrdSet<V, P>, P>,
    total: usize,
}

// Manual Clone to avoid spurious `P: Clone` bound from derive.
impl<K: Clone, V: Clone, P: SharedPointerKind> Clone for GenericOrdMultiMap<K, V, P> {
    fn clone(&self) -> Self {
        GenericOrdMultiMap {
            map: self.map.clone(),
            total: self.total,
        }
    }
}

impl<K, V, P: SharedPointerKind> GenericOrdMultiMap<K, V, P> {
    /// Create an empty multimap.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdMultiMap {
            map: GenericOrdMap::default(),
            total: 0,
        }
    }

    /// Test whether the multimap is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Return the number of distinct keys.
    #[must_use]
    pub fn keys_len(&self) -> usize {
        self.map.len()
    }

    /// Return the total number of (key, value) pairs across all keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.total
    }

    /// Test whether two multimaps share the same underlying allocation.
    ///
    /// Returns `true` if `self` and `other` are the same version of
    /// the multimap — i.e. one is a clone of the other with no
    /// intervening mutations. This is a cheap pointer comparison, not
    /// a structural equality check.
    ///
    /// Time: O(1)
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.map.ptr_eq(&other.map)
    }
}

impl<K, V, P> GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    /// Insert a key-value pair.
    ///
    /// If the value already exists for this key, no change is made (value sets
    /// do not hold duplicates). Returns `true` if the value was newly inserted.
    pub fn insert(&mut self, key: K, value: V) -> bool {
        let set = self.map.entry(key).or_default();
        let prev_len = set.len();
        set.insert(value);
        let inserted = set.len() > prev_len;
        if inserted {
            self.total += 1;
        }
        inserted
    }

    /// Remove a single value for a key.
    ///
    /// Returns `true` if the pair was present and removed. If the key's value
    /// set becomes empty after removal, the key is removed entirely.
    pub fn remove<QK, QV>(&mut self, key: &QK, value: &QV) -> bool
    where
        QK: Comparable<K> + ?Sized,
        QV: Comparable<V> + ?Sized,
    {
        let should_remove_key;
        let removed;
        {
            let Some(set) = self.map.get_mut(key) else {
                return false;
            };
            removed = set.remove(value).is_some();
            should_remove_key = set.is_empty();
        }
        if removed {
            self.total -= 1;
            if should_remove_key {
                self.map.remove(key);
            }
        }
        removed
    }

    /// Remove all values for a key, returning the removed value set.
    ///
    /// Returns an empty set if the key is not present.
    pub fn remove_all<Q>(&mut self, key: &Q) -> GenericOrdSet<V, P>
    where
        Q: Comparable<K> + ?Sized,
    {
        match self.map.remove_with_key(key) {
            Some((_, set)) => {
                self.total -= set.len();
                set
            }
            None => GenericOrdSet::default(),
        }
    }

    /// Get the sorted value set for a key.
    ///
    /// Returns an empty set if the key is not present.
    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> GenericOrdSet<V, P>
    where
        Q: Comparable<K> + ?Sized,
    {
        self.map.get(key).cloned().unwrap_or_default()
    }

    /// Test whether a specific key-value pair is present.
    #[must_use]
    pub fn contains<QK, QV>(&self, key: &QK, value: &QV) -> bool
    where
        QK: Comparable<K> + ?Sized,
        QV: Comparable<V> + ?Sized,
    {
        self.map.get(key).is_some_and(|set| set.contains(value))
    }

    /// Test whether a key is present (has at least one value).
    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        self.map.contains_key(key)
    }

    /// Return the number of values for a key.
    #[must_use]
    pub fn key_count<Q>(&self, key: &Q) -> usize
    where
        Q: Comparable<K> + ?Sized,
    {
        self.map.get(key).map_or(0, GenericOrdSet::len)
    }

    /// Iterate over all (key, value) pairs in sorted (key, value) order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        self.map
            .iter()
            .flat_map(|(k, set)| set.iter().map(move |v| (k, v)))
    }

    /// Iterate over keys and their value sets in sorted key order.
    pub fn iter_sets(&self) -> impl Iterator<Item = (&K, &GenericOrdSet<V, P>)> + '_ {
        self.map.iter()
    }

    /// Iterate over all distinct keys in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &K> + '_ {
        self.map.iter().map(|(k, _)| k)
    }

    /// Return the multiset union of two multimaps.
    ///
    /// For a key present in both, the result's value set is the union of both
    /// value sets. For a key in only one map, all its values are kept.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut result = self.clone();
        for (k, other_set) in other.map.iter() {
            let entry = result.map.entry(k.clone()).or_default();
            let prev_len = entry.len();
            for v in other_set.iter() {
                entry.insert(v.clone());
            }
            result.total += entry.len() - prev_len;
        }
        result
    }

    /// Return entries whose keys are in `self` but not in `other`.
    ///
    /// For keys present only in `self`, all values are kept.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        let mut result = Self::new();
        for (k, set) in self.map.iter() {
            if !other.contains_key(k) {
                result.map.insert(k.clone(), set.clone());
                result.total += set.len();
            }
        }
        result
    }

    /// Return entries whose keys are in both `self` and `other`; `self`'s value sets are kept.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut result = Self::new();
        for (k, set) in self.map.iter() {
            if other.contains_key(k) {
                result.map.insert(k.clone(), set.clone());
                result.total += set.len();
            }
        }
        result
    }

    /// Return entries whose keys are in exactly one of `self` or `other`.
    ///
    /// Keys present in both maps (regardless of their value sets) are excluded.
    #[must_use]
    pub fn symmetric_difference(&self, other: &Self) -> Self {
        let mut result = Self::new();
        for (k, set) in self.map.iter() {
            if !other.contains_key(k) {
                result.map.insert(k.clone(), set.clone());
                result.total += set.len();
            }
        }
        for (k, set) in other.map.iter() {
            if !self.contains_key(k) {
                result.map.insert(k.clone(), set.clone());
                result.total += set.len();
            }
        }
        result
    }
}

impl<K, V, P> Default for GenericOrdMultiMap<K, V, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericOrdMultiMap {
            map: GenericOrdMap::default(),
            total: 0,
        }
    }
}

impl<K, V, P> PartialEq for GenericOrdMultiMap<K, V, P>
where
    K: Ord,
    V: Ord,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.total == other.total && self.map == other.map
    }
}

impl<K, V, P> Eq for GenericOrdMultiMap<K, V, P>
where
    K: Ord,
    V: Ord,
    P: SharedPointerKind,
{
}

impl<K, V, P> PartialOrd for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K, V, P> Ord for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn cmp(&self, other: &Self) -> Ordering {
        // Lexicographic over (key, value) pairs in canonical sorted order.
        self.iter().cmp(other.iter())
    }
}

impl<K, V, P> Hash for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Hash,
    V: Ord + Hash,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        // Iteration is in canonical sorted (key, value) order — hash sequentially.
        self.total.hash(state);
        for (k, set) in self.map.iter() {
            k.hash(state);
            for v in set.iter() {
                v.hash(state);
            }
        }
    }
}

impl<K, V, P> Debug for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Debug,
    V: Ord + Debug,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, set) in self.map.iter() {
            d.entry(k, set);
        }
        d.finish()
    }
}

impl<K, V, P> FromIterator<(K, V)> for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut mm = Self::new();
        for (k, v) in iter {
            mm.insert(k, v);
        }
        mm
    }
}

impl<K, V, P> Extend<(K, V)> for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<K, V, P> From<Vec<(K, V)>> for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(v: Vec<(K, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K, V, const N: usize, P> From<[(K, V); N]> for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(arr: [(K, V); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, K, V, P> From<&'a [(K, V)]> for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(slice: &'a [(K, V)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, K, V, P> From<&'a Vec<(K, V)>> for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<(K, V)>) -> Self {
        v.iter().cloned().collect()
    }
}

/// Index by key, returning a reference to the stored value set.
///
/// Panics if the key is not present. `IndexMut` is not implemented because
/// mutating the inner set directly would silently corrupt the `total` counter.
impl<Q, K, V, P> Index<&Q> for GenericOrdMultiMap<K, V, P>
where
    Q: Comparable<K> + ?Sized,
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    type Output = GenericOrdSet<V, P>;

    fn index(&self, key: &Q) -> &Self::Output {
        match self.map.get(key) {
            Some(set) => set,
            None => panic!("OrdMultiMap::index: key not found"),
        }
    }
}

/// A consuming iterator over the (key, value) pairs of a [`GenericOrdMultiMap`].
///
/// Yields each `(K, V)` pair in ascending key order; values within each key are
/// in ascending value order.
pub struct ConsumingIter<K, V, P: SharedPointerKind> {
    outer: MapConsumingIter<K, GenericOrdSet<V, P>, P>,
    inner: Option<(K, SetConsumingIter<V, P>)>,
}

impl<K, V, P> Iterator for ConsumingIter<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((ref k, ref mut inner)) = self.inner {
                if let Some(v) = inner.next() {
                    return Some((k.clone(), v));
                }
                self.inner = None;
            }
            let (k, set) = self.outer.next()?;
            self.inner = Some((k, set.into_iter()));
        }
    }
}

impl<K, V, P> IntoIterator for GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
    P: SharedPointerKind,
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<K, V, P>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            outer: self.map.into_iter(),
            inner: None,
        }
    }
}

impl<'a, K, V, P> IntoIterator for &'a GenericOrdMultiMap<K, V, P>
where
    K: Ord + Clone,
    V: Ord + Clone,
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

    assert_impl_all!(OrdMultiMap<i32, i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let mm: OrdMultiMap<&str, i32> = OrdMultiMap::new();
        assert!(mm.is_empty());
        assert_eq!(mm.len(), 0);
        assert_eq!(mm.keys_len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        mm.insert("a", 2);
        mm.insert("b", 3);

        let a_vals = mm.get("a");
        assert_eq!(a_vals.len(), 2);
        assert!(a_vals.contains(&1));
        assert!(a_vals.contains(&2));
        assert_eq!(mm.len(), 3);
        assert_eq!(mm.keys_len(), 2);
    }

    #[test]
    fn insert_duplicate_value() {
        let mut mm = OrdMultiMap::new();
        assert!(mm.insert("a", 1));
        assert!(!mm.insert("a", 1));
        assert_eq!(mm.len(), 1);
        assert_eq!(mm.key_count("a"), 1);
    }

    #[test]
    fn remove_single_value() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        mm.insert("a", 2);
        assert!(mm.remove("a", &1));
        assert_eq!(mm.key_count("a"), 1);
        assert!(!mm.contains("a", &1));
        assert!(mm.contains("a", &2));
        assert_eq!(mm.len(), 1);
    }

    #[test]
    fn remove_last_value_removes_key() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        mm.remove("a", &1);
        assert!(!mm.contains_key("a"));
        assert!(mm.is_empty());
    }

    #[test]
    fn remove_absent() {
        let mut mm: OrdMultiMap<&str, i32> = OrdMultiMap::new();
        assert!(!mm.remove("a", &1));
    }

    #[test]
    fn remove_all() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        mm.insert("a", 2);
        mm.insert("b", 3);
        let removed = mm.remove_all("a");
        assert_eq!(removed.len(), 2);
        assert!(!mm.contains_key("a"));
        assert_eq!(mm.len(), 1);
    }

    #[test]
    fn remove_all_absent() {
        let mut mm: OrdMultiMap<&str, i32> = OrdMultiMap::new();
        let removed = mm.remove_all("a");
        assert!(removed.is_empty());
    }

    #[test]
    fn contains() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        assert!(mm.contains("a", &1));
        assert!(!mm.contains("a", &2));
        assert!(!mm.contains("b", &1));
    }

    #[test]
    fn contains_key() {
        let mut mm = OrdMultiMap::new();
        assert!(!mm.contains_key("a"));
        mm.insert("a", 1);
        assert!(mm.contains_key("a"));
    }

    #[test]
    fn iter_is_sorted() {
        let mut mm = OrdMultiMap::new();
        mm.insert("b", 2);
        mm.insert("a", 3);
        mm.insert("a", 1);
        mm.insert("b", 1);
        let pairs: Vec<_> = mm.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(pairs, vec![("a", 1), ("a", 3), ("b", 1), ("b", 2)]);
    }

    #[test]
    fn iter_sets() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        mm.insert("a", 2);
        mm.insert("b", 3);
        let sets: Vec<_> = mm.iter_sets().map(|(&k, s)| (k, s.len())).collect();
        assert_eq!(sets, vec![("a", 2), ("b", 1)]);
    }

    #[test]
    fn keys() {
        let mut mm = OrdMultiMap::new();
        mm.insert("b", 1);
        mm.insert("a", 1);
        let keys: Vec<_> = mm.keys().copied().collect();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn union_merges_value_sets() {
        let mut a = OrdMultiMap::new();
        a.insert(1, "x");
        let mut b = OrdMultiMap::new();
        b.insert(1, "y");
        b.insert(2, "z");
        let c = a.union(&b);
        assert_eq!(c.key_count(&1), 2);
        assert!(c.contains(&1, &"x") && c.contains(&1, &"y"));
        assert_eq!(c.key_count(&2), 1);
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn difference_by_key() {
        let mut a = OrdMultiMap::new();
        a.insert(1, "x");
        a.insert(2, "y");
        let mut b = OrdMultiMap::new();
        b.insert(2, "z");
        let c = a.difference(&b);
        assert!(c.contains_key(&1));
        assert!(!c.contains_key(&2));
    }

    #[test]
    fn intersection_by_key() {
        let mut a = OrdMultiMap::new();
        a.insert(1, "x");
        a.insert(2, "y");
        let mut b = OrdMultiMap::new();
        b.insert(2, "z");
        b.insert(3, "w");
        let c = a.intersection(&b);
        assert!(!c.contains_key(&1));
        assert!(c.contains_key(&2));
        assert!(c.contains(&2, &"y")); // self's values kept
    }

    #[test]
    fn symmetric_difference_by_key() {
        let mut a = OrdMultiMap::new();
        a.insert(1, "x");
        a.insert(2, "y");
        a.insert(3, "z");
        let mut b = OrdMultiMap::new();
        b.insert(2, "w");
        b.insert(4, "v");
        let c = a.symmetric_difference(&b);
        assert!(c.contains_key(&1));
        assert!(!c.contains_key(&2)); // in both — excluded
        assert!(c.contains_key(&3));
        assert!(c.contains_key(&4));
        assert_eq!(c.keys_len(), 3);
    }

    #[test]
    fn ord_comparison() {
        let a: OrdMultiMap<i32, i32> = vec![(1, 10), (2, 20)].into_iter().collect();
        let b: OrdMultiMap<i32, i32> = vec![(1, 10), (2, 30)].into_iter().collect();
        assert!(a < b);
    }

    #[test]
    fn equality() {
        let mut a = OrdMultiMap::new();
        a.insert("x", 1);
        a.insert("x", 2);
        let mut b = OrdMultiMap::new();
        b.insert("x", 2);
        b.insert("x", 1);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_same_for_equal_maps() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(m: &OrdMultiMap<i32, i32>) -> u64 {
            let mut h = DefaultHasher::new();
            m.hash(&mut h);
            h.finish()
        }
        let mut a = OrdMultiMap::new();
        a.insert(1, 10);
        a.insert(2, 20);
        let mut b = OrdMultiMap::new();
        b.insert(2, 20);
        b.insert(1, 10);
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn from_iterator() {
        let mm: OrdMultiMap<&str, i32> = vec![("a", 1), ("a", 2), ("b", 3)].into_iter().collect();
        assert_eq!(mm.len(), 3);
        assert_eq!(mm.keys_len(), 2);
        assert_eq!(mm.key_count("a"), 2);
    }

    #[test]
    fn clone_shares_structure() {
        let mut mm = OrdMultiMap::new();
        mm.insert("a", 1);
        let mm2 = mm.clone();
        assert_eq!(mm, mm2);
    }

    #[test]
    fn into_iter_owned_sorted() {
        let mut mm = OrdMultiMap::new();
        mm.insert(2, "b");
        mm.insert(1, "a");
        mm.insert(1, "c");
        let pairs: Vec<_> = mm.into_iter().collect();
        assert_eq!(pairs, vec![(1, "a"), (1, "c"), (2, "b")]);
    }

    #[test]
    fn into_iter_ref() {
        let mut mm = OrdMultiMap::new();
        mm.insert(1, "a");
        mm.insert(2, "b");
        let pairs: Vec<_> = (&mm).into_iter().collect();
        assert_eq!(pairs, vec![(&1, &"a"), (&2, &"b")]);
    }

    #[test]
    fn debug_format() {
        let mut mm = OrdMultiMap::new();
        mm.insert(1i32, 10i32);
        let s = format!("{:?}", mm);
        assert!(!s.is_empty());
    }

    #[test]
    fn default_is_empty() {
        let mm: OrdMultiMap<i32, i32> = OrdMultiMap::default();
        assert!(mm.is_empty());
    }

    #[test]
    fn extend_adds_pairs() {
        let mut mm: OrdMultiMap<i32, i32> = OrdMultiMap::new();
        mm.extend(vec![(1, 10), (1, 11), (2, 20)]);
        assert_eq!(mm.len(), 3);
        assert_eq!(mm.key_count(&1), 2);
    }

    #[test]
    fn from_vec() {
        let mm: OrdMultiMap<i32, i32> = vec![(1, 10), (1, 11), (2, 20)].into();
        assert_eq!(mm.len(), 3);
    }

    #[test]
    fn from_array() {
        let mm: OrdMultiMap<i32, i32> = [(1, 10), (2, 20)].into();
        assert_eq!(mm.len(), 2);
    }

    #[test]
    fn from_slice() {
        let mm: OrdMultiMap<i32, i32> = [(1, 10), (2, 20)][..].into();
        assert_eq!(mm.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let mm: OrdMultiMap<i32, i32> = OrdMultiMap::from(&v);
        assert_eq!(mm.len(), 2);
    }

    #[test]
    fn index_returns_set() {
        let mut mm = OrdMultiMap::new();
        mm.insert(1i32, 10i32);
        mm.insert(1, 11);
        let set = &mm[&1];
        assert_eq!(set.len(), 2);
        assert!(set.contains(&10));
    }

    #[test]
    #[should_panic(expected = "key not found")]
    fn index_panics_on_missing() {
        let mm: OrdMultiMap<i32, i32> = OrdMultiMap::new();
        let _ = &mm[&99];
    }

    #[test]
    fn get_absent_key() {
        let mm: OrdMultiMap<&str, i32> = OrdMultiMap::new();
        assert!(mm.get("a").is_empty());
    }
}
