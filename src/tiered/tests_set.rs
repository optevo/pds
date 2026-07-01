//! Tests for T.6 — `SetBackend`, `TieredSet`, and `TieredOrdSet`.

#[cfg(test)]
mod tests {
    use super::super::{
        policy::PropagationPolicy,
        set::TieredSet,
        set_backend::SetBackend,
        set_backends::{
            PdsHashSetBackend, PdsOrdSetBackend, StdBTreeSetBackend, StdHashSetBackend,
        },
    };

    // --- Type aliases ---

    type StdPdsSet = TieredSet<String, StdHashSetBackend<String>, PdsHashSetBackend<String>>;

    fn std_pds_set(policy: PropagationPolicy) -> StdPdsSet {
        TieredSet::new(StdHashSetBackend::new(), PdsHashSetBackend::new(), policy)
    }

    // --- Test 1: insert_contains_from_hot ---

    /// Inserting an element should make it immediately visible via `contains`
    /// without flushing (the value lives in the hot tier).
    #[test]
    fn insert_contains_from_hot() {
        let ts = std_pds_set(PropagationPolicy::Manual);
        let inserted = ts.insert("alpha".to_string());
        assert!(inserted, "first insert should return true");
        assert!(ts.contains(&"alpha".to_string()));
        // Cold snapshot should be empty — no flush has occurred.
        assert!(ts.cold_snapshot().is_empty());
    }

    // --- Test 2: remove_not_visible_via_cold_fallback ---

    /// Insert an element, flush (so it moves to cold), remove it. `contains`
    /// must return false even though the element is still in cold, because it
    /// is in `pending_removes`.
    #[test]
    fn remove_not_visible_via_cold_fallback() {
        let ts = std_pds_set(PropagationPolicy::Manual);
        ts.insert("beta".to_string());
        ts.flush();
        // Element is now only in cold.
        assert!(ts.contains(&"beta".to_string()));
        // Remove it — pending_removes should mask the cold value.
        let removed = ts.remove(&"beta".to_string());
        assert!(
            removed,
            "remove should return true when element was present"
        );
        assert!(
            !ts.contains(&"beta".to_string()),
            "element still visible after remove"
        );
    }

    // --- Test 3: flush_propagates_to_cold ---

    /// After a flush, the element should be in the cold tier snapshot.
    #[test]
    fn flush_propagates_to_cold() {
        let ts = std_pds_set(PropagationPolicy::Manual);
        ts.insert("gamma".to_string());
        ts.flush();
        assert!(ts.cold_snapshot().contains(&"gamma".to_string()));
    }

    // --- Test 4: flush_applies_removes_to_cold ---

    /// Insert → flush → remove → flush: cold snapshot must not contain the element.
    #[test]
    fn flush_applies_removes_to_cold() {
        let ts = std_pds_set(PropagationPolicy::Manual);
        ts.insert("delta".to_string());
        ts.flush();
        assert!(ts.cold_snapshot().contains(&"delta".to_string()));
        ts.remove(&"delta".to_string());
        ts.flush();
        assert!(
            !ts.cold_snapshot().contains(&"delta".to_string()),
            "element still in cold after remove+flush"
        );
    }

    // --- Test 5: batched_auto_flush_set ---

    /// With `Batched(3)`, the third write triggers an automatic flush.
    #[test]
    fn batched_auto_flush_set() {
        let ts: TieredSet<String, StdHashSetBackend<String>, PdsHashSetBackend<String>> =
            TieredSet::new(
                StdHashSetBackend::new(),
                PdsHashSetBackend::new(),
                PropagationPolicy::Batched(3),
            );
        ts.insert("x1".to_string());
        ts.insert("x2".to_string());
        // Two inserts — not yet at threshold.
        assert!(ts.cold_snapshot().is_empty());
        // Third insert triggers auto-flush.
        ts.insert("x3".to_string());
        let snap = ts.cold_snapshot();
        assert!(snap.contains(&"x1".to_string()));
        assert!(snap.contains(&"x2".to_string()));
        assert!(snap.contains(&"x3".to_string()));
    }

    // --- Test 6: concurrent_inserts ---

    /// Two threads each insert 50 elements via cloned handles. After both join
    /// and an explicit flush, all 100 elements must be in the cold snapshot.
    #[test]
    fn concurrent_inserts() {
        let ts: TieredSet<i32, StdHashSetBackend<i32>, PdsHashSetBackend<i32>> = TieredSet::new(
            StdHashSetBackend::new(),
            PdsHashSetBackend::new(),
            PropagationPolicy::Manual,
        );

        let ts_a = ts.clone();
        let ts_b = ts.clone();

        let t1 = std::thread::spawn(move || {
            for i in 0..50_i32 {
                ts_a.insert(i);
            }
        });
        let t2 = std::thread::spawn(move || {
            for i in 50..100_i32 {
                ts_b.insert(i);
            }
        });
        t1.join().expect("thread 1 panicked");
        t2.join().expect("thread 2 panicked");

        ts.flush();
        let snap = ts.cold_snapshot();
        for i in 0..100_i32 {
            assert!(snap.contains(&i), "{i} missing from cold snapshot");
        }
    }

    // --- Test 7: ordered_iter_ordered ---

    /// `iter_ordered` on a `TieredOrdSet` backed by `StdBTreeSet`/`PdsOrdSet`
    /// returns elements in sorted order spanning hot and cold.
    #[test]
    fn ordered_iter_ordered() {
        use super::super::set::TieredSetOrdExt;

        let ts: TieredSet<i32, StdBTreeSetBackend<i32>, PdsOrdSetBackend<i32>> = TieredSet::new(
            StdBTreeSetBackend::new(),
            PdsOrdSetBackend::new(),
            PropagationPolicy::Manual,
        );

        // Insert into hot, then flush some to cold.
        ts.insert(3);
        ts.insert(1);
        ts.flush(); // 1, 3 → cold
        ts.insert(2);
        ts.insert(4); // 2, 4 in hot

        let ordered = ts.iter_ordered();
        assert_eq!(ordered, vec![1, 2, 3, 4]);
    }

    // --- Test 8: ordered_range ---

    /// `range` on a `TieredOrdSet` returns elements in the range spanning both tiers.
    #[test]
    fn ordered_range() {
        use super::super::set::TieredSetOrdExt;

        let ts: TieredSet<i32, StdBTreeSetBackend<i32>, PdsOrdSetBackend<i32>> = TieredSet::new(
            StdBTreeSetBackend::new(),
            PdsOrdSetBackend::new(),
            PropagationPolicy::Manual,
        );

        // Insert 1..=10, flush, then insert 11..=20.
        for i in 1..=10_i32 {
            ts.insert(i);
        }
        ts.flush();
        for i in 11..=20_i32 {
            ts.insert(i);
        }

        // Range 5..=15 should span both tiers.
        let result = ts.range(5..=15_i32);
        assert_eq!(result, vec![5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    }

    // --- Test 9: three_tier_set_composition ---

    /// Three-tier set composition: Std → Pds → Pds.
    #[test]
    fn three_tier_set_composition() {
        type Inner = TieredSet<i32, PdsHashSetBackend<i32>, PdsHashSetBackend<i32>>;
        type Outer = TieredSet<i32, StdHashSetBackend<i32>, Inner>;

        let inner: Inner = TieredSet::new(
            PdsHashSetBackend::new(),
            PdsHashSetBackend::new(),
            PropagationPolicy::Manual,
        );
        let outer: Outer =
            TieredSet::new(StdHashSetBackend::new(), inner, PropagationPolicy::Manual);

        // Insert via outer hot tier.
        outer.insert(10);
        outer.insert(20);

        // Flush outer: Std → Inner (Inner acts as cold).
        outer.flush();

        // The outer cold tier is the Inner. Verify elements are present
        // via `contains` (which reads hot first, then cold).
        assert!(outer.contains(&10));
        assert!(outer.contains(&20));

        // Now flush inner (Inner's hot → Inner's cold).
        {
            let inner_snap = outer.cold_snapshot();
            inner_snap.flush();
        }

        // Elements should still be visible.
        assert!(outer.contains(&10));
        assert!(outer.contains(&20));
    }

    // --- Test 10: duplicate_insert_returns_false ---

    /// Inserting a duplicate element returns `false`.
    #[test]
    fn duplicate_insert_returns_false() {
        let ts = std_pds_set(PropagationPolicy::Manual);
        assert!(ts.insert("dup".to_string()));
        // Second insert of same element returns false.
        assert!(!ts.insert("dup".to_string()));
        // Insert after flush — element is in cold, hot is empty.
        ts.flush();
        assert!(!ts.insert("dup".to_string()));
    }

    // --- Test 11: SetBackend impl for TieredSet ---

    /// Verifies `drain` on a `TieredSet` used as a `SetBackend`.
    #[test]
    fn tiered_set_as_backend_drain() {
        let mut ts = std_pds_set(PropagationPolicy::Manual);
        ts.insert("d1".to_string());
        ts.insert("d2".to_string());
        ts.flush();
        ts.insert("d3".to_string());

        let mut elems = SetBackend::drain(&mut ts);
        elems.sort();
        assert_eq!(elems, vec!["d1", "d2", "d3"]);
        assert!(ts.is_empty());
    }
}
