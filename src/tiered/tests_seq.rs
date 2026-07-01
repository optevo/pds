//! Tests for Phase T.0c — Vector tiered backends.

#[cfg(test)]
mod tests {
    use crate::tiered::{
        policy::PropagationPolicy,
        sequence::TieredSequence,
        sequence_backend::SequenceBackend,
        sequence_backends::{PdsVectorBackend, StdVecBackend},
        TieredVector,
    };

    // --- Type alias for brevity ---

    type VecPds = TieredVector<i32, StdVecBackend<i32>, PdsVectorBackend<i32>>;

    fn vec_pds(policy: PropagationPolicy) -> VecPds {
        TieredSequence::new(StdVecBackend::new(), PdsVectorBackend::new(), policy)
    }

    // --- Test 1: push_back then get(0) — readable from hot before flush ---

    /// Elements pushed to hot are immediately visible via `get` without flushing.
    #[test]
    fn push_back_get_from_hot() {
        let ts = vec_pds(PropagationPolicy::Manual);
        ts.push_back(10);
        ts.push_back(20);
        assert_eq!(ts.get(0), Some(10));
        assert_eq!(ts.get(1), Some(20));
        // Cold snapshot should be empty — no flush has occurred.
        let cold = ts.cold_snapshot();
        assert!(cold.is_empty());
    }

    // --- Test 2: push_back then flush — visible in cold_snapshot ---

    /// After a flush, elements appear in the cold tier's snapshot.
    #[test]
    fn push_back_then_flush_visible_in_cold() {
        let ts = vec_pds(PropagationPolicy::Manual);
        ts.push_back(1);
        ts.push_back(2);
        ts.push_back(3);
        ts.flush();
        let cold = ts.cold_snapshot();
        assert_eq!(cold.len(), 3);
        assert_eq!(cold.get(0), Some(1));
        assert_eq!(cold.get(2), Some(3));
    }

    // --- Test 3: get(i) spanning cold + hot ---

    /// After partial flush, indices spanning cold and hot resolve correctly.
    #[test]
    fn get_spanning_cold_and_hot() {
        let ts = vec_pds(PropagationPolicy::Manual);
        // Push 0..5 and flush to cold.
        for i in 0..5i32 {
            ts.push_back(i);
        }
        ts.flush();
        // Push 5..8 to hot (uncommitted).
        for i in 5..8i32 {
            ts.push_back(i);
        }

        let cold_len = ts.cold_snapshot().len();
        assert_eq!(cold_len, 5);

        // Boundary: cold[4] and hot[0].
        assert_eq!(ts.get(cold_len - 1), Some(4)); // last cold entry
        assert_eq!(ts.get(cold_len), Some(5)); // first hot entry
        assert_eq!(ts.get(7), Some(7)); // last hot entry
        assert_eq!(ts.get(8), None); // out of bounds
    }

    // --- Test 4: pop_back from hot; when hot empty drains cold ---

    /// pop_back drains hot first; when hot is empty it drains cold.
    #[test]
    fn pop_back_hot_then_cold() {
        let ts = vec_pds(PropagationPolicy::Manual);
        ts.push_back(1);
        ts.push_back(2);
        ts.flush(); // 1, 2 → cold
        ts.push_back(3); // 3 → hot

        // Pop 3 from hot.
        assert_eq!(ts.pop_back(), Some(3));
        // Pop 2 from cold (hot is now empty).
        assert_eq!(ts.pop_back(), Some(2));
        // Pop 1 from cold.
        assert_eq!(ts.pop_back(), Some(1));
        // Both tiers empty.
        assert_eq!(ts.pop_back(), None);
        assert!(ts.is_empty());
    }

    // --- Test 5: Batched(3) auto-flush after 3 push_backs ---

    /// With `Batched(3)`, the third `push_back` triggers an automatic flush.
    #[test]
    fn batched_auto_flush_sequence() {
        let ts = vec_pds(PropagationPolicy::Batched(3));
        ts.push_back(10);
        ts.push_back(20);
        // Cold still empty after two pushes.
        assert!(ts.cold_snapshot().is_empty());
        // Third push triggers flush.
        ts.push_back(30);
        let cold = ts.cold_snapshot();
        assert_eq!(cold.len(), 3);
        assert_eq!(cold.get(0), Some(10));
        assert_eq!(cold.get(2), Some(30));
    }

    // --- Test 6: Concurrent push_back from two threads ---

    /// Two threads push 50 elements each; all 100 are present after flush.
    #[test]
    fn concurrent_push_back() {
        let ts = vec_pds(PropagationPolicy::Manual);
        let ts_a = ts.clone();
        let ts_b = ts.clone();

        let t1 = std::thread::spawn(move || {
            for i in 0..50i32 {
                ts_a.push_back(i);
            }
        });
        let t2 = std::thread::spawn(move || {
            for i in 50..100i32 {
                ts_b.push_back(i);
            }
        });
        t1.join().expect("thread 1 panicked");
        t2.join().expect("thread 2 panicked");

        ts.flush();
        let cold = ts.cold_snapshot();
        assert_eq!(cold.len(), 100);

        // All values 0..100 must be present (order is non-deterministic).
        let mut all: Vec<i32> = (0..cold.len()).map(|i| cold.get(i).unwrap()).collect();
        all.sort();
        let expected: Vec<i32> = (0..100).collect();
        assert_eq!(all, expected);
    }

    // --- Test 7: Three-tier StdVec → PdsVector → PdsVector ---

    /// A TieredSequence can itself act as a tier in another TieredSequence,
    /// enabling three-tier compositions.
    #[test]
    fn three_tier_sequence_composition() {
        use crate::tiered::sequence_backend::SequenceBackend;

        type Inner = TieredVector<i32, StdVecBackend<i32>, PdsVectorBackend<i32>>;
        type Outer = TieredVector<i32, StdVecBackend<i32>, Inner>;

        let inner: Inner = TieredSequence::new(
            StdVecBackend::new(),
            PdsVectorBackend::new(),
            PropagationPolicy::Manual,
        );
        let mut outer: Outer =
            TieredSequence::new(StdVecBackend::new(), inner, PropagationPolicy::Manual);

        for i in 0..5i32 {
            outer.push_back(i);
        }
        // Flush outer hot → inner hot.
        outer.flush();
        // Flush inner hot → inner cold.
        {
            let inner_snap = outer.cold_snapshot();
            inner_snap.flush();
        }

        // Now drain outer (which internally flushes both tiers).
        let all: Vec<i32> = SequenceBackend::drain(&mut outer);
        let mut sorted = all.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
    }

    // --- Test 8: TieredSequence::len is exact ---

    /// `len` returns `cold.len() + hot.len()`, which is exact for sequences.
    #[test]
    fn len_is_exact() {
        let ts = vec_pds(PropagationPolicy::Manual);
        assert_eq!(ts.len(), 0);
        ts.push_back(1);
        ts.push_back(2);
        assert_eq!(ts.len(), 2);
        ts.flush();
        assert_eq!(ts.len(), 2);
        ts.push_back(3);
        assert_eq!(ts.len(), 3);
    }
}
