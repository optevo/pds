//! Tests for T.12 — `TrieBackend`, `PdsTrieBackend`, `PdsOrdTrieBackend`,
//! `TieredTrie`, and `TieredOrdTrie`.

#[cfg(test)]
mod tests {
    use super::super::{
        policy::PropagationPolicy,
        trie::TieredTrie,
        trie_backend::TrieBackend,
        trie_backends::{PdsOrdTrieBackend, PdsTrieBackend},
    };

    // --- Test 1: exact-key insert and get from hot ---

    /// Insertions are immediately visible via `get` before any flush.
    #[test]
    fn insert_and_get_hot() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["a", "b", "c"], 1);
        assert_eq!(tt.get(&["a", "b", "c"]), Some(1));
        assert_eq!(tt.get(&["a", "b"]), None);
        assert_eq!(tt.get(&["a"]), None);
    }

    // --- Test 2: cold fallback after flush ---

    /// After a flush, values are visible via cold-tier lookup.
    #[test]
    fn cold_fallback_after_flush() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["x", "y"], 42);
        tt.flush();

        // Hot is now empty; cold should answer.
        assert_eq!(tt.get(&["x", "y"]), Some(42));
        assert!(tt.hot_snapshot().is_empty());
    }

    // --- Test 3: hot wins over cold on exact path ---

    /// When the same path exists in both tiers, hot's value is returned.
    #[test]
    fn hot_wins_over_cold() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["p", "q"], 10);
        tt.flush(); // cold: p/q=10

        tt.insert(vec!["p", "q"], 99); // hot: p/q=99

        assert_eq!(tt.get(&["p", "q"]), Some(99));
    }

    // --- Test 4: remove suppresses cold ---

    /// Removing a cold path hides it before the next flush.
    #[test]
    fn remove_suppresses_cold() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["gone"], 7);
        tt.flush();

        let prev = tt.remove(&["gone"]);
        assert_eq!(prev, Some(7));
        assert_eq!(tt.get(&["gone"]), None);
    }

    // --- Test 5: flush applies pending deletes ---

    /// After a flush following a remove, the path is absent from cold.
    #[test]
    fn flush_applies_pending_deletes() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["k"], 5);
        tt.flush();
        tt.remove(&["k"]);
        tt.flush();

        let cold = tt.cold_snapshot();
        assert!(cold.get(&["k"]).is_none());
    }

    // --- Test 6: prefix_get merges hot and cold ---

    /// `prefix_get` returns entries from both tiers under a shared prefix.
    #[test]
    fn prefix_get_merges_tiers() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["a", "b"], 1);
        tt.insert(vec!["a", "c"], 2);
        tt.flush(); // cold: a/b=1, a/c=2

        tt.insert(vec!["a", "d"], 3); // hot: a/d=3

        let mut results = tt.prefix_get(&["a"]);
        results.sort_by_key(|(p, _)| p.clone());

        assert_eq!(results.len(), 3);
        assert_eq!(results[0], (vec!["a", "b"], 1));
        assert_eq!(results[1], (vec!["a", "c"], 2));
        assert_eq!(results[2], (vec!["a", "d"], 3));
    }

    // --- Test 7: hot wins in prefix_get ---

    /// When hot and cold both have a path, hot's value appears exactly once.
    #[test]
    fn prefix_get_hot_wins() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["r", "s"], 10);
        tt.flush(); // cold: r/s=10

        tt.insert(vec!["r", "s"], 20); // hot: r/s=20

        let results = tt.prefix_get(&["r"]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, 20); // hot wins
    }

    // --- Test 8: pending-deleted paths excluded from prefix_get ---

    /// Paths with a pending delete are excluded from `prefix_get` results.
    #[test]
    fn prefix_get_excludes_pending_deletes() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["ns", "keep"], 1);
        tt.insert(vec!["ns", "drop"], 2);
        tt.flush();

        tt.remove(&["ns", "drop"]);

        let results = tt.prefix_get(&["ns"]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], (vec!["ns", "keep"], 1));
    }

    // --- Test 9: contains_path ---

    /// `contains_path` correctly reflects hot, cold, and pending-delete state.
    #[test]
    fn contains_path() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["m"], 1);
        assert!(tt.contains_path(&["m"]));

        tt.flush();
        assert!(tt.contains_path(&["m"])); // in cold

        tt.remove(&["m"]);
        assert!(!tt.contains_path(&["m"])); // pending delete suppresses
    }

    // --- Test 10: Immediate policy ---

    /// With `Immediate` policy every insert is flushed to cold at once.
    #[test]
    fn immediate_policy() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Immediate,
            );

        tt.insert(vec!["auto"], 99);
        let cold = tt.cold_snapshot();
        assert_eq!(cold.get(&["auto"]), Some(99));
        assert!(tt.hot_snapshot().is_empty());
    }

    // --- Test 11: OrdTrie backend ---

    /// `PdsOrdTrieBackend` works under `TieredTrie`.
    #[test]
    fn ord_trie_backend() {
        let tt: TieredTrie<&str, i32, PdsOrdTrieBackend<&str, i32>, PdsOrdTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsOrdTrieBackend::new(),
                PdsOrdTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        tt.insert(vec!["a"], 1);
        tt.insert(vec!["b"], 2);
        tt.flush();
        tt.insert(vec!["c"], 3);

        assert_eq!(tt.get(&["a"]), Some(1));
        assert_eq!(tt.get(&["b"]), Some(2));
        assert_eq!(tt.get(&["c"]), Some(3));

        let mut results = tt.prefix_get(&[]);
        results.sort_by_key(|(p, _)| p.clone());
        assert_eq!(results.len(), 3);
    }

    // --- Test 12: is_empty and len ---

    /// `is_empty` and `len` reflect combined hot+cold state.
    #[test]
    fn is_empty_and_len() {
        let tt: TieredTrie<&str, i32, PdsTrieBackend<&str, i32>, PdsTrieBackend<&str, i32>> =
            TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );

        assert!(tt.is_empty());

        tt.insert(vec!["a"], 1);
        assert!(!tt.is_empty());
        assert_eq!(tt.len(), 1);

        tt.flush();
        // hot empty, cold has 1
        assert_eq!(tt.len(), 1);

        tt.insert(vec!["b"], 2);
        // hot=1, cold=1 → approx 2
        assert_eq!(tt.len(), 2);
    }
}
