// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent bidirectional map (bijection between two distinct types).
//!
//! A `BiMap<K, V>` maintains a one-to-one mapping between keys and values,
//! backed by two [`HashMap`][crate::HashMap]s (forward: K→V, backward: V→K).
//! Both directions support O(log n) lookup.
//!
//! # Bijection invariant
//!
//! Every key maps to exactly one value, and every value maps to exactly one
//! key. Inserting a pair `(k, v)` will remove any existing mapping for `k`
//! *and* any existing mapping for `v` before establishing the new pair.
//!
//! # Examples
//!
//! ```
//! use pds::BiMap;
//!
//! let mut bm = BiMap::new();
//! bm.insert("alice", 1);
//! bm.insert("bob", 2);
//!
//! assert_eq!(bm.get_by_key(&"alice"), Some(&1));
//! assert_eq!(bm.get_by_value(&2), Some(&"bob"));
//! ```

#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::FromIterator;
use core::ops::Index;

use archery::SharedPointerKind;
use equivalent::Equivalent;

use crate::hash_width::HashWidth;
use crate::hashmap::GenericHashMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericBiMap`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type BiMap<K, V> = GenericBiMap<K, V, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericBiMap`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type BiMap<K, V> = GenericBiMap<K, V, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent bidirectional map backed by two [`GenericHashMap`]s.
///
/// Maintains a bijection: each key maps to exactly one value and vice versa.
/// Clone is O(1) via structural sharing.
pub struct GenericBiMap<
    K,
    V,
    S,
    P: SharedPointerKind = DefaultSharedPtr,
    H: HashWidth = u64,
> {
    pub(crate) forward: GenericHashMap<K, V, S, P, H>,
    pub(crate) backward: GenericHashMap<V, K, S, P, H>,
}

// Manual Clone — avoid derive's spurious `P: Clone` bound.
impl<K: Clone, V: Clone, S: Clone, P: SharedPointerKind, H: HashWidth> Clone
    for GenericBiMap<K, V, S, P, H>
{
    fn clone(&self) -> Self {
        GenericBiMap {
            forward: self.forward.clone(),
            backward: self.backward.clone(),
        }
    }
}

#[cfg(feature = "std")]
impl<K, V, P> GenericBiMap<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty BiMap.
    #[must_use]
    pub fn new() -> Self {
        GenericBiMap {
            forward: GenericHashMap::new(),
            backward: GenericHashMap::new(),
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> GenericBiMap<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty BiMap (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericBiMap {
            forward: GenericHashMap::new(),
            backward: GenericHashMap::new(),
        }
    }
}

impl<K, V, S, P, H: HashWidth> GenericBiMap<K, V, S, P, H>
where
    P: SharedPointerKind,
{
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

impl<K, V, S, P, H: HashWidth> GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    /// Insert a key-value pair, maintaining the bijection invariant.
    ///
    /// If `key` already maps to a value, the old value's backward entry is
    /// removed. If `value` already maps to a key, the old key's forward entry
    /// is removed. Then the new pair is established in both directions.
    ///
    /// Returns `None` if neither `key` nor `value` was previously present.
    /// Returns `Some((old_key, old_value))` if an existing mapping was displaced
    /// (either or both may differ from the inserted pair).
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

    /// Look up a value by its key.
    #[must_use]
    pub fn get_by_key<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.forward.get(key)
    }

    /// Look up a key by its value.
    #[must_use]
    pub fn get_by_value<Q>(&self, value: &Q) -> Option<&K>
    where
        Q: Hash + Equivalent<V> + ?Sized,
    {
        self.backward.get(value)
    }

    /// Test whether a key is present.
    #[must_use]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.forward.contains_key(key)
    }

    /// Test whether a value is present.
    #[must_use]
    pub fn contains_value<Q>(&self, value: &Q) -> bool
    where
        Q: Hash + Equivalent<V> + ?Sized,
    {
        self.backward.contains_key(value)
    }

    /// Remove a pair by key. Returns the removed value, if present.
    pub fn remove_by_key<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
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
        Q: Hash + Equivalent<V> + ?Sized,
    {
        if let Some(key) = self.backward.remove(value) {
            self.forward.remove(&key);
            Some(key)
        } else {
            None
        }
    }

    /// Iterate over all key-value pairs (forward direction).
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.forward.iter()
    }

    /// Iterate over all keys.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.forward.keys()
    }

    /// Iterate over all values.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.forward.values()
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
}

impl<K, V, S, P, H: HashWidth> Default for GenericBiMap<K, V, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericBiMap {
            forward: GenericHashMap::default(),
            backward: GenericHashMap::default(),
        }
    }
}

impl<K, V, S, P, H: HashWidth> PartialEq for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq,
    V: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.forward == other.forward
    }
}

impl<K, V, S, P, H: HashWidth> Eq for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq,
    V: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<K, V, S, P, H: HashWidth> Hash for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        self.len().hash(state);
        // Order-independent: wrapping_add of per-entry hashes.
        let mut combined: u64 = 0;
        for (k, v) in self.iter() {
            let mut h = crate::util::FnvHasher::new();
            k.hash(&mut h);
            v.hash(&mut h);
            combined = combined.wrapping_add(h.finish());
        }
        combined.hash(state);
    }
}

impl<K, V, S, P, H: HashWidth> Debug for GenericBiMap<K, V, S, P, H>
where
    K: Debug + Hash + Eq + Clone,
    V: Debug + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
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

impl<K, V, S, P, H: HashWidth> FromIterator<(K, V)> for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut bm = GenericBiMap {
            forward: GenericHashMap::default(),
            backward: GenericHashMap::default(),
        };
        for (k, v) in iter {
            bm.insert(k, v);
        }
        bm
    }
}

impl<K, V, S, P, H: HashWidth> From<Vec<(K, V)>> for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: Vec<(K, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K, V, S, const N: usize, P, H: HashWidth> From<[(K, V); N]>
    for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(arr: [(K, V); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a [(K, V)]> for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [(K, V)]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a Vec<(K, V)>> for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: &'a Vec<(K, V)>) -> Self {
        v.iter().cloned().collect()
    }
}

/// Index by key (forward direction), returning the mapped value.
///
/// Panics if the key is not present. Note: `IndexMut` is not implemented
/// because mutating a value via a mutable reference would silently invalidate
/// the reverse lookup (`value → key`) stored in the backward map.
impl<Q, K, V, S, P, H: HashWidth> Index<&Q> for GenericBiMap<K, V, S, P, H>
where
    Q: Hash + Equivalent<K> + ?Sized,
    K: Hash + Eq,
    V: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        // Access forward map directly to avoid the S: Default bound on get_by_key.
        match self.forward.get(key) {
            Some(v) => v,
            None => panic!("BiMap::index: key not found"),
        }
    }
}

impl<K, V, S, P, H: HashWidth> Extend<(K, V)> for GenericBiMap<K, V, S, P, H>
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

/// A consuming iterator over the key-value pairs of a [`GenericBiMap`].
pub struct ConsumingIter<K: Eq, V: Hash + Eq, P: SharedPointerKind, H: HashWidth = u64> {
    inner: crate::hashmap::ConsumingIter<(K, V), P, H>,
}

impl<K, V, P, H: HashWidth> Iterator for ConsumingIter<K, V, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    P: SharedPointerKind,
{
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V, P, H: HashWidth> ExactSizeIterator for ConsumingIter<K, V, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    P: SharedPointerKind,
{
}

impl<K, V, S, P, H: HashWidth> IntoIterator for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<K, V, P, H>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            inner: self.forward.into_iter(),
        }
    }
}

impl<'a, K, V, S, P, H: HashWidth> IntoIterator for &'a GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
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

    assert_impl_all!(crate::BiMap<i32, i32>: Send, Sync);

    #[test]
    fn new_is_empty() {
        let bm: BiMap<&str, i32> = BiMap::new();
        assert!(bm.is_empty());
        assert_eq!(bm.len(), 0);
    }

    #[test]
    fn insert_and_lookup() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        bm.insert("bob", 2);

        assert_eq!(bm.get_by_key(&"alice"), Some(&1));
        assert_eq!(bm.get_by_key(&"bob"), Some(&2));
        assert_eq!(bm.get_by_value(&1), Some(&"alice"));
        assert_eq!(bm.get_by_value(&2), Some(&"bob"));
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn insert_overwrites_key() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        bm.insert("alice", 2);

        assert_eq!(bm.get_by_key(&"alice"), Some(&2));
        assert_eq!(bm.get_by_value(&1), None); // old value gone
        assert_eq!(bm.get_by_value(&2), Some(&"alice"));
        assert_eq!(bm.len(), 1);
    }

    #[test]
    fn insert_overwrites_value() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        bm.insert("bob", 1);

        assert_eq!(bm.get_by_key(&"alice"), None); // old key gone
        assert_eq!(bm.get_by_key(&"bob"), Some(&1));
        assert_eq!(bm.get_by_value(&1), Some(&"bob"));
        assert_eq!(bm.len(), 1);
    }

    #[test]
    fn insert_overwrites_both() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        bm.insert("bob", 2);
        // This displaces both alice→1 and bob→2
        bm.insert("alice", 2);

        assert_eq!(bm.get_by_key(&"alice"), Some(&2));
        assert_eq!(bm.get_by_key(&"bob"), None);
        assert_eq!(bm.get_by_value(&1), None);
        assert_eq!(bm.get_by_value(&2), Some(&"alice"));
        assert_eq!(bm.len(), 1);
    }

    #[test]
    fn contains() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        assert!(bm.contains_key(&"alice"));
        assert!(!bm.contains_key(&"bob"));
        assert!(bm.contains_value(&1));
        assert!(!bm.contains_value(&2));
    }

    #[test]
    fn remove_by_key() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        assert_eq!(bm.remove_by_key(&"alice"), Some(1));
        assert!(bm.is_empty());
        assert_eq!(bm.get_by_value(&1), None);
    }

    #[test]
    fn remove_by_value() {
        let mut bm = BiMap::new();
        bm.insert("alice", 1);
        assert_eq!(bm.remove_by_value(&1), Some("alice"));
        assert!(bm.is_empty());
        assert_eq!(bm.get_by_key(&"alice"), None);
    }

    #[test]
    fn remove_absent() {
        let mut bm: BiMap<&str, i32> = BiMap::new();
        assert_eq!(bm.remove_by_key(&"alice"), None);
        assert_eq!(bm.remove_by_value(&1), None);
    }

    #[test]
    fn from_iterator() {
        let bm: BiMap<&str, i32> =
            vec![("a", 1), ("b", 2), ("c", 3)].into_iter().collect();
        assert_eq!(bm.len(), 3);
        assert_eq!(bm.get_by_key(&"b"), Some(&2));
        assert_eq!(bm.get_by_value(&3), Some(&"c"));
    }

    #[test]
    fn from_array() {
        let bm: BiMap<&str, i32> = BiMap::from([("a", 1), ("b", 2)]);
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn clone_shares_structure() {
        let mut bm = BiMap::new();
        bm.insert("a", 1);
        let bm2 = bm.clone();
        assert_eq!(bm, bm2);
    }

    #[test]
    fn equality() {
        let mut a = BiMap::new();
        a.insert("x", 1);
        a.insert("y", 2);

        let mut b = BiMap::new();
        b.insert("y", 2);
        b.insert("x", 1);

        assert_eq!(a, b);
    }

    #[test]
    fn inequality() {
        let mut a = BiMap::new();
        a.insert("x", 1);

        let mut b = BiMap::new();
        b.insert("x", 2);

        assert_ne!(a, b);
    }

    #[test]
    fn into_iter_owned() {
        let mut bm = BiMap::new();
        bm.insert(1, "a");
        bm.insert(2, "b");

        let mut pairs: Vec<_> = bm.into_iter().collect();
        pairs.sort();
        assert_eq!(pairs, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn into_iter_ref() {
        let mut bm = BiMap::new();
        bm.insert(1, "a");
        bm.insert(2, "b");

        let mut pairs: Vec<_> = (&bm).into_iter().collect();
        pairs.sort_by_key(|(&k, _)| k);
        assert_eq!(pairs, vec![(&1, &"a"), (&2, &"b")]);
    }

    #[test]
    fn for_loop() {
        let mut bm = BiMap::new();
        bm.insert("x", 1);
        bm.insert("y", 2);

        let mut sum = 0;
        for (_, &v) in &bm {
            sum += v;
        }
        assert_eq!(sum, 3);
    }

    #[test]
    fn extend_trait() {
        let mut bm = BiMap::new();
        bm.insert("a", 1);
        bm.extend(vec![("b", 2), ("c", 3)]);
        assert_eq!(bm.len(), 3);
    }

    #[test]
    fn union_method() {
        let mut a = BiMap::new();
        a.insert("a", 1);
        a.insert("b", 2);
        let mut b = BiMap::new();
        b.insert("b", 99); // conflict: b wins
        b.insert("c", 3);
        let c = a.union(b);
        assert_eq!(c.len(), 3);
        assert_eq!(c.get_by_key(&"b"), Some(&99));
    }

    #[test]
    fn default_is_empty() {
        let bm: BiMap<String, i32> = Default::default();
        assert!(bm.is_empty());
    }

    #[test]
    fn debug_output() {
        let mut bm = BiMap::new();
        bm.insert("a", 1);
        let s = format!("{:?}", bm);
        assert!(s.contains("\"a\""));
        assert!(s.contains('1'));
    }

    #[test]
    fn hash_order_independent() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(m: &BiMap<i32, i32>) -> u64 {
            let mut h = DefaultHasher::new();
            m.hash(&mut h);
            h.finish()
        }
        let mut a = BiMap::new();
        a.insert(1, 10); a.insert(2, 20);
        let mut b = BiMap::new();
        b.insert(2, 20); b.insert(1, 10); // different insertion order
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn index_by_key() {
        let mut bm = BiMap::new();
        bm.insert(1i32, 10i32);
        bm.insert(2, 20);
        assert_eq!(bm[&1], 10);
        assert_eq!(bm[&2], 20);
    }

    #[test]
    #[should_panic(expected = "key not found")]
    fn index_panics_on_missing() {
        let bm: BiMap<i32, i32> = BiMap::new();
        let _ = bm[&99];
    }

    #[test]
    fn from_vec() {
        let bm: BiMap<i32, i32> = vec![(1, 10), (2, 20)].into();
        assert_eq!(bm.len(), 2);
        assert_eq!(bm.get_by_value(&10), Some(&1));
    }

    #[test]
    fn from_slice() {
        let bm: BiMap<i32, i32> = [(1i32, 10i32), (2, 20)][..].into();
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(1i32, 10i32), (2, 20)];
        let bm: BiMap<i32, i32> = BiMap::from(&v);
        assert_eq!(bm.len(), 2);
        assert_eq!(bm.get_by_key(&1), Some(&10));
    }
}
