//! Integration tests for `FolioOrdMap`, `FolioOrdSet`, and `FolioVector`.
//!
//! Covers:
//! - `FolioOrdMap` range query correctness with many keys
//! - `FolioVector` concat/split round-trips
//! - Snapshot isolation across both types
//! - `FolioOrdSet` ordered traversal
//! - Proptest: Vector concat/split inverse; OrdMap sorted order invariant

use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds_folio::{
    btree::BTREE_ORDER, codec::PostcardCodec, folio_ordmap::FolioOrdMap, folio_ordset::FolioOrdSet,
    folio_vector::FolioVector, vector::BRANCHING_FACTOR,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_store() -> FolioStore<MemBackend> {
    let backend = MemBackend::new(4096, 512);
    FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
        .expect("store creation must succeed")
}

type TMap = FolioOrdMap<u32, u32, PostcardCodec, MemBackend>;
type TSet = FolioOrdSet<u32, PostcardCodec, MemBackend>;
type TVec = FolioVector<u32, PostcardCodec, MemBackend>;

fn empty_map() -> TMap {
    FolioOrdMap::new(make_store())
}

fn empty_set() -> TSet {
    FolioOrdSet::new(make_store())
}

fn empty_vec() -> TVec {
    FolioVector::new(make_store())
}

// ---------------------------------------------------------------------------
// FolioOrdMap — deterministic integration tests
// ---------------------------------------------------------------------------

#[test]
fn ordmap_insert_many_and_verify_all() {
    let n = 2 * BTREE_ORDER + 5; // force multiple levels of splits
    let mut m = empty_map();
    for i in 0..n {
        m = m.insert(i as u32, (i as u32) * 10).unwrap();
    }
    assert_eq!(m.len(), n);
    for i in 0..n {
        assert_eq!(
            m.get(&(i as u32)).unwrap(),
            Some((i as u32) * 10),
            "key {i} missing"
        );
    }
}

#[test]
fn ordmap_insert_reverse_order() {
    let n = BTREE_ORDER + 5;
    let mut m = empty_map();
    for i in (0..n).rev() {
        m = m.insert(i as u32, i as u32).unwrap();
    }
    assert_eq!(m.len(), n);
    // Sorted iteration must still be in ascending order.
    let pairs = m.iter().unwrap();
    assert_eq!(pairs.len(), n);
    for (i, (k, _v)) in pairs.iter().enumerate() {
        assert_eq!(*k, i as u32, "key at index {i} out of order");
    }
}

#[test]
fn ordmap_range_query_across_leaf_boundary() {
    let n = BTREE_ORDER + 10;
    let mut m = empty_map();
    for i in 0..n {
        m = m.insert(i as u32, (i as u32) * 2).unwrap();
    }
    // Range that spans the leaf split boundary.
    let start = (BTREE_ORDER / 2) as u32;
    let end = (BTREE_ORDER + 5) as u32;
    let pairs = m.range(start..=end).unwrap();
    assert_eq!(pairs.len(), (end - start + 1) as usize);
    for (i, (k, v)) in pairs.iter().enumerate() {
        let expected_k = start + i as u32;
        assert_eq!(*k, expected_k);
        assert_eq!(*v, expected_k * 2);
    }
}

#[test]
fn ordmap_remove_half_and_verify_remaining() {
    let n = BTREE_ORDER + 5;
    let mut m = empty_map();
    for i in 0..n {
        m = m.insert(i as u32, i as u32).unwrap();
    }
    // Remove all even keys.
    for i in (0..n).filter(|x| x % 2 == 0) {
        let (new_m, evicted) = m.remove(&(i as u32)).unwrap();
        assert_eq!(
            evicted,
            Some(i as u32),
            "evicted value mismatch for key {i}"
        );
        m = new_m;
    }
    let expected_len = n - (n + 1) / 2;
    assert_eq!(m.len(), expected_len);
    for i in 0..n {
        let expected = if i % 2 == 1 { Some(i as u32) } else { None };
        assert_eq!(m.get(&(i as u32)).unwrap(), expected, "key {i}");
    }
}

#[test]
fn ordmap_snapshot_isolation_insert_does_not_affect_original() {
    let n = BTREE_ORDER / 2;
    let mut m0 = empty_map();
    for i in 0..n {
        m0 = m0.insert(i as u32, i as u32).unwrap();
    }
    // m1 inserts additional keys.
    let mut m1 = m0.clone();
    for i in n..2 * n {
        m1 = m1.insert(i as u32, i as u32).unwrap();
    }
    // m0 must be unchanged.
    assert_eq!(m0.len(), n);
    for i in n..2 * n {
        assert_eq!(m0.get(&(i as u32)).unwrap(), None);
    }
    // m1 has all keys.
    assert_eq!(m1.len(), 2 * n);
}

#[test]
fn ordmap_snapshot_isolation_remove_does_not_affect_original() {
    let n = BTREE_ORDER / 2;
    let mut m0 = empty_map();
    for i in 0..n {
        m0 = m0.insert(i as u32, i as u32).unwrap();
    }
    // m1 removes all keys.
    let mut m1 = m0.clone();
    for i in 0..n {
        let (new_m, _) = m1.remove(&(i as u32)).unwrap();
        m1 = new_m;
    }
    // m0 still has all keys.
    assert_eq!(m0.len(), n);
    for i in 0..n {
        assert_eq!(m0.get(&(i as u32)).unwrap(), Some(i as u32));
    }
    // m1 is empty.
    assert_eq!(m1.len(), 0);
}

// ---------------------------------------------------------------------------
// FolioOrdSet — deterministic integration tests
// ---------------------------------------------------------------------------

#[test]
fn ordset_insert_many_and_verify_sorted() {
    let n = BTREE_ORDER + 5;
    let mut s = empty_set();
    for i in (0..n).rev() {
        s = s.insert(i as u32).unwrap();
    }
    assert_eq!(s.len(), n);
    let elems = s.iter().unwrap();
    assert_eq!(elems.len(), n);
    for (i, &e) in elems.iter().enumerate() {
        assert_eq!(e, i as u32);
    }
}

#[test]
fn ordset_range_query() {
    let n = BTREE_ORDER + 5;
    let mut s = empty_set();
    for i in 0..n {
        s = s.insert(i as u32).unwrap();
    }
    let mid = (n / 2) as u32;
    let elems = s.range(mid..mid + 5).unwrap();
    assert_eq!(elems, vec![mid, mid + 1, mid + 2, mid + 3, mid + 4]);
}

// ---------------------------------------------------------------------------
// FolioVector — deterministic integration tests
// ---------------------------------------------------------------------------

#[test]
fn vector_concat_split_round_trip_small() {
    let v = (0..5u32).fold(empty_vec(), |acc, i| acc.push_back(i).unwrap());
    let (left, right) = v.split_at(3).unwrap();
    assert_eq!(left.len(), 3);
    assert_eq!(right.len(), 2);
    let merged = left.concat(&right).unwrap();
    assert_eq!(merged.len(), 5);
    for i in 0..5u32 {
        assert_eq!(merged.get(i as usize).unwrap(), Some(i));
    }
}

#[test]
fn vector_concat_split_round_trip_cross_boundary() {
    let n = BRANCHING_FACTOR + 5;
    let v = (0..n as u32).fold(empty_vec(), |acc, i| acc.push_back(i).unwrap());
    let mid = n / 2;
    let (left, right) = v.split_at(mid).unwrap();
    assert_eq!(left.len(), mid);
    assert_eq!(right.len(), n - mid);
    let merged = left.concat(&right).unwrap();
    assert_eq!(merged.len(), n);
    for i in 0..n {
        assert_eq!(merged.get(i).unwrap(), Some(i as u32));
    }
}

#[test]
fn vector_snapshot_isolation() {
    let v0 = (0..5u32).fold(empty_vec(), |acc, i| acc.push_back(i).unwrap());
    let v1 = v0.push_back(5u32).unwrap();
    assert_eq!(v0.len(), 5);
    assert_eq!(v0.get(5).unwrap(), None);
    assert_eq!(v1.len(), 6);
    assert_eq!(v1.get(5).unwrap(), Some(5));
}

#[test]
fn vector_large_n_push_and_get() {
    let n = BRANCHING_FACTOR * BRANCHING_FACTOR / 2;
    let mut v = empty_vec();
    for i in 0..n {
        v = v.push_back(i as u32).unwrap();
    }
    assert_eq!(v.len(), n);
    for i in 0..n {
        assert_eq!(v.get(i).unwrap(), Some(i as u32), "index {i}");
    }
}

// ---------------------------------------------------------------------------
// Proptest: FolioVector concat/split inverse
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 20, ..Default::default() })]

    #[test]
    fn prop_vector_concat_split_inverse(
        left_elems in proptest::collection::vec(0u32..1000, 0..=16usize),
        right_elems in proptest::collection::vec(0u32..1000, 0..=16usize),
    ) {
        let left = left_elems.iter().fold(empty_vec(), |acc, &x| acc.push_back(x).unwrap());
        let right = right_elems.iter().fold(empty_vec(), |acc, &x| acc.push_back(x).unwrap());
        let merged = left.concat(&right).unwrap();
        let (split_l, split_r) = merged.split_at(left.len()).unwrap();

        prop_assert_eq!(split_l.len(), left.len());
        prop_assert_eq!(split_r.len(), right.len());
        for (i, &expected) in left_elems.iter().enumerate() {
            prop_assert_eq!(split_l.get(i).unwrap(), Some(expected));
        }
        for (i, &expected) in right_elems.iter().enumerate() {
            prop_assert_eq!(split_r.get(i).unwrap(), Some(expected));
        }
    }

    #[test]
    fn prop_ordmap_sorted_order_invariant(
        keys in proptest::collection::vec(0u32..200, 0..=20usize),
    ) {
        let mut m = empty_map();
        for &k in &keys {
            m = m.insert(k, k * 2).unwrap();
        }
        let pairs = m.iter().unwrap();
        // Must be in strictly ascending key order (duplicates eliminated by insert).
        for w in pairs.windows(2) {
            prop_assert!(w[0].0 < w[1].0, "order violation: {:?} >= {:?}", w[0].0, w[1].0);
        }
        // All unique keys from input must be present with correct values.
        let mut unique_keys: Vec<u32> = keys.clone();
        unique_keys.sort_unstable();
        unique_keys.dedup();
        prop_assert_eq!(pairs.len(), unique_keys.len());
        for &k in &unique_keys {
            prop_assert_eq!(m.get(&k).unwrap(), Some(k * 2));
        }
    }

    #[test]
    fn prop_ordmap_range_matches_full_iter_filtered(
        keys in proptest::collection::vec(0u32..200, 0..=20usize),
        range_start in 0u32..200,
        range_end in 0u32..200,
    ) {
        let mut m = empty_map();
        for &k in &keys {
            m = m.insert(k, k).unwrap();
        }
        let (lo, hi) = if range_start <= range_end {
            (range_start, range_end)
        } else {
            (range_end, range_start)
        };
        let range_pairs = m.range(lo..=hi).unwrap();
        let all_pairs = m.iter().unwrap();
        let expected: Vec<_> = all_pairs.into_iter().filter(|(k, _)| *k >= lo && *k <= hi).collect();
        prop_assert_eq!(range_pairs, expected);
    }
}
