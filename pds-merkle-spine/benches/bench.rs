//! Criterion benchmarks for pds-merkle-spine `VersionedHamt`.
//!
//! All benchmarks use `MemBackend` (no disk I/O), `u64` keys, `u64` values,
//! and `PostcardCodec`.
//!
//! Run via:
//!   `direnv exec . cargo bench -p pds-merkle-spine 2>&1 | tee /private/tmp/bench_versioned_hamt_$(date +%s).txt`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds_folio::codec::PostcardCodec;
use pds_merkle_spine::{VersionId, VersionedHamt};

// ---------------------------------------------------------------------------
// Store helper
// ---------------------------------------------------------------------------

/// Page size used for all benchmarks.
const PAGE_SIZE: u32 = 4096;

/// Creates a `FolioStore` with enough pages for `n` entries.
///
/// Each insert in a `VersionedHamt` recomputes the Merkle root and creates
/// O(log n) new HAMT pages; 32× headroom is used to avoid store exhaustion.
fn make_store(n: usize) -> FolioStore<MemBackend> {
    let pages = (n as u64) * 32 + 64;
    let backend = MemBackend::new(PAGE_SIZE, pages);
    FolioStore::create(backend, PAGE_SIZE, pages, ChecksumKind::Xxh3, true)
        .expect("store creation must succeed")
}

// ---------------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------------

/// Builds a `VersionedHamt` with `n` sequential u64 → u64 entries.
///
/// Returns the final map at version n.
fn make_versioned(n: usize) -> VersionedHamt<u64, u64, PostcardCodec, MemBackend> {
    let mut vh = VersionedHamt::new(make_store(n));
    for i in 0..n as u64 {
        vh = vh.insert(i, i * 2).unwrap();
    }
    vh
}

/// Builds a `VersionedHamt` with `n` entries, capturing the mid-history `VersionId`.
///
/// Returns (final_map, mid_version_id) so benchmarks can exercise historical access
/// without rebuilding inside the timing loop.
fn make_versioned_with_mid(
    n: usize,
) -> (
    VersionedHamt<u64, u64, PostcardCodec, MemBackend>,
    VersionId,
) {
    let mid = n / 2;
    let mut vh = VersionedHamt::new(make_store(n));
    let mut mid_version = vh.version(); // v0 fallback
    for i in 0..n as u64 {
        vh = vh.insert(i, i * 2).unwrap();
        if i as usize == mid.saturating_sub(1) {
            // Capture the version after the mid-th insert.
            mid_version = vh.version();
        }
    }
    (vh, mid_version)
}

// ---------------------------------------------------------------------------
// versioned_hamt_insert
// ---------------------------------------------------------------------------

/// Measures building a `VersionedHamt` from empty: n sequential inserts, each
/// creating a new version (includes Merkle root recomputation per insert).
fn bench_versioned_hamt_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("versioned_hamt_insert");
    for &n in &[10usize, 100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut vh =
                    VersionedHamt::<u64, u64, PostcardCodec, MemBackend>::new(make_store(n));
                for i in 0..n as u64 {
                    vh = vh.insert(black_box(i), black_box(i * 2)).unwrap();
                }
                black_box(vh.len())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// versioned_hamt_get_current
// ---------------------------------------------------------------------------

/// Measures a single `get` on the most-recent version of a pre-built map.
///
/// The map is built outside the timing loop so only the get is measured.
fn bench_versioned_hamt_get_current(c: &mut Criterion) {
    let mut group = c.benchmark_group("versioned_hamt_get_current");
    for &n in &[10usize, 100, 1000] {
        let vh = make_versioned(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let key = black_box(n as u64 / 2);
                black_box(vh.get(&key).unwrap())
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// versioned_hamt_get_at_version
// ---------------------------------------------------------------------------

/// Measures `get_at(mid_version, key)` — a lookup in a historical snapshot.
///
/// The mid-history version is captured once before the timing loop; the key
/// at `n/2 - 1` is guaranteed to exist at that version.
fn bench_versioned_hamt_get_at_version(c: &mut Criterion) {
    let mut group = c.benchmark_group("versioned_hamt_get_at_version");
    for &n in &[10usize, 100, 1000] {
        let (vh, mid_version) = make_versioned_with_mid(n);
        // The key inserted at the mid-point; guaranteed to exist at mid_version.
        let mid_key = (n / 2).saturating_sub(1) as u64;
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                black_box(
                    vh.get_at(black_box(mid_version), &black_box(mid_key))
                        .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// versioned_hamt_checkout
// ---------------------------------------------------------------------------

/// Measures `checkout(mid_version)` — restoring a historical snapshot.
///
/// `checkout` is O(1): it clones the stored `HamtMap` at that version
/// (increments a refcount) and wraps it in a new `VersionedHamt`.
fn bench_versioned_hamt_checkout(c: &mut Criterion) {
    let mut group = c.benchmark_group("versioned_hamt_checkout");
    for &n in &[10usize, 100, 1000] {
        let (vh, mid_version) = make_versioned_with_mid(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(vh.checkout(black_box(mid_version)).unwrap()));
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// versioned_hamt_prove
// ---------------------------------------------------------------------------

/// Measures `prove_inclusion(key)` — generating a Merkle inclusion proof.
///
/// The map is built outside the timing loop; key `n/2` is always present.
fn bench_versioned_hamt_prove(c: &mut Criterion) {
    let mut group = c.benchmark_group("versioned_hamt_prove");
    for &n in &[10usize, 100, 1000] {
        let vh = make_versioned(n);
        let key = (n as u64) / 2;
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(vh.prove_inclusion(&black_box(key)).unwrap()));
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// versioned_hamt_clone
// ---------------------------------------------------------------------------

/// Measures `Clone` of a `VersionedHamt` — expected O(1) (refcount increment).
///
/// The map is built outside the timing loop.
fn bench_versioned_hamt_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("versioned_hamt_clone");
    for &n in &[10usize, 100, 1000] {
        let vh = make_versioned(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // O(1): increments the HAMT root refcount and clones the Arc.
                let snap = black_box(vh.clone());
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
    versioned_hamt_benches,
    bench_versioned_hamt_insert,
    bench_versioned_hamt_get_current,
    bench_versioned_hamt_get_at_version,
    bench_versioned_hamt_checkout,
    bench_versioned_hamt_prove,
    bench_versioned_hamt_clone,
);

criterion_main!(versioned_hamt_benches);
