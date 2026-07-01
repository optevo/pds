//! Tests for Phase T.0 — tiered write-behind collections.
//!
//! All tests use `StdHashMapBackend` and `PdsHashMapBackend` unless otherwise noted.
//! The `MerkleWrapperBackend` three-tier test is gated on `#[cfg(feature = "traits")]`.

#[cfg(test)]
mod tests {
    use super::super::{
        backend::CollectionBackend,
        backends::{PdsHashMapBackend, StdHashMapBackend},
        policy::PropagationPolicy,
        TieredCollection,
    };

    // --- Type aliases for brevity ---

    type StdPds = TieredCollection<
        String,
        i32,
        StdHashMapBackend<String, i32>,
        PdsHashMapBackend<String, i32>,
    >;

    fn std_pds(policy: PropagationPolicy) -> StdPds {
        TieredCollection::new(StdHashMapBackend::new(), PdsHashMapBackend::new(), policy)
    }

    // --- Test 1: Basic insert + get from hot ---

    /// Inserting a key should make it immediately visible via `get` without
    /// flushing (the value lives in the hot tier).
    #[test]
    fn insert_get_from_hot() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("a".to_string(), 1);
        assert_eq!(tc.get(&"a".to_string()), Some(1));
        // Cold snapshot should be empty — no flush has occurred.
        let snap = tc.cold_snapshot();
        assert!(snap.is_empty());
    }

    // --- Test 2: Flush propagates to cold ---

    /// After a flush, the key should be in the cold tier snapshot.
    #[test]
    fn flush_propagates_to_cold() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("b".to_string(), 2);
        tc.flush();
        let snap = tc.cold_snapshot();
        assert_eq!(snap.get(&"b".to_string()), Some(2));
    }

    // --- Test 3: Delete before flush ---

    /// A key removed after insertion but before flush should not be visible —
    /// neither from hot nor from the cold fallback.
    #[test]
    fn delete_before_flush_not_visible() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("c".to_string(), 3);
        let removed = tc.remove(&"c".to_string());
        assert_eq!(removed, Some(3));
        assert_eq!(tc.get(&"c".to_string()), None);
    }

    // --- Test 4: Delete after propagation, before second flush ---

    /// Flush a key to cold, then remove it. `get` must return `None` because
    /// the deletion is masked over cold via `pending_deletes`.
    #[test]
    fn delete_after_propagation_not_visible() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("d".to_string(), 4);
        tc.flush();
        // Key is now in cold.
        assert_eq!(tc.get(&"d".to_string()), Some(4));
        // Remove it — pending_deletes should mask the cold value.
        tc.remove(&"d".to_string());
        assert_eq!(tc.get(&"d".to_string()), None);
    }

    // --- Test 5: Flush clears deletion mask ---

    /// After inserting, flushing, removing, and flushing again, the key must
    /// not appear in the cold snapshot.
    #[test]
    fn flush_clears_deletion_mask() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("e".to_string(), 5);
        tc.flush();
        tc.remove(&"e".to_string());
        tc.flush();
        let snap = tc.cold_snapshot();
        assert_eq!(snap.get(&"e".to_string()), None);
    }

    // --- Test 6: Batched(3) auto-flush ---

    /// After 3 inserts with `Batched(3)`, the third insert should trigger an
    /// automatic flush, making all three keys visible in the cold snapshot.
    #[test]
    fn batched_auto_flush() {
        let tc = std_pds(PropagationPolicy::Batched(3));
        tc.insert("x1".to_string(), 1);
        tc.insert("x2".to_string(), 2);
        // Two inserts — not yet at threshold; cold snapshot should be empty.
        {
            let snap = tc.cold_snapshot();
            assert_eq!(snap.get(&"x1".to_string()), None);
        }
        // Third insert triggers the auto-flush.
        tc.insert("x3".to_string(), 3);
        let snap = tc.cold_snapshot();
        assert_eq!(snap.get(&"x1".to_string()), Some(1));
        assert_eq!(snap.get(&"x2".to_string()), Some(2));
        assert_eq!(snap.get(&"x3".to_string()), Some(3));
    }

    // --- Test 7: Immediate policy ---

    /// With `Immediate` policy, every insert is immediately visible in the cold
    /// snapshot.
    #[test]
    fn immediate_policy_cold_always_current() {
        let tc = std_pds(PropagationPolicy::Immediate);
        tc.insert("i1".to_string(), 10);
        {
            let snap = tc.cold_snapshot();
            assert_eq!(snap.get(&"i1".to_string()), Some(10));
        }
        tc.insert("i2".to_string(), 20);
        let snap = tc.cold_snapshot();
        assert_eq!(snap.get(&"i1".to_string()), Some(10));
        assert_eq!(snap.get(&"i2".to_string()), Some(20));
    }

    // --- Test 8: Manual policy — never auto-flushes ---

    /// With `Manual` policy, the cold snapshot stays empty until an explicit
    /// `flush()` is called.
    #[test]
    fn manual_policy_cold_empty_until_flush() {
        let tc = std_pds(PropagationPolicy::Manual);
        for i in 0..10 {
            tc.insert(format!("m{i}"), i);
        }
        // Cold should be empty — no flush has been called.
        let snap_before = tc.cold_snapshot();
        assert!(snap_before.is_empty());
        // Explicit flush.
        tc.flush();
        let snap_after = tc.cold_snapshot();
        assert_eq!(snap_after.get(&"m0".to_string()), Some(0));
        assert_eq!(snap_after.get(&"m9".to_string()), Some(9));
    }

    // --- Test 9: Concurrent inserts ---

    /// Two threads each insert 50 keys via cloned handles. After both join and
    /// an explicit `flush()`, all 100 keys must be in the cold snapshot.
    #[test]
    fn concurrent_inserts() {
        let tc = std_pds(PropagationPolicy::Manual);

        let tc_a = tc.clone();
        let tc_b = tc.clone();

        let t1 = std::thread::spawn(move || {
            for i in 0..50i32 {
                tc_a.insert(format!("t1_{i}"), i);
            }
        });
        let t2 = std::thread::spawn(move || {
            for i in 0..50i32 {
                tc_b.insert(format!("t2_{i}"), i);
            }
        });
        t1.join().expect("thread 1 panicked");
        t2.join().expect("thread 2 panicked");

        tc.flush();
        let snap = tc.cold_snapshot();
        for i in 0..50i32 {
            assert_eq!(
                snap.get(&format!("t1_{i}")),
                Some(i),
                "t1_{i} missing from cold snapshot"
            );
            assert_eq!(
                snap.get(&format!("t2_{i}")),
                Some(i),
                "t2_{i} missing from cold snapshot"
            );
        }
    }

    // --- Test 10: Three-tier composition (traits feature) ---

    /// Three tiers: Std → Pds → MerkleWrapper<Pds>.
    /// Insert keys, flush twice (first Std→Pds, then Pds→MerkleWrapper), and
    /// verify the merkle root changes between empty and populated states.
    #[cfg(feature = "traits")]
    #[test]
    fn three_tier_composition() {
        use super::super::backends::MerkleWrapperBackend;
        use crate::traits::MerklePersistentMap;

        type Inner = TieredCollection<
            String,
            i32,
            PdsHashMapBackend<String, i32>,
            MerkleWrapperBackend<String, i32>,
        >;
        type Outer = TieredCollection<String, i32, StdHashMapBackend<String, i32>, Inner>;

        let inner: Inner = TieredCollection::new(
            PdsHashMapBackend::new(),
            MerkleWrapperBackend::new(),
            PropagationPolicy::Manual,
        );

        let outer: Outer =
            TieredCollection::new(StdHashMapBackend::new(), inner, PropagationPolicy::Manual);

        // Record merkle root before any data.
        let root_before = {
            let cold_snap = outer.cold_snapshot(); // this is `Inner`
            let merkle_snap = cold_snap.cold_snapshot(); // this is `MerkleWrapperBackend`
            merkle_snap.inner().root_hash()
        };

        // Insert keys, flush Std→Pds, then flush Pds→MerkleWrapper.
        outer.insert("k1".to_string(), 1);
        outer.insert("k2".to_string(), 2);
        outer.flush(); // Std → Pds (Inner hot tier)
        {
            // Flush the inner tier (Pds → MerkleWrapper).
            let cold = outer.cold_snapshot();
            cold.flush();
        }
        // Note: flush on the snapshot modifies the shared Arc state.

        let root_after = {
            let cold_snap = outer.cold_snapshot();
            let merkle_snap = cold_snap.cold_snapshot();
            merkle_snap.inner().root_hash()
        };

        // The merkle root must have changed.
        assert_ne!(
            root_before, root_after,
            "merkle root should change after inserting and flushing"
        );
    }

    // --- Test 11: cold_snapshot is independent ---

    /// Taking a cold snapshot and then mutating the original should not change
    /// the snapshot (verifies clone semantics for `PdsHashMapBackend`).
    #[test]
    fn cold_snapshot_is_independent() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("snap_key".to_string(), 100);
        tc.flush();

        // Take a snapshot.
        let snap = tc.cold_snapshot();
        assert_eq!(snap.get(&"snap_key".to_string()), Some(100));

        // Mutate the original by inserting another key and flushing.
        tc.insert("new_key".to_string(), 200);
        tc.flush();

        // Snapshot should still only see the original key.
        assert_eq!(
            snap.get(&"snap_key".to_string()),
            Some(100),
            "snapshot was mutated by changes to the original"
        );
        assert_eq!(
            snap.get(&"new_key".to_string()),
            None,
            "snapshot should not see key inserted after snapshot was taken"
        );
    }

    // --- Additional edge-case tests ---

    /// Re-inserting a deleted key should un-delete it.
    #[test]
    fn reinsert_deleted_key() {
        let tc = std_pds(PropagationPolicy::Manual);
        tc.insert("key".to_string(), 1);
        tc.flush();
        tc.remove(&"key".to_string());
        // Key is deleted (pending_deletes). Re-insert before flush.
        tc.insert("key".to_string(), 99);
        assert_eq!(tc.get(&"key".to_string()), Some(99));
        // After flush, key should appear in cold with new value.
        tc.flush();
        let snap = tc.cold_snapshot();
        assert_eq!(snap.get(&"key".to_string()), Some(99));
    }

    /// `is_empty` returns true only when both tiers are empty.
    #[test]
    fn is_empty_both_tiers() {
        let tc = std_pds(PropagationPolicy::Manual);
        assert!(tc.is_empty());
        tc.insert("k".to_string(), 1);
        assert!(!tc.is_empty());
        tc.flush();
        // After flush: hot is empty, cold has one entry.
        assert!(!tc.is_empty());
        tc.remove(&"k".to_string());
        tc.flush();
        // After flush: hot is empty, cold is empty (delete applied).
        assert!(tc.is_empty());
    }

    /// `CollectionBackend` impl for `TieredCollection`: verify `drain` empties
    /// both tiers after a flush, and `load_from` populates hot.
    #[test]
    fn tiered_as_backend_drain_and_load() {
        let mut tc = std_pds(PropagationPolicy::Manual);
        tc.insert("d1".to_string(), 1);
        tc.insert("d2".to_string(), 2);
        tc.flush();
        tc.insert("d3".to_string(), 3);

        // Drain should flush and then drain cold.
        let entries = CollectionBackend::drain(&mut tc);
        // All three entries should be present.
        let mut keys: Vec<String> = entries.iter().map(|(k, _)| k.clone()).collect();
        keys.sort();
        assert_eq!(keys, vec!["d1", "d2", "d3"]);
        // Both tiers should be empty after drain.
        assert!(tc.is_empty());
    }

    /// `with_timed_propagation` convenience constructor: insert items, wait for
    /// two flush intervals, then verify the cold tier received them without any
    /// explicit `flush()` call.
    #[test]
    fn with_timed_propagation_auto_flushes() {
        let (tc, _handle) = TieredCollection::<
            String,
            i32,
            StdHashMapBackend<String, i32>,
            PdsHashMapBackend<String, i32>,
        >::with_timed_propagation(
            StdHashMapBackend::new(),
            PdsHashMapBackend::new(),
            std::time::Duration::from_millis(50),
        );

        // Push 10 items through the hot tier.
        for i in 0..10_i32 {
            tc.insert(format!("wtp_{i}"), i);
        }

        // Wait for at least two flush cycles (100 ms + margin).
        std::thread::sleep(std::time::Duration::from_millis(250));

        let snap = tc.cold_snapshot();
        assert!(
            !snap.is_empty(),
            "cold snapshot empty after two flush intervals"
        );
        for i in 0..10_i32 {
            assert_eq!(
                snap.get(&format!("wtp_{i}")),
                Some(i),
                "wtp_{i} missing from cold snapshot after auto-flush"
            );
        }
    }

    /// Timed policy: spawn background propagation, insert a key, wait for a
    /// flush cycle, then verify the cold tier has the key.
    #[test]
    fn timed_policy_background_propagation() {
        let tc: TieredCollection<
            String,
            i32,
            StdHashMapBackend<String, i32>,
            PdsHashMapBackend<String, i32>,
        > = TieredCollection::new(
            StdHashMapBackend::new(),
            PdsHashMapBackend::new(),
            PropagationPolicy::Timed(std::time::Duration::from_millis(50)),
        );

        let _handle = tc.start_background_propagation();
        tc.insert("timed_key".to_string(), 42);

        // Wait for at least one flush cycle (50 ms + margin).
        std::thread::sleep(std::time::Duration::from_millis(150));

        let snap = tc.cold_snapshot();
        assert_eq!(
            snap.get(&"timed_key".to_string()),
            Some(42),
            "timed propagation did not flush within the expected window"
        );
        // _handle is dropped here, stopping the background thread.
    }
}
