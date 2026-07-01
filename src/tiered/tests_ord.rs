//! Tests for Phase T.0b — OrdMap tiered backends.

#[cfg(test)]
mod tests {
    use crate::tiered::{
        backend::{CollectionBackend, OrderedCollectionBackend},
        backends::{PdsOrdMapBackend, StdBTreeMapBackend},
        policy::PropagationPolicy,
        TieredCollection, TieredCollectionOrdExt, TieredOrdMap,
    };

    // --- Type alias for brevity ---

    type BTreePds = TieredOrdMap<
        i32,
        &'static str,
        StdBTreeMapBackend<i32, &'static str>,
        PdsOrdMapBackend<i32, &'static str>,
    >;

    fn btree_pds(policy: PropagationPolicy) -> BTreePds {
        TieredCollection::new(StdBTreeMapBackend::new(), PdsOrdMapBackend::new(), policy)
    }

    // --- Test 1: StdBTreeMapBackend basic CollectionBackend ops ---

    /// Verify StdBTreeMapBackend implements CollectionBackend correctly.
    #[test]
    fn btree_backend_basic_ops() {
        let mut b: StdBTreeMapBackend<i32, &'static str> = StdBTreeMapBackend::new();
        assert!(b.is_empty());
        assert_eq!(b.insert(1, "a"), None);
        assert_eq!(b.insert(2, "b"), None);
        assert_eq!(b.get(&1), Some("a"));
        assert_eq!(b.len(), 2);
        assert_eq!(b.remove(&1), Some("a"));
        assert_eq!(b.get(&1), None);
        assert_eq!(b.len(), 1);
    }

    /// StdBTreeMapBackend ordered methods return entries in ascending key order.
    #[test]
    fn btree_backend_ordered_methods() {
        let mut b: StdBTreeMapBackend<i32, &'static str> = StdBTreeMapBackend::new();
        b.insert(3, "c");
        b.insert(1, "a");
        b.insert(2, "b");

        let ordered = b.iter_ordered();
        assert_eq!(ordered, vec![(1, "a"), (2, "b"), (3, "c")]);

        assert_eq!(b.first_key(), Some(1));
        assert_eq!(b.last_key(), Some(3));

        let range_result = b.range(1..3);
        assert_eq!(range_result, vec![(1, "a"), (2, "b")]);
    }

    // --- Test 2: PdsOrdMapBackend basic CollectionBackend ops ---

    /// Verify PdsOrdMapBackend implements CollectionBackend correctly.
    #[test]
    fn pds_ord_backend_basic_ops() {
        let mut b: PdsOrdMapBackend<i32, &'static str> = PdsOrdMapBackend::new();
        assert!(b.is_empty());
        assert_eq!(b.insert(10, "x"), None);
        assert_eq!(b.insert(20, "y"), None);
        assert_eq!(b.get(&10), Some("x"));
        assert_eq!(b.len(), 2);
        assert_eq!(b.remove(&10), Some("x"));
        assert_eq!(b.get(&10), None);
        assert_eq!(b.len(), 1);
    }

    /// PdsOrdMapBackend ordered methods return entries in ascending key order.
    #[test]
    fn pds_ord_backend_ordered_methods() {
        let mut b: PdsOrdMapBackend<i32, &'static str> = PdsOrdMapBackend::new();
        b.insert(30, "c");
        b.insert(10, "a");
        b.insert(20, "b");

        let ordered = b.iter_ordered();
        assert_eq!(ordered, vec![(10, "a"), (20, "b"), (30, "c")]);

        assert_eq!(b.first_key(), Some(10));
        assert_eq!(b.last_key(), Some(30));

        let range_result = b.range(10..25);
        assert_eq!(range_result, vec![(10, "a"), (20, "b")]);
    }

    // --- Test 3: TieredOrdMap range before and after flush ---

    /// Insert a span of keys, query a sub-range before and after flush.
    #[test]
    fn tiered_ord_map_range_before_and_after_flush() {
        let tc = btree_pds(PropagationPolicy::Manual);
        for i in 0..10i32 {
            tc.insert(i, "v");
        }

        // Before flush: all entries in hot — range should still return them.
        let before_flush = tc.range(2..6i32);
        let keys_before: Vec<i32> = before_flush.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys_before, vec![2, 3, 4, 5]);

        // After flush: entries move to cold.
        tc.flush();
        let after_flush = tc.range(2..6i32);
        let keys_after: Vec<i32> = after_flush.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys_after, vec![2, 3, 4, 5]);
    }

    // --- Test 4: iter_ordered — hot + cold merged in ascending order ---

    /// After inserting into both hot and cold, `iter_ordered` returns all entries
    /// in ascending key order with hot winning on duplicates.
    #[test]
    fn iter_ordered_hot_cold_merged() {
        let tc = btree_pds(PropagationPolicy::Manual);

        // Insert and flush 0..5 to cold.
        for i in 0..5i32 {
            tc.insert(i, "cold");
        }
        tc.flush();

        // Insert 3..8 to hot (3 and 4 overlap with cold, hot should win).
        for i in 3..8i32 {
            tc.insert(i, "hot");
        }

        let ordered = tc.iter_ordered();
        assert_eq!(ordered.len(), 8); // 0,1,2,3,4,5,6,7

        // Keys should be in ascending order.
        let keys: Vec<i32> = ordered.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![0, 1, 2, 3, 4, 5, 6, 7]);

        // Hot values win for overlapping keys.
        let val_3 = ordered.iter().find(|(k, _)| *k == 3).map(|(_, v)| *v);
        assert_eq!(val_3, Some("hot"));
        let val_1 = ordered.iter().find(|(k, _)| *k == 1).map(|(_, v)| *v);
        assert_eq!(val_1, Some("cold"));
    }

    // --- Test 5: Deleted key not returned in range or iter_ordered ---

    /// A key removed after flushing to cold must not appear in `range` or
    /// `iter_ordered`, even though it is still in the cold tier.
    #[test]
    fn deleted_key_excluded_from_range() {
        let tc = btree_pds(PropagationPolicy::Manual);
        for i in 0..5i32 {
            tc.insert(i, "v");
        }
        tc.flush();

        // Delete key 2 — it is now in pending_deletes.
        tc.remove(&2);

        let range = tc.range(0..5i32);
        let keys: Vec<i32> = range.into_iter().map(|(k, _)| k).collect();
        assert!(!keys.contains(&2), "deleted key 2 must not appear in range");
        assert_eq!(keys, vec![0, 1, 3, 4]);

        let ordered = tc.iter_ordered();
        let keys_o: Vec<i32> = ordered.into_iter().map(|(k, _)| k).collect();
        assert!(
            !keys_o.contains(&2),
            "deleted key 2 must not appear in iter_ordered"
        );
    }

    // --- Test 6: Three-tier composition (StdBTreeMap → PdsOrdMap → PdsOrdMap) ---

    /// Three-tier: Std BTreeMap (hot) → PdsOrdMap (mid) → PdsOrdMap (cold).
    #[test]
    fn three_tier_ord_map_composition() {
        type Mid = TieredOrdMap<
            i32,
            &'static str,
            StdBTreeMapBackend<i32, &'static str>,
            PdsOrdMapBackend<i32, &'static str>,
        >;
        type Outer = TieredOrdMap<i32, &'static str, StdBTreeMapBackend<i32, &'static str>, Mid>;

        let mid: Mid = TieredCollection::new(
            StdBTreeMapBackend::new(),
            PdsOrdMapBackend::new(),
            PropagationPolicy::Manual,
        );
        let outer: Outer =
            TieredCollection::new(StdBTreeMapBackend::new(), mid, PropagationPolicy::Manual);

        // Insert into outer hot, flush to mid hot, flush mid to mid cold.
        for i in 0..5i32 {
            outer.insert(i, "v");
        }
        outer.flush(); // outer hot → mid hot
        {
            let mid_snap = outer.cold_snapshot(); // clone of mid (Arc<Mutex>)
            mid_snap.flush(); // mid hot → mid cold
        }

        // Snapshot mid cold (PdsOrdMapBackend).
        let mid_snap = outer.cold_snapshot();
        let mid_cold = mid_snap.cold_snapshot();

        let pairs: Vec<(i32, &'static str)> =
            mid_cold.inner().iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs.len(), 5);
        let keys: Vec<i32> = pairs.iter().map(|(k, _)| *k).collect();
        // OrdMap iterates in sorted order.
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
