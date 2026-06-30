// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Integration tests for the `MemOnly` and `Pipelined` storage presets (D.10).
//!
//! All tests are gated on the `tiered` feature.

#![cfg(feature = "tiered")]

use pds_durable::{MemOnlyMap, PipelinedMap, TieredConfig};
use tempfile::TempDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn tmp_path() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pipelined.dat");
    (dir, path)
}

fn pipelined_open(path: &std::path::Path) -> PipelinedMap<String, u64> {
    PipelinedMap::open(path, TieredConfig::default()).unwrap()
}

// ── MemOnly tests ─────────────────────────────────────────────────────────────

/// Insert 10 keys into `MemOnlyMap`, then get all of them.
#[test]
fn mem_only_insert_get() {
    let mut m: MemOnlyMap<String, u64> = MemOnlyMap::new();
    for i in 0u64..10 {
        let prev = m.insert(format!("k{i}"), i * 2);
        assert_eq!(prev, None);
    }
    for i in 0u64..10 {
        assert_eq!(m.get(&format!("k{i}")), Some(&(i * 2)));
    }
    assert_eq!(m.len(), 10);
    assert!(!m.is_empty());
}

/// Insert N into `MemOnlyMap`, freeze via `into_persistent()`, verify all keys present.
#[test]
fn mem_only_into_persistent() {
    const N: usize = 50;
    let mut m: MemOnlyMap<String, u64> = MemOnlyMap::new();
    for i in 0..N {
        m.insert(format!("key{i:04}"), i as u64);
    }
    let persistent = m.into_persistent();
    assert_eq!(persistent.len(), N);
    for i in 0..N {
        assert_eq!(persistent.get(&format!("key{i:04}")), Some(&(i as u64)));
    }
}

/// `MemOnlyMap` default is empty.
#[test]
fn mem_only_default_is_empty() {
    let m: MemOnlyMap<String, u64> = MemOnlyMap::default();
    assert!(m.is_empty());
    assert_eq!(m.len(), 0);
}

/// `MemOnlyMap` remove returns previous value.
#[test]
fn mem_only_remove() {
    let mut m: MemOnlyMap<String, u64> = MemOnlyMap::new();
    m.insert("a".to_string(), 1);
    assert_eq!(m.remove(&"a".to_string()), Some(1));
    assert_eq!(m.get(&"a".to_string()), None);
    assert!(!m.contains_key(&"a".to_string()));
}

/// `MemOnlyMap` contains_key works correctly.
#[test]
fn mem_only_contains_key() {
    let mut m: MemOnlyMap<String, u64> = MemOnlyMap::new();
    assert!(!m.contains_key(&"x".to_string()));
    m.insert("x".to_string(), 42);
    assert!(m.contains_key(&"x".to_string()));
}

// ── Pipelined — crash simulation tests ───────────────────────────────────────

/// Insert N keys, no commit: t0 has the data but t2 does not.
///
/// Demonstrates the data loss window: uncommitted t0 mutations are NOT
/// in the durable t2 store.  A "crash" (drop) at this point would lose them.
/// The MemBackend is in-memory, so we verify the invariant inline.
#[test]
fn pipelined_t0_lost_on_crash() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    for i in 0u64..10 {
        m.insert(format!("k{i}"), i);
    }
    // Keys are in t0 but NOT in t2 — they would be lost in a real crash.
    for i in 0u64..10 {
        assert_eq!(
            m.get(&format!("k{i}")),
            Some(&i),
            "key must be visible in t0"
        );
        let in_t2 = m.get_from_t2(&format!("k{i}")).unwrap();
        assert_eq!(in_t2, None, "key must NOT be in t2 before flush");
    }
    assert_eq!(m.pending_flush(), 10);
}

/// Insert N, commit (t0→t1), no flush: t1 has the data but t2 does not.
///
/// Committed-but-unflushed mutations are in t1 and NOT in t2 — they would be
/// lost in a real crash before `flush()` is called.
#[test]
fn pipelined_t1_lost_on_crash() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    for i in 0u64..10 {
        m.insert(format!("k{i}"), i);
    }
    m.commit();
    // Keys are now in t1 but NOT yet in t2.
    for i in 0u64..10 {
        assert_eq!(m.get(&format!("k{i}")), Some(&i), "key must be in t1");
        let in_t2 = m.get_from_t2(&format!("k{i}")).unwrap();
        assert_eq!(in_t2, None, "key must NOT be in t2 before flush");
    }
    assert_eq!(m.pending_flush(), 10);
    // t0 is now empty (was drained by commit).
    assert_eq!(m.t0().len(), 0);
}

/// Insert N, commit_and_flush: keys are persisted in t2 (durable).
///
/// After `commit_and_flush()`, all keys are in both t1 and t2.  t0 is empty
/// (it was drained by commit), and `dirty` is cleared (it was drained by flush).
/// The MemBackend is in-memory so we verify via `get_from_t2` on the same handle.
#[test]
fn pipelined_flush_survives_crash() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    for i in 0u64..10 {
        m.insert(format!("k{i}"), i);
    }
    let vid = m.commit_and_flush().unwrap();
    // After flush:
    // - t0 is empty (was drained by commit).
    // - t1 holds all keys (was populated by commit).
    // - t2 holds all keys (was populated by flush).
    // - dirty is empty (was drained by flush).
    assert_eq!(m.t0().len(), 0, "t0 must be empty after commit_and_flush");
    assert_eq!(m.pending_flush(), 0, "dirty must be empty after flush");
    assert_eq!(m.pending_commit(), 0, "t0_count must be zero after commit");
    for i in 0u64..10 {
        // Keys accessible via t1 through get().
        assert_eq!(m.get(&format!("k{i}")), Some(&i), "key must be in t1");
        // Keys also durable in t2.
        let v = m
            .get_from_t2(&format!("k{i}"))
            .unwrap()
            .expect("key must be in t2 after flush");
        assert_eq!(v, i);
    }
    // Version recorded at flush time is the current latest.
    assert_eq!(m.latest_version().unwrap().seq, vid.seq);
}

// ── Pipelined — get waterfall tests ──────────────────────────────────────────

/// `get` returns the t0 value when key is only in t0.
#[test]
fn pipelined_get_waterfall_t0_only() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    m.insert("a".to_string(), 1u64);
    // Key is in t0, not committed to t1.
    assert_eq!(m.get(&"a".to_string()), Some(&1));
    assert_eq!(m.t0().get(&"a".to_string()), Some(&1));
    assert!(m.t1().get(&"a".to_string()).is_none());
}

/// `get` returns the t1 value when key is only in t1.
#[test]
fn pipelined_get_waterfall_t1_only() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    m.insert("b".to_string(), 2u64);
    m.commit();
    // After commit, key moves from t0 to t1.
    assert_eq!(m.t0().get(&"b".to_string()), None);
    assert_eq!(m.get(&"b".to_string()), Some(&2));
}

/// `get_from_t2` returns the t2 value; key in t2 via commit_and_flush.
///
/// After `commit_and_flush()`, the key is in t1 (committed) AND t2 (flushed).
/// After inserting a second key to shadow t1 and then manually clearing t1
/// by starting a new PipelinedMap, we can demonstrate t2-only access.
/// For simplicity we just verify that `get_from_t2` returns the correct value.
#[test]
fn pipelined_get_waterfall_t2_only() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    m.insert("c".to_string(), 3u64);
    m.commit_and_flush().unwrap();
    // After commit_and_flush:
    // - t0 is empty.
    // - t1 has the key (from commit).
    // - t2 has the key (from flush).
    assert_eq!(m.t0().get(&"c".to_string()), None, "t0 must be empty");
    assert_eq!(
        m.t1().get(&"c".to_string()),
        Some(&3),
        "t1 holds committed data"
    );
    // get() finds the key in t1.
    assert_eq!(m.get(&"c".to_string()), Some(&3));
    // get_from_t2 also finds the key (it was flushed).
    let v = m.get_from_t2(&"c".to_string()).unwrap();
    assert_eq!(v, Some(3), "key must be in t2 after flush");
}

// ── Pipelined — auto-commit test ──────────────────────────────────────────────

/// With `commit_every = 5`, inserting 15 entries triggers 3 auto-commits.
#[test]
fn pipelined_auto_commit() {
    let (_dir, path) = tmp_path();
    let config = TieredConfig {
        commit_every: 5,
        ..TieredConfig::default()
    };
    let mut m: PipelinedMap<String, u64> = PipelinedMap::open(&path, config).unwrap();

    // After 5 inserts, auto-commit fires: t0 → t1.
    for i in 0u64..5 {
        m.insert(format!("k{i}"), i);
    }
    // t0 should be empty; t1 should have the 5 keys.
    assert_eq!(m.t0().len(), 0, "t0 must be empty after auto-commit");
    assert_eq!(
        m.t1().len(),
        5,
        "t1 must have 5 keys after first auto-commit"
    );

    // Second batch of 5 → second auto-commit.
    for i in 5u64..10 {
        m.insert(format!("k{i}"), i);
    }
    // After the second auto-commit, t1 is replaced with the second batch only.
    assert_eq!(m.t0().len(), 0);
    assert_eq!(m.t1().len(), 5);

    // Third batch.
    for i in 10u64..15 {
        m.insert(format!("k{i}"), i);
    }
    assert_eq!(m.t0().len(), 0);
    assert_eq!(m.t1().len(), 5);
}

// ── Pipelined — version advances test ────────────────────────────────────────

/// `latest_version()` changes after each `flush()`.
#[test]
fn pipelined_version_advances() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);
    let v0 = m.latest_version().unwrap();

    m.insert("a".to_string(), 1u64);
    m.commit_and_flush().unwrap();
    let v1 = m.latest_version().unwrap();
    assert!(v1.seq > v0.seq, "version must advance after first flush");

    m.insert("b".to_string(), 2u64);
    m.commit_and_flush().unwrap();
    let v2 = m.latest_version().unwrap();
    assert!(v2.seq > v1.seq, "version must advance after second flush");
}

// ── Pipelined — round-trip test ───────────────────────────────────────────────

/// Insert N key-value pairs, commit_and_flush, verify all N are in t2.
#[test]
fn pipelined_round_trip() {
    const N: usize = 100;
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);

    for i in 0..N {
        m.insert(format!("key{i:04}"), i as u64);
    }
    m.commit_and_flush().unwrap();

    // All keys must be in t2.
    for i in 0..N {
        let v = m
            .get_from_t2(&format!("key{i:04}"))
            .unwrap()
            .expect("key must be in t2");
        assert_eq!(v, i as u64);
    }
}

/// pending_commit and pending_flush track state correctly.
#[test]
fn pipelined_pending_counts() {
    let (_dir, path) = tmp_path();
    let mut m = pipelined_open(&path);

    assert_eq!(m.pending_commit(), 0);
    assert_eq!(m.pending_flush(), 0);

    m.insert("a".to_string(), 1u64);
    m.insert("b".to_string(), 2u64);
    assert_eq!(m.pending_commit(), 2);
    assert_eq!(m.pending_flush(), 2); // dirty tracks all inserted keys

    m.commit();
    assert_eq!(m.pending_commit(), 0); // t0 was drained
    assert_eq!(m.pending_flush(), 2); // dirty not yet flushed

    m.flush().unwrap();
    assert_eq!(m.pending_commit(), 0);
    assert_eq!(m.pending_flush(), 0);
}
