//! Cross-collection comparison benchmarks for pds-folio.
//!
//! Places `HamtMap`, `FolioOrdMap`, and `FolioVector` side-by-side at identical
//! operations and sizes so results can be read from a single table.
//!
//! Operations:
//!   - `compare_insert`  — build from empty: n sequential inserts
//!   - `compare_get`     — single element read at index/key n/2
//!   - `compare_clone`   — O(1) structural snapshot
//!
//! Run via:
//!   `direnv exec . cargo bench -p pds-folio --bench compare 2>&1 | tee /private/tmp/bench_folio_compare_$(date +%s).txt`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds_folio::{
    codec::PostcardCodec, folio_ordmap::FolioOrdMap, folio_vector::FolioVector, hamt::HamtMap,
};

// ---------------------------------------------------------------------------
// Store helpers
// ---------------------------------------------------------------------------

/// Page size for all benchmarks.
const PAGE_SIZE: u32 = 4096;

/// Creates a store with `pages` pages of capacity.
fn make_store(pages: u64) -> FolioStore<MemBackend> {
    let backend = MemBackend::new(PAGE_SIZE, pages);
    FolioStore::create(backend, PAGE_SIZE, pages, ChecksumKind::Xxh3, true)
        .expect("store creation must succeed")
}

/// Page headroom for HamtMap inserts (HAMT creates ~4 pages per insert).
fn hamt_pages(n: usize) -> u64 {
    (n as u64) * 4 + 64
}

/// Page headroom for FolioOrdMap inserts (B+ tree path-copy: O(log n) pages each).
/// Use 16× to accommodate version accumulation.
fn ordmap_pages(n: usize) -> u64 {
    (n as u64) * 16 + 256
}

/// Page headroom for FolioVector inserts (trie path-copy, similar to ordmap).
fn vector_pages(n: usize) -> u64 {
    (n as u64) * 4 + 64
}

// ---------------------------------------------------------------------------
// compare_insert
// ---------------------------------------------------------------------------

/// Builds each collection from empty with n sequential inserts inside the
/// timing loop.  Measures the total construction cost per collection type.

fn bench_compare_insert_hamt(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_insert/HamtMap");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut map: HamtMap<u64, u64> =
                    HamtMap::new(make_store(hamt_pages(n)));
                for i in 0..n as u64 {
                    map = map.insert(black_box(i), black_box(i * 7)).unwrap();
                }
                black_box(map.len())
            });
        });
    }
    group.finish();
}

fn bench_compare_insert_ordmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_insert/FolioOrdMap");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut map: FolioOrdMap<u64, u64> =
                    FolioOrdMap::new(make_store(ordmap_pages(n)));
                for i in 0..n as u64 {
                    map = map.insert(black_box(i), black_box(i * 7)).unwrap();
                }
                black_box(map.len())
            });
        });
    }
    group.finish();
}

fn bench_compare_insert_vector(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_insert/FolioVector");
    for &n in &[10usize, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut v: FolioVector<u64, PostcardCodec> =
                    FolioVector::new(make_store(vector_pages(n)));
                for i in 0..n as u64 {
                    v = v.push_back(black_box(i)).unwrap();
                }
                black_box(v.len())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// compare_get
// ---------------------------------------------------------------------------

/// Reads element at position/key n/2 from a pre-built collection.
///
/// Each collection is built once outside the timing loop.

fn bench_compare_get_hamt(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_get/HamtMap");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut map: HamtMap<u64, u64> = HamtMap::new(make_store(hamt_pages(n)));
        for i in 0..n as u64 {
            map = map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let key = black_box(n as u64 / 2);
                black_box(map.get(&key).unwrap())
            });
        });
    }
    group.finish();
}

fn bench_compare_get_ordmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_get/FolioOrdMap");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut map: FolioOrdMap<u64, u64> = FolioOrdMap::new(make_store(ordmap_pages(n)));
        for i in 0..n as u64 {
            map = map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let key = black_box(n as u64 / 2);
                black_box(map.get(&key).unwrap())
            });
        });
    }
    group.finish();
}

fn bench_compare_get_vector(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_get/FolioVector");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut v: FolioVector<u64, PostcardCodec> = FolioVector::new(make_store(vector_pages(n)));
        for i in 0..n as u64 {
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
// compare_clone
// ---------------------------------------------------------------------------

/// Clones (snapshots) a pre-built collection.
///
/// All three collections support O(1) structural sharing via refcount increment.
/// The collection is built once outside the timing loop.

fn bench_compare_clone_hamt(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_clone/HamtMap");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut map: HamtMap<u64, u64> = HamtMap::new(make_store(hamt_pages(n)));
        for i in 0..n as u64 {
            map = map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let snap = black_box(map.clone());
                black_box(snap.len())
            });
        });
    }
    group.finish();
}

fn bench_compare_clone_ordmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_clone/FolioOrdMap");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut map: FolioOrdMap<u64, u64> = FolioOrdMap::new(make_store(ordmap_pages(n)));
        for i in 0..n as u64 {
            map = map.insert(i, i * 7).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let snap = black_box(map.clone());
                black_box(snap.len())
            });
        });
    }
    group.finish();
}

fn bench_compare_clone_vector(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_clone/FolioVector");
    for &n in &[10usize, 100, 1000, 10000] {
        let mut v: FolioVector<u64, PostcardCodec> = FolioVector::new(make_store(vector_pages(n)));
        for i in 0..n as u64 {
            v = v.push_back(i).unwrap();
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let snap = black_box(v.clone());
                black_box(snap.len())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion groups
// ---------------------------------------------------------------------------

criterion_group!(
    compare_insert_benches,
    bench_compare_insert_hamt,
    bench_compare_insert_ordmap,
    bench_compare_insert_vector,
);

criterion_group!(
    compare_get_benches,
    bench_compare_get_hamt,
    bench_compare_get_ordmap,
    bench_compare_get_vector,
);

criterion_group!(
    compare_clone_benches,
    bench_compare_clone_hamt,
    bench_compare_clone_ordmap,
    bench_compare_clone_vector,
);

criterion_main!(compare_insert_benches, compare_get_benches, compare_clone_benches);
