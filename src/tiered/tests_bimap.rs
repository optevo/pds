//! Tests for T.9 — `BiMapBackend` and `TieredBiMap`.

#[cfg(test)]
mod tests {
    use super::super::{
        bimap::TieredBiMap,
        bimap_backend::BiMapBackend,
        bimap_backends::{PdsBiMapBackend, PdsOrdBiMapBackend},
        policy::PropagationPolicy,
    };

    // --- Test 1: basic insert and get_by_key / get_by_value ---

    /// Inserting a pair and reading it back in both directions.
    #[test]
    fn insert_and_get_both_directions() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("alice".to_string(), 1);
        tb.insert("bob".to_string(), 2);

        assert_eq!(tb.get_by_key(&"alice".to_string()), Some(1));
        assert_eq!(tb.get_by_key(&"bob".to_string()), Some(2));
        assert_eq!(tb.get_by_value(&1), Some("alice".to_string()));
        assert_eq!(tb.get_by_value(&2), Some("bob".to_string()));
        assert_eq!(tb.get_by_key(&"charlie".to_string()), None);
        assert_eq!(tb.get_by_value(&99), None);
    }

    // --- Test 2: cold fallback ---

    /// After a flush, get_by_key / get_by_value should fall back to cold.
    #[test]
    fn get_falls_back_to_cold() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("x".to_string(), 10);
        tb.flush(); // cold: x↔10

        assert_eq!(tb.get_by_key(&"x".to_string()), Some(10));
        assert_eq!(tb.get_by_value(&10), Some("x".to_string()));
    }

    // --- Test 3: hot wins over cold ---

    /// After updating a key in hot, the hot value should take precedence.
    #[test]
    fn hot_wins_over_cold() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("k".to_string(), 1);
        tb.flush(); // cold: k↔1

        tb.insert("k".to_string(), 2); // hot: k↔2
        assert_eq!(tb.get_by_key(&"k".to_string()), Some(2));
        assert_eq!(tb.get_by_value(&2), Some("k".to_string()));
    }

    // --- Test 4: remove_by_key suppresses cold ---

    /// Removing by key should suppress the cold entry until flush.
    #[test]
    fn remove_by_key_suppresses_cold() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("a".to_string(), 100);
        tb.flush(); // cold: a↔100

        let displaced = tb.remove_by_key(&"a".to_string());
        assert_eq!(displaced, Some(100));

        // Both directions should report absent.
        assert_eq!(tb.get_by_key(&"a".to_string()), None);
        assert_eq!(tb.get_by_value(&100), None);
        assert!(!tb.contains_key(&"a".to_string()));
        assert!(!tb.contains_value(&100));

        // After flush, cold should also be clean.
        tb.flush();
        let cold = tb.cold_snapshot();
        assert!(cold.get_by_key(&"a".to_string()).is_none());
        assert!(cold.get_by_value(&100).is_none());
    }

    // --- Test 5: remove_by_value suppresses cold ---

    /// Removing by value should suppress the cold entry until flush.
    #[test]
    fn remove_by_value_suppresses_cold() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("b".to_string(), 200);
        tb.flush(); // cold: b↔200

        let displaced_key = tb.remove_by_value(&200);
        assert_eq!(displaced_key, Some("b".to_string()));

        assert_eq!(tb.get_by_key(&"b".to_string()), None);
        assert_eq!(tb.get_by_value(&200), None);

        tb.flush();
        let cold = tb.cold_snapshot();
        assert!(cold.get_by_key(&"b".to_string()).is_none());
        assert!(cold.get_by_value(&200).is_none());
    }

    // --- Test 6: flush merges hot into cold ---

    /// Hot entries are transferred to cold on flush, with cold retaining
    /// pre-existing entries not touched by hot.
    #[test]
    fn flush_merges_hot_into_cold() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("p".to_string(), 1);
        tb.flush(); // cold: p↔1

        tb.insert("q".to_string(), 2);
        tb.flush(); // cold: p↔1, q↔2

        let cold = tb.cold_snapshot();
        assert_eq!(cold.get_by_key(&"p".to_string()), Some(1));
        assert_eq!(cold.get_by_key(&"q".to_string()), Some(2));
    }

    // --- Test 7: Immediate policy propagates on every write ---

    /// With `Immediate` policy, each insert flushes straight to cold.
    #[test]
    fn immediate_policy_auto_flushes() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Immediate,
        );

        tb.insert("z".to_string(), 42);
        // After immediate flush, hot should be empty and cold should hold the pair.
        let cold = tb.cold_snapshot();
        assert_eq!(cold.get_by_key(&"z".to_string()), Some(42));
        assert!(tb.hot_snapshot().is_empty());
    }

    // --- Test 8: OrdBiMap backend ---

    /// `PdsOrdBiMapBackend` works correctly via `TieredBiMap`.
    #[test]
    fn ord_bimap_backend() {
        let tb: TieredBiMap<i32, i32, PdsOrdBiMapBackend<i32, i32>, PdsOrdBiMapBackend<i32, i32>> =
            TieredBiMap::new(
                PdsOrdBiMapBackend::new(),
                PdsOrdBiMapBackend::new(),
                PropagationPolicy::Manual,
            );

        tb.insert(1, 10);
        tb.insert(2, 20);
        tb.insert(3, 30);

        assert_eq!(tb.get_by_key(&1), Some(10));
        assert_eq!(tb.get_by_value(&20), Some(2));

        tb.flush();
        assert_eq!(tb.get_by_key(&3), Some(30));
        assert_eq!(tb.get_by_value(&10), Some(1));
    }

    // --- Test 9: re-insert after remove un-shadows the pair ---

    /// Inserting a key that was previously removed should restore visibility.
    #[test]
    fn reinsert_after_remove() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tb.insert("m".to_string(), 7);
        tb.flush(); // cold: m↔7

        tb.remove_by_key(&"m".to_string());
        assert_eq!(tb.get_by_key(&"m".to_string()), None);

        // Re-insert: should cancel the pending remove.
        tb.insert("m".to_string(), 8);
        assert_eq!(tb.get_by_key(&"m".to_string()), Some(8));
        assert_eq!(tb.get_by_value(&8), Some("m".to_string()));
    }

    // --- Test 10: is_empty / len ---

    /// `is_empty` and `len` report correct values.
    #[test]
    fn is_empty_and_len() {
        let tb: TieredBiMap<
            String,
            i32,
            PdsBiMapBackend<String, i32>,
            PdsBiMapBackend<String, i32>,
        > = TieredBiMap::new(
            PdsBiMapBackend::new(),
            PdsBiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        assert!(tb.is_empty());
        assert_eq!(tb.len(), 0);

        tb.insert("a".to_string(), 1);
        assert!(!tb.is_empty());
        // len() may over-count when cold is non-empty; after flush hot is empty.
        tb.flush();
        assert_eq!(tb.len(), 1);
    }
}
