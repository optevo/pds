//! Criterion benchmarks for pds-folio persistent data structures.
//!
//! Covers: HamtMap, HamtSet, FolioVector, FolioOrdMap, FolioOrdSet.
//! Run via:  `cargo bench -p pds-folio 2>&1 | tee /private/tmp/bench_pds_folio_$(date +%s).txt`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds_folio::{
    codec::{PodCodec, PostcardCodec},
    folio_ordmap::FolioOrdMap,
    folio_ordset::FolioOrdSet,
    folio_vector::FolioVector,
    hamt::HamtMap,
    set::HamtSet,
};

// ---------------------------------------------------------------------------
// Store helpers
// ---------------------------------------------------------------------------

/// Creates a fresh `MemBackend`-backed `FolioStore` with generous capacity.
fn make_store(pages: u64) -> FolioStore<MemBackend> {
    let backend = MemBackend::new(4096, pages);
    FolioStore::create(backend, 4096, pages, ChecksumKind::Xxh3, true)
        .expect("store creation must succeed")
}

// ---------------------------------------------------------------------------
// HamtMap benchmarks
// ---------------------------------------------------------------------------

fn bench_hamt_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamt_insert");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut map: HamtMap<u64, u64> = HamtMap::new(make_store((n as u64) * 4 + 64));
                for i in 0..n as u64 {
                    map = map.insert(black_box(i), black_box(i * 7)).unwrap();
                }
                black_box(map.len())
            });
        });
    }
    group.finish();
}

fn bench_hamt_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamt_get");
    for &n in &[10usize, 100, 1000, 10000] {
        // Build map once outside the timing loop.
        let mut map: HamtMap<u64, u64> = HamtMap::new(make_store((n as u64) * 4 + 64));
        for i in 0..n as u64 {
            map = map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let key = black_box((n as u64) / 2);
                black_box(map.get(&key).unwrap())
            });
        });
    }
    group.finish();
}

fn bench_hamt_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamt_remove");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                // Build + remove all in the timing loop to exercise the remove path.
                let mut map: HamtMap<u64, u64> = HamtMap::new(make_store((n as u64) * 8 + 64));
                for i in 0..n as u64 {
                    map = map.insert(i, i).unwrap();
                }
                for i in 0..n as u64 {
                    let (new_map, _) = map.remove(&black_box(i)).unwrap();
                    map = new_map;
                }
                black_box(map.len())
            });
        });
    }
    group.finish();
}

fn bench_hamt_clone_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamt_clone_snapshot");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut map: HamtMap<u64, u64> = HamtMap::new(make_store((n as u64) * 4 + 64));
        for i in 0..n as u64 {
            map = map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // O(1) clone — just increments a refcount.
                let snap = black_box(map.clone());
                black_box(snap.len())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// PERF-2: PodCodec vs PostcardCodec head-to-head
// ---------------------------------------------------------------------------

/// PERF-2: Compares `PodCodec<u64, u64>` vs `PostcardCodec` on `hamt_get` at
/// n=1_000 and n=10_000.  Both maps are pre-built outside the timing loop so
/// the benchmark measures only the get path (descent + decode).
fn bench_pod_codec_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("pod_codec/get");
    for &n in &[1000usize, 10000] {
        // PostcardCodec map.
        let mut pc_map: HamtMap<u64, u64, PostcardCodec> =
            HamtMap::new(make_store((n as u64) * 4 + 64));
        for i in 0..n as u64 {
            pc_map = pc_map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(
            BenchmarkId::new("PostcardCodec", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let key = black_box((n as u64) / 2);
                    black_box(pc_map.get(&key).unwrap())
                });
            },
        );

        // PodCodec map.
        let mut pod_map: HamtMap<u64, u64, PodCodec> =
            HamtMap::new(make_store((n as u64) * 4 + 64));
        for i in 0..n as u64 {
            pod_map = pod_map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(
            BenchmarkId::new("PodCodec", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let key = black_box((n as u64) / 2);
                    black_box(pod_map.get(&key).unwrap())
                });
            },
        );
    }
    group.finish();
}

/// PERF-2: Compares `PodCodec<u64, u64>` vs `PostcardCodec` on `hamt_insert` at
/// n=1_000 and n=10_000.  The map is rebuilt from scratch inside the timing loop.
fn bench_pod_codec_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("pod_codec/insert");
    for &n in &[1000usize, 10000] {
        group.bench_with_input(
            BenchmarkId::new("PostcardCodec", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let mut map: HamtMap<u64, u64, PostcardCodec> =
                        HamtMap::new(make_store((n as u64) * 4 + 64));
                    for i in 0..n as u64 {
                        map = map.insert(black_box(i), black_box(i * 7)).unwrap();
                    }
                    black_box(map.len())
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("PodCodec", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let mut map: HamtMap<u64, u64, PodCodec> =
                        HamtMap::new(make_store((n as u64) * 4 + 64));
                    for i in 0..n as u64 {
                        map = map.insert(black_box(i), black_box(i * 7)).unwrap();
                    }
                    black_box(map.len())
                });
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// HamtSet benchmarks
// ---------------------------------------------------------------------------

fn bench_hamtset_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamtset_contains");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut set: HamtSet<u64, PostcardCodec> = HamtSet::new(make_store((n as u64) * 4 + 64));
        for i in 0..n as u64 {
            set = set.insert(i).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let key = black_box((n as u64) / 2);
                black_box(set.contains(&key).unwrap())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// FolioVector benchmarks
// ---------------------------------------------------------------------------

fn bench_vector_push_back(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_push_back");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut v: FolioVector<u32, PostcardCodec> =
                    FolioVector::new(make_store((n as u64) * 4 + 64));
                for i in 0..n as u32 {
                    v = v.push_back(black_box(i)).unwrap();
                }
                black_box(v.len())
            });
        });
    }
    group.finish();
}

fn bench_vector_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_get");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut v: FolioVector<u32, PostcardCodec> =
            FolioVector::new(make_store((n as u64) * 4 + 64));
        for i in 0..n as u32 {
            v = v.push_back(i).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let idx = black_box(n / 2);
                black_box(v.get(idx).unwrap())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// FolioOrdMap benchmarks
// ---------------------------------------------------------------------------

fn bench_ordmap_insert_sequential(c: &mut Criterion) {
    let mut group = c.benchmark_group("ordmap_insert_sequential");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                // B+ tree path-copy creates O(log N) new pages per insert.
                // Use 16× headroom so the store never exhausts during the run.
                let mut m: FolioOrdMap<u32, u32> =
                    FolioOrdMap::new(make_store((n as u64) * 16 + 256));
                for i in 0..n as u32 {
                    m = m.insert(black_box(i), black_box(i * 3)).unwrap();
                }
                black_box(m.len())
            });
        });
    }
    group.finish();
}

fn bench_ordmap_insert_random(c: &mut Criterion) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut group = c.benchmark_group("ordmap_insert_random");
    for &n in &[10usize, 100, 1000, 10000] {
        // Pre-generate random-ish keys (deterministic).
        let keys: Vec<u32> = (0..n as u64)
            .map(|i| {
                let mut h = DefaultHasher::new();
                i.hash(&mut h);
                h.finish() as u32
            })
            .collect();

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                // B+ tree path-copy creates O(log N) new pages per insert.
                // Use 16× headroom so the store never exhausts during the run.
                let mut m: FolioOrdMap<u32, u32> =
                    FolioOrdMap::new(make_store((n as u64) * 16 + 256));
                for &k in &keys {
                    m = m.insert(black_box(k), black_box(k)).unwrap();
                }
                black_box(m.len())
            });
        });
    }
    group.finish();
}

fn bench_ordmap_range_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("ordmap_range_scan");
    for &n in &[10usize, 100, 1000, 10000] {
        // Build once outside the timing loop; 16× headroom for path-copy.
        let mut m: FolioOrdMap<u32, u32> = FolioOrdMap::new(make_store((n as u64) * 16 + 256));
        for i in 0..n as u32 {
            m = m.insert(i, i).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // Full range scan.
                black_box(m.iter().unwrap())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// FolioOrdSet benchmarks
// ---------------------------------------------------------------------------

fn bench_ordset_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("ordset_insert");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                // B+ tree path-copy creates O(log N) new pages per insert.
                // Use 16× headroom so the store never exhausts during the run.
                let mut s: FolioOrdSet<u32> = FolioOrdSet::new(make_store((n as u64) * 16 + 256));
                for i in 0..n as u32 {
                    s = s.insert(black_box(i)).unwrap();
                }
                black_box(s.len())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion groups
// ---------------------------------------------------------------------------

criterion_group!(
    hamt_benches,
    bench_hamt_insert,
    bench_hamt_get,
    bench_hamt_remove,
    bench_hamt_clone_snapshot,
    bench_hamtset_contains,
);

criterion_group!(pod_codec_benches, bench_pod_codec_get, bench_pod_codec_insert,);

criterion_group!(vector_benches, bench_vector_push_back, bench_vector_get,);

criterion_group!(
    ordmap_benches,
    bench_ordmap_insert_sequential,
    bench_ordmap_insert_random,
    bench_ordmap_range_scan,
    bench_ordset_insert,
);

criterion_main!(hamt_benches, vector_benches, ordmap_benches, pod_codec_benches);
