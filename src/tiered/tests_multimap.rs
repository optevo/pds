//! Tests for T.8 — `MultiMapBackend` and `TieredMultiMap`.

#[cfg(test)]
mod tests {
    use super::super::{
        multimap::TieredMultiMap,
        multimap_backend::MultiMapBackend,
        multimap_backends::{PdsHashMultiMapBackend, PdsOrdMultiMapBackend},
        policy::PropagationPolicy,
    };

    // --- Test 1: insert multiple values per key ---

    /// Inserting multiple values for the same key and reading them back.
    #[test]
    fn insert_multiple_values_per_key() {
        let tm: TieredMultiMap<
            String,
            i32,
            PdsHashMultiMapBackend<String, i32>,
            PdsHashMultiMapBackend<String, i32>,
        > = TieredMultiMap::new(
            PdsHashMultiMapBackend::new(),
            PdsHashMultiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("fruits".to_string(), 1);
        tm.insert("fruits".to_string(), 2);
        tm.insert("fruits".to_string(), 3);

        let mut vals = tm.get_all(&"fruits".to_string());
        vals.sort();
        assert_eq!(vals, vec![1, 2, 3]);
    }

    // --- Test 2: get_all spanning tiers ---

    /// Insert some values, flush (move to cold), insert more. `get_all` should
    /// merge hot and cold.
    #[test]
    fn get_all_spanning_tiers() {
        let tm: TieredMultiMap<
            String,
            i32,
            PdsHashMultiMapBackend<String, i32>,
            PdsHashMultiMapBackend<String, i32>,
        > = TieredMultiMap::new(
            PdsHashMultiMapBackend::new(),
            PdsHashMultiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("k".to_string(), 10);
        tm.insert("k".to_string(), 20);
        tm.flush(); // cold: k→{10,20}

        tm.insert("k".to_string(), 30); // hot: k→{30}

        let mut vals = tm.get_all(&"k".to_string());
        vals.sort();
        assert_eq!(vals, vec![10, 20, 30]);
    }

    // --- Test 3: remove_entry ---

    /// Remove a specific (key, value) pair.
    #[test]
    fn remove_entry_works() {
        let tm: TieredMultiMap<
            String,
            i32,
            PdsHashMultiMapBackend<String, i32>,
            PdsHashMultiMapBackend<String, i32>,
        > = TieredMultiMap::new(
            PdsHashMultiMapBackend::new(),
            PdsHashMultiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("k".to_string(), 1);
        tm.insert("k".to_string(), 2);
        tm.flush(); // cold: k→{1,2}

        let removed = tm.remove_entry(&"k".to_string(), &1);
        assert!(removed);

        let vals = tm.get_all(&"k".to_string());
        assert!(!vals.contains(&1));
        assert!(vals.contains(&2));

        // Flush applies the pending entry remove to cold.
        tm.flush();
        let cold = tm.cold_snapshot();
        let cold_vals = cold.get_all(&"k".to_string());
        assert!(!cold_vals.contains(&1));
        assert!(cold_vals.contains(&2));
    }

    // --- Test 4: remove_key ---

    /// Remove all values for a key.
    #[test]
    fn remove_key_works() {
        let tm: TieredMultiMap<
            String,
            i32,
            PdsHashMultiMapBackend<String, i32>,
            PdsHashMultiMapBackend<String, i32>,
        > = TieredMultiMap::new(
            PdsHashMultiMapBackend::new(),
            PdsHashMultiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("a".to_string(), 1);
        tm.insert("a".to_string(), 2);
        tm.insert("b".to_string(), 99);
        tm.flush(); // cold: a→{1,2}, b→{99}

        let removed = tm.remove_key(&"a".to_string());
        assert!(removed);
        assert!(tm.get_all(&"a".to_string()).is_empty());
        // "b" should still be present.
        assert!(!tm.get_all(&"b".to_string()).is_empty());

        // After flush, cold should have no values for "a".
        tm.flush();
        let cold = tm.cold_snapshot();
        assert!(cold.get_all(&"a".to_string()).is_empty());
        assert!(!cold.get_all(&"b".to_string()).is_empty());
    }

    // --- Test 5: flush merges hot values into cold (union) ---

    /// Hot values are unioned into cold on flush, not replacing cold.
    #[test]
    fn flush_unions_values() {
        let tm: TieredMultiMap<
            String,
            i32,
            PdsHashMultiMapBackend<String, i32>,
            PdsHashMultiMapBackend<String, i32>,
        > = TieredMultiMap::new(
            PdsHashMultiMapBackend::new(),
            PdsHashMultiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert("k".to_string(), 1);
        tm.flush(); // cold: k→{1}

        tm.insert("k".to_string(), 2);
        tm.flush(); // cold: k→{1,2}

        let cold = tm.cold_snapshot();
        let mut vals = cold.get_all(&"k".to_string());
        vals.sort();
        assert_eq!(vals, vec![1, 2]);
    }

    // --- Test 6: OrdMultiMap backend ---

    /// OrdMultiMap backend works for ordered key iteration.
    #[test]
    fn ord_multimap_backend() {
        let tm: TieredMultiMap<
            i32,
            i32,
            PdsOrdMultiMapBackend<i32, i32>,
            PdsOrdMultiMapBackend<i32, i32>,
        > = TieredMultiMap::new(
            PdsOrdMultiMapBackend::new(),
            PdsOrdMultiMapBackend::new(),
            PropagationPolicy::Manual,
        );

        tm.insert(3, 30);
        tm.insert(1, 10);
        tm.insert(2, 20);
        tm.insert(1, 11); // key 1 has values {10, 11}

        let mut vals_1 = tm.get_all(&1);
        vals_1.sort();
        assert_eq!(vals_1, vec![10, 11]);
        assert_eq!(tm.get_all(&2), vec![20]);
        assert_eq!(tm.get_all(&3), vec![30]);
    }
}
