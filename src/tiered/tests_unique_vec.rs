//! Tests for T.13 — `UniqueVecBackend`, `PdsUniqueVecBackend`,
//! `TieredUniqueVector`, and `TieredPdsUniqueVector`.

#[cfg(test)]
mod tests {
    use super::super::{
        policy::PropagationPolicy, unique_vec::TieredUniqueVector,
        unique_vec_backend::UniqueVecBackend, unique_vec_backends::PdsUniqueVecBackend,
    };

    // --- Test 1: push_back and contains (hot only) ---

    /// Elements pushed to the hot tier are visible via `contains`.
    #[test]
    fn push_back_contains_hot() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        assert!(tv.push_back(1));
        assert!(tv.push_back(2));
        assert!(tv.push_back(3));
        assert!(tv.contains(&1));
        assert!(tv.contains(&2));
        assert!(tv.contains(&3));
        assert!(!tv.contains(&99));
    }

    // --- Test 2: duplicate rejection in hot ---

    /// Pushing a duplicate element already in hot returns `false`.
    #[test]
    fn duplicate_rejection_hot() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        assert!(tv.push_back(42));
        assert!(!tv.push_back(42)); // duplicate
    }

    // --- Test 3: cold fallback after flush ---

    /// Elements flushed to cold are visible via `contains`.
    #[test]
    fn cold_fallback_after_flush() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(10);
        tv.push_back(20);
        tv.flush();

        assert!(tv.contains(&10));
        assert!(tv.contains(&20));
        assert!(tv.hot_snapshot().is_empty());
    }

    // --- Test 4: duplicate rejection cross-tier ---

    /// An element in cold cannot be pushed to hot again.
    #[test]
    fn duplicate_rejection_cross_tier() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(5);
        tv.flush(); // cold: [5]

        assert!(!tv.push_back(5)); // already in cold
    }

    // --- Test 5: remove_by_value suppresses cold ---

    /// Removing a cold element hides it before the next flush.
    #[test]
    fn remove_suppresses_cold() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(7);
        tv.flush();

        assert!(tv.remove_by_value(&7));
        assert!(!tv.contains(&7));
        assert!(!tv.remove_by_value(&7)); // already removed
    }

    // --- Test 6: re-insert after remove ---

    /// An element can be re-inserted after it has been removed.
    #[test]
    fn reinsert_after_remove() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(3);
        tv.flush();
        tv.remove_by_value(&3);
        assert!(tv.push_back(3)); // should succeed after remove
        assert!(tv.contains(&3));
    }

    // --- Test 7: iter_all preserves order (cold then hot) ---

    /// `iter_all` yields cold elements first, then hot elements.
    #[test]
    fn iter_all_order() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(1);
        tv.push_back(2);
        tv.flush(); // cold: [1, 2]

        tv.push_back(3);
        tv.push_back(4); // hot: [3, 4]

        assert_eq!(tv.iter_all(), vec![1, 2, 3, 4]);
    }

    // --- Test 8: iter_all excludes pending removes ---

    /// Removed elements are absent from `iter_all` before the flush.
    #[test]
    fn iter_all_excludes_removed() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(10);
        tv.push_back(20);
        tv.push_back(30);
        tv.flush(); // cold: [10, 20, 30]

        tv.remove_by_value(&20);

        assert_eq!(tv.iter_all(), vec![10, 30]);
    }

    // --- Test 9: get by logical index ---

    /// `get` returns the element at the logical index across both tiers.
    #[test]
    fn get_by_index() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(100);
        tv.push_back(200);
        tv.flush();
        tv.push_back(300);

        assert_eq!(tv.get(0), Some(100));
        assert_eq!(tv.get(1), Some(200));
        assert_eq!(tv.get(2), Some(300));
        assert_eq!(tv.get(3), None);
    }

    // --- Test 10: flush applies pending removes to cold ---

    /// After flush, elements previously removed from cold are gone from cold.
    #[test]
    fn flush_applies_pending_removes() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        tv.push_back(1);
        tv.push_back(2);
        tv.flush();
        tv.remove_by_value(&1);
        tv.flush();

        let cold = tv.cold_snapshot();
        assert!(!cold.contains(&1));
        assert!(cold.contains(&2));
    }

    // --- Test 11: Immediate policy ---

    /// With `Immediate` policy every push is flushed to cold at once.
    #[test]
    fn immediate_policy() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Immediate,
            );

        tv.push_back(55);
        let cold = tv.cold_snapshot();
        assert!(cold.contains(&55));
        assert!(tv.hot_snapshot().is_empty());
    }

    // --- Test 12: is_empty and len ---

    /// `is_empty` and `len` reflect combined hot+cold state.
    #[test]
    fn is_empty_and_len() {
        let tv: TieredUniqueVector<i32, PdsUniqueVecBackend<i32>, PdsUniqueVecBackend<i32>> =
            TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );

        assert!(tv.is_empty());

        tv.push_back(1);
        assert!(!tv.is_empty());
        assert_eq!(tv.len(), 1);

        tv.flush();
        assert_eq!(tv.len(), 1);

        tv.push_back(2);
        // hot=1, cold=1 → approx 2
        assert_eq!(tv.len(), 2);
    }
}
