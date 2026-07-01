//! Tiered collection benchmarks — Phase T.0d.
//!
//! Covers all four compositions from the spec:
//! 1. `StdHashMap → PdsHashMap` (TieredCollection)
//! 2. `StdBTreeMap → PdsOrdMap` (TieredOrdMap)
//! 3. `StdVec → PdsVector` (TieredVector)
//! 4. `StdHashMap → TieredCollection<PdsHashMap, MerkleWrapper>` (3-tier, cfg traits)
//!
//! Operations per composition:
//! - `insert_n` at n = 100, 1_000, 10_000 (Manual policy — pure hot-tier write cost)
//! - `get_hit` — read from hot (no flush)
//! - `get_cold_fallback` — read from cold (flush first)
//! - `flush_n` — flush 1_000 accumulated writes to cold
//! - `cold_snapshot` — clone the cold tier

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pds::tiered::{
    backends::{PdsHashMapBackend, PdsOrdMapBackend, StdBTreeMapBackend, StdHashMapBackend},
    sequence::TieredSequence,
    sequence_backends::{PdsVectorBackend, StdVecBackend},
    PropagationPolicy, TieredCollection, TieredOrdMap, TieredVector,
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

#[cfg(feature = "traits")]
criterion_group!(
    three_tier_benches,
    bench_3tier_hash_insert,
    bench_3tier_hash_get_hit,
    bench_3tier_hash_flush,
    bench_3tier_cold_snapshot,
);

#[cfg(not(feature = "traits"))]
criterion_group!(three_tier_benches,);

criterion_main!(
    hash_benches,
    ord_benches,
    vec_benches,
    policy_benches,
    three_tier_benches,
);
