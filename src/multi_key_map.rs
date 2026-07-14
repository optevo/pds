// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent multi-key map: multiple keys may point to the same value,
//! with insertion-order iteration and O(1) preferred-key lookup.
//!
//! [`MultiKeyMap<K, V>`] maps each key `K` to a value `V` (many-to-one),
//! and tracks the *preferred* key for each value — the first key registered
//! for that value, by insertion order.
//!
//! # Semantics
//!
//! - Multiple keys may map to the same value. A key maps to exactly one value.
//! - The first key registered for a value is its *preferred key*.
//! - Iteration is in key-insertion order (the order keys were first inserted).
//! - `insert(k, v)` where `k` already maps to a *different* value atomically
//!   updates `k`'s value to `v` without changing `k`'s insertion order position.
//! - `insert(k, v)` where `k` already maps to the same value is idempotent.
//!
//! # Examples
//!
//! ```
//! use pds::MultiKeyMap;
//!
//! let mut mm: MultiKeyMap<&str, u32> = MultiKeyMap::new();
//! mm.insert("a", 1);
//! mm.insert("b", 1);   // second key for value 1
//! mm.insert("c", 2);
//!
//! assert_eq!(mm.get(&"a"), Some(&1));
//! assert_eq!(mm.get(&"b"), Some(&1));
//! assert_eq!(mm.preferred_key(&1), Some(&"a")); // "a" was first
//! assert_eq!(mm.len(), 3);
//! ```

use core::fmt;
use core::hash::{BuildHasher, Hash};
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
use foldhash::fast::RandomState;
#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;

use crate::hashmap::HashMap;
use crate::ordmap::OrdMap;

/// A persistent multi-key map with insertion-order iteration.
///
/// This is a type alias for [`GenericMultiKeyMap`] with the default
/// [`RandomState`] hasher.
///
/// See the [module-level documentation](self) for details.
pub type MultiKeyMap<K, V> = GenericMultiKeyMap<K, V, RandomState>;

/// A persistent multi-key map with a configurable hasher.
///
/// Multiple keys may map to the same value `V`. For each value the
/// *preferred key* is the key with the lowest insertion index — i.e. the
/// key registered first for that value.
///
/// `V` must implement [`Hash`] because the forward map stores `(V, usize)` pairs
/// in a pds [`HashMap`], which requires all stored values to be hashable.
///
/// # Internal structure
///
/// Three coordinated maps maintain the full bidirectional index:
///
/// - `fwd` — `HashMap<K, (V, usize)>`: forward lookup, O(log n).
///   The `usize` is the insertion index of the key.
/// - `seq` — `OrdMap<usize, K>`: insertion-index → key, used for insertion-order
///   iteration.
/// - `rev` — `OrdMap<V, OrdMap<usize, K>>`: value → inner map of all
///   (insertion-index, key) pairs for that value. The minimum key in the inner
///   map is the preferred key.
///
/// # Invariant
///
/// Every `(k, v, idx)` triple registered via [`insert`][Self::insert] is
/// simultaneously present in all three maps. `seq.len() == fwd.len()` at all
/// times. In debug builds, [`assert_invariants`][Self::assert_invariants]
/// checks this property.
pub struct GenericMultiKeyMap<K, V, S = RandomState>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
{
    /// Forward map: key → (value, insertion index). O(log n) lookup.
    fwd: HashMap<K, (V, usize)>,
    /// Sequence map: insertion index → key. Gives insertion-order iteration.
    seq: OrdMap<usize, K>,
    /// Reverse map: value → inner map of (insertion_idx → key).
    /// Minimum entry in the inner map is the preferred key for that value.
    rev: OrdMap<V, OrdMap<usize, K>>,
    /// Monotonically increasing counter; never reused, even after removes.
    next_idx: usize,
    /// Phantom to hold the hasher type parameter.
    _hasher: std::marker::PhantomData<S>,
}

// Manual Clone — pds style avoids spurious bounds.
impl<K, V, S> Clone for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
{
    fn clone(&self) -> Self {
        GenericMultiKeyMap {
            fwd: self.fwd.clone(),
            seq: self.seq.clone(),
            rev: self.rev.clone(),
            next_idx: self.next_idx,
            _hasher: std::marker::PhantomData,
        }
    }
}

// Manual Debug — requires K: Debug, V: Debug.
impl<K, V, S> fmt::Debug for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone + fmt::Debug,
    V: Ord + Clone + Hash + fmt::Debug,
    S: BuildHasher,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

// ── Construction ──────────────────────────────────────────────────────────────

impl<K, V> GenericMultiKeyMap<K, V, RandomState>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
{
    /// Constructs an empty `MultiKeyMap`.
    ///
    /// Time: O(1)
    #[must_use]
    pub fn new() -> Self {
        GenericMultiKeyMap {
            fwd: HashMap::new(),
            seq: OrdMap::new(),
            rev: OrdMap::new(),
            next_idx: 0,
            _hasher: std::marker::PhantomData,
        }
    }
}

// ── Core operations ───────────────────────────────────────────────────────────

impl<K, V, S> GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
    S: BuildHasher,
{
    /// Registers a key→value mapping.
    ///
    /// - If `k` already maps to `v`, this is a no-op (idempotent).
    /// - If `k` already maps to a *different* value, `k` is atomically moved
    ///   to `v`. The key retains its original insertion-order position.
    /// - If `k` is new, it is assigned the next insertion index.
    ///
    /// Time: O(log n)
    pub fn insert(&mut self, k: K, v: V) {
        if let Some((old_v, idx)) = self.fwd.get(&k) {
            if old_v == &v {
                // Exact duplicate — idempotent.
                return;
            }
            // Key maps to a different value. Update atomically:
            let idx = *idx;
            let old_v = old_v.clone();
            // 1. Remove k from old_v's inner rev map.
            if let Some(inner) = self.rev.get(&old_v) {
                let new_inner = inner.without(&idx);
                if new_inner.is_empty() {
                    self.rev = self.rev.without(&old_v);
                } else {
                    self.rev = self.rev.update(old_v, new_inner);
                }
            }
            // 2. Add k to new_v's inner rev map (reuse same idx).
            let inner = self.rev.get(&v).cloned().unwrap_or_default();
            let new_inner = inner.update(idx, k.clone());
            self.rev = self.rev.update(v.clone(), new_inner);
            // 3. Update fwd.
            self.fwd.insert(k, (v, idx));
        } else {
            // New key.
            let idx = self.next_idx;
            self.next_idx = self.next_idx.saturating_add(1);
            // Update fwd.
            self.fwd.insert(k.clone(), (v.clone(), idx));
            // Update seq.
            self.seq = self.seq.update(idx, k.clone());
            // Update rev.
            let inner = self.rev.get(&v).cloned().unwrap_or_default();
            let new_inner = inner.update(idx, k);
            self.rev = self.rev.update(v, new_inner);
        }
    }

    /// Removes the registration for `k`.
    ///
    /// Returns the removed value, or `None` if `k` was not present.
    /// If the value still has other keys registered, those registrations remain.
    ///
    /// Time: O(log n)
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let (v, idx) = self.fwd.remove(k)?;
        // Remove from seq.
        self.seq = self.seq.without(&idx);
        // Remove from rev inner map.
        if let Some(inner) = self.rev.get(&v) {
            let new_inner = inner.without(&idx);
            if new_inner.is_empty() {
                self.rev = self.rev.without(&v);
            } else {
                self.rev = self.rev.update(v.clone(), new_inner);
            }
        }
        Some(v)
    }

    /// Returns the value registered for `k`, if any.
    ///
    /// Time: O(log n)
    pub fn get(&self, k: &K) -> Option<&V> {
        self.fwd.get(k).map(|(v, _)| v)
    }

    /// Returns the preferred (first-inserted) key for value `v`, if any.
    ///
    /// The preferred key is the key with the minimum insertion index among
    /// all keys registered for `v`.
    ///
    /// Time: O(log n)
    pub fn preferred_key(&self, v: &V) -> Option<&K> {
        self.rev.get(v)?.get_min().map(|(_, k)| k)
    }

    /// Returns an iterator over all keys for value `v` in insertion order.
    ///
    /// Time: O(k log n) to iterate k keys
    pub fn keys_for(&self, v: &V) -> impl Iterator<Item = &K> {
        self.rev.get(v).into_iter().flat_map(|inner| inner.values())
    }

    /// Returns an iterator over all `(key, value)` pairs in insertion order.
    ///
    /// Time: O(n log n)
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.seq
            .iter()
            .filter_map(|(_, k)| self.fwd.get(k).map(|(v, _)| (k, v)))
    }

    /// Returns the number of key→value registrations.
    ///
    /// Time: O(1)
    pub fn len(&self) -> usize {
        self.fwd.len()
    }

    /// Tests whether the map contains no registrations.
    ///
    /// Time: O(1)
    pub fn is_empty(&self) -> bool {
        self.fwd.is_empty()
    }
}

// ── Standard traits ───────────────────────────────────────────────────────────

impl<K, V, S> PartialEq for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash + PartialEq,
    S: BuildHasher,
{
    /// Tests whether two maps have the same key→value registrations.
    ///
    /// Two maps are equal when they have the same set of (k, v) pairs regardless
    /// of insertion order or next_idx state.
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for (k, (v, _)) in self.fwd.iter() {
            if other.fwd.get(k).map(|(v2, _)| v2) != Some(v) {
                return false;
            }
        }
        true
    }
}

impl<K, V, S> Eq for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash + Eq,
    S: BuildHasher,
{
}

impl<K, V> Default for GenericMultiKeyMap<K, V, RandomState>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> core::hash::Hash for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
    S: BuildHasher,
{
    /// Hashes the map in an order-independent manner using wrapping-add combination.
    ///
    /// Each (k, v) pair is hashed independently using `FnvHasher` and the results
    /// are combined with `wrapping_add` so that the overall hash is independent of
    /// insertion order. Uses `crate::util::FnvHasher` for no_std compatibility.
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        use core::hash::Hasher as _;
        self.len().hash(state);
        // Order-independent: wrapping_add of per-entry hashes.
        let mut combined: u64 = 0;
        for (k, (v, _)) in self.fwd.iter() {
            let mut h = crate::util::FnvHasher::new();
            k.hash(&mut h);
            v.hash(&mut h);
            combined = combined.wrapping_add(h.finish());
        }
        combined.hash(state);
    }
}

impl<K, V> FromIterator<(K, V)> for GenericMultiKeyMap<K, V, RandomState>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

/// Owned consuming iterator over a [`GenericMultiKeyMap`].
///
/// Yields `(K, V)` pairs in insertion order.
pub struct ConsumingIter<K, V>
where
    K: Hash + Eq + Clone,
    V: Hash + Clone,
{
    /// Keys in insertion order.
    seq: Vec<K>,
    /// Map from key to value — backed by pds's persistent HashMap for no_std
    /// compatibility.
    values: HashMap<K, V>,
    /// Current position in seq.
    pos: usize,
}

impl<K, V> Iterator for ConsumingIter<K, V>
where
    K: Hash + Eq + Clone,
    V: Hash + Clone,
{
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < self.seq.len() {
            let k = self.seq[self.pos].clone();
            self.pos += 1;
            if let Some(v) = self.values.remove(&k) {
                return Some((k, v));
            }
        }
        None
    }
}

impl<K, V, S> IntoIterator for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
    S: BuildHasher,
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        // Build ordered key sequence from seq.
        let seq: Vec<K> = self.seq.iter().map(|(_, k)| k.clone()).collect();
        // Build pds HashMap for O(log n) value lookup during consumption.
        // Uses pds's own persistent HashMap (no_std-compatible) instead of
        // std::collections::HashMap.
        let values: HashMap<K, V> = self.fwd.into_iter().map(|(k, (v, _))| (k, v)).collect();
        ConsumingIter {
            seq,
            values,
            pos: 0,
        }
    }
}

impl<'a, K, V, S> IntoIterator for &'a GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
    S: BuildHasher,
{
    type Item = (&'a K, &'a V);
    type IntoIter = RefIter<'a, K, V, S>;

    fn into_iter(self) -> Self::IntoIter {
        RefIter {
            map: self,
            // Collect seq keys in order, then iterate.
            keys: self.seq.iter().map(|(_, k)| k).collect(),
            pos: 0,
        }
    }
}

/// Borrowed iterator over a [`GenericMultiKeyMap`].
///
/// Yields `(&K, &V)` pairs in insertion order.
pub struct RefIter<'a, K, V, S = RandomState>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
{
    /// Reference to the map for fwd lookups.
    map: &'a GenericMultiKeyMap<K, V, S>,
    /// Keys in insertion order.
    keys: Vec<&'a K>,
    /// Current position.
    pos: usize,
}

impl<'a, K, V, S> Iterator for RefIter<'a, K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
    S: BuildHasher,
{
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < self.keys.len() {
            let k = self.keys[self.pos];
            self.pos += 1;
            if let Some(v) = self.map.fwd.get(k).map(|(v, _)| v) {
                return Some((k, v));
            }
        }
        None
    }
}

impl<K, V, S> Extend<(K, V)> for GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone,
    V: Ord + Clone + Hash,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

// ── Invariant checking (debug builds only) ────────────────────────────────────

#[cfg(debug_assertions)]
impl<K, V, S> GenericMultiKeyMap<K, V, S>
where
    K: Hash + Eq + Clone + fmt::Debug,
    V: Ord + Clone + Hash + fmt::Debug,
    S: BuildHasher,
{
    /// Returns a list of all invariant violations.
    ///
    /// An empty result means the map is internally consistent. Checks:
    /// - Every `fwd` entry has a matching `seq` entry.
    /// - Every `fwd` entry has a matching `rev` entry.
    /// - Every `seq` entry has a matching `fwd` entry.
    /// - Every `rev` inner entry has a matching `fwd` entry.
    /// - `seq.len() == fwd.len()`.
    ///
    /// Time: O(n log n)
    pub fn check_invariants(&self) -> Vec<String> {
        let mut v = Vec::new();

        // seq.len() == fwd.len()
        if self.seq.len() != self.fwd.len() {
            v.push(format!(
                "seq.len()={} ≠ fwd.len()={}",
                self.seq.len(),
                self.fwd.len()
            ));
        }

        // Every fwd entry must have a matching seq entry and rev entry.
        for (k, (val, idx)) in self.fwd.iter() {
            // Check seq.
            match self.seq.get(idx) {
                None => v.push(format!(
                    "fwd has {k:?}→(_, {idx}) but seq has no entry for idx={idx}"
                )),
                Some(seq_k) if seq_k != k => {
                    v.push(format!("fwd has {k:?}→(_, {idx}) but seq[{idx}]={seq_k:?}"));
                }
                _ => {}
            }
            // Check rev.
            match self.rev.get(val) {
                None => v.push(format!(
                    "fwd has {k:?}→({val:?}, {idx}) but rev has no entry for that value"
                )),
                Some(inner) => match inner.get(idx) {
                    None => v.push(format!(
                        "fwd has {k:?}→({val:?}, {idx}) but rev[{val:?}] has no entry for idx={idx}"
                    )),
                    Some(rev_k) if rev_k != k => v.push(format!(
                        "fwd has {k:?}→({val:?}, {idx}) but rev[{val:?}][{idx}]={rev_k:?}"
                    )),
                    _ => {}
                },
            }
        }

        // Every seq entry must have a matching fwd entry.
        for (idx, k) in self.seq.iter() {
            match self.fwd.get(k) {
                None => v.push(format!(
                    "seq has [{idx}]={k:?} but fwd has no entry for that key"
                )),
                Some((_, fwd_idx)) if fwd_idx != idx => v.push(format!(
                    "seq has [{idx}]={k:?} but fwd[{k:?}].idx={fwd_idx}"
                )),
                _ => {}
            }
        }

        // Every rev entry must have matching fwd entries.
        for (val, inner) in self.rev.iter() {
            for (idx, k) in inner.iter() {
                match self.fwd.get(k) {
                    None => v.push(format!(
                        "rev has [{val:?}][{idx}]={k:?} but fwd has no entry for that key"
                    )),
                    Some((fwd_val, fwd_idx)) => {
                        if fwd_val != val {
                            v.push(format!(
                                "rev has [{val:?}][{idx}]={k:?} but fwd[{k:?}].value={fwd_val:?}"
                            ));
                        }
                        if fwd_idx != idx {
                            v.push(format!(
                                "rev has [{val:?}][{idx}]={k:?} but fwd[{k:?}].idx={fwd_idx}"
                            ));
                        }
                    }
                }
            }
        }

        v
    }

    /// Panics with the full violation list if any invariant is broken.
    ///
    /// # Panics
    ///
    /// Panics when [`check_invariants`][Self::check_invariants] returns a
    /// non-empty list.
    pub fn assert_invariants(&self) {
        let violations = self.check_invariants();
        if !violations.is_empty() {
            panic!(
                "MultiKeyMap invariant violations ({}):\n{}",
                violations.len(),
                violations
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("  {}: {s}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    type MM = MultiKeyMap<String, u32>;

    fn mkmap(pairs: &[(&str, u32)]) -> MM {
        let mut m = MM::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), *v);
        }
        m
    }

    // ── Basic unit tests ──────────────────────────────────────────────────────

    #[test]
    fn empty() {
        let m: MM = MultiKeyMap::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert_eq!(m.get(&"a".to_string()), None);
        assert_eq!(m.preferred_key(&0), None);
        m.assert_invariants();
    }

    #[test]
    fn round_trip_insert() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("a".to_string(), 1);
        assert_eq!(m.get(&"a".to_string()), Some(&1));
        assert_eq!(m.preferred_key(&1), Some(&"a".to_string()));
        assert_eq!(m.len(), 1);
        m.assert_invariants();
    }

    #[test]
    fn idempotent_insert() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("a".to_string(), 1);
        m.insert("a".to_string(), 1);
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&"a".to_string()), Some(&1));
        m.assert_invariants();
    }

    #[test]
    fn update_key_to_new_value() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 1);
        m.insert("a".to_string(), 2); // update a → 2
        assert_eq!(m.get(&"a".to_string()), Some(&2));
        assert_eq!(m.get(&"b".to_string()), Some(&1));
        // "b" is now the only key for value 1.
        assert_eq!(m.preferred_key(&1), Some(&"b".to_string()));
        // "a" is now the only key for value 2.
        assert_eq!(m.preferred_key(&2), Some(&"a".to_string()));
        m.assert_invariants();
    }

    #[test]
    fn remove_clears_all_three_maps() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 1);
        let removed = m.remove(&"a".to_string());
        assert_eq!(removed, Some(1));
        assert_eq!(m.get(&"a".to_string()), None);
        // "b" still maps to 1.
        assert_eq!(m.get(&"b".to_string()), Some(&1));
        assert_eq!(m.preferred_key(&1), Some(&"b".to_string()));
        assert_eq!(m.len(), 1);
        m.assert_invariants();
    }

    #[test]
    fn remove_last_key_for_value_clears_rev_entry() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("a".to_string(), 1);
        m.remove(&"a".to_string());
        assert!(m.is_empty());
        assert_eq!(m.preferred_key(&1), None);
        m.assert_invariants();
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let mut m: MM = MultiKeyMap::new();
        assert_eq!(m.remove(&"missing".to_string()), None);
    }

    #[test]
    fn preferred_key_is_first_inserted() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("b".to_string(), 42);
        m.insert("a".to_string(), 42);
        m.insert("c".to_string(), 42);
        // "b" was inserted first.
        assert_eq!(m.preferred_key(&42), Some(&"b".to_string()));
        m.assert_invariants();
    }

    #[test]
    fn preferred_key_promotes_after_remove() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("b".to_string(), 42);
        m.insert("a".to_string(), 42);
        m.remove(&"b".to_string());
        // "b" removed; "a" is now preferred.
        assert_eq!(m.preferred_key(&42), Some(&"a".to_string()));
        m.assert_invariants();
    }

    #[test]
    fn keys_for_all_in_insertion_order() {
        let m = mkmap(&[("x", 5), ("y", 5), ("z", 5)]);
        let keys: Vec<&String> = m.keys_for(&5).collect();
        assert_eq!(keys.len(), 3);
        let key_strs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        assert!(key_strs.contains(&"x"));
        assert!(key_strs.contains(&"y"));
        assert!(key_strs.contains(&"z"));
        m.assert_invariants();
    }

    #[test]
    fn iter_insertion_order() {
        let m = mkmap(&[("c", 3), ("a", 1), ("b", 2)]);
        let pairs: Vec<(&String, &u32)> = m.iter().collect();
        // Should come out in insertion order: c, a, b.
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["c", "a", "b"]);
        m.assert_invariants();
    }

    #[test]
    fn multiple_values() {
        let m = mkmap(&[("a", 1), ("b", 2), ("c", 1), ("d", 2)]);
        assert_eq!(m.len(), 4);
        assert_eq!(m.preferred_key(&1), Some(&"a".to_string()));
        assert_eq!(m.preferred_key(&2), Some(&"b".to_string()));
        let mut keys_1: Vec<&str> = m.keys_for(&1).map(String::as_str).collect();
        let mut keys_2: Vec<&str> = m.keys_for(&2).map(String::as_str).collect();
        keys_1.sort_unstable();
        keys_2.sort_unstable();
        assert_eq!(keys_1, vec!["a", "c"]);
        assert_eq!(keys_2, vec!["b", "d"]);
        m.assert_invariants();
    }

    #[test]
    fn len_and_is_empty() {
        let mut m: MM = MultiKeyMap::new();
        assert!(m.is_empty());
        m.insert("a".to_string(), 1);
        assert!(!m.is_empty());
        assert_eq!(m.len(), 1);
        m.insert("b".to_string(), 1);
        assert_eq!(m.len(), 2);
        m.remove(&"a".to_string());
        assert_eq!(m.len(), 1);
        m.remove(&"b".to_string());
        assert!(m.is_empty());
        m.assert_invariants();
    }

    #[test]
    fn from_iterator() {
        let pairs = vec![
            ("a".to_string(), 1u32),
            ("b".to_string(), 2u32),
            ("c".to_string(), 1u32),
        ];
        let m: MM = pairs.into_iter().collect();
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&"a".to_string()), Some(&1));
        assert_eq!(m.get(&"c".to_string()), Some(&1));
        assert_eq!(m.preferred_key(&1), Some(&"a".to_string()));
        m.assert_invariants();
    }

    #[test]
    fn extend_test() {
        let mut m: MM = MultiKeyMap::new();
        m.insert("a".to_string(), 1);
        m.extend([("b".to_string(), 2u32), ("c".to_string(), 1u32)]);
        assert_eq!(m.len(), 3);
        m.assert_invariants();
    }

    #[test]
    fn into_iter_owned() {
        let m = mkmap(&[("x", 10), ("y", 20)]);
        let pairs: Vec<(String, u32)> = m.into_iter().collect();
        // Should be in insertion order.
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("x".to_string(), 10));
        assert_eq!(pairs[1], ("y".to_string(), 20));
    }

    #[test]
    fn into_iter_ref() {
        let m = mkmap(&[("p", 7), ("q", 8)]);
        let pairs: Vec<(&String, &u32)> = (&m).into_iter().collect();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn equality() {
        let m1 = mkmap(&[("a", 1), ("b", 2)]);
        // Different insertion order — same (k,v) set.
        let m2 = mkmap(&[("b", 2), ("a", 1)]);
        assert_eq!(m1, m2);
        let m3 = mkmap(&[("a", 1)]);
        assert_ne!(m1, m3);
    }

    #[test]
    fn default_is_empty() {
        let m: MM = MultiKeyMap::default();
        assert!(m.is_empty());
        m.assert_invariants();
    }

    #[test]
    fn clone_test() {
        let m1 = mkmap(&[("a", 1), ("b", 1)]);
        let m2 = m1.clone();
        assert_eq!(m1, m2);
        m2.assert_invariants();
    }

    // ── Proptest suites ───────────────────────────────────────────────────────

    fn arb_key() -> impl Strategy<Value = String> {
        "[a-z]{1,4}".prop_map(|s| s)
    }

    fn arb_val() -> impl Strategy<Value = u32> {
        0u32..10
    }

    proptest! {
        /// Invariants hold after arbitrary insert/update sequences.
        #[test]
        fn invariants_hold_after_inserts_and_updates(
            ops in prop::collection::vec((arb_key(), arb_val()), 0..=20)
        ) {
            let mut m: MM = MultiKeyMap::new();
            for (k, v) in ops {
                m.insert(k, v);
                m.assert_invariants();
            }
        }

        /// preferred_key is always the first key inserted for a value.
        #[test]
        fn preferred_key_is_first_inserted_prop(
            key1 in arb_key(),
            key2 in arb_key(),
            val in arb_val(),
        ) {
            prop_assume!(key1 != key2);
            let mut m: MM = MultiKeyMap::new();
            m.insert(key1.clone(), val);
            m.insert(key2.clone(), val);
            prop_assert_eq!(m.preferred_key(&val), Some(&key1));
            m.assert_invariants();
        }

        /// After remove and re-insert, invariants still hold.
        #[test]
        fn remove_then_reinsert(
            k in arb_key(),
            v1 in arb_val(),
            v2 in arb_val(),
        ) {
            let mut m: MM = MultiKeyMap::new();
            m.insert(k.clone(), v1);
            m.remove(&k);
            m.insert(k.clone(), v2);
            prop_assert_eq!(m.get(&k), Some(&v2));
            m.assert_invariants();
        }
    }
}
