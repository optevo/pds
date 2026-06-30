//! Integration tests and proptest suite for HamtMap and HamtSet (G.7).
//!
//! These tests exercise:
//! - Random insert/remove sequences with correctness verification
//! - Snapshot isolation (modify A, B is unchanged)
//! - Round-trip key lookup correctness
//! - PersistentMap / PersistentSet trait consistency
//! - HamtMap via PostcardCodec and PodCodec

use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds::traits::{PersistentMap, PersistentSet};
use pds_folio::{
    codec::{PodCodec, PostcardCodec},
    hamt::HamtMap,
    set::HamtSet,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_store() -> FolioStore<MemBackend> {
    let backend = MemBackend::new(4096, 512);
    FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
        .expect("store creation must succeed")
}

// ---------------------------------------------------------------------------
// Deterministic integration: HamtMap
// ---------------------------------------------------------------------------

#[test]
fn insert_many_and_verify_all_via_trait() {
    let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
    let n = 128u64;
    let mut m: HamtMap<String, u64, PostcardCodec, MemBackend> = map;
    for i in 0..n {
        m = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m, format!("key{i}"), i);
    }
    assert_eq!(m.len(), n as usize);
    for i in 0..n {
        assert_eq!(m.get_cloned(&format!("key{i}")), Some(i));
    }
}

#[test]
fn insert_remove_half_and_verify_remaining() {
    let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
    let n = 64u64;
    let mut m: HamtMap<String, u64, PostcardCodec, MemBackend> = map;
    for i in 0..n {
        m = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m, format!("k{i}"), i * 10);
    }
    // Remove the even-numbered keys.
    for i in (0..n).filter(|x| x % 2 == 0) {
        let (new_m, removed) =
            <HamtMap<_, _, _, _> as PersistentMap<_, _>>::remove(&m, &format!("k{i}"));
        assert_eq!(removed, Some(i * 10), "expected to remove k{i}");
        m = new_m;
    }
    assert_eq!(m.len(), (n / 2) as usize);
    // Odd keys still present.
    for i in (0..n).filter(|x| x % 2 != 0) {
        assert_eq!(m.get_cloned(&format!("k{i}")), Some(i * 10));
    }
    // Even keys absent.
    for i in (0..n).filter(|x| x % 2 == 0) {
        assert_eq!(m.get_cloned(&format!("k{i}")), None);
    }
}

#[test]
fn snapshot_isolation_insert_does_not_affect_sibling() {
    let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
    let base = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&map, "shared".to_string(), 0);
    let a = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&base, "only_a".to_string(), 1);
    let b = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&base, "only_b".to_string(), 2);

    // a does not see only_b; b does not see only_a.
    assert_eq!(a.get_cloned(&"only_a".to_string()), Some(1));
    assert_eq!(a.get_cloned(&"only_b".to_string()), None);
    assert_eq!(b.get_cloned(&"only_b".to_string()), Some(2));
    assert_eq!(b.get_cloned(&"only_a".to_string()), None);
    // Both see the shared key.
    assert_eq!(a.get_cloned(&"shared".to_string()), Some(0));
    assert_eq!(b.get_cloned(&"shared".to_string()), Some(0));
    // base still unchanged.
    assert_eq!(base.len(), 1);
    assert_eq!(base.get_cloned(&"shared".to_string()), Some(0));
}

#[test]
fn snapshot_isolation_remove_does_not_affect_original() {
    let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
    let m1 = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&map, "a".to_string(), 1);
    let m2 = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m1, "b".to_string(), 2);

    let (m3, removed) = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::remove(&m2, &"a".to_string());
    assert_eq!(removed, Some(1));

    // m2 still has "a".
    assert_eq!(m2.get_cloned(&"a".to_string()), Some(1));
    // m3 does not.
    assert_eq!(m3.get_cloned(&"a".to_string()), None);
    // m3 still has "b".
    assert_eq!(m3.get_cloned(&"b".to_string()), Some(2));
}

#[test]
fn overwrite_updates_without_growing_map() {
    let map: HamtMap<String, u64, PostcardCodec, MemBackend> = HamtMap::new(make_store());
    let m1 = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&map, "key".to_string(), 1);
    let m2 = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m1, "key".to_string(), 2);
    assert_eq!(m1.len(), 1);
    assert_eq!(m2.len(), 1); // overwrite, not new insert
    assert_eq!(m1.get_cloned(&"key".to_string()), Some(1));
    assert_eq!(m2.get_cloned(&"key".to_string()), Some(2));
}

#[test]
fn pod_codec_u64_keys_large_insertion() {
    let map: HamtMap<u64, u64, PodCodec, MemBackend> = HamtMap::new(make_store());
    let mut m: HamtMap<u64, u64, PodCodec, MemBackend> = map;
    let n = 100u64;
    for i in 0..n {
        m = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m, i, i * i);
    }
    assert_eq!(m.len(), n as usize);
    for i in 0..n {
        assert_eq!(m.get_cloned(&i), Some(i * i));
    }
}

// ---------------------------------------------------------------------------
// Deterministic integration: HamtSet
// ---------------------------------------------------------------------------

#[test]
fn set_insert_many_and_verify() {
    let s: HamtSet<String, PostcardCodec, MemBackend> = HamtSet::new(make_store());
    let n = 64usize;
    let mut current: HamtSet<String, PostcardCodec, MemBackend> = s;
    for i in 0..n {
        current = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&current, format!("elem{i}"));
    }
    assert_eq!(current.len(), n);
    for i in 0..n {
        assert!(<HamtSet<_, _, _> as PersistentSet<_>>::contains(
            &current,
            &format!("elem{i}")
        ));
    }
}

#[test]
fn set_snapshot_isolation() {
    let s: HamtSet<String, PostcardCodec, MemBackend> = HamtSet::new(make_store());
    let base = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&s, "shared".to_string());
    let a = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&base, "only_a".to_string());
    let b = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&base, "only_b".to_string());

    assert!(<HamtSet<_, _, _> as PersistentSet<_>>::contains(
        &a,
        &"only_a".to_string()
    ));
    assert!(!<HamtSet<_, _, _> as PersistentSet<_>>::contains(
        &a,
        &"only_b".to_string()
    ));
    assert!(<HamtSet<_, _, _> as PersistentSet<_>>::contains(
        &b,
        &"only_b".to_string()
    ));
    assert!(!<HamtSet<_, _, _> as PersistentSet<_>>::contains(
        &b,
        &"only_a".to_string()
    ));
    assert_eq!(base.len(), 1);
}

// ---------------------------------------------------------------------------
// Proptest: HamtMap
// ---------------------------------------------------------------------------

// Note: folio's MemBackend has per-operation I/O overhead.  Each proptest case
// allocates a new store and runs multiple insert/remove ops — keep both case
// count and op count low to stay under ~10s total for this test.
proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(20))]

    #[test]
    fn prop_hamt_map_matches_std_hashmap(
        ops in prop::collection::vec(
            prop_oneof![
                // Insert: key in 0..16, value in 0..100
                (0u64..16, 0u64..100).prop_map(|(k, v)| (true, k, v)),
                // Remove: key in 0..16 (value ignored)
                (0u64..16, 0u64..1).prop_map(|(k, _v)| (false, k, 0u64)),
            ],
            0..20,
        )
    ) {
        let mut reference: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        let mut hamt: HamtMap<u64, u64, PodCodec, MemBackend> = HamtMap::new(make_store());

        for (is_insert, key, value) in ops {
            if is_insert {
                reference.insert(key, value);
                hamt = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&hamt, key, value);
            } else {
                reference.remove(&key);
                let (new_hamt, _) = hamt.remove(&key).expect("remove must succeed");
                hamt = new_hamt;
            }
        }

        // Verify lengths match.
        prop_assert_eq!(hamt.len(), reference.len());

        // Verify all entries in reference are in hamt.
        for (k, v) in &reference {
            // Use the PersistentMap trait (get_cloned is trait-only, returns Option<V>).
            let got = <HamtMap<_, _, _, _> as PersistentMap<u64, u64>>::get_cloned(&hamt, k);
            prop_assert_eq!(got, Some(*v), "key {} expected value {}", k, v);
        }

        // Verify no extra keys in hamt via contains_key for keys 0..16.
        for k in 0u64..16 {
            let in_ref = reference.contains_key(&k);
            let in_hamt =
                <HamtMap<_, _, _, _> as PersistentMap<u64, u64>>::contains_key(&hamt, &k);
            prop_assert_eq!(in_hamt, in_ref, "key {}: hamt={}, ref={}", k, in_hamt, in_ref);
        }
    }
}

proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(20))]

    #[test]
    fn prop_snapshot_isolation(
        base_inserts in prop::collection::vec((0u64..10, 0u64..50), 1..6),
        a_inserts in prop::collection::vec((10u64..20, 0u64..50), 0..6),
        b_inserts in prop::collection::vec((20u64..30, 0u64..50), 0..6),
    ) {
        let mut base: HamtMap<u64, u64, PodCodec, MemBackend> = HamtMap::new(make_store());
        for (k, v) in &base_inserts {
            base = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&base, *k, *v);
        }

        let mut snapshot_a = base.clone();
        let mut snapshot_b = base.clone();

        for (k, v) in &a_inserts {
            snapshot_a = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&snapshot_a, *k, *v);
        }
        for (k, v) in &b_inserts {
            snapshot_b = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&snapshot_b, *k, *v);
        }

        // snapshot_a must not see B-only keys.
        for (k, _) in &b_inserts {
            // Only assert if k is not in base.
            let in_base = base_inserts.iter().any(|(bk, _)| bk == k);
            if !in_base {
                prop_assert_eq!(
                    snapshot_a.get_cloned(k),
                    None,
                    "a should not see B key {}",
                    k
                );
            }
        }

        // snapshot_b must not see A-only keys.
        for (k, _) in &a_inserts {
            let in_base = base_inserts.iter().any(|(bk, _)| bk == k);
            if !in_base {
                prop_assert_eq!(
                    snapshot_b.get_cloned(k),
                    None,
                    "b should not see A key {}",
                    k
                );
            }
        }

        // base is unchanged.
        let base_len: u64 = {
            let mut ref_map: std::collections::HashMap<u64, u64> =
                std::collections::HashMap::new();
            for (k, v) in &base_inserts {
                ref_map.insert(*k, *v);
            }
            ref_map.len() as u64
        };
        prop_assert_eq!(base.len() as u64, base_len);
    }
}

proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(20))]

    #[test]
    fn prop_round_trip_key_lookup(
        kvs in prop::collection::vec((any::<u64>(), any::<u64>()), 0..15),
    ) {
        // Deduplicate to get a canonical key → value mapping.
        let mut canonical: std::collections::HashMap<u64, u64> =
            std::collections::HashMap::new();
        for (k, v) in &kvs {
            canonical.insert(*k, *v);
        }

        let mut m: HamtMap<u64, u64, PodCodec, MemBackend> = HamtMap::new(make_store());
        for (k, v) in &canonical {
            m = <HamtMap<_, _, _, _> as PersistentMap<_, _>>::insert(&m, *k, *v);
        }

        for (k, v) in &canonical {
            let got = m.get_cloned(k);
            prop_assert_eq!(got, Some(*v), "round-trip failed for key {}", k);
        }
    }
}

// ---------------------------------------------------------------------------
// Proptest: HamtSet
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(20))]

    #[test]
    fn prop_hamt_set_matches_std_hashset(
        ops in prop::collection::vec(
            prop_oneof![
                (0u64..16).prop_map(|k| (true, k)),
                (0u64..16).prop_map(|k| (false, k)),
            ],
            0..20,
        )
    ) {
        let mut reference: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut hamt: HamtSet<u64, PodCodec, MemBackend> = HamtSet::new(make_store());

        for (is_insert, key) in ops {
            if is_insert {
                reference.insert(key);
                hamt = <HamtSet<_, _, _> as PersistentSet<_>>::insert(&hamt, key);
            } else {
                reference.remove(&key);
                let (new_hamt, _) = hamt.remove(&key).expect("remove must succeed");
                hamt = new_hamt;
            }
        }

        prop_assert_eq!(hamt.len(), reference.len());

        for k in 0u64..16 {
            let in_ref = reference.contains(&k);
            let in_hamt = <HamtSet<_, _, _> as PersistentSet<_>>::contains(&hamt, &k);
            prop_assert_eq!(in_hamt, in_ref, "key {}: hamt={}, ref={}", k, in_hamt, in_ref);
        }
    }
}
