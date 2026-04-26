/// Head-to-head criterion benchmarks: OrdMap vs HashMap.
///
/// Each benchmark group places both implementations under the same group name so
/// criterion plots them together. Key type: i64 (satisfies both Ord and Hash+Eq).
/// Sizes: 100 / 1K / 10K / 100K for scalar ops; 10K / 100K for parallel ops.
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pds::hashmap::HashMap;
use pds::ordmap::OrdMap;
use std::hint::black_box;

mod utils;
use utils::*;

// ── build helpers ─────────────────────────────────────────────────────────────

fn build_hm(keys: &[i64]) -> HashMap<i64, i64> {
    keys.iter().copied().zip(keys.iter().copied()).collect()
}

fn build_om(keys: &[i64]) -> OrdMap<i64, i64> {
    keys.iter().copied().zip(keys.iter().copied()).collect()
}

// ── lookup ────────────────────────────────────────────────────────────────────

fn bench_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/lookup");
    for &size in &[100usize, 1_000, 10_000, 100_000] {
        let keys = i64::generate(size);
        let order = reorder(&keys);
        let hm = build_hm(&keys);
        let om = build_om(&keys);
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| {
                for k in &order {
                    black_box(hm.get(k));
                }
            })
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| {
                for k in &order {
                    black_box(om.get(k));
                }
            })
        });
    }
    group.finish();
}

// ── insert_mut ────────────────────────────────────────────────────────────────

fn bench_insert_mut(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/insert_mut");
    for &size in &[100usize, 1_000, 10_000, 100_000] {
        let keys = i64::generate(size);
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| {
                let mut m = HashMap::new();
                for &k in &keys {
                    m.insert(k, k);
                }
                black_box(m)
            })
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| {
                let mut m = OrdMap::new();
                for &k in &keys {
                    m.insert(k, k);
                }
                black_box(m)
            })
        });
    }
    group.finish();
}

// ── remove_mut ────────────────────────────────────────────────────────────────

fn bench_remove_mut(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/remove_mut");
    for &size in &[100usize, 1_000, 10_000, 100_000] {
        let keys = i64::generate(size);
        let order = reorder(&keys);
        let hm = build_hm(&keys);
        let om = build_om(&keys);
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| {
                let mut m = hm.clone();
                for k in &order {
                    m.remove(k);
                }
                black_box(m)
            })
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| {
                let mut m = om.clone();
                for k in &order {
                    m.remove(k);
                }
                black_box(m)
            })
        });
    }
    group.finish();
}

// ── iter ──────────────────────────────────────────────────────────────────────

fn bench_iter(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/iter");
    for &size in &[100usize, 1_000, 10_000, 100_000] {
        let keys = i64::generate(size);
        let hm = build_hm(&keys);
        let om = build_om(&keys);
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| {
                for p in hm.iter() {
                    black_box(p);
                }
            })
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| {
                for p in om.iter() {
                    black_box(p);
                }
            })
        });
    }
    group.finish();
}

// ── from_iter ─────────────────────────────────────────────────────────────────

fn bench_from_iter(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/from_iter");
    for &size in &[100usize, 1_000, 10_000, 100_000] {
        let keys = i64::generate(size);
        let pairs: Vec<(i64, i64)> = keys.iter().copied().zip(keys.iter().copied()).collect();
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| {
                let m: HashMap<i64, i64> = black_box(pairs.clone()).into_iter().collect();
                black_box(m)
            })
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| {
                let m: OrdMap<i64, i64> = black_box(pairs.clone()).into_iter().collect();
                black_box(m)
            })
        });
    }
    group.finish();
}

// ── par_union ─────────────────────────────────────────────────────────────────

#[cfg(feature = "rayon")]
fn bench_par_union(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/par_union");
    for &size in &[10_000usize, 100_000] {
        // Two maps with 50% overlap
        let all_keys = i64::generate(size * 2);
        let keys_a = &all_keys[..size + size / 2]; // first 75%
        let keys_b = &all_keys[size / 2..]; // last 75% — 50% overlap in the middle
        let hm_a = build_hm(keys_a);
        let hm_b = build_hm(keys_b);
        let om_a = build_om(keys_a);
        let om_b = build_om(keys_b);
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| black_box(hm_a.clone().par_union(hm_b.clone())))
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| black_box(om_a.clone().par_union(om_b.clone())))
        });
    }
    group.finish();
}

// ── par_intersection ──────────────────────────────────────────────────────────

#[cfg(feature = "rayon")]
fn bench_par_intersection(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare/par_intersection");
    for &size in &[10_000usize, 100_000] {
        let all_keys = i64::generate(size * 2);
        let keys_a = &all_keys[..size + size / 2];
        let keys_b = &all_keys[size / 2..];
        let hm_a = build_hm(keys_a);
        let hm_b = build_hm(keys_b);
        let om_a = build_om(keys_a);
        let om_b = build_om(keys_b);
        group.bench_with_input(BenchmarkId::new("HashMap", size), &size, |b, _| {
            b.iter(|| black_box(hm_a.clone().par_intersection(hm_b.clone())))
        });
        group.bench_with_input(BenchmarkId::new("OrdMap", size), &size, |b, _| {
            b.iter(|| black_box(om_a.clone().par_intersection(om_b.clone())))
        });
    }
    group.finish();
}

#[cfg(not(feature = "rayon"))]
criterion_group!(
    benches,
    bench_lookup,
    bench_insert_mut,
    bench_remove_mut,
    bench_iter,
    bench_from_iter,
);

#[cfg(feature = "rayon")]
criterion_group!(
    benches,
    bench_lookup,
    bench_insert_mut,
    bench_remove_mut,
    bench_iter,
    bench_from_iter,
    bench_par_union,
    bench_par_intersection,
);

criterion_main!(benches);
