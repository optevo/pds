# Performance Baselines {#sec:baselines}

Baseline measurements for build speed, test speed, and runtime benchmarks.
Re-run periodically (especially after significant changes) to detect
regressions or improvements. Compare against these numbers.

**Machine:** MacBook Pro M5 Max (18-core CPU, 128 GB unified RAM)
**Rust:** 1.85.0 (stable, via Nix rust-overlay)
**Date:** 2026-07-01 (fresh session — prior session results discarded; machine was idle)
**Files:** tee'd to `/private/tmp/bench_folio_1782868375.txt`, `/private/tmp/bench_merkle_*.txt`, `/private/tmp/bench_durable_1782869551.txt`

---

## Contents

- [pds-folio baselines](#pds-folio-baselines)
- [pds-merkle-spine baselines](#pds-merkle-spine-baselines)
- [pds-durable baselines](#pds-durable-baselines)
- [Interruption guard results](#interruption-guard-results)
- [How to re-run](#how-to-re-run)

---

## pds-folio baselines {#sec:pds-folio-baselines}

**Bench command:** `direnv exec . cargo bench -p pds-folio`
**Notes:** All median times from criterion. MemBackend (in-memory, no disk I/O).
Folio one-shot xxhash optimisation (PERF-FOLIO-001) is applied.

### HamtMap<u64, u64>

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `hamt_insert` (build from empty) | 21.4 µs | 396.8 µs | 6.78 ms | 83.5 ms |
| `hamt_get` (single lookup, n/2 key) | 43.5 ns | 78.6 ns | 115.2 ns | 119.1 ns |
| `hamt_remove` (build + remove all) | 36.8 µs | 812 µs | 14.5 ms | 181.8 ms |
| `hamt_clone_snapshot` (O(1) refcount) | 37.1 ns | 37.2 ns | 37.1 ns | 37.4 ns |

### HamtSet<u64>

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `hamtset_contains` (single probe) | 39.5 ns | 71.6 ns | 103.2 ns | 103.4 ns |

### FolioVector<u32>

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `vector_push_back` (build from empty) | 21.2 µs | 342.8 µs | 4.59 ms | 67.3 ms |
| `vector_get` (single read, n/2 index) | 254.9 ns | 509.9 ns | 509.0 ns | 770.2 ns |

### FolioOrdMap<u32, u32> (B+ tree, BTREE_ORDER=32)

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `ordmap_insert_sequential` | 50.7 µs | 622.5 µs | 9.95 ms | 124.2 ms |
| `ordmap_insert_random` | 51.4 µs | 640.4 µs | 9.51 ms | 124.1 ms |
| `ordmap_range_scan` | 333.5 ns | 2.27 µs | 22.1 µs | 235.1 µs |

### FolioOrdSet<u32> (B+ tree)

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `ordset_insert` (build from empty) | 50.3 µs | 580.3 µs | 9.60 ms | 119.7 ms |

### Codec comparison (PodCodec vs PostcardCodec, HamtMap)

| Benchmark | PostcardCodec n=1K | PodCodec n=1K | PostcardCodec n=10K | PodCodec n=10K |
|-----------|-----------------:|-------------:|--------------------:|---------------:|
| `pod_codec/get` | 114.1 ns | 114.8 ns | 115.5 ns | 115.9 ns |
| `pod_codec/insert` | 6.95 ms | 6.98 ms | 87.4 ms | 87.5 ms |

**Finding:** PodCodec and PostcardCodec are indistinguishable at this granularity for
`u64` keys/values — both codecs encode Pod types to the same byte sequence. See
`docs/decisions.md` for the full analysis.

### Collection comparison (insert)

| n | HamtMap | FolioOrdMap | FolioVector |
|--:|--------:|------------:|------------:|
| 10 | 21.7 µs | 51.2 µs | 21.2 µs |
| 100 | 403.3 µs | 633.9 µs | 343.5 µs |
| 1 000 | 6.94 ms | 10.3 ms | 4.53 ms |
| 10 000 | 84.5 ms | 127.3 ms | 67.8 ms |

### Collection comparison (get, single element at n/2)

| n | HamtMap | FolioOrdMap | FolioVector |
|--:|--------:|------------:|------------:|
| 10 | 46.9 ns | 268.6 ns | 251.3 ns |
| 100 | 82.5 ns | 517.9 ns | 502.5 ns |
| 1 000 | 117.4 ns | 825.1 ns | 509.5 ns |
| 10 000 | 118.4 ns | 1.06 µs | 767.4 ns |

### Collection comparison (clone, O(1) structural sharing)

| n | HamtMap | FolioOrdMap | FolioVector |
|--:|--------:|------------:|------------:|
| 10 | 39.7 ns | 41.97 ns | 42.6 ns |
| 100 | 39.8 ns | 42.2 ns | 42.6 ns |
| 1 000 | 41.1 ns | 43.3 ns | 42.1 ns |
| 10 000 | 39.6 ns | 42.7 ns | 43.4 ns |

---

## pds-merkle-spine baselines {#sec:pds-merkle-spine-baselines}

**Bench command:** `direnv exec . cargo bench -p pds-merkle-spine`
**Notes:** All median times from criterion. MemBackend (in-memory, no disk I/O).
H.9 lazy Merkle root optimisation is applied — root hash is computed lazily
on demand, not on every insert.

| Benchmark | n=10 | n=100 | n=1 000 |
|-----------|-----:|------:|--------:|
| `versioned_hamt_insert` (build from empty) | 43.2 µs | 652.2 µs | 10.43 ms |
| `versioned_hamt_get_current` (get at current version) | 42.6 ns | 76.6 ns | 110.4 ns |
| `versioned_hamt_get_at_version` (get at mid-history) | 52.7 ns | 86.3 ns | 120.4 ns |
| `versioned_hamt_checkout` (restore snapshot, O(1)) | 53.1 ns | 53.1 ns | 52.6 ns |
| `versioned_hamt_prove` (Merkle inclusion proof) | 208.5 ns | 245.0 ns | 279.3 ns |
| `versioned_hamt_clone` (O(1) structural share) | 41.9 ns | 41.9 ns | 41.9 ns |

### Key observations

- **Insert (build from empty) is now dominated by hamt_insert cost, not BLAKE3.**
  The H.9 lazy root means the root hash is not computed during the insert loop.
  At n=1 000: `versioned_hamt_insert` costs 10.43 ms vs 6.78 ms for plain `HamtMap`
  insert — approximately 1.54× overhead, down from ~16× before H.9. The overhead
  is primarily Mutex acquisition + per-version bookkeeping.
- **Get is slightly slower than plain HamtMap** due to Mutex acquire. At n=1 000:
  `get_current` = 110.4 ns vs 115.2 ns for HamtMap (within noise range).
- **Checkout is O(1) at ~53 ns** regardless of history depth (Mutex + Arc clone only).
- **Clone is O(1) at ~42 ns for all sizes** — pure refcount increment, no data copied.
- **Prove is O(log N)** for key lookup + two BLAKE3 hash calls (208–279 ns at n=10–1000).

---

## pds-durable baselines {#sec:pds-durable-baselines}

**Bench command:** `direnv exec . cargo bench -p pds-durable`
**Notes:** All median times from criterion. File-backed WAL on macOS tmpfs
(`tempfile::tempdir()`). macOS tmpfs fsync latency ≈ 4.1 ms per `sync_data()` call.
N = 1 000 entries per iteration for all benchmarks.
PERF-001 group commit (insert_batch) is implemented.

| Benchmark | Time (N=1 000) | Notes |
|-----------|---------------:|-------|
| `durable_map_strict_insert` | 4.09 s | 1 fsync/entry × 1 000 entries |
| `durable_map_strict_insert_batch` | 5.71 ms | 1 fsync for 1 000 entries (group commit) |
| `durable_map_relaxed_insert` | 225 µs | No fsync; write-only to pending buffer |
| `durable_map_relaxed_insert_flush` | 6.40 ms | 100 inserts + 1 fsync (flush) |
| `durable_map_get` | 24.97 µs | 500 lookups across 1 000-entry map |
| `durable_map_checkpoint` | 4.15 s | 1 000 inserts (strict) + checkpoint |
| `heap_reference` | 65.5 µs | Plain pds::HashMap, 1 000 inserts; no WAL |

### Comparison: Strict vs Relaxed vs Heap (insert, N=1 000)

| Mode | Time | vs heap | Notes |
|------|-----:|--------:|-------|
| `insert_comparison/heap_only` | 66.2 µs | 1.0× | pds::HashMap only |
| `insert_comparison/relaxed_no_flush` | 220 µs | 3.3× | WAL write, no fsync |
| `insert_comparison/strict_fsync` | 4.16 s | ~63 000× | 1 fsync/entry on macOS tmpfs |

### Key observations

- **Strict insert is entirely fsync-dominated.** macOS tmpfs costs ≈ 4.1 ms per
  `sync_data()`. 1 000 inserts × 4.1 ms = 4.1 s total.
- **Group commit (PERF-001) achieves 317× improvement** for bulk workloads:
  5.71 ms vs 4 090 ms when using `insert_batch()` instead of per-entry `insert()`.
- **Relaxed mode is 3.3× heap cost** — WAL serialisation + file write without fsync.
- **Checkpoint adds one extra fsync** atop the 1 000 strict inserts already in the
  benchmark; the additional cost ≈ 60 ms (two-pass WAL write for snapshotting).
- **Real-disk fsync** will be ~100 µs/call (NVMe) vs 4.1 ms (macOS tmpfs), giving
  ~100 s vs 4.1 s for 1 000 strict inserts — still orders of magnitude faster with
  group commit.

---

## Interruption guard results {#sec:interruption-guard}

Applied the 5× median and 20% stddev/mean checks to every suite before recording.

| Suite | Max range / median | Verdict |
|-------|--------------------|---------|
| pds-folio (all benches) | ≤ 1.5% | PASS |
| pds-merkle-spine (all benches) | ≤ 1.0% | PASS |
| pds-durable: strict_insert | 1.2% | PASS |
| pds-durable: strict_insert_batch | 0.5% | PASS |
| pds-durable: relaxed_insert | 2.1% (I/O variance) | PASS |
| pds-durable: relaxed_insert_flush | 16.4% range (≈6% stddev/mean) | PASS — I/O jitter, no interruption; 5× check: max 7.04 ms << 5× median 32 ms |
| pds-durable: checkpoint | 1.6% | PASS |

No re-runs required.

---

## How to re-run {#sec:how-to-rerun}

```bash
# All suites (one at a time; never in parallel):
direnv exec . cargo bench -p pds-folio     2>&1 | tee /private/tmp/bench_folio_$(date +%s).txt
direnv exec . cargo bench -p pds-merkle-spine 2>&1 | tee /private/tmp/bench_merkle_$(date +%s).txt
direnv exec . cargo bench -p pds-durable   2>&1 | tee /private/tmp/bench_durable_$(date +%s).txt

# Single benchmark (filter):
direnv exec . cargo bench -p pds-folio --bench bench -- hamt_insert

# Before/after comparison (criterion baseline):
direnv exec . cargo bench -p pds-folio -- --save-baseline before
# ... make changes ...
direnv exec . cargo bench -p pds-folio -- --baseline before
```

When updating this document, note the date, Rust version, and any
significant changes that may have affected the numbers.
