// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent multimap (key to set of values).
//!
//! A `HashMultiMap` maps each key to a [`HashSet`][crate::HashSet] of
//! values, backed by a [`HashMap<K, HashSet<V>>`][crate::HashMap].
//! All operations are O(log n) with structural sharing.
//!
//! # Examples
//!
//! ```
//! use imbl::HashMultiMap;
//!
//! let mut mm = HashMultiMap::new();
//! mm.insert("fruit", "apple");
//! mm.insert("fruit", "banana");
//! mm.insert("veggie", "carrot");
//!
//! assert_eq!(mm.get("fruit").len(), 2);
//! assert!(mm.contains("fruit", &"apple"));
//! assert!(!mm.contains("fruit", &"pear"));
//! ```

#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash};
use core::iter::FromIterator;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hashmap::GenericHashMap;
use crate::hashset::GenericHashSet;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericHashMultiMap`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type HashMultiMap<K, V> = GenericHashMultiMap<K, V, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericHashMultiMap`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type HashMultiMap<K, V> =
    GenericHashMultiMap<K, V, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent multimap backed by [`GenericHashMap`] and [`GenericHashSet`].
///
/// Each key maps to a set of values. Clone is O(1) via structural sharing.
pub struct GenericHashMultiMap<
    K,
    V,
    S,
    P: SharedPointerKind = DefaultSharedPtr,
> {
    map: GenericHashMap<K, GenericHashSet<V, S, P>, S, P>,
    total: usize,
}

// Manual Clone — avoid derive's spurious `P: Clone` bound.
impl<K: Clone, V: Clone, S: Clone, P: SharedPointerKind> Clone
    for GenericHashMultiMap<K, V, S, P>
{
    fn clone(&self) -> Self {
        GenericHashMultiMap {
            map: self.map.clone(),
            total: self.total,
        }
    }
}

#[cfg(feature = "std")]
impl<K, V, P> GenericHashMultiMap<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty multimap.
    #[must_use]
    pub fn new() -> Self {
        GenericHashMultiMap {
            map: GenericHashMap::new(),
            total: 0,
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> GenericHashMultiMap<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty multimap (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericHashMultiMap {
            map: GenericHashMap::new(),
            total: 0,
        }
    }
}

impl<K, V, S, P> GenericHashMultiMap<K, V, S, P>
where
    P: SharedPointerKind,
{
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

    /// Return the total number of key-value pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.total
    }
}

impl<K, V, S, P> GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    /// Insert a key-value pair. If the value already exists for this
    /// key, no change is made (sets do not have duplicates).
    ///
    /// Returns `true` if the value was newly inserted.
    pub fn insert(&mut self, key: K, value: V) -> bool {
        let set = self
            .map
            .entry(key)
            .or_default();
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
    /// Returns `true` if the value was present and removed. If the
    /// key's set becomes empty, the key is removed entirely.
    pub fn remove<QK, QV>(&mut self, key: &QK, value: &QV) -> bool
    where
        QK: Hash + Equivalent<K> + ?Sized,
        QV: Hash + Equivalent<V> + ?Sized,
    {
        let should_remove_key;
        let removed = if let Some(set) = self.map.get_mut(key) {
            let prev_len = set.len();
            set.remove(value);
            should_remove_key = set.is_empty();
            set.len() < prev_len
        } else {
            return false;
        };
        if removed {
            self.total -= 1;
            if should_remove_key {
                self.map.remove(key);
            }
        }
        removed
    }

    /// Remove all values for a key, returning the set of removed values.
    pub fn remove_all<Q>(&mut self, key: &Q) -> GenericHashSet<V, S, P>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        match self.map.remove_with_key(key) {
            Some((_, set)) => {
                self.total -= set.len();
                set
            }
            None => GenericHashSet::default(),
        }
    }

    /// Get the set of values for a key.
    ///
    /// Returns an empty set if the key is not present.
    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> GenericHashSet<V, S, P>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.map
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Test whether a specific key-value pair is present.
    #[must_use]
    pub fn contains<QK, QV>(&self, key: &QK, value: &QV) -> bool
    where
        QK: Hash + Equivalent<K> + ?Sized,
        QV: Hash + Equivalent<V> + ?Sized,
    {
        self.map
            .get(key)
            .is_some_and(|set| set.contains(value))
    }

    /// Test whether a key is present (has at least one value).
    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.map.contains_key(key)
    }

    /// Return the number of values for a key.
    #[must_use]
    pub fn key_count<Q>(&self, key: &Q) -> usize
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.map.get(key).map_or(0, GenericHashSet::len)
    }

    /// Iterate over all key-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map
            .iter()
            .flat_map(|(k, set)| set.iter().map(move |v| (k, v)))
    }

    /// Iterate over keys and their value sets.
    pub fn iter_sets(&self) -> impl Iterator<Item = (&K, &GenericHashSet<V, S, P>)> {
        self.map.iter()
    }

    /// Iterate over all distinct keys.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }
}

#[cfg(feature = "std")]
impl<K, V, P> Default for GenericHashMultiMap<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> Default for GenericHashMultiMap<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S, P> PartialEq for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq,
    V: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.total == other.total && self.map == other.map
    }
}

impl<K, V, S, P> Eq for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq,
    V: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<K, V, S, P> Debug for GenericHashMultiMap<K, V, S, P>
where
    K: Debug + Hash + Eq + Clone,
    V: Debug + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, set) in self.iter_sets() {
            d.entry(k, set);
        }
        d.finish()
    }
}

impl<K, V, S, P> FromIterator<(K, V)> for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut mm = GenericHashMultiMap {
            map: GenericHashMap::default(),
            total: 0,
        };
        for (k, v) in iter {
            mm.insert(k, v);
        }
        mm
    }
}

impl<K, V, S, P> Extend<(K, V)> for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn new_is_empty() {
        let mm: HashMultiMap<&str, i32> = HashMultiMap::new();
        assert!(mm.is_empty());
        assert_eq!(mm.len(), 0);
        assert_eq!(mm.keys_len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut mm = HashMultiMap::new();
        mm.insert("a", 1);
        mm.insert("a", 2);
        mm.insert("b", 3);

        let a_vals = mm.get("a");
        assert_eq!(a_vals.len(), 2);
        assert!(a_vals.contains(&1));
        assert!(a_vals.contains(&2));

        let b_vals = mm.get("b");
        assert_eq!(b_vals.len(), 1);
        assert!(b_vals.contains(&3));

        assert_eq!(mm.len(), 3);
        assert_eq!(mm.keys_len(), 2);
    }

    #[test]
    fn insert_duplicate_value() {
        let mut mm = HashMultiMap::new();
        assert!(mm.insert("a", 1));
        assert!(!mm.insert("a", 1));
        assert_eq!(mm.len(), 1);
        assert_eq!(mm.key_count("a"), 1);
    }

    #[test]
    fn remove_single_value() {
        let mut mm = HashMultiMap::new();
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
        let mut mm = HashMultiMap::new();
        mm.insert("a", 1);
        mm.remove("a", &1);
        assert!(!mm.contains_key("a"));
        assert!(mm.is_empty());
    }

    #[test]
    fn remove_absent() {
        let mut mm: HashMultiMap<&str, i32> = HashMultiMap::new();
        assert!(!mm.remove("a", &1));
    }

    #[test]
    fn remove_all() {
        let mut mm = HashMultiMap::new();
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
        let mut mm: HashMultiMap<&str, i32> = HashMultiMap::new();
        let removed = mm.remove_all("a");
        assert!(removed.is_empty());
    }

    #[test]
    fn contains() {
        let mut mm = HashMultiMap::new();
        mm.insert("a", 1);
        assert!(mm.contains("a", &1));
        assert!(!mm.contains("a", &2));
        assert!(!mm.contains("b", &1));
    }

    #[test]
    fn contains_key() {
        let mut mm = HashMultiMap::new();
        assert!(!mm.contains_key("a"));
        mm.insert("a", 1);
        assert!(mm.contains_key("a"));
    }

    #[test]
    fn key_count() {
        let mut mm = HashMultiMap::new();
        assert_eq!(mm.key_count("a"), 0);
        mm.insert("a", 1);
        mm.insert("a", 2);
        assert_eq!(mm.key_count("a"), 2);
    }

    #[test]
    fn get_absent_key() {
        let mm: HashMultiMap<&str, i32> = HashMultiMap::new();
        assert!(mm.get("a").is_empty());
    }

    #[test]
    fn from_iterator() {
        let mm: HashMultiMap<&str, i32> =
            vec![("a", 1), ("a", 2), ("b", 3)].into_iter().collect();
        assert_eq!(mm.len(), 3);
        assert_eq!(mm.keys_len(), 2);
        assert_eq!(mm.key_count("a"), 2);
    }

    #[test]
    fn clone_shares_structure() {
        let mut mm = HashMultiMap::new();
        mm.insert("a", 1);
        mm.insert("a", 2);
        let mm2 = mm.clone();
        assert_eq!(mm, mm2);
    }

    #[test]
    fn equality() {
        let mut a = HashMultiMap::new();
        a.insert("x", 1);
        a.insert("x", 2);

        let mut b = HashMultiMap::new();
        b.insert("x", 2);
        b.insert("x", 1);

        assert_eq!(a, b);
    }

    #[test]
    fn inequality() {
        let mut a = HashMultiMap::new();
        a.insert("x", 1);

        let mut b = HashMultiMap::new();
        b.insert("x", 2);

        assert_ne!(a, b);
    }

    #[test]
    fn iter_all_pairs() {
        let mut mm = HashMultiMap::new();
        mm.insert(1, "a");
        mm.insert(1, "b");
        mm.insert(2, "c");

        let mut pairs: Vec<_> = mm.iter().map(|(&k, &v)| (k, v)).collect();
        pairs.sort();
        assert_eq!(pairs, vec![(1, "a"), (1, "b"), (2, "c")]);
    }
}
