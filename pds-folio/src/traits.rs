//! Implementations of the `pds` cross-variant traits for folio-backed collections.
//!
//! This module implements:
//!
//! - [`PersistentCollection`] for [`HamtMap`] and [`HamtSet`]
//! - [`PersistentMap<K, V>`] for [`HamtMap<K, V, C, B>`]
//! - [`PersistentSet<A>`] for [`HamtSet<A, C, B>`]
//!
//! # Infallible trait methods over fallible storage
//!
//! The `pds` cross-variant traits define infallible methods (returning `V`
//! directly, not `Result<V>`).  `HamtMap` and `HamtSet` methods return
//! `Result<_, HamtError>` because they perform folio I/O.
//!
//! The trait impls below unwrap results with `expect()`.  This is correct
//! because:
//! 1. All folio I/O errors in the test and standard use paths originate from
//!    the in-memory `MemBackend`, which never fails in practice.
//! 2. Codec errors can only arise from malformed page data, which indicates
//!    an invariant violation — an appropriate panic site.
//!
//! If a storage backend returns errors in production, callers should use the
//! direct `HamtMap` / `HamtSet` methods (which return `Result`) rather than
//! going through the trait.

use std::hash::Hash;

use folio_core::{backend::Backend, error::BackendError};
use serde::{Deserialize, Serialize};

use pds::traits::{PersistentCollection, PersistentMap, PersistentSet};

use crate::{
    codec::Codec,
    hamt::HamtMap,
    set::HamtSet,
};

// ---------------------------------------------------------------------------
// PersistentCollection
// ---------------------------------------------------------------------------

impl<K, V, C, B> PersistentCollection for HamtMap<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
}

impl<A, C, B> PersistentCollection for HamtSet<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
}

// ---------------------------------------------------------------------------
// PersistentMap<K, V> for HamtMap<K, V, C, B>
// ---------------------------------------------------------------------------

impl<K, V, C, B> PersistentMap<K, V> for HamtMap<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Returns a clone of the value associated with `key`, or `None` if absent.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    /// In normal use (in-memory or healthy folio stores) this will not occur.
    ///
    /// Time: O(log N).
    fn get_cloned(&self, key: &K) -> Option<V> {
        self.get(key).expect("HamtMap::get failed in PersistentMap::get_cloned")
    }

    /// Returns a new map with `key` mapped to `value`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    ///
    /// Time: O(log N).
    fn insert(&self, key: K, value: V) -> Self {
        HamtMap::insert(self, key, value)
            .expect("HamtMap::insert failed in PersistentMap::insert")
    }

    /// Returns a new map with `key` removed, plus the evicted value.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    ///
    /// Time: O(log N).
    fn remove(&self, key: &K) -> (Self, Option<V>) {
        HamtMap::remove(self, key)
            .expect("HamtMap::remove failed in PersistentMap::remove")
    }

    /// Returns the number of key-value pairs.
    ///
    /// Time: O(1).
    fn len(&self) -> usize {
        HamtMap::len(self)
    }

    /// Tests whether `key` is present.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    ///
    /// Time: O(log N).
    fn contains_key(&self, key: &K) -> bool {
        HamtMap::contains_key(self, key)
            .expect("HamtMap::contains_key failed in PersistentMap::contains_key")
    }
}

// ---------------------------------------------------------------------------
// PersistentSet<A> for HamtSet<A, C, B>
// ---------------------------------------------------------------------------

impl<A, C, B> PersistentSet<A> for HamtSet<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Tests whether `value` is a member of the set.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    ///
    /// Time: O(log N).
    fn contains(&self, value: &A) -> bool {
        HamtSet::contains(self, value)
            .expect("HamtSet::contains failed in PersistentSet::contains")
    }

    /// Returns a new set with `value` inserted.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    ///
    /// Time: O(log N).
    fn insert(&self, value: A) -> Self {
        HamtSet::insert(self, value)
            .expect("HamtSet::insert failed in PersistentSet::insert")
    }

    /// Returns a new set with `value` removed.
    ///
    /// # Panics
    ///
    /// Panics if the underlying folio store returns an I/O or codec error.
    ///
    /// Time: O(log N).
    fn remove(&self, value: &A) -> Self {
        let (new_set, _removed) = HamtSet::remove(self, value)
            .expect("HamtSet::remove failed in PersistentSet::remove");
        new_set
    }

    /// Returns the number of elements in the set.
    ///
    /// Time: O(1).
    fn len(&self) -> usize {
        HamtSet::len(self)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::PostcardCodec;
    use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
    use pds::traits::{PersistentMap, PersistentSet};

    fn make_store() -> FolioStore<MemBackend> {
        let backend = MemBackend::new(4096, 256);
        FolioStore::create(backend, 4096, 256, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    fn make_store2() -> FolioStore<MemBackend> {
        let backend = MemBackend::new(4096, 256);
        FolioStore::create(backend, 4096, 256, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    // -----------------------------------------------------------------------
    // Generic helper functions that accept the trait (same pattern as pds tests)
    // -----------------------------------------------------------------------

    /// Verifies basic get/insert/contains_key on any PersistentMap.
    fn pm_get_insert_contains<M: PersistentMap<String, u64>>(empty: M) {
        let m = empty.insert("hello".to_string(), 42u64);
        assert_eq!(m.get_cloned(&"hello".to_string()), Some(42u64));
        assert_eq!(m.get_cloned(&"world".to_string()), None);
        assert!(m.contains_key(&"hello".to_string()));
        assert!(!m.contains_key(&"absent".to_string()));
        assert_eq!(m.len(), 1);
        assert!(!m.is_empty());
    }

    /// Verifies remove returns the evicted value and leaves original unchanged.
    fn pm_remove<M: PersistentMap<String, u64>>(empty: M) {
        let m = empty
            .insert("a".to_string(), 1u64)
            .insert("b".to_string(), 2u64);
        let (m2, removed) = m.remove(&"a".to_string());
        assert_eq!(removed, Some(1u64));
        assert!(!m2.contains_key(&"a".to_string()));
        assert!(m2.contains_key(&"b".to_string()));
        // Original unchanged.
        assert!(m.contains_key(&"a".to_string()));
    }

    /// Verifies is_empty on empty and non-empty maps.
    fn pm_is_empty<M: PersistentMap<String, u64>>(empty: M) {
        assert!(empty.is_empty());
        let m = empty.insert("x".to_string(), 0u64);
        assert!(!m.is_empty());
    }

    /// Verifies remove-absent returns (clone, None).
    fn pm_remove_absent<M: PersistentMap<String, u64>>(empty: M) {
        let m = empty.insert("a".to_string(), 1u64);
        let (m2, removed) = m.remove(&"missing".to_string());
        assert_eq!(removed, None);
        assert_eq!(m2.len(), 1);
        assert_eq!(m2.get_cloned(&"a".to_string()), Some(1u64));
    }

    // -----------------------------------------------------------------------
    // PersistentMap via HamtMap
    // -----------------------------------------------------------------------

    #[test]
    fn hamt_map_persistent_map_get_insert_contains() {
        let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        pm_get_insert_contains(map);
    }

    #[test]
    fn hamt_map_persistent_map_remove() {
        let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        pm_remove(map);
    }

    #[test]
    fn hamt_map_persistent_map_is_empty() {
        let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        pm_is_empty(map);
    }

    #[test]
    fn hamt_map_persistent_map_remove_absent() {
        let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        pm_remove_absent(map);
    }

    /// Snapshot isolation: inserting into clone A does not affect clone B.
    #[test]
    fn hamt_map_snapshot_isolation() {
        // Use a generic function so all calls route through the PersistentMap trait.
        fn check_isolation<M: PersistentMap<String, u64>>(
            a: &M,
            b: &M,
            only_a: &String,
            only_b: &String,
            base_key: &String,
        ) {
            assert!(a.contains_key(only_a));
            assert!(!a.contains_key(only_b));
            assert!(b.contains_key(only_b));
            assert!(!b.contains_key(only_a));
            assert!(a.contains_key(base_key));
            assert!(b.contains_key(base_key));
        }

        let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        let base = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(
            &map,
            "base".to_string(),
            0u64,
        );
        let a = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(
            &base,
            "only_a".to_string(),
            1u64,
        );
        let b = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(
            &base,
            "only_b".to_string(),
            2u64,
        );

        check_isolation(
            &a,
            &b,
            &"only_a".to_string(),
            &"only_b".to_string(),
            &"base".to_string(),
        );
    }

    // -----------------------------------------------------------------------
    // Generic helper functions for PersistentSet
    // -----------------------------------------------------------------------

    fn ps_insert_contains<S: PersistentSet<String>>(empty: S) {
        let s = empty.insert("a".to_string()).insert("b".to_string());
        assert!(s.contains(&"a".to_string()));
        assert!(s.contains(&"b".to_string()));
        assert!(!s.contains(&"c".to_string()));
        assert_eq!(s.len(), 2);
        assert!(!s.is_empty());
    }

    fn ps_remove<S: PersistentSet<String>>(empty: S) {
        let s = empty.insert("a".to_string()).insert("b".to_string());
        let s2 = PersistentSet::remove(&s, &"a".to_string());
        assert!(!s2.contains(&"a".to_string()));
        assert!(s2.contains(&"b".to_string()));
        // Original unchanged.
        assert!(s.contains(&"a".to_string()));
    }

    fn ps_is_empty<S: PersistentSet<String>>(empty: S) {
        assert!(empty.is_empty());
        assert!(!empty.insert("x".to_string()).is_empty());
    }

    // -----------------------------------------------------------------------
    // PersistentSet via HamtSet
    // -----------------------------------------------------------------------

    #[test]
    fn hamt_set_persistent_set_insert_contains() {
        let s: HamtSet<String, PostcardCodec, MemBackend> = HamtSet::new(make_store());
        ps_insert_contains(s);
    }

    #[test]
    fn hamt_set_persistent_set_remove() {
        let s: HamtSet<String, PostcardCodec, MemBackend> = HamtSet::new(make_store());
        ps_remove(s);
    }

    #[test]
    fn hamt_set_persistent_set_is_empty() {
        let s: HamtSet<String, PostcardCodec, MemBackend> = HamtSet::new(make_store());
        ps_is_empty(s);
    }

    /// Snapshot isolation for HamtSet: modifying clone A does not affect clone B.
    #[test]
    fn hamt_set_snapshot_isolation() {
        // Use a generic function so all calls go through the PersistentSet trait.
        fn check_isolation<S: PersistentSet<String>>(
            a: &S,
            b: &S,
            only_a: &String,
            only_b: &String,
            shared: &String,
        ) {
            assert!(a.contains(only_a));
            assert!(!a.contains(only_b));
            assert!(b.contains(only_b));
            assert!(!b.contains(only_a));
            assert!(a.contains(shared));
            assert!(b.contains(shared));
        }

        let base: HamtSet<String, PostcardCodec, MemBackend> = HamtSet::new(make_store());
        let base = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&base, "shared".to_string());
        let a = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&base, "only_a".to_string());
        let b = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&base, "only_b".to_string());

        check_isolation(
            &a,
            &b,
            &"only_a".to_string(),
            &"only_b".to_string(),
            &"shared".to_string(),
        );
    }

    /// Round-trip key lookup: insert N keys, look them all up via the trait.
    #[test]
    fn hamt_map_round_trip_key_lookup() {
        let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        let mut m: HamtMap<String, u64, PostcardCodec, MemBackend> = map;
        let n = 64u64;
        for i in 0..n {
            m = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(
                &m,
                format!("k{i}"),
                i * 10,
            );
        }
        assert_eq!(m.len(), n as usize);
        for i in 0..n {
            assert_eq!(m.get_cloned(&format!("k{i}")), Some(i * 10));
        }
    }

    /// Verify that PersistentMap works the same regardless of which store
    /// the two maps are backed by (type-compatibility check).
    #[test]
    fn two_hamt_maps_same_type_different_stores() {
        let m1: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
        let m2: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store2());

        let m1 = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m1, "a".to_string(), 1u64);
        let m2 = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m2, "b".to_string(), 2u64);

        // Each only sees its own keys.
        assert_eq!(m1.get_cloned(&"a".to_string()), Some(1u64));
        assert_eq!(m1.get_cloned(&"b".to_string()), None);
        assert_eq!(m2.get_cloned(&"b".to_string()), Some(2u64));
        assert_eq!(m2.get_cloned(&"a".to_string()), None);
    }
}
