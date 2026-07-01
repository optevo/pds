//! Tests for T.11 — `InsertionOrderMapBackend`, `InsertionOrderSetBackend`,
//! `TieredInsertionOrderMap`, and `TieredInsertionOrderSet`.

#[cfg(test)]
mod tests {
    use super::super::{
        insertion_order::{TieredInsertionOrderMap, TieredInsertionOrderSet},
        insertion_order_backend::InsertionOrderMapBackend,
        insertion_order_backends::{
            PdsInsertionOrderMapBackend, PdsInsertionOrderSetBackend,
            PdsOrdInsertionOrderMapBackend, PdsOrdInsertionOrderSetBackend,
        },
        policy::PropagationPolicy,
    };

    // --- Test 1: insertion-order preserved across hot ---

    /// Elements inserted into hot are returned in insertion order by
    /// `iter_insertion_order`.
    #[test]
    fn hot_insertion_order_preserved() {
        let tm: TieredInsertionOrderMap<
            &str,
            i32,
            PdsInsertionOrderMapBackend<&str, i32>,
            PdsInsertionOrderMapBackend<&str, i32>,
        > = TieredInsertionOrderMap::new(
            PdsInsertionOrderMapBackend::new(),
            PdsInsertionOrderMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("c", 3);
        tm.insert("a", 1);
        tm.insert("b", 2);

        let order: Vec<_> = tm
            .iter_insertion_order()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(order, vec!["c", "a", "b"]);
    }

    // --- Test 2: insertion-order preserved across flush ---

    /// After a flush, cold entries appear before hot entries in cross-tier
    /// iteration order.
    #[test]
    fn cross_tier_insertion_order() {
        let tm: TieredInsertionOrderMap<
            &str,
            i32,
            PdsInsertionOrderMapBackend<&str, i32>,
            PdsInsertionOrderMapBackend<&str, i32>,
        > = TieredInsertionOrderMap::new(
            PdsInsertionOrderMapBackend::new(),
            PdsInsertionOrderMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("first", 1);
        tm.insert("second", 2);
        tm.flush(); // cold: [first, second]

        tm.insert("third", 3);
        tm.insert("fourth", 4); // hot: [third, fourth]

        let order: Vec<_> = tm
            .iter_insertion_order()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(order, vec!["first", "second", "third", "fourth"]);
    }

    // --- Test 3: hot wins for duplicate keys in iter_insertion_order ---

    /// When the same key exists in both hot and cold, the cold entry is excluded
    /// from `iter_insertion_order` and the hot entry is used.
    #[test]
    fn hot_wins_in_iter_order() {
        let tm: TieredInsertionOrderMap<
            &str,
            i32,
            PdsInsertionOrderMapBackend<&str, i32>,
            PdsInsertionOrderMapBackend<&str, i32>,
        > = TieredInsertionOrderMap::new(
            PdsInsertionOrderMapBackend::new(),
            PdsInsertionOrderMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("x", 10);
        tm.flush(); // cold: [x=10]

        tm.insert("x", 20); // hot: [x=20], cold still has x=10

        let result = tm.iter_insertion_order();
        // Cold's x should be excluded; hot's x=20 is the only x entry.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("x", 20));
    }

    // --- Test 4: update preserves insertion position ---

    /// Updating a key's value does not change its position in insertion order.
    #[test]
    fn update_preserves_insertion_position() {
        let tm: TieredInsertionOrderMap<
            &str,
            i32,
            PdsInsertionOrderMapBackend<&str, i32>,
            PdsInsertionOrderMapBackend<&str, i32>,
        > = TieredInsertionOrderMap::new(
            PdsInsertionOrderMapBackend::new(),
            PdsInsertionOrderMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("a", 1);
        tm.insert("b", 2);
        tm.insert("a", 99); // update "a" — should not move it to the end

        let order: Vec<_> = tm
            .iter_insertion_order()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        // "a" was first inserted before "b", so it should still come first.
        assert_eq!(order, vec!["a", "b"]);
        assert_eq!(tm.get(&"a"), Some(99)); // value is updated
    }

    // --- Test 5: remove is hidden before flush ---

    /// After removing a key that is in cold, the entry is invisible until a flush
    /// cleans it from cold.
    #[test]
    fn remove_hidden_before_flush() {
        let tm: TieredInsertionOrderMap<
            &str,
            i32,
            PdsInsertionOrderMapBackend<&str, i32>,
            PdsInsertionOrderMapBackend<&str, i32>,
        > = TieredInsertionOrderMap::new(
            PdsInsertionOrderMapBackend::new(),
            PdsInsertionOrderMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("gone", 42);
        tm.flush(); // cold: [gone=42]

        let prev = tm.remove(&"gone");
        assert_eq!(prev, Some(42));
        assert_eq!(tm.get(&"gone"), None);

        let order: Vec<_> = tm
            .iter_insertion_order()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert!(!order.contains(&"gone"));

        // Flush applies the delete to cold.
        tm.flush();
        let cold = tm.cold_snapshot();
        assert!(cold.get(&"gone").is_none());
    }

    // --- Test 6: InsertionOrderSet preserves insertion order ---

    /// Elements in `TieredInsertionOrderSet` are iterated in insertion order
    /// across tiers.
    #[test]
    fn set_insertion_order() {
        let ts: TieredInsertionOrderSet<
            i32,
            PdsInsertionOrderSetBackend<i32>,
            PdsInsertionOrderSetBackend<i32>,
        > = TieredInsertionOrderSet::new(
            PdsInsertionOrderSetBackend::new(),
            PdsInsertionOrderSetBackend::new(),
            PropagationPolicy::Manual,
        );

        ts.insert(30);
        ts.insert(10);
        ts.insert(20);
        ts.flush(); // cold: [30, 10, 20]

        ts.insert(50);
        ts.insert(40); // hot: [50, 40]

        let order = ts.iter_insertion_order();
        assert_eq!(order, vec![30, 10, 20, 50, 40]);
    }

    // --- Test 7: set duplicate rejection ---

    /// Inserting a duplicate into `TieredInsertionOrderSet` returns `false` and
    /// does not add a second copy.
    #[test]
    fn set_duplicate_rejection() {
        let ts: TieredInsertionOrderSet<
            i32,
            PdsInsertionOrderSetBackend<i32>,
            PdsInsertionOrderSetBackend<i32>,
        > = TieredInsertionOrderSet::new(
            PdsInsertionOrderSetBackend::new(),
            PdsInsertionOrderSetBackend::new(),
            PropagationPolicy::Manual,
        );

        assert!(ts.insert(1));
        assert!(!ts.insert(1)); // duplicate — should return false
        ts.flush();
        assert!(!ts.insert(1)); // duplicate in cold — should return false
    }

    // --- Test 8: Ord variants ---

    /// `PdsOrdInsertionOrderMapBackend` and `PdsOrdInsertionOrderSetBackend`
    /// work correctly under TieredInsertionOrder types.
    #[test]
    fn ord_backends_work() {
        let tm: TieredInsertionOrderMap<
            i32,
            &str,
            PdsOrdInsertionOrderMapBackend<i32, &str>,
            PdsOrdInsertionOrderMapBackend<i32, &str>,
        > = TieredInsertionOrderMap::new(
            PdsOrdInsertionOrderMapBackend::new(),
            PdsOrdInsertionOrderMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert(3, "three");
        tm.insert(1, "one");
        tm.insert(2, "two");
        tm.flush();

        assert_eq!(tm.get(&1), Some("one"));
        assert_eq!(tm.get(&2), Some("two"));
        assert_eq!(tm.get(&3), Some("three"));

        let ts: TieredInsertionOrderSet<
            i32,
            PdsOrdInsertionOrderSetBackend<i32>,
            PdsOrdInsertionOrderSetBackend<i32>,
        > = TieredInsertionOrderSet::new(
            PdsOrdInsertionOrderSetBackend::new(),
            PdsOrdInsertionOrderSetBackend::new(),
            PropagationPolicy::Manual,
        );
        ts.insert(5);
        ts.insert(3);
        ts.flush();
        assert!(ts.contains(&5));
        assert!(ts.contains(&3));
    }

    // --- Test 9: Immediate policy ---

    /// With `Immediate` policy, each insert flushes straight to cold.
    #[test]
    fn immediate_policy_map() {
        let tm: TieredInsertionOrderMap<
            &str,
            i32,
            PdsInsertionOrderMapBackend<&str, i32>,
            PdsInsertionOrderMapBackend<&str, i32>,
        > = TieredInsertionOrderMap::new(
            PdsInsertionOrderMapBackend::new(),
            PdsInsertionOrderMapBackend::new(),
            PropagationPolicy::Immediate,
        );

        tm.insert("auto", 1);
        let cold = tm.cold_snapshot();
        assert_eq!(cold.get(&"auto"), Some(1));
        assert!(tm.hot_snapshot().is_empty());
    }
}
