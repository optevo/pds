//! Integration tests for `VersionedHamt`.
//!
//! Covers:
//! - Large mutation sequences with historical access verification
//! - Structural diff correctness across non-adjacent versions
//! - Snapshot isolation: mutations on clones do not affect originals
//! - Merkle proof round-trips at current and historical versions
//! - Cross-crate: `VersionedHamt` used via `PersistentMap`, `VersionedPersistentMap`,
//!   and `MerklePersistentMap` traits
//! - Proptest: mutation sequence → historical values correct at every version

use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds_folio::codec::PostcardCodec;
use pds_merkle_spine::{VersionId, VersionedHamt};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_store() -> FolioStore<MemBackend> {
    let backend = MemBackend::new(4096, 512);
    FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
        .expect("store creation must succeed")
}

type TMap = VersionedHamt<u32, u64, PostcardCodec, MemBackend>;

fn empty_map() -> TMap {
    VersionedHamt::new(make_store())
}

// ---------------------------------------------------------------------------
// Large mutation sequences
// ---------------------------------------------------------------------------

#[test]
fn large_insert_sequence_all_versions_correct() {
    let n = 50usize;
    let mut map = empty_map();
    let mut versions: Vec<(VersionId, Vec<(u32, u64)>)> = vec![(map.version(), vec![])];
    let mut current_entries: Vec<(u32, u64)> = Vec::new();

    for i in 0..n as u32 {
        let v = u64::from(i) * 100;
        map = map.insert(i, v).expect("insert failed");
        current_entries.push((i, v));
        versions.push((map.version(), current_entries.clone()));
    }

    // Verify each historical version.
    for (version, entries) in &versions {
        for (k, expected_v) in entries {
            let got = map.get_at(*version, k).expect("get_at failed");
            assert_eq!(
                got,
                Some(*expected_v),
                "key {k} at version {:?}",
                version.seq
            );
        }
        // Keys inserted after this version must be absent.
        let max_k = entries.len() as u32;
        if max_k < n as u32 {
            let got = map.get_at(*version, &max_k).expect("get_at failed");
            assert_eq!(
                got, None,
                "key {max_k} should be absent at version {:?}",
                version.seq
            );
        }
    }
}

#[test]
fn remove_sequence_all_versions_correct() {
    let n = 20usize;
    let mut map = empty_map();
    // Insert all keys.
    for i in 0..n as u32 {
        map = map.insert(i, u64::from(i)).expect("insert failed");
    }
    let full_version = map.version();

    // Remove half the keys and verify the intermediate version.
    for i in (0..n as u32).filter(|x| x % 2 == 0) {
        let (new_map, evicted) = map.remove(&i).expect("remove failed");
        assert_eq!(evicted, Some(u64::from(i)));
        map = new_map;
    }

    // All even keys must be absent in current version.
    for i in (0..n as u32).filter(|x| x % 2 == 0) {
        assert_eq!(map.get(&i).unwrap(), None, "key {i} should be absent");
    }
    // All odd keys must still be present.
    for i in (0..n as u32).filter(|x| x % 2 == 1) {
        assert_eq!(map.get(&i).unwrap(), Some(u64::from(i)));
    }

    // Full version must still have all keys.
    for i in 0..n as u32 {
        let got = map.get_at(full_version, &i).unwrap();
        assert_eq!(got, Some(u64::from(i)), "key {i} at full_version");
    }
}

// ---------------------------------------------------------------------------
// Structural diff across non-adjacent versions
// ---------------------------------------------------------------------------

#[test]
fn diff_non_adjacent_versions() {
    let mut m = empty_map();
    let v0 = m.version();

    // Insert 5 keys.
    for i in 0u32..5 {
        m = m.insert(i, u64::from(i)).unwrap();
    }
    let v5 = m.version();

    // Remove key 2, update key 3.
    let (m2, _) = m.remove(&2u32).unwrap();
    let m3 = m2.insert(3u32, 99u64).unwrap();
    let v7 = m3.version();

    // Diff from v0 to v5: all 5 are insertions.
    let d05 = m3.diff(v0, v5).unwrap();
    assert_eq!(d05.len(), 5);
    assert!(d05.iter().all(|e| matches!(
        e,
        pds_merkle_spine::versioned_hamt::DiffEntry::Inserted { .. }
    )));

    // Diff from v5 to v7: one removal, one update.
    let d57 = m3.diff(v5, v7).unwrap();
    // key 2 removed; key 3 updated; keys 0, 1, 4 unchanged.
    let removed: Vec<_> = d57
        .iter()
        .filter(|e| {
            matches!(
                e,
                pds_merkle_spine::versioned_hamt::DiffEntry::Removed { .. }
            )
        })
        .collect();
    let updated: Vec<_> = d57
        .iter()
        .filter(|e| {
            matches!(
                e,
                pds_merkle_spine::versioned_hamt::DiffEntry::Updated { .. }
            )
        })
        .collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(updated.len(), 1);
}

// ---------------------------------------------------------------------------
// Snapshot isolation
// ---------------------------------------------------------------------------

#[test]
fn clone_is_independent_snapshot() {
    let m0 = empty_map().insert(1u32, 10u64).unwrap();
    let m0_clone = m0.clone();

    // Mutate the clone.
    let m1 = m0_clone.insert(2u32, 20u64).unwrap();

    // Original must be unchanged.
    assert_eq!(m0.len(), 1);
    assert_eq!(m0.get(&2u32).unwrap(), None);

    // Clone has both keys.
    assert_eq!(m1.len(), 2);
    assert_eq!(m1.get(&2u32).unwrap(), Some(20));
}

#[test]
fn checkout_branch_is_independent() {
    let m0 = empty_map();
    let v0 = m0.version();
    let m1 = m0.insert(10u32, 100u64).unwrap();

    // Check out v0 and create a different branch.
    let branch = m1.checkout(v0).unwrap().unwrap();
    let branch2 = branch.insert(20u32, 200u64).unwrap();

    // m1 is unaffected.
    assert_eq!(m1.get(&20u32).unwrap(), None);
    assert_eq!(m1.len(), 1);

    // branch2 has only key 20.
    assert_eq!(branch2.len(), 1);
    assert_eq!(branch2.get(&10u32).unwrap(), None);
    assert_eq!(branch2.get(&20u32).unwrap(), Some(200));
}

// ---------------------------------------------------------------------------
// Merkle proof round-trips
// ---------------------------------------------------------------------------

#[test]
fn merkle_proof_for_all_keys() {
    let keys: Vec<u32> = (0..10).collect();
    let mut m = empty_map();
    for &k in &keys {
        m = m.insert(k, u64::from(k) * 7).unwrap();
    }

    let root = m.root_hash();
    for &k in &keys {
        let proof = m.prove_inclusion(&k).unwrap().unwrap();
        assert_eq!(proof.root_hash, root);
        let valid = VersionedHamt::<u32, u64, PostcardCodec, MemBackend>::verify_proof(
            &root,
            &k,
            &(u64::from(k) * 7),
            &proof,
        )
        .unwrap();
        assert!(valid, "proof verification failed for key {k}");
    }
}

#[test]
fn merkle_proof_at_historical_version() {
    let m0 = empty_map().insert(1u32, 10u64).unwrap();
    let v1 = m0.version();
    let root1 = m0.root_hash();

    let m1 = m0.insert(2u32, 20u64).unwrap();

    // Proof at v1 for key 1.
    let proof = m1.prove_inclusion_at(v1, &1u32).unwrap().unwrap();
    assert_eq!(proof.root_hash, root1);
    let valid = VersionedHamt::<u32, u64, PostcardCodec, MemBackend>::verify_proof(
        &root1, &1u32, &10u64, &proof,
    )
    .unwrap();
    assert!(valid);
}

// ---------------------------------------------------------------------------
// Cross-crate trait usage
// ---------------------------------------------------------------------------

#[test]
fn versioned_persistent_map_via_trait() {
    fn exercise<M: pds::traits::VersionedPersistentMap<u32, u64>>(empty: M) {
        let v0 = empty.version();
        let m1 = empty.insert(1u32, 100u64);
        let m2 = m1.insert(2u32, 200u64);

        assert_eq!(m2.get_cloned(&1u32), Some(100));
        assert_eq!(m2.get_cloned(&2u32), Some(200));
        assert_eq!(m2.get_at(v0, &1u32), None);

        let m3 = m2.remove(&1u32).0;
        assert_eq!(m3.get_cloned(&1u32), None);
        assert_eq!(m3.get_cloned(&2u32), Some(200));
    }
    exercise(empty_map());
}

#[test]
fn merkle_persistent_map_via_trait() {
    fn exercise<M: pds::traits::MerklePersistentMap<u32, u64>>(empty: M) {
        let m = empty.insert(42u32, 1000u64);
        let rh = m.root_hash();

        let proof = m.prove_inclusion(&42u32).unwrap();
        assert!(M::verify_proof(&rh, &42u32, &1000u64, &proof));

        // Non-existent key has no proof.
        assert!(m.prove_inclusion(&99u32).is_none());
    }
    exercise(empty_map());
}

// ---------------------------------------------------------------------------
// Proptest: mutation sequence correctness
// ---------------------------------------------------------------------------

/// A single operation in a mutation sequence.
#[derive(Debug, Clone)]
enum Op {
    /// Insert key → value.
    Insert(u32, u64),
    /// Remove key.
    Remove(u32),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u32..20, 0u64..100).prop_map(|(k, v)| Op::Insert(k, v)),
        (0u32..20).prop_map(Op::Remove),
    ]
}

proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 20, ..Default::default() })]

    #[test]
    fn prop_historical_values_correct(
        ops in proptest::collection::vec(arb_op(), 0..=30usize),
    ) {
        let mut map = empty_map();
        // Reference model: HashMap tracking expected state.
        let mut reference: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        // Snapshots: (VersionId, reference_at_that_point)
        let mut snapshots: Vec<(VersionId, std::collections::HashMap<u32, u64>)> =
            vec![(map.version(), std::collections::HashMap::new())];

        for op in &ops {
            match op {
                Op::Insert(k, v) => {
                    map = map.insert(*k, *v).unwrap();
                    reference.insert(*k, *v);
                }
                Op::Remove(k) => {
                    let (new_map, _) = map.remove(k).unwrap();
                    map = new_map;
                    reference.remove(k);
                }
            }
            snapshots.push((map.version(), reference.clone()));
        }

        // Verify historical access for every (version, key) pair.
        for (version, ref_state) in &snapshots {
            // Check all keys in ref_state.
            for (k, expected_v) in ref_state {
                let got = map.get_at(*version, k).unwrap();
                prop_assert_eq!(got, Some(*expected_v), "key {} at version {:?}", k, version.seq);
            }
            // Spot-check a key that should be absent (key = u32::MAX never inserted).
            let sentinel = 19u32 + 1; // just outside key range
            if !ref_state.contains_key(&sentinel) {
                let got = map.get_at(*version, &sentinel).unwrap();
                prop_assert_eq!(got, None);
            }
        }
    }

    #[test]
    fn prop_diff_inverse_of_mutations(
        keys_a in proptest::collection::vec(0u32..15, 0..=10usize),
        keys_b in proptest::collection::vec(0u32..15, 0..=10usize),
    ) {
        // Build map A.
        let mut ma = empty_map();
        for &k in &keys_a {
            ma = ma.insert(k, u64::from(k)).unwrap();
        }
        let va = ma.version();

        // Build map B by starting from ma and inserting keys_b.
        let mut mb = ma.clone();
        for &k in &keys_b {
            mb = mb.insert(k, u64::from(k) + 100).unwrap();
        }
        let vb = mb.version();

        // Compute diff a→b.
        let diff = mb.diff(va, vb).unwrap();

        // Every key in diff must be actually different.
        for entry in &diff {
            let key = match entry {
                pds_merkle_spine::versioned_hamt::DiffEntry::Inserted { key, .. } => key,
                pds_merkle_spine::versioned_hamt::DiffEntry::Removed { key, .. } => key,
                pds_merkle_spine::versioned_hamt::DiffEntry::Updated { key, .. } => key,
            };
            let in_a = mb.get_at(va, key).unwrap();
            let in_b = mb.get_at(vb, key).unwrap();
            prop_assert!(in_a != in_b, "key {key}: diff entry but values identical");
        }
    }
}
