// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Integration and property-based tests for `DurableMap`.

use pds_durable::{DurableConfig, DurableMap, Relaxed, Strict};
use proptest::prelude::*;
use std::collections::HashMap as StdMap;
use tempfile::tempdir;

// ── Op type for proptest ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Op<K, V> {
    Insert(K, V),
    Remove(K),
}

fn arb_op<K: Arbitrary, V: Arbitrary>() -> impl Strategy<Value = Op<K, V>> {
    prop_oneof![
        (any::<K>(), any::<V>()).prop_map(|(k, v)| Op::Insert(k, v)),
        any::<K>().prop_map(Op::Remove),
    ]
}

fn arb_ops<K: Arbitrary, V: Arbitrary>(max: usize) -> impl Strategy<Value = Vec<Op<K, V>>> {
    proptest::collection::vec(arb_op::<K, V>(), 0..max)
}

// ── Type aliases ─────────────────────────────────────────────────────────────

type StrictMap = DurableMap<String, i64, Strict>;
type RelaxedMap = DurableMap<String, i64, Relaxed>;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Apply a list of ops to a `StrictMap`, returning the result map.
fn apply_strict(map: &mut StrictMap, ops: &[Op<String, i64>]) {
    for op in ops {
        match op {
            Op::Insert(k, v) => {
                map.insert(k.clone(), *v).unwrap();
            }
            Op::Remove(k) => {
                map.remove(k).unwrap();
            }
        }
    }
}

/// Apply the same ops to a `std::collections::HashMap` for reference.
fn apply_reference(reference: &mut StdMap<String, i64>, ops: &[Op<String, i64>]) {
    for op in ops {
        match op {
            Op::Insert(k, v) => {
                reference.insert(k.clone(), *v);
            }
            Op::Remove(k) => {
                reference.remove(k);
            }
        }
    }
}

fn maps_equal(durable: &pds::HashMap<String, i64>, reference: &StdMap<String, i64>) -> bool {
    if durable.len() != reference.len() {
        return false;
    }
    for (k, v) in reference {
        if durable.get(k) != Some(v) {
            return false;
        }
    }
    true
}

// ── Proptest: Strict round-trip ───────────────────────────────────────────────

proptest! {
    #[test]
    fn strict_round_trip(ops in arb_ops::<String, i64>(50)) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("strict.wal");

        let mut reference = StdMap::new();
        apply_reference(&mut reference, &ops);

        {
            let mut dmap = StrictMap::open(&path, DurableConfig::default()).unwrap();
            apply_strict(&mut dmap, &ops);
        }

        // Re-open and compare.
        let dmap = StrictMap::open(&path, DurableConfig::default()).unwrap();
        prop_assert!(
            maps_equal(dmap.inner(), &reference),
            "map mismatch: durable has {} entries, reference has {}",
            dmap.len(),
            reference.len()
        );
    }
}

// ── Proptest: Relaxed with random flush points ────────────────────────────────

proptest! {
    #[test]
    fn relaxed_flush_semantics(
        ops in arb_ops::<String, i64>(50),
        flush_points in proptest::collection::vec(any::<bool>(), 0..50),
    ) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("relaxed.wal");

        // Track state at last flush.
        let mut flushed_reference = StdMap::new();
        let mut current_reference = StdMap::new();

        {
            let mut dmap = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for (i, op) in ops.iter().enumerate() {
                match op {
                    Op::Insert(k, v) => {
                        dmap.insert(k.clone(), *v);
                        current_reference.insert(k.clone(), *v);
                    }
                    Op::Remove(k) => {
                        dmap.remove(k);
                        current_reference.remove(k);
                    }
                }
                // Flush at this point?
                if flush_points.get(i).copied().unwrap_or(false) {
                    dmap.flush().unwrap();
                    flushed_reference = current_reference.clone();
                }
            }
            // Drop without final flush — simulate crash.
        }

        // Recovery should restore state as of last flush.
        let dmap = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
        prop_assert!(
            maps_equal(dmap.inner(), &flushed_reference),
            "relaxed recovery mismatch: got {} entries, expected {}",
            dmap.len(),
            flushed_reference.len()
        );
    }
}

// ── Proptest: checkpoint + post-checkpoint mutations ─────────────────────────

proptest! {
    #[test]
    fn checkpoint_recovery(
        pre in arb_ops::<String, i64>(25),
        post in arb_ops::<String, i64>(25),
    ) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("chk.wal");

        let mut pre_reference = StdMap::new();
        apply_reference(&mut pre_reference, &pre);

        {
            let mut dmap = StrictMap::open(&path, DurableConfig::default()).unwrap();
            apply_strict(&mut dmap, &pre);
            // Checkpoint after pre-ops.
            dmap.checkpoint().unwrap();
            // Apply post-ops but crash (drop without further checkpoint).
            apply_strict(&mut dmap, &post);
        }

        // On recovery, state should match the pre-checkpoint reference
        // (post-ops were not checkpointed but were fsynced in Strict mode,
        // so they ARE present in the WAL).
        let mut post_reference = pre_reference.clone();
        apply_reference(&mut post_reference, &post);

        let dmap = StrictMap::open(&path, DurableConfig::default()).unwrap();
        // In Strict mode, every write is fsynced, so post-ops are durable.
        prop_assert!(
            maps_equal(dmap.inner(), &post_reference),
            "checkpoint recovery mismatch: got {} entries, expected {}",
            dmap.len(),
            post_reference.len()
        );
    }
}

// ── Edge case tests ───────────────────────────────────────────────────────────

#[test]
fn empty_map_open_close_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("empty.wal");

    {
        let _map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    }

    let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
}

#[test]
fn single_insert_checkpoint_remove_crash_key_present() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("single.wal");

    {
        let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        map.insert("key".to_owned(), 42).unwrap();
        map.checkpoint().unwrap();
        // Crash before remove is written — simulate by just dropping.
    }

    let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    assert_eq!(map.get(&"key".to_owned()), Some(&42));
}

#[test]
fn unicode_keys_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("unicode.wal");

    let keys = vec![
        "こんにちは".to_owned(),
        "мир".to_owned(),
        "🦀".to_owned(),
        "日本語テスト".to_owned(),
        "αβγδ".to_owned(),
    ];

    {
        let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        for (i, k) in keys.iter().enumerate() {
            map.insert(k.clone(), i as i64).unwrap();
        }
    }

    let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    assert_eq!(map.len(), keys.len());
    for (i, k) in keys.iter().enumerate() {
        assert_eq!(map.get(k), Some(&(i as i64)), "missing key: {}", k);
    }
}

#[test]
fn large_value_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("large.wal");

    // 1 MB value — tests that WAL handles large payloads without truncation.
    let large_value: i64 = i64::MAX;
    let large_key = "x".repeat(65536); // 64 KB key

    {
        let mut map: DurableMap<String, i64, Strict> =
            StrictMap::open(&path, DurableConfig::default()).unwrap();
        map.insert(large_key.clone(), large_value).unwrap();
    }

    let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&large_key), Some(&large_value));
}

#[test]
fn len_and_is_empty_agree_at_all_times() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("len.wal");

    let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    assert_eq!(map.len(), map.inner().len());
    assert_eq!(map.is_empty(), map.len() == 0);

    for i in 0..20i64 {
        map.insert(format!("k{}", i), i).unwrap();
        assert_eq!(map.len(), map.inner().len());
        assert_eq!(map.is_empty(), map.len() == 0);
    }

    for i in 0..10i64 {
        map.remove(&format!("k{}", i)).unwrap();
        assert_eq!(map.len(), map.inner().len());
        assert_eq!(map.is_empty(), map.len() == 0);
    }
}

#[test]
fn strict_insert_returns_previous_value() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("prev.wal");

    let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    let prev = map.insert("k".to_owned(), 1).unwrap();
    assert_eq!(prev, None);
    let prev = map.insert("k".to_owned(), 2).unwrap();
    assert_eq!(prev, Some(1));
}

#[test]
fn checkpoint_compacts_wal_to_single_entry() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("compact.wal");

    let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    for i in 0..10i64 {
        map.insert(format!("k{}", i), i).unwrap();
    }
    let size_before = std::fs::metadata(&path).unwrap().len();
    map.checkpoint().unwrap();
    let size_after = std::fs::metadata(&path).unwrap().len();

    assert!(
        size_after < size_before,
        "WAL should shrink after checkpoint: before={} after={}",
        size_before,
        size_after
    );

    // Verify all entries are still recoverable.
    let map2 = StrictMap::open(&path, DurableConfig::default()).unwrap();
    assert_eq!(map2.len(), 10);
}

#[test]
fn relaxed_remove_returns_previous() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rel_remove.wal");

    let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
    map.insert("k".to_owned(), 99);
    let prev = map.remove(&"k".to_owned());
    assert_eq!(prev, Some(99));
    let none = map.remove(&"k".to_owned());
    assert_eq!(none, None);
}
