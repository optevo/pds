// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Criterion benchmarks for `pds-durable`.
//!
//! All benchmarks use `tempfile::tempdir()` for isolation; the tmpfs on macOS
//! means fsync latency is near zero.  Real-disk numbers will be dominated by
//! the ~100 µs fsync cost per Strict insert.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use pds_durable::{DurableConfig, DurableMap, Relaxed, Strict};
use tempfile::tempdir;

const N: usize = 1000;

// Type aliases to avoid ambiguity between Strict/Relaxed `open` methods.
type StrictMap = DurableMap<String, i64, Strict>;
type RelaxedMap = DurableMap<String, i64, Relaxed>;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_kv(i: usize) -> (String, i64) {
    (format!("key{:06}", i), i as i64)
}

// ── Benchmarks ───────────────────────────────────────────────────────────────

fn bench_strict_insert(c: &mut Criterion) {
    c.bench_function("durable_map_strict_insert", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v)).unwrap();
            }
        });
    });
}

fn bench_relaxed_insert(c: &mut Criterion) {
    c.bench_function("durable_map_relaxed_insert", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v));
            }
            // Drop without flush — entries stay in pending buffer.
        });
    });
}

fn bench_relaxed_insert_flush(c: &mut Criterion) {
    c.bench_function("durable_map_relaxed_insert_flush", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..100 {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v));
            }
            map.flush().unwrap();
        });
    });
}

fn bench_strict_insert_batch(c: &mut Criterion) {
    c.bench_function("durable_map_strict_insert_batch", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            let pairs: Vec<(String, i64)> = (0..N).map(make_kv).collect();
            map.insert_batch(black_box(pairs)).unwrap();
        });
    });
}

fn bench_get(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bench.wal");
    let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
    for i in 0..N {
        let (k, v) = make_kv(i);
        map.insert(k, v).unwrap();
    }

    c.bench_function("durable_map_get", |b| {
        b.iter(|| {
            for i in (0..N).step_by(2) {
                let k = format!("key{:06}", i);
                black_box(map.get(black_box(&k)));
            }
        });
    });
}

fn bench_checkpoint(c: &mut Criterion) {
    c.bench_function("durable_map_checkpoint", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(k, v).unwrap();
            }
            map.checkpoint().unwrap();
        });
    });
}

fn bench_heap_reference(c: &mut Criterion) {
    c.bench_function("heap_reference", |b| {
        b.iter(|| {
            let mut map: pds::HashMap<String, i64> = pds::HashMap::new();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v));
            }
            black_box(map.len());
        });
    });
}

// ── Grouped comparison: Strict vs Relaxed vs Heap ─────────────────────────────

fn bench_insert_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_comparison");

    group.bench_function(BenchmarkId::new("heap_only", N), |b| {
        b.iter(|| {
            let mut map: pds::HashMap<String, i64> = pds::HashMap::new();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v));
            }
        });
    });

    group.bench_function(BenchmarkId::new("relaxed_no_flush", N), |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v));
            }
        });
    });

    group.bench_function(BenchmarkId::new("strict_fsync", N), |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("bench.wal");
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..N {
                let (k, v) = make_kv(i);
                map.insert(black_box(k), black_box(v)).unwrap();
            }
        });
    });

    group.finish();
}

// ── TieredMap benchmarks (feature = "tiered") ────────────────────────────────

#[cfg(feature = "tiered")]
mod tiered_benches {
    use super::*;
    use pds_durable::{Durable, MemOnlyMap, PipelinedMap, TieredConfig, TieredMap, WriteBack};

    // Use Durable/WriteBack directly to avoid open() method ambiguity with aliases.
    type TieredStrict = TieredMap<String, i64, Durable>;
    type TieredRelaxed = TieredMap<String, i64, WriteBack>;

    pub fn bench_tiered_strict_insert(c: &mut Criterion) {
        c.bench_function("tiered_strict_insert", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.tiered");
                let mut map = TieredStrict::open(&path, TieredConfig::default()).unwrap();
                for i in 0..N {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v)).unwrap();
                }
            });
        });
    }

    pub fn bench_tiered_relaxed_insert(c: &mut Criterion) {
        // Hot path: write to front only; zero back involvement.
        // Expected to be near-identical to heap_reference.
        c.bench_function("tiered_relaxed_insert", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.tiered");
                let mut map = TieredRelaxed::open(&path, TieredConfig::default()).unwrap();
                for i in 0..N {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                // Drop without flush.
            });
        });
    }

    pub fn bench_tiered_relaxed_flush(c: &mut Criterion) {
        // 100 inserts + one flush (one new version in back).
        c.bench_function("tiered_relaxed_flush", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.tiered");
                let mut map = TieredRelaxed::open(&path, TieredConfig::default()).unwrap();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                map.flush().unwrap();
            });
        });
    }

    pub fn bench_tiered_get_warm(c: &mut Criterion) {
        // Get for a front-cached key.
        let dir = tempdir().unwrap();
        let path = dir.path().join("bench.tiered");
        let mut map = TieredStrict::open(&path, TieredConfig::default()).unwrap();
        for i in 0..N {
            let (k, v) = make_kv(i);
            map.insert(k, v).unwrap();
        }
        c.bench_function("tiered_get_warm", |b| {
            b.iter(|| {
                for i in (0..N).step_by(2) {
                    let k = format!("key{:06}", i);
                    black_box(map.get(black_box(&k)));
                }
            });
        });
    }

    pub fn bench_tiered_get_cold(c: &mut Criterion) {
        // Get for an evicted key (back read at latest version).
        let dir = tempdir().unwrap();
        let path = dir.path().join("bench.tiered");
        let config = TieredConfig {
            max_front_entries: 10, // evict aggressively
            ..TieredConfig::default()
        };
        let mut map = TieredStrict::open(&path, config).unwrap();
        for i in 0..N {
            let (k, v) = make_kv(i);
            map.insert(k, v).unwrap();
        }
        // At this point, only the 10 most recent keys are in front.
        c.bench_function("tiered_get_cold", |b| {
            b.iter(|| {
                // Keys k0-k9 were evicted (inserted first, evicted by later inserts).
                for i in 0..10 {
                    let k = format!("key{:06}", i);
                    // Cold fetch — falls through to back.
                    black_box(map.get_or_fetch(black_box(&k)).unwrap());
                }
            });
        });
    }

    pub fn bench_tiered_eviction(c: &mut Criterion) {
        // Insert beyond max_front_entries; exercises eviction + dirty-flush path.
        c.bench_function("tiered_eviction", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.tiered");
                let config = TieredConfig {
                    max_front_entries: 100,
                    ..TieredConfig::default()
                };
                let mut map = TieredStrict::open(&path, config).unwrap();
                for i in 0..N {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v)).unwrap();
                }
                black_box(map.len());
            });
        });
    }

    // ── D.10 Pipeline benchmarks ──────────────────────────────────────────────

    pub fn bench_pipelined_insert(c: &mut Criterion) {
        // t0 in-place insert — should be near-identical to MemOnly and faster
        // than WriteBack (which hits persistent pds::HashMap).
        c.bench_function("pipelined_insert", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.pipelined");
                let mut map: PipelinedMap<String, i64> =
                    PipelinedMap::open(&path, TieredConfig::default()).unwrap();
                for i in 0..N {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                black_box(map.pending_flush());
            });
        });
    }

    pub fn bench_pipelined_commit(c: &mut Criterion) {
        // commit() after 100 inserts — O(N) t0→t1 conversion.
        c.bench_function("pipelined_commit", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.pipelined");
                let mut map: PipelinedMap<String, i64> =
                    PipelinedMap::open(&path, TieredConfig::default()).unwrap();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                map.commit();
                black_box(map.pending_flush());
            });
        });
    }

    pub fn bench_pipelined_flush(c: &mut Criterion) {
        // commit_and_flush() with 100 dirty keys — writes all dirty t1→t2.
        c.bench_function("pipelined_flush", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.pipelined");
                let mut map: PipelinedMap<String, i64> =
                    PipelinedMap::open(&path, TieredConfig::default()).unwrap();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                map.commit_and_flush().unwrap();
                black_box(map.pending_flush());
            });
        });
    }

    pub fn bench_mem_only_insert(c: &mut Criterion) {
        // MemOnly insert — plain std::HashMap; baseline for t0 speed.
        c.bench_function("mem_only_insert", |b| {
            b.iter(|| {
                let mut map: MemOnlyMap<String, i64> = MemOnlyMap::new();
                for i in 0..N {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                black_box(map.len());
            });
        });
    }

    pub fn bench_policy_comparison(c: &mut Criterion) {
        // Side-by-side: MemOnly / WriteBack / Pipelined / Durable, 100 inserts each.
        let mut group = c.benchmark_group("policy_comparison");

        group.bench_function("MemOnly", |b| {
            b.iter(|| {
                let mut map: MemOnlyMap<String, i64> = MemOnlyMap::new();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                black_box(map.len());
            });
        });

        group.bench_function("WriteBack", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.wb");
                let mut map = TieredRelaxed::open(&path, TieredConfig::default()).unwrap();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                black_box(map.pending_count());
            });
        });

        group.bench_function("Pipelined", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.pipe");
                let mut map: PipelinedMap<String, i64> =
                    PipelinedMap::open(&path, TieredConfig::default()).unwrap();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v));
                }
                black_box(map.pending_flush());
            });
        });

        group.bench_function("Durable", |b| {
            b.iter(|| {
                let dir = tempdir().unwrap();
                let path = dir.path().join("bench.dur");
                let mut map = TieredStrict::open(&path, TieredConfig::default()).unwrap();
                for i in 0..100 {
                    let (k, v) = make_kv(i);
                    map.insert(black_box(k), black_box(v)).unwrap();
                }
                black_box(map.len());
            });
        });

        group.finish();
    }
}

#[cfg(not(feature = "tiered"))]
criterion_group!(
    benches,
    bench_strict_insert,
    bench_strict_insert_batch,
    bench_relaxed_insert,
    bench_relaxed_insert_flush,
    bench_get,
    bench_checkpoint,
    bench_heap_reference,
    bench_insert_comparison,
);

#[cfg(feature = "tiered")]
criterion_group!(
    benches,
    bench_strict_insert,
    bench_strict_insert_batch,
    bench_relaxed_insert,
    bench_relaxed_insert_flush,
    bench_get,
    bench_checkpoint,
    bench_heap_reference,
    bench_insert_comparison,
    tiered_benches::bench_tiered_strict_insert,
    tiered_benches::bench_tiered_relaxed_insert,
    tiered_benches::bench_tiered_relaxed_flush,
    tiered_benches::bench_tiered_get_warm,
    tiered_benches::bench_tiered_get_cold,
    tiered_benches::bench_tiered_eviction,
    tiered_benches::bench_pipelined_insert,
    tiered_benches::bench_pipelined_commit,
    tiered_benches::bench_pipelined_flush,
    tiered_benches::bench_mem_only_insert,
    tiered_benches::bench_policy_comparison,
);

criterion_main!(benches);
