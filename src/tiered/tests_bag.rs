//! Tests for T.7 — `BagBackend`, `TieredBag`, and `TieredOrdBag`.

#[cfg(test)]
mod tests {
    use super::super::{
        bag::TieredBag,
        bag_backend::{BagBackend, OrderedBagBackend},
        bag_backends::{PdsBagBackend, PdsOrdBagBackend},
        policy::PropagationPolicy,
    };

    // --- Test 1: insert_and_count ---

    /// Inserting the same element 3 times gives a count of 3.
    #[test]
    fn insert_and_count() {
        let tb: TieredBag<String, PdsBagBackend<String>, PdsBagBackend<String>> = TieredBag::new(
            PdsBagBackend::new(),
            PdsBagBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("apple".to_string());
        tb.insert("apple".to_string());
        tb.insert("apple".to_string());
        assert_eq!(tb.count(&"apple".to_string()), 3);
        assert_eq!(tb.len(), 3);
    }

    // --- Test 2: remove_decrements ---

    /// Inserting 2, then removing 1 gives a count of 1.
    #[test]
    fn remove_decrements() {
        let tb: TieredBag<String, PdsBagBackend<String>, PdsBagBackend<String>> = TieredBag::new(
            PdsBagBackend::new(),
            PdsBagBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("banana".to_string());
        tb.insert("banana".to_string());
        let removed = tb.remove(&"banana".to_string());
        assert!(
            removed,
            "remove should return true when element was present"
        );
        assert_eq!(tb.count(&"banana".to_string()), 1);
        assert_eq!(tb.len(), 1);
    }

    // --- Test 3: count_spans_tiers ---

    /// Insert 2 to hot, flush (cold count=2), insert 1 more (hot count=1).
    /// Total count should be 3.
    #[test]
    fn count_spans_tiers() {
        let tb: TieredBag<String, PdsBagBackend<String>, PdsBagBackend<String>> = TieredBag::new(
            PdsBagBackend::new(),
            PdsBagBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("cherry".to_string());
        tb.insert("cherry".to_string());
        tb.flush(); // cold: cherry×2, hot: empty
        assert_eq!(tb.cold_snapshot().count(&"cherry".to_string()), 2);

        tb.insert("cherry".to_string()); // hot: cherry×1
        assert_eq!(tb.count(&"cherry".to_string()), 3);
        assert_eq!(tb.len(), 3);
    }

    // --- Test 4: flush_adds_counts ---

    /// Flushing twice accumulates counts in cold rather than replacing.
    #[test]
    fn flush_adds_counts() {
        let tb: TieredBag<String, PdsBagBackend<String>, PdsBagBackend<String>> = TieredBag::new(
            PdsBagBackend::new(),
            PdsBagBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("date".to_string());
        tb.insert("date".to_string());
        tb.flush(); // cold: date×2

        tb.insert("date".to_string());
        tb.insert("date".to_string());
        tb.insert("date".to_string());
        tb.flush(); // cold: date×5 (2 + 3)

        assert_eq!(tb.cold_snapshot().count(&"date".to_string()), 5);
    }

    // --- Test 5: pending_remove_applied_on_flush ---

    /// Insert 3 to hot, flush, remove 2 (one from cold via pending_removes,
    /// one cancels a hot insert), then flush again. Cold count should be 1.
    #[test]
    fn pending_remove_applied_on_flush() {
        let tb: TieredBag<String, PdsBagBackend<String>, PdsBagBackend<String>> = TieredBag::new(
            PdsBagBackend::new(),
            PdsBagBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("elderberry".to_string());
        tb.insert("elderberry".to_string());
        tb.insert("elderberry".to_string());
        tb.flush(); // cold: elderberry×3

        // Remove 2: these come from cold (pending_removes bumped by 2).
        tb.remove(&"elderberry".to_string());
        tb.remove(&"elderberry".to_string());

        // Logical count should be 1.
        assert_eq!(tb.count(&"elderberry".to_string()), 1);

        // Flush applies the pending removes to cold.
        tb.flush();
        assert_eq!(tb.cold_snapshot().count(&"elderberry".to_string()), 1);
    }

    // --- Test 6: ordered_iter_ordered ---

    /// `PdsOrdBagBackend` delivers `iter_ordered` in ascending element order with counts.
    #[test]
    fn ordered_iter_ordered() {
        let mut hot = PdsOrdBagBackend::new();
        hot.insert(3_i32);
        hot.insert(1_i32);
        hot.insert(1_i32);
        hot.insert(2_i32);

        let ordered = hot.iter_ordered();
        assert_eq!(ordered, vec![(1, 2), (2, 1), (3, 1)]);
    }

    // --- Test 7: remove_absent_returns_false ---

    /// Removing an element that was never inserted returns `false`.
    #[test]
    fn remove_absent_returns_false() {
        let tb: TieredBag<String, PdsBagBackend<String>, PdsBagBackend<String>> = TieredBag::new(
            PdsBagBackend::new(),
            PdsBagBackend::new(),
            PropagationPolicy::Manual,
        );

        let result = tb.remove(&"fig".to_string());
        assert!(!result, "remove on absent element should return false");
    }
}
