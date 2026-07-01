//! Tiered collection benchmarks — Phase T.0d + new-type coverage (T.6–T.13).
//!
//! Covers all compositions from the spec plus new types:
//! 1. `StdHashMap → PdsHashMap` (TieredCollection)
//! 2. `StdBTreeMap → PdsOrdMap` (TieredOrdMap)
//! 3. `StdVec → PdsVector` (TieredVector)
//! 4. `StdHashMap → TieredCollection<PdsHashMap, MerkleWrapper>` (3-tier, cfg traits)
//! 5. TieredSet (StdHashSet → PdsHashSet)
//! 6. TieredBag (PdsBag → PdsBag)
//! 7. TieredMultiMap (PdsHashMultiMap → PdsHashMultiMap)
//! 8. TieredBiMap (PdsBiMap → PdsBiMap)
//! 9. TieredSymMap (PdsSymMap → PdsSymMap)
//! 10. TieredInsertionOrderMap (PdsInsertionOrderMap → PdsInsertionOrderMap)
//! 11. TieredTrie (PdsTrie → PdsTrie)
//! 12. TieredUniqueVector (PdsUniqueVec → PdsUniqueVec)
//!
//! Operations per composition:
//! - `insert_n` at n = 100, 1_000, 10_000 (Manual policy — pure hot-tier write cost)
//! - `get_hit` / `contains_hit` — read from hot (no flush)
//! - `get_cold_fallback` / `contains_cold_fallback` — read from cold (flush first)
//! - `flush_n` — flush 1_000 accumulated writes to cold
//! - `cold_snapshot` — clone the cold tier

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pds::tiered::{
    backends::{PdsHashMapBackend, PdsOrdMapBackend, StdBTreeMapBackend, StdHashMapBackend},
    bag_backends::PdsBagBackend,
    bimap_backends::PdsBiMapBackend,
    insertion_order_backends::PdsInsertionOrderMapBackend,
    multimap_backends::PdsHashMultiMapBackend,
    sequence::TieredSequence,
    sequence_backends::{PdsVectorBackend, StdVecBackend},
    set_backends::{PdsHashSetBackend, StdHashSetBackend},
    symmap_backend::SymMapDirection,
    symmap_backends::PdsSymMapBackend,
    trie_backends::PdsTrieBackend,
    unique_vec_backends::PdsUniqueVecBackend,
    PropagationPolicy, TieredBag, TieredBiMap, TieredCollection, TieredInsertionOrderMap,
    TieredMultiMap, TieredOrdMap, TieredSet, TieredSymMap, TieredTrie, TieredUniqueVector,
    TieredVector,
};

// -----------------------------------------------------------------------
// Composition 1: StdHashMap → PdsHashMap
// -----------------------------------------------------------------------

fn bench_hash_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_hash/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tc: TieredCollection<
                    usize,
                    usize,
                    StdHashMapBackend<usize, usize>,
                    PdsHashMapBackend<usize, usize>,
                > = TieredCollection::new(
                    StdHashMapBackend::new(),
                    PdsHashMapBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    tc.insert(std::hint::black_box(i), i);
                }
                tc
            });
        });
    }
    group.finish();
}

fn bench_hash_get_hit(c: &mut Criterion) {
    let tc: TieredCollection<
        usize,
        usize,
        StdHashMapBackend<usize, usize>,
        PdsHashMapBackend<usize, usize>,
    > = TieredCollection::new(
        StdHashMapBackend::new(),
        PdsHashMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tc.insert(i, i);
    }
    c.bench_function("tiered_hash/get_hit", |b| {
        b.iter(|| tc.get(std::hint::black_box(&500)));
    });
}

fn bench_hash_get_cold_fallback(c: &mut Criterion) {
    let tc: TieredCollection<
        usize,
        usize,
        StdHashMapBackend<usize, usize>,
        PdsHashMapBackend<usize, usize>,
    > = TieredCollection::new(
        StdHashMapBackend::new(),
        PdsHashMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tc.insert(i, i);
    }
    tc.flush(); // move all to cold
    c.bench_function("tiered_hash/get_cold_fallback", |b| {
        b.iter(|| tc.get(std::hint::black_box(&500)));
    });
}

fn bench_hash_flush(c: &mut Criterion) {
    c.bench_function("tiered_hash/flush_1000", |b| {
        b.iter(|| {
            let tc: TieredCollection<
                usize,
                usize,
                StdHashMapBackend<usize, usize>,
                PdsHashMapBackend<usize, usize>,
            > = TieredCollection::new(
                StdHashMapBackend::new(),
                PdsHashMapBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tc.insert(std::hint::black_box(i), i);
            }
            tc.flush();
            tc
        });
    });
}

fn bench_hash_cold_snapshot(c: &mut Criterion) {
    let tc: TieredCollection<
        usize,
        usize,
        StdHashMapBackend<usize, usize>,
        PdsHashMapBackend<usize, usize>,
    > = TieredCollection::new(
        StdHashMapBackend::new(),
        PdsHashMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tc.insert(i, i);
    }
    tc.flush();
    c.bench_function("tiered_hash/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tc.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// Composition 2: StdBTreeMap → PdsOrdMap
// -----------------------------------------------------------------------

fn bench_ord_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_ord/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tc: TieredOrdMap<
                    usize,
                    usize,
                    StdBTreeMapBackend<usize, usize>,
                    PdsOrdMapBackend<usize, usize>,
                > = TieredCollection::new(
                    StdBTreeMapBackend::new(),
                    PdsOrdMapBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    tc.insert(std::hint::black_box(i), i);
                }
                tc
            });
        });
    }
    group.finish();
}

fn bench_ord_get_hit(c: &mut Criterion) {
    let tc: TieredOrdMap<
        usize,
        usize,
        StdBTreeMapBackend<usize, usize>,
        PdsOrdMapBackend<usize, usize>,
    > = TieredCollection::new(
        StdBTreeMapBackend::new(),
        PdsOrdMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tc.insert(i, i);
    }
    c.bench_function("tiered_ord/get_hit", |b| {
        b.iter(|| tc.get(std::hint::black_box(&500)));
    });
}

fn bench_ord_get_cold_fallback(c: &mut Criterion) {
    let tc: TieredOrdMap<
        usize,
        usize,
        StdBTreeMapBackend<usize, usize>,
        PdsOrdMapBackend<usize, usize>,
    > = TieredCollection::new(
        StdBTreeMapBackend::new(),
        PdsOrdMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tc.insert(i, i);
    }
    tc.flush();
    c.bench_function("tiered_ord/get_cold_fallback", |b| {
        b.iter(|| tc.get(std::hint::black_box(&500)));
    });
}

fn bench_ord_flush(c: &mut Criterion) {
    c.bench_function("tiered_ord/flush_1000", |b| {
        b.iter(|| {
            let tc: TieredOrdMap<
                usize,
                usize,
                StdBTreeMapBackend<usize, usize>,
                PdsOrdMapBackend<usize, usize>,
            > = TieredCollection::new(
                StdBTreeMapBackend::new(),
                PdsOrdMapBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tc.insert(std::hint::black_box(i), i);
            }
            tc.flush();
            tc
        });
    });
}

fn bench_ord_cold_snapshot(c: &mut Criterion) {
    let tc: TieredOrdMap<
        usize,
        usize,
        StdBTreeMapBackend<usize, usize>,
        PdsOrdMapBackend<usize, usize>,
    > = TieredCollection::new(
        StdBTreeMapBackend::new(),
        PdsOrdMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tc.insert(i, i);
    }
    tc.flush();
    c.bench_function("tiered_ord/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tc.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// Composition 3: StdVec → PdsVector
// -----------------------------------------------------------------------

fn bench_vec_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_vec/push_back");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let ts: TieredVector<usize, StdVecBackend<usize>, PdsVectorBackend<usize>> =
                    TieredSequence::new(
                        StdVecBackend::new(),
                        PdsVectorBackend::new(),
                        PropagationPolicy::Manual,
                    );
                for i in 0..n {
                    ts.push_back(std::hint::black_box(i));
                }
                ts
            });
        });
    }
    group.finish();
}

fn bench_vec_get_hit(c: &mut Criterion) {
    let ts: TieredVector<usize, StdVecBackend<usize>, PdsVectorBackend<usize>> =
        TieredSequence::new(
            StdVecBackend::new(),
            PdsVectorBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        ts.push_back(i);
    }
    c.bench_function("tiered_vec/get_hit", |b| {
        b.iter(|| ts.get(std::hint::black_box(500)));
    });
}

fn bench_vec_get_cold_fallback(c: &mut Criterion) {
    let ts: TieredVector<usize, StdVecBackend<usize>, PdsVectorBackend<usize>> =
        TieredSequence::new(
            StdVecBackend::new(),
            PdsVectorBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        ts.push_back(i);
    }
    ts.flush();
    c.bench_function("tiered_vec/get_cold_fallback", |b| {
        b.iter(|| ts.get(std::hint::black_box(500)));
    });
}

fn bench_vec_flush(c: &mut Criterion) {
    c.bench_function("tiered_vec/flush_1000", |b| {
        b.iter(|| {
            let ts: TieredVector<usize, StdVecBackend<usize>, PdsVectorBackend<usize>> =
                TieredSequence::new(
                    StdVecBackend::new(),
                    PdsVectorBackend::new(),
                    PropagationPolicy::Manual,
                );
            for i in 0..1_000usize {
                ts.push_back(std::hint::black_box(i));
            }
            ts.flush();
            ts
        });
    });
}

fn bench_vec_cold_snapshot(c: &mut Criterion) {
    let ts: TieredVector<usize, StdVecBackend<usize>, PdsVectorBackend<usize>> =
        TieredSequence::new(
            StdVecBackend::new(),
            PdsVectorBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        ts.push_back(i);
    }
    ts.flush();
    c.bench_function("tiered_vec/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(ts.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// Composition 4: StdHashMap → TieredCollection<PdsHashMap, MerkleWrapper>
// (3-tier, requires `traits` feature)
// -----------------------------------------------------------------------

#[cfg(feature = "traits")]
fn bench_3tier_hash_insert(c: &mut Criterion) {
    use pds::tiered::backends::MerkleWrapperBackend;

    type Mid = TieredCollection<
        usize,
        usize,
        PdsHashMapBackend<usize, usize>,
        MerkleWrapperBackend<usize, usize>,
    >;
    type Outer = TieredCollection<usize, usize, StdHashMapBackend<usize, usize>, Mid>;

    let mut group = c.benchmark_group("tiered_3tier_hash/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mid: Mid = TieredCollection::new(
                    PdsHashMapBackend::new(),
                    MerkleWrapperBackend::new(),
                    PropagationPolicy::Manual,
                );
                let outer: Outer =
                    TieredCollection::new(StdHashMapBackend::new(), mid, PropagationPolicy::Manual);
                for i in 0..n {
                    outer.insert(std::hint::black_box(i), i);
                }
                outer
            });
        });
    }
    group.finish();
}

#[cfg(feature = "traits")]
fn bench_3tier_hash_get_hit(c: &mut Criterion) {
    use pds::tiered::backends::MerkleWrapperBackend;

    type Mid = TieredCollection<
        usize,
        usize,
        PdsHashMapBackend<usize, usize>,
        MerkleWrapperBackend<usize, usize>,
    >;
    type Outer = TieredCollection<usize, usize, StdHashMapBackend<usize, usize>, Mid>;

    let mid: Mid = TieredCollection::new(
        PdsHashMapBackend::new(),
        MerkleWrapperBackend::new(),
        PropagationPolicy::Manual,
    );
    let outer: Outer =
        TieredCollection::new(StdHashMapBackend::new(), mid, PropagationPolicy::Manual);
    for i in 0..1_000usize {
        outer.insert(i, i);
    }
    c.bench_function("tiered_3tier_hash/get_hit", |b| {
        b.iter(|| outer.get(std::hint::black_box(&500)));
    });
}

#[cfg(feature = "traits")]
fn bench_3tier_hash_flush(c: &mut Criterion) {
    use pds::tiered::backends::MerkleWrapperBackend;

    type Mid = TieredCollection<
        usize,
        usize,
        PdsHashMapBackend<usize, usize>,
        MerkleWrapperBackend<usize, usize>,
    >;
    type Outer = TieredCollection<usize, usize, StdHashMapBackend<usize, usize>, Mid>;

    c.bench_function("tiered_3tier_hash/flush_1000", |b| {
        b.iter(|| {
            let mid: Mid = TieredCollection::new(
                PdsHashMapBackend::new(),
                MerkleWrapperBackend::new(),
                PropagationPolicy::Manual,
            );
            let outer: Outer =
                TieredCollection::new(StdHashMapBackend::new(), mid, PropagationPolicy::Manual);
            for i in 0..1_000usize {
                outer.insert(std::hint::black_box(i), i);
            }
            outer.flush();
            outer
        });
    });
}

#[cfg(feature = "traits")]
fn bench_3tier_cold_snapshot(c: &mut Criterion) {
    use pds::tiered::backends::MerkleWrapperBackend;

    type Mid = TieredCollection<
        usize,
        usize,
        PdsHashMapBackend<usize, usize>,
        MerkleWrapperBackend<usize, usize>,
    >;
    type Outer = TieredCollection<usize, usize, StdHashMapBackend<usize, usize>, Mid>;

    let mid: Mid = TieredCollection::new(
        PdsHashMapBackend::new(),
        MerkleWrapperBackend::new(),
        PropagationPolicy::Manual,
    );
    let outer: Outer =
        TieredCollection::new(StdHashMapBackend::new(), mid, PropagationPolicy::Manual);
    for i in 0..1_000usize {
        outer.insert(i, i);
    }
    outer.flush();
    c.bench_function("tiered_3tier_hash/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(outer.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// Candidate 5: Immediate vs Manual policy overhead
// -----------------------------------------------------------------------

fn bench_immediate_vs_manual(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_hash/policy_overhead_1000");
    group.bench_function("manual", |b| {
        b.iter(|| {
            let tc: TieredCollection<
                usize,
                usize,
                StdHashMapBackend<usize, usize>,
                PdsHashMapBackend<usize, usize>,
            > = TieredCollection::new(
                StdHashMapBackend::new(),
                PdsHashMapBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tc.insert(std::hint::black_box(i), i);
            }
            tc
        });
    });
    group.bench_function("immediate", |b| {
        b.iter(|| {
            let tc: TieredCollection<
                usize,
                usize,
                StdHashMapBackend<usize, usize>,
                PdsHashMapBackend<usize, usize>,
            > = TieredCollection::new(
                StdHashMapBackend::new(),
                PdsHashMapBackend::new(),
                PropagationPolicy::Immediate,
            );
            for i in 0..1_000usize {
                tc.insert(std::hint::black_box(i), i);
            }
            tc
        });
    });
    group.finish();
}

// -----------------------------------------------------------------------
// T.6: TieredSet (StdHashSet → PdsHashSet)
// -----------------------------------------------------------------------

fn bench_set_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_set/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let ts: TieredSet<usize, StdHashSetBackend<usize>, PdsHashSetBackend<usize>> =
                    TieredSet::new(
                        StdHashSetBackend::new(),
                        PdsHashSetBackend::new(),
                        PropagationPolicy::Manual,
                    );
                for i in 0..n {
                    ts.insert(std::hint::black_box(i));
                }
                ts
            });
        });
    }
    group.finish();
}

fn bench_set_contains_hit(c: &mut Criterion) {
    let ts: TieredSet<usize, StdHashSetBackend<usize>, PdsHashSetBackend<usize>> = TieredSet::new(
        StdHashSetBackend::new(),
        PdsHashSetBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        ts.insert(i);
    }
    c.bench_function("tiered_set/contains_hit", |b| {
        b.iter(|| ts.contains(std::hint::black_box(&500)));
    });
}

fn bench_set_contains_cold(c: &mut Criterion) {
    let ts: TieredSet<usize, StdHashSetBackend<usize>, PdsHashSetBackend<usize>> = TieredSet::new(
        StdHashSetBackend::new(),
        PdsHashSetBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        ts.insert(i);
    }
    ts.flush();
    c.bench_function("tiered_set/contains_cold_fallback", |b| {
        b.iter(|| ts.contains(std::hint::black_box(&500)));
    });
}

fn bench_set_flush(c: &mut Criterion) {
    c.bench_function("tiered_set/flush_1000", |b| {
        b.iter(|| {
            let ts: TieredSet<usize, StdHashSetBackend<usize>, PdsHashSetBackend<usize>> =
                TieredSet::new(
                    StdHashSetBackend::new(),
                    PdsHashSetBackend::new(),
                    PropagationPolicy::Manual,
                );
            for i in 0..1_000usize {
                ts.insert(std::hint::black_box(i));
            }
            ts.flush();
            ts
        });
    });
}

fn bench_set_cold_snapshot(c: &mut Criterion) {
    let ts: TieredSet<usize, StdHashSetBackend<usize>, PdsHashSetBackend<usize>> = TieredSet::new(
        StdHashSetBackend::new(),
        PdsHashSetBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        ts.insert(i);
    }
    ts.flush();
    c.bench_function("tiered_set/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(ts.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.7: TieredBag (PdsBag → PdsBag)
// -----------------------------------------------------------------------

fn bench_bag_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_bag/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tb: TieredBag<usize, PdsBagBackend<usize>, PdsBagBackend<usize>> =
                    TieredBag::new(
                        PdsBagBackend::new(),
                        PdsBagBackend::new(),
                        PropagationPolicy::Manual,
                    );
                for i in 0..n {
                    // Use modulo so elements repeat — tests multiset semantics.
                    tb.insert(std::hint::black_box(i % 100));
                }
                tb
            });
        });
    }
    group.finish();
}

fn bench_bag_count_hit(c: &mut Criterion) {
    let tb: TieredBag<usize, PdsBagBackend<usize>, PdsBagBackend<usize>> = TieredBag::new(
        PdsBagBackend::new(),
        PdsBagBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tb.insert(i % 100);
    }
    c.bench_function("tiered_bag/count_hit", |b| {
        b.iter(|| tb.count(std::hint::black_box(&50)));
    });
}

fn bench_bag_count_cold(c: &mut Criterion) {
    let tb: TieredBag<usize, PdsBagBackend<usize>, PdsBagBackend<usize>> = TieredBag::new(
        PdsBagBackend::new(),
        PdsBagBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tb.insert(i % 100);
    }
    tb.flush();
    c.bench_function("tiered_bag/count_cold_fallback", |b| {
        b.iter(|| tb.count(std::hint::black_box(&50)));
    });
}

fn bench_bag_flush(c: &mut Criterion) {
    c.bench_function("tiered_bag/flush_1000", |b| {
        b.iter(|| {
            let tb: TieredBag<usize, PdsBagBackend<usize>, PdsBagBackend<usize>> = TieredBag::new(
                PdsBagBackend::new(),
                PdsBagBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tb.insert(std::hint::black_box(i % 100));
            }
            tb.flush();
            tb
        });
    });
}

fn bench_bag_cold_snapshot(c: &mut Criterion) {
    let tb: TieredBag<usize, PdsBagBackend<usize>, PdsBagBackend<usize>> = TieredBag::new(
        PdsBagBackend::new(),
        PdsBagBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tb.insert(i % 100);
    }
    tb.flush();
    c.bench_function("tiered_bag/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tb.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.8: TieredMultiMap (PdsHashMultiMap → PdsHashMultiMap)
// -----------------------------------------------------------------------

fn bench_multimap_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_multimap/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tm: TieredMultiMap<
                    usize,
                    usize,
                    PdsHashMultiMapBackend<usize, usize>,
                    PdsHashMultiMapBackend<usize, usize>,
                > = TieredMultiMap::new(
                    PdsHashMultiMapBackend::new(),
                    PdsHashMultiMapBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    // Use key modulo so we get multiple values per key.
                    tm.insert(std::hint::black_box(i % 100), i);
                }
                tm
            });
        });
    }
    group.finish();
}

fn bench_multimap_get_all_hit(c: &mut Criterion) {
    let tm: TieredMultiMap<
        usize,
        usize,
        PdsHashMultiMapBackend<usize, usize>,
        PdsHashMultiMapBackend<usize, usize>,
    > = TieredMultiMap::new(
        PdsHashMultiMapBackend::new(),
        PdsHashMultiMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tm.insert(i % 100, i);
    }
    c.bench_function("tiered_multimap/get_all_hit", |b| {
        b.iter(|| tm.get_all(std::hint::black_box(&50)));
    });
}

fn bench_multimap_get_all_cold(c: &mut Criterion) {
    let tm: TieredMultiMap<
        usize,
        usize,
        PdsHashMultiMapBackend<usize, usize>,
        PdsHashMultiMapBackend<usize, usize>,
    > = TieredMultiMap::new(
        PdsHashMultiMapBackend::new(),
        PdsHashMultiMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tm.insert(i % 100, i);
    }
    tm.flush();
    c.bench_function("tiered_multimap/get_all_cold_fallback", |b| {
        b.iter(|| tm.get_all(std::hint::black_box(&50)));
    });
}

fn bench_multimap_flush(c: &mut Criterion) {
    c.bench_function("tiered_multimap/flush_1000", |b| {
        b.iter(|| {
            let tm: TieredMultiMap<
                usize,
                usize,
                PdsHashMultiMapBackend<usize, usize>,
                PdsHashMultiMapBackend<usize, usize>,
            > = TieredMultiMap::new(
                PdsHashMultiMapBackend::new(),
                PdsHashMultiMapBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tm.insert(std::hint::black_box(i % 100), i);
            }
            tm.flush();
            tm
        });
    });
}

fn bench_multimap_cold_snapshot(c: &mut Criterion) {
    let tm: TieredMultiMap<
        usize,
        usize,
        PdsHashMultiMapBackend<usize, usize>,
        PdsHashMultiMapBackend<usize, usize>,
    > = TieredMultiMap::new(
        PdsHashMultiMapBackend::new(),
        PdsHashMultiMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tm.insert(i % 100, i);
    }
    tm.flush();
    c.bench_function("tiered_multimap/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tm.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.9: TieredBiMap (PdsBiMap → PdsBiMap)
// -----------------------------------------------------------------------

fn bench_bimap_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_bimap/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tb: TieredBiMap<
                    usize,
                    usize,
                    PdsBiMapBackend<usize, usize>,
                    PdsBiMapBackend<usize, usize>,
                > = TieredBiMap::new(
                    PdsBiMapBackend::new(),
                    PdsBiMapBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    // Offset value by n to keep key→value bijection.
                    tb.insert(std::hint::black_box(i), i + n);
                }
                tb
            });
        });
    }
    group.finish();
}

fn bench_bimap_get_by_key_hit(c: &mut Criterion) {
    let tb: TieredBiMap<
        usize,
        usize,
        PdsBiMapBackend<usize, usize>,
        PdsBiMapBackend<usize, usize>,
    > = TieredBiMap::new(
        PdsBiMapBackend::new(),
        PdsBiMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tb.insert(i, i + 1_000);
    }
    c.bench_function("tiered_bimap/get_by_key_hit", |b| {
        b.iter(|| tb.get_by_key(std::hint::black_box(&500)));
    });
}

fn bench_bimap_get_by_key_cold(c: &mut Criterion) {
    let tb: TieredBiMap<
        usize,
        usize,
        PdsBiMapBackend<usize, usize>,
        PdsBiMapBackend<usize, usize>,
    > = TieredBiMap::new(
        PdsBiMapBackend::new(),
        PdsBiMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tb.insert(i, i + 1_000);
    }
    tb.flush();
    c.bench_function("tiered_bimap/get_by_key_cold_fallback", |b| {
        b.iter(|| tb.get_by_key(std::hint::black_box(&500)));
    });
}

fn bench_bimap_flush(c: &mut Criterion) {
    c.bench_function("tiered_bimap/flush_1000", |b| {
        b.iter(|| {
            let tb: TieredBiMap<
                usize,
                usize,
                PdsBiMapBackend<usize, usize>,
                PdsBiMapBackend<usize, usize>,
            > = TieredBiMap::new(
                PdsBiMapBackend::new(),
                PdsBiMapBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tb.insert(std::hint::black_box(i), i + 1_000);
            }
            tb.flush();
            tb
        });
    });
}

fn bench_bimap_cold_snapshot(c: &mut Criterion) {
    let tb: TieredBiMap<
        usize,
        usize,
        PdsBiMapBackend<usize, usize>,
        PdsBiMapBackend<usize, usize>,
    > = TieredBiMap::new(
        PdsBiMapBackend::new(),
        PdsBiMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tb.insert(i, i + 1_000);
    }
    tb.flush();
    c.bench_function("tiered_bimap/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tb.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.10: TieredSymMap (PdsSymMap → PdsSymMap)
// -----------------------------------------------------------------------

fn bench_symmap_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_symmap/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let ts: TieredSymMap<usize, PdsSymMapBackend<usize>, PdsSymMapBackend<usize>> =
                    TieredSymMap::new(
                        PdsSymMapBackend::new(),
                        PdsSymMapBackend::new(),
                        PropagationPolicy::Manual,
                    );
                for i in 0..n {
                    // Offset b so a≠b (symmetric map requires distinct pairs).
                    ts.insert(std::hint::black_box(i), i + n);
                }
                ts
            });
        });
    }
    group.finish();
}

fn bench_symmap_get_hit(c: &mut Criterion) {
    let ts: TieredSymMap<usize, PdsSymMapBackend<usize>, PdsSymMapBackend<usize>> =
        TieredSymMap::new(
            PdsSymMapBackend::new(),
            PdsSymMapBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        ts.insert(i, i + 1_000);
    }
    c.bench_function("tiered_symmap/get_hit", |b| {
        b.iter(|| ts.get(SymMapDirection::Forward, std::hint::black_box(&500)));
    });
}

fn bench_symmap_get_cold(c: &mut Criterion) {
    let ts: TieredSymMap<usize, PdsSymMapBackend<usize>, PdsSymMapBackend<usize>> =
        TieredSymMap::new(
            PdsSymMapBackend::new(),
            PdsSymMapBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        ts.insert(i, i + 1_000);
    }
    ts.flush();
    c.bench_function("tiered_symmap/get_cold_fallback", |b| {
        b.iter(|| ts.get(SymMapDirection::Forward, std::hint::black_box(&500)));
    });
}

fn bench_symmap_flush(c: &mut Criterion) {
    c.bench_function("tiered_symmap/flush_1000", |b| {
        b.iter(|| {
            let ts: TieredSymMap<usize, PdsSymMapBackend<usize>, PdsSymMapBackend<usize>> =
                TieredSymMap::new(
                    PdsSymMapBackend::new(),
                    PdsSymMapBackend::new(),
                    PropagationPolicy::Manual,
                );
            for i in 0..1_000usize {
                ts.insert(std::hint::black_box(i), i + 1_000);
            }
            ts.flush();
            ts
        });
    });
}

fn bench_symmap_cold_snapshot(c: &mut Criterion) {
    let ts: TieredSymMap<usize, PdsSymMapBackend<usize>, PdsSymMapBackend<usize>> =
        TieredSymMap::new(
            PdsSymMapBackend::new(),
            PdsSymMapBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        ts.insert(i, i + 1_000);
    }
    ts.flush();
    c.bench_function("tiered_symmap/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(ts.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.11: TieredInsertionOrderMap (PdsInsertionOrderMap → PdsInsertionOrderMap)
// -----------------------------------------------------------------------

fn bench_iom_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_iom/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tm: TieredInsertionOrderMap<
                    usize,
                    usize,
                    PdsInsertionOrderMapBackend<usize, usize>,
                    PdsInsertionOrderMapBackend<usize, usize>,
                > = TieredInsertionOrderMap::new(
                    PdsInsertionOrderMapBackend::new(),
                    PdsInsertionOrderMapBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    tm.insert(std::hint::black_box(i), i);
                }
                tm
            });
        });
    }
    group.finish();
}

fn bench_iom_get_hit(c: &mut Criterion) {
    let tm: TieredInsertionOrderMap<
        usize,
        usize,
        PdsInsertionOrderMapBackend<usize, usize>,
        PdsInsertionOrderMapBackend<usize, usize>,
    > = TieredInsertionOrderMap::new(
        PdsInsertionOrderMapBackend::new(),
        PdsInsertionOrderMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tm.insert(i, i);
    }
    c.bench_function("tiered_iom/get_hit", |b| {
        b.iter(|| tm.get(std::hint::black_box(&500)));
    });
}

fn bench_iom_get_cold(c: &mut Criterion) {
    let tm: TieredInsertionOrderMap<
        usize,
        usize,
        PdsInsertionOrderMapBackend<usize, usize>,
        PdsInsertionOrderMapBackend<usize, usize>,
    > = TieredInsertionOrderMap::new(
        PdsInsertionOrderMapBackend::new(),
        PdsInsertionOrderMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tm.insert(i, i);
    }
    tm.flush();
    c.bench_function("tiered_iom/get_cold_fallback", |b| {
        b.iter(|| tm.get(std::hint::black_box(&500)));
    });
}

fn bench_iom_flush(c: &mut Criterion) {
    c.bench_function("tiered_iom/flush_1000", |b| {
        b.iter(|| {
            let tm: TieredInsertionOrderMap<
                usize,
                usize,
                PdsInsertionOrderMapBackend<usize, usize>,
                PdsInsertionOrderMapBackend<usize, usize>,
            > = TieredInsertionOrderMap::new(
                PdsInsertionOrderMapBackend::new(),
                PdsInsertionOrderMapBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tm.insert(std::hint::black_box(i), i);
            }
            tm.flush();
            tm
        });
    });
}

fn bench_iom_cold_snapshot(c: &mut Criterion) {
    let tm: TieredInsertionOrderMap<
        usize,
        usize,
        PdsInsertionOrderMapBackend<usize, usize>,
        PdsInsertionOrderMapBackend<usize, usize>,
    > = TieredInsertionOrderMap::new(
        PdsInsertionOrderMapBackend::new(),
        PdsInsertionOrderMapBackend::new(),
        PropagationPolicy::Manual,
    );
    for i in 0..1_000usize {
        tm.insert(i, i);
    }
    tm.flush();
    c.bench_function("tiered_iom/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tm.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.12: TieredTrie (PdsTrie → PdsTrie)
// -----------------------------------------------------------------------

fn bench_trie_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_trie/insert");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tt: TieredTrie<
                    usize,
                    usize,
                    PdsTrieBackend<usize, usize>,
                    PdsTrieBackend<usize, usize>,
                > = TieredTrie::new(
                    PdsTrieBackend::new(),
                    PdsTrieBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    // 3-component path: [i/100, i/10 % 10, i % 10].
                    tt.insert(vec![std::hint::black_box(i / 100), i / 10 % 10, i % 10], i);
                }
                tt
            });
        });
    }
    group.finish();
}

fn bench_trie_get_hit(c: &mut Criterion) {
    let tt: TieredTrie<usize, usize, PdsTrieBackend<usize, usize>, PdsTrieBackend<usize, usize>> =
        TieredTrie::new(
            PdsTrieBackend::new(),
            PdsTrieBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        tt.insert(vec![i / 100, i / 10 % 10, i % 10], i);
    }
    c.bench_function("tiered_trie/get_hit", |b| {
        // key = 500 → [5, 0, 0]
        b.iter(|| tt.get(std::hint::black_box(&[5usize, 0, 0])));
    });
}

fn bench_trie_get_cold(c: &mut Criterion) {
    let tt: TieredTrie<usize, usize, PdsTrieBackend<usize, usize>, PdsTrieBackend<usize, usize>> =
        TieredTrie::new(
            PdsTrieBackend::new(),
            PdsTrieBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        tt.insert(vec![i / 100, i / 10 % 10, i % 10], i);
    }
    tt.flush();
    c.bench_function("tiered_trie/get_cold_fallback", |b| {
        b.iter(|| tt.get(std::hint::black_box(&[5usize, 0, 0])));
    });
}

fn bench_trie_flush(c: &mut Criterion) {
    c.bench_function("tiered_trie/flush_1000", |b| {
        b.iter(|| {
            let tt: TieredTrie<
                usize,
                usize,
                PdsTrieBackend<usize, usize>,
                PdsTrieBackend<usize, usize>,
            > = TieredTrie::new(
                PdsTrieBackend::new(),
                PdsTrieBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tt.insert(vec![std::hint::black_box(i / 100), i / 10 % 10, i % 10], i);
            }
            tt.flush();
            tt
        });
    });
}

fn bench_trie_cold_snapshot(c: &mut Criterion) {
    let tt: TieredTrie<usize, usize, PdsTrieBackend<usize, usize>, PdsTrieBackend<usize, usize>> =
        TieredTrie::new(
            PdsTrieBackend::new(),
            PdsTrieBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        tt.insert(vec![i / 100, i / 10 % 10, i % 10], i);
    }
    tt.flush();
    c.bench_function("tiered_trie/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tt.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// T.13: TieredUniqueVector (PdsUniqueVec → PdsUniqueVec)
// -----------------------------------------------------------------------

fn bench_unique_vec_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiered_unique_vec/push_back");
    for n in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let tv: TieredUniqueVector<
                    usize,
                    PdsUniqueVecBackend<usize>,
                    PdsUniqueVecBackend<usize>,
                > = TieredUniqueVector::new(
                    PdsUniqueVecBackend::new(),
                    PdsUniqueVecBackend::new(),
                    PropagationPolicy::Manual,
                );
                for i in 0..n {
                    tv.push_back(std::hint::black_box(i));
                }
                tv
            });
        });
    }
    group.finish();
}

fn bench_unique_vec_contains_hit(c: &mut Criterion) {
    let tv: TieredUniqueVector<usize, PdsUniqueVecBackend<usize>, PdsUniqueVecBackend<usize>> =
        TieredUniqueVector::new(
            PdsUniqueVecBackend::new(),
            PdsUniqueVecBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        tv.push_back(i);
    }
    c.bench_function("tiered_unique_vec/contains_hit", |b| {
        b.iter(|| tv.contains(std::hint::black_box(&500)));
    });
}

fn bench_unique_vec_contains_cold(c: &mut Criterion) {
    let tv: TieredUniqueVector<usize, PdsUniqueVecBackend<usize>, PdsUniqueVecBackend<usize>> =
        TieredUniqueVector::new(
            PdsUniqueVecBackend::new(),
            PdsUniqueVecBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        tv.push_back(i);
    }
    tv.flush();
    c.bench_function("tiered_unique_vec/contains_cold_fallback", |b| {
        b.iter(|| tv.contains(std::hint::black_box(&500)));
    });
}

fn bench_unique_vec_flush(c: &mut Criterion) {
    c.bench_function("tiered_unique_vec/flush_1000", |b| {
        b.iter(|| {
            let tv: TieredUniqueVector<
                usize,
                PdsUniqueVecBackend<usize>,
                PdsUniqueVecBackend<usize>,
            > = TieredUniqueVector::new(
                PdsUniqueVecBackend::new(),
                PdsUniqueVecBackend::new(),
                PropagationPolicy::Manual,
            );
            for i in 0..1_000usize {
                tv.push_back(std::hint::black_box(i));
            }
            tv.flush();
            tv
        });
    });
}

fn bench_unique_vec_cold_snapshot(c: &mut Criterion) {
    let tv: TieredUniqueVector<usize, PdsUniqueVecBackend<usize>, PdsUniqueVecBackend<usize>> =
        TieredUniqueVector::new(
            PdsUniqueVecBackend::new(),
            PdsUniqueVecBackend::new(),
            PropagationPolicy::Manual,
        );
    for i in 0..1_000usize {
        tv.push_back(i);
    }
    tv.flush();
    c.bench_function("tiered_unique_vec/cold_snapshot", |b| {
        b.iter(|| std::hint::black_box(tv.cold_snapshot()));
    });
}

// -----------------------------------------------------------------------
// Groups
// -----------------------------------------------------------------------

criterion_group!(
    hash_benches,
    bench_hash_insert,
    bench_hash_get_hit,
    bench_hash_get_cold_fallback,
    bench_hash_flush,
    bench_hash_cold_snapshot,
);

criterion_group!(
    ord_benches,
    bench_ord_insert,
    bench_ord_get_hit,
    bench_ord_get_cold_fallback,
    bench_ord_flush,
    bench_ord_cold_snapshot,
);

criterion_group!(
    vec_benches,
    bench_vec_push,
    bench_vec_get_hit,
    bench_vec_get_cold_fallback,
    bench_vec_flush,
    bench_vec_cold_snapshot,
);

criterion_group!(policy_benches, bench_immediate_vs_manual,);

criterion_group!(
    set_benches,
    bench_set_insert,
    bench_set_contains_hit,
    bench_set_contains_cold,
    bench_set_flush,
    bench_set_cold_snapshot,
);

criterion_group!(
    bag_benches,
    bench_bag_insert,
    bench_bag_count_hit,
    bench_bag_count_cold,
    bench_bag_flush,
    bench_bag_cold_snapshot,
);

criterion_group!(
    multimap_benches,
    bench_multimap_insert,
    bench_multimap_get_all_hit,
    bench_multimap_get_all_cold,
    bench_multimap_flush,
    bench_multimap_cold_snapshot,
);

criterion_group!(
    bimap_benches,
    bench_bimap_insert,
    bench_bimap_get_by_key_hit,
    bench_bimap_get_by_key_cold,
    bench_bimap_flush,
    bench_bimap_cold_snapshot,
);

criterion_group!(
    symmap_benches,
    bench_symmap_insert,
    bench_symmap_get_hit,
    bench_symmap_get_cold,
    bench_symmap_flush,
    bench_symmap_cold_snapshot,
);

criterion_group!(
    iom_benches,
    bench_iom_insert,
    bench_iom_get_hit,
    bench_iom_get_cold,
    bench_iom_flush,
    bench_iom_cold_snapshot,
);

criterion_group!(
    trie_benches,
    bench_trie_insert,
    bench_trie_get_hit,
    bench_trie_get_cold,
    bench_trie_flush,
    bench_trie_cold_snapshot,
);

criterion_group!(
    unique_vec_benches,
    bench_unique_vec_push,
    bench_unique_vec_contains_hit,
    bench_unique_vec_contains_cold,
    bench_unique_vec_flush,
    bench_unique_vec_cold_snapshot,
);

#[cfg(feature = "traits")]
criterion_group!(
    three_tier_benches,
    bench_3tier_hash_insert,
    bench_3tier_hash_get_hit,
    bench_3tier_hash_flush,
    bench_3tier_cold_snapshot,
);

// When traits feature is absent, exclude the three_tier group entirely.
// The `criterion_group!` macro requires at least one benchmark, so we must
// not create an empty group. Instead, exclude it from criterion_main! via cfg.

#[cfg(not(feature = "traits"))]
criterion_main!(
    hash_benches,
    ord_benches,
    vec_benches,
    policy_benches,
    set_benches,
    bag_benches,
    multimap_benches,
    bimap_benches,
    symmap_benches,
    iom_benches,
    trie_benches,
    unique_vec_benches,
);

#[cfg(feature = "traits")]
criterion_main!(
    hash_benches,
    ord_benches,
    vec_benches,
    policy_benches,
    three_tier_benches,
    set_benches,
    bag_benches,
    multimap_benches,
    bimap_benches,
    symmap_benches,
    iom_benches,
    trie_benches,
    unique_vec_benches,
);
