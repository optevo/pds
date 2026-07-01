//! Tests for T.10 — `SymMapBackend` and `TieredSymMap`.

#[cfg(test)]
mod tests {
    use super::super::{
        policy::PropagationPolicy,
        symmap::TieredSymMap,
        symmap_backend::{SymMapBackend, SymMapDirection},
        symmap_backends::{PdsOrdSymMapBackend, PdsSymMapBackend},
    };

    // --- Test 1: insert and get in both directions ---

    /// Inserting `(a, b)` makes it look-up-able in both directions.
    #[test]
    fn insert_and_get_both_directions() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("hello".to_string(), "hola".to_string());

        assert_eq!(
            ts.get(SymMapDirection::Forward, &"hello".to_string()),
            Some("hola".to_string())
        );
        assert_eq!(
            ts.get(SymMapDirection::Backward, &"hola".to_string()),
            Some("hello".to_string())
        );
        assert_eq!(
            ts.get(SymMapDirection::Forward, &"missing".to_string()),
            None
        );
    }

    // --- Test 2: cold fallback ---

    /// After a flush, lookups should fall back to cold.
    #[test]
    fn cold_fallback() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("cat".to_string(), "gato".to_string());
        ts.flush(); // cold: cat↔gato

        assert_eq!(
            ts.get(SymMapDirection::Forward, &"cat".to_string()),
            Some("gato".to_string())
        );
        assert_eq!(
            ts.get(SymMapDirection::Backward, &"gato".to_string()),
            Some("cat".to_string())
        );
    }

    // --- Test 3: hot wins over cold ---

    /// A hot insert for an existing cold key shadows the cold value.
    #[test]
    fn hot_wins_over_cold() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("dog".to_string(), "perro".to_string());
        ts.flush(); // cold: dog↔perro

        ts.insert("dog".to_string(), "canino".to_string()); // hot overrides
        assert_eq!(
            ts.get(SymMapDirection::Forward, &"dog".to_string()),
            Some("canino".to_string())
        );
    }

    // --- Test 4: remove forward suppresses cold ---

    /// Removing by forward key suppresses the cold entry and partner.
    #[test]
    fn remove_forward_suppresses_cold() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("bird".to_string(), "pajaro".to_string());
        ts.flush(); // cold: bird↔pajaro

        let partner = ts.remove(SymMapDirection::Forward, &"bird".to_string());
        assert_eq!(partner, Some("pajaro".to_string()));

        assert_eq!(ts.get(SymMapDirection::Forward, &"bird".to_string()), None);
        assert_eq!(
            ts.get(SymMapDirection::Backward, &"pajaro".to_string()),
            None
        );

        // After flush, cold should also not contain the pair.
        ts.flush();
        let cold = ts.cold_snapshot();
        assert!(cold
            .get(SymMapDirection::Forward, &"bird".to_string())
            .is_none());
        assert!(cold
            .get(SymMapDirection::Backward, &"pajaro".to_string())
            .is_none());
    }

    // --- Test 5: remove backward suppresses cold ---

    /// Removing by backward key suppresses the cold entry and forward partner.
    #[test]
    fn remove_backward_suppresses_cold() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("fish".to_string(), "pez".to_string());
        ts.flush(); // cold: fish↔pez

        let partner = ts.remove(SymMapDirection::Backward, &"pez".to_string());
        assert_eq!(partner, Some("fish".to_string()));

        assert_eq!(ts.get(SymMapDirection::Forward, &"fish".to_string()), None);
        assert_eq!(ts.get(SymMapDirection::Backward, &"pez".to_string()), None);
    }

    // --- Test 6: flush propagates hot to cold ---

    /// Hot pairs are transferred to cold on flush.
    #[test]
    fn flush_propagates_to_cold() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("apple".to_string(), "manzana".to_string());
        ts.insert("pear".to_string(), "pera".to_string());
        ts.flush();

        let cold = ts.cold_snapshot();
        assert_eq!(
            cold.get(SymMapDirection::Forward, &"apple".to_string()),
            Some("manzana".to_string())
        );
        assert_eq!(
            cold.get(SymMapDirection::Backward, &"pera".to_string()),
            Some("pear".to_string())
        );
    }

    // --- Test 7: Immediate policy auto-flushes ---

    /// With `Immediate` policy, each insert flushes straight to cold.
    #[test]
    fn immediate_policy() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Immediate,
            );

        ts.insert("moon".to_string(), "luna".to_string());
        let cold = ts.cold_snapshot();
        assert_eq!(
            cold.get(SymMapDirection::Forward, &"moon".to_string()),
            Some("luna".to_string())
        );
        assert!(ts.hot_snapshot().is_empty());
    }

    // --- Test 8: OrdSymMap backend ---

    /// `PdsOrdSymMapBackend` works correctly via `TieredSymMap`.
    #[test]
    fn ord_symmap_backend() {
        let ts: TieredSymMap<String, PdsOrdSymMapBackend<String>, PdsOrdSymMapBackend<String>> =
            TieredSymMap::new(
                PdsOrdSymMapBackend::new(),
                PdsOrdSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("sun".to_string(), "sol".to_string());
        ts.insert("star".to_string(), "estrella".to_string());

        assert_eq!(
            ts.get(SymMapDirection::Forward, &"sun".to_string()),
            Some("sol".to_string())
        );
        assert_eq!(
            ts.get(SymMapDirection::Backward, &"estrella".to_string()),
            Some("star".to_string())
        );

        ts.flush();
        let cold = ts.cold_snapshot();
        assert_eq!(
            cold.get(SymMapDirection::Forward, &"sun".to_string()),
            Some("sol".to_string())
        );
    }

    // --- Test 9: re-insert after remove cancels pending remove ---

    /// Inserting a pair that was removed should restore visibility.
    #[test]
    fn reinsert_after_remove() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("rain".to_string(), "lluvia".to_string());
        ts.flush(); // cold: rain↔lluvia

        ts.remove(SymMapDirection::Forward, &"rain".to_string());
        assert_eq!(ts.get(SymMapDirection::Forward, &"rain".to_string()), None);

        ts.insert("rain".to_string(), "precipitacion".to_string());
        assert_eq!(
            ts.get(SymMapDirection::Forward, &"rain".to_string()),
            Some("precipitacion".to_string())
        );
    }

    // --- Test 10: contains forwards to get ---

    /// `contains` returns true iff the key is reachable in the given direction.
    #[test]
    fn contains_reflects_get() {
        let ts: TieredSymMap<String, PdsSymMapBackend<String>, PdsSymMapBackend<String>> =
            TieredSymMap::new(
                PdsSymMapBackend::new(),
                PdsSymMapBackend::new(),
                PropagationPolicy::Manual,
            );

        ts.insert("fire".to_string(), "fuego".to_string());
        assert!(ts.contains(SymMapDirection::Forward, &"fire".to_string()));
        assert!(ts.contains(SymMapDirection::Backward, &"fuego".to_string()));
        assert!(!ts.contains(SymMapDirection::Forward, &"water".to_string()));
    }
}
