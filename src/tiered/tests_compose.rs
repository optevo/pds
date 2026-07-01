//! Phase T.0d composability smoke tests.
//!
//! Verifies that every combination in the composability matrix compiles and
//! produces correct results. Each test inserts a handful of entries, flushes,
//! and verifies the cold snapshot contains the expected data.

#[cfg(test)]
mod tests {
    use crate::tiered::{
        backend::CollectionBackend,
        backends::{PdsHashMapBackend, PdsOrdMapBackend, StdBTreeMapBackend, StdHashMapBackend},
        policy::PropagationPolicy,
        sequence::TieredSequence,
        sequence_backend::SequenceBackend,
        sequence_backends::{PdsVectorBackend, StdVecBackend},
        TieredCollection, TieredOrdMap, TieredVector,
    };

    // -----------------------------------------------------------------------
    // Map combinations
    // -----------------------------------------------------------------------

    /// `StdHashMapBackend` → `PdsHashMapBackend` (the canonical two-tier map).
    #[test]
    fn compose_std_hash_to_pds_hash() {
        let tc: TieredCollection<
            i32,
            &str,
            StdHashMapBackend<i32, &str>,
            PdsHashMapBackend<i32, &str>,
        > = TieredCollection::new(
            StdHashMapBackend::new(),
            PdsHashMapBackend::new(),
            PropagationPolicy::Manual,
        );
        for i in 0..5i32 {
            tc.insert(i, "v");
        }
        tc.flush();
        let cold = tc.cold_snapshot();
        for i in 0..5i32 {
            assert_eq!(cold.get(&i), Some("v"), "missing key {i}");
        }
    }

    /// `StdHashMapBackend` → `MerkleWrapperBackend` (requires `traits` feature).
    #[cfg(feature = "traits")]
    #[test]
    fn compose_std_hash_to_merkle() {
        use crate::tiered::backends::MerkleWrapperBackend;
        let tc: TieredCollection<
            i32,
            i32,
            StdHashMapBackend<i32, i32>,
            MerkleWrapperBackend<i32, i32>,
        > = TieredCollection::new(
            StdHashMapBackend::new(),
            MerkleWrapperBackend::new(),
            PropagationPolicy::Manual,
        );
        for i in 0..3i32 {
            tc.insert(i, i * 10);
        }
        tc.flush();
        let cold = tc.cold_snapshot();
        for i in 0..3i32 {
            assert_eq!(cold.get(&i), Some(i * 10), "missing key {i}");
        }
    }

    /// `PdsHashMapBackend` → `MerkleWrapperBackend` (requires `traits` feature).
    #[cfg(feature = "traits")]
    #[test]
    fn compose_pds_hash_to_merkle() {
        use crate::tiered::backends::MerkleWrapperBackend;
        let tc: TieredCollection<
            i32,
            i32,
            PdsHashMapBackend<i32, i32>,
            MerkleWrapperBackend<i32, i32>,
        > = TieredCollection::new(
            PdsHashMapBackend::new(),
            MerkleWrapperBackend::new(),
            PropagationPolicy::Manual,
        );
        for i in 0..3i32 {
            tc.insert(i, i * 100);
        }
        tc.flush();
        let cold = tc.cold_snapshot();
        for i in 0..3i32 {
            assert_eq!(cold.get(&i), Some(i * 100), "missing key {i}");
        }
    }

    /// Three-tier: `StdHashMapBackend` → `TieredCollection<PdsHashMap, Merkle>`
    /// (requires `traits` feature).
    #[cfg(feature = "traits")]
    #[test]
    fn compose_std_hash_to_tiered_pds_merkle() {
        use crate::tiered::backends::MerkleWrapperBackend;
        type Mid = TieredCollection<
            i32,
            i32,
            PdsHashMapBackend<i32, i32>,
            MerkleWrapperBackend<i32, i32>,
        >;
        type Outer = TieredCollection<i32, i32, StdHashMapBackend<i32, i32>, Mid>;

        let mid: Mid = TieredCollection::new(
            PdsHashMapBackend::new(),
            MerkleWrapperBackend::new(),
            PropagationPolicy::Manual,
        );
        let outer: Outer = TieredCollection::new(
            StdHashMapBackend::new(),
            mid,
            PropagationPolicy::Manual,
        );

        for i in 0..4i32 {
            outer.insert(i, i * 7);
        }
        outer.flush(); // outer hot → mid hot
        {
            let mid_snap = outer.cold_snapshot();
            mid_snap.flush(); // mid hot → Merkle cold
        }
        let mid_snap = outer.cold_snapshot();
        let merkle_snap = mid_snap.cold_snapshot();
        for i in 0..4i32 {
            assert_eq!(merkle_snap.get(&i), Some(i * 7), "missing key {i} in merkle cold");
        }
    }

    // -----------------------------------------------------------------------
    // OrdMap combinations
    // -----------------------------------------------------------------------

    /// `StdBTreeMapBackend` → `PdsOrdMapBackend` (canonical ordered two-tier).
    #[test]
    fn compose_btree_to_pds_ord() {
        let tc: TieredOrdMap<
            i32,
            &str,
            StdBTreeMapBackend<i32, &str>,
            PdsOrdMapBackend<i32, &str>,
        > = TieredCollection::new(
            StdBTreeMapBackend::new(),
            PdsOrdMapBackend::new(),
            PropagationPolicy::Manual,
        );
        for i in 0..5i32 {
            tc.insert(i, "v");
        }
        tc.flush();
        let cold = tc.cold_snapshot();
        for i in 0..5i32 {
            assert_eq!(cold.get(&i), Some("v"), "missing key {i}");
        }
    }

    /// `PdsOrdMapBackend` → `PdsOrdMapBackend`.
    #[test]
    fn compose_pds_ord_to_pds_ord() {
        let tc: TieredOrdMap<
            i32,
            i32,
            PdsOrdMapBackend<i32, i32>,
            PdsOrdMapBackend<i32, i32>,
        > = TieredCollection::new(
            PdsOrdMapBackend::new(),
            PdsOrdMapBackend::new(),
            PropagationPolicy::Manual,
        );
        for i in 0..5i32 {
            tc.insert(i, i * 3);
        }
        tc.flush();
        let cold = tc.cold_snapshot();
        for i in 0..5i32 {
            assert_eq!(cold.get(&i), Some(i * 3), "missing key {i}");
        }
    }

    /// Three-tier: `StdBTreeMap` → `TieredOrdMap<PdsOrd, PdsOrd>`.
    #[test]
    fn compose_btree_to_tiered_pds_ord_pds_ord() {
        type Mid = TieredOrdMap<
            i32,
            i32,
            StdBTreeMapBackend<i32, i32>,
            PdsOrdMapBackend<i32, i32>,
        >;
        type Outer =
            TieredOrdMap<i32, i32, StdBTreeMapBackend<i32, i32>, Mid>;

        let mid: Mid = TieredCollection::new(
            StdBTreeMapBackend::new(),
            PdsOrdMapBackend::new(),
            PropagationPolicy::Manual,
        );
        let outer: Outer = TieredCollection::new(
            StdBTreeMapBackend::new(),
            mid,
            PropagationPolicy::Manual,
        );

        for i in 0..5i32 {
            outer.insert(i, i * 11);
        }
        outer.flush(); // outer hot → mid hot
        {
            let mid_snap = outer.cold_snapshot();
            mid_snap.flush(); // mid hot → mid cold (PdsOrdMap)
        }
        let mid_snap = outer.cold_snapshot();
        let cold = mid_snap.cold_snapshot();
        for i in 0..5i32 {
            assert_eq!(cold.get(&i), Some(i * 11), "missing key {i} in mid cold");
        }
    }

    // -----------------------------------------------------------------------
    // Vector (sequence) combinations
    // -----------------------------------------------------------------------

    /// `StdVecBackend` → `PdsVectorBackend` (canonical two-tier vector).
    #[test]
    fn compose_std_vec_to_pds_vec() {
        let ts: TieredVector<i32, StdVecBackend<i32>, PdsVectorBackend<i32>> =
            TieredSequence::new(StdVecBackend::new(), PdsVectorBackend::new(), PropagationPolicy::Manual);
        for i in 0..5i32 {
            ts.push_back(i);
        }
        ts.flush();
        let cold = ts.cold_snapshot();
        assert_eq!(cold.len(), 5);
        for i in 0..5usize {
            assert_eq!(cold.get(i), Some(i as i32));
        }
    }

    /// `PdsVectorBackend` → `PdsVectorBackend`.
    #[test]
    fn compose_pds_vec_to_pds_vec() {
        let ts: TieredVector<i32, PdsVectorBackend<i32>, PdsVectorBackend<i32>> =
            TieredSequence::new(PdsVectorBackend::new(), PdsVectorBackend::new(), PropagationPolicy::Manual);
        for i in 0..5i32 {
            ts.push_back(i);
        }
        ts.flush();
        let cold = ts.cold_snapshot();
        assert_eq!(cold.len(), 5);
    }

    /// Three-tier: `StdVec` → `TieredVector<PdsVec, PdsVec>`.
    #[test]
    fn compose_std_vec_to_tiered_pds_vec_pds_vec() {
        type Inner = TieredVector<i32, PdsVectorBackend<i32>, PdsVectorBackend<i32>>;
        type Outer = TieredVector<i32, StdVecBackend<i32>, Inner>;

        let inner: Inner = TieredSequence::new(
            PdsVectorBackend::new(),
            PdsVectorBackend::new(),
            PropagationPolicy::Manual,
        );
        let outer: Outer = TieredSequence::new(
            StdVecBackend::new(),
            inner,
            PropagationPolicy::Manual,
        );

        for i in 0..5i32 {
            outer.push_back(i);
        }
        outer.flush(); // outer hot → inner hot
        {
            let inner_snap = outer.cold_snapshot();
            inner_snap.flush(); // inner hot → inner cold
        }
        let inner_snap = outer.cold_snapshot();
        let cold = inner_snap.cold_snapshot();
        assert_eq!(cold.len(), 5);
        for i in 0..5usize {
            assert_eq!(cold.get(i), Some(i as i32));
        }
    }
}
