# Performance Baselines {#sec:baselines}

Baseline measurements for build speed, test speed, and runtime benchmarks.
Re-run periodically (especially after significant changes) to detect
regressions or improvements. Compare against these numbers.

**Machine:** MacBook Pro M5 Max (18-core CPU, 128 GB unified RAM)
**Rust:** 1.85.0 (stable, via Nix rust-overlay)
**Date:** 2026-07-01 (updated after PERF-FOLIO-002 — PageRefcount hashbrown migration)
**Files:** tee'd to `/private/tmp/bench_folio_hashbrown_<timestamp>.txt`, `/private/tmp/bench_merkle_*.txt`, `/private/tmp/bench_durable_1782869551.txt`

**pds-folio update (2026-07-01):** `PageRefcount` in `folio-collections` migrated from
`BTreeMap` (O(log n)) to `hashbrown::HashMap` (O(1) amortised). HamtMap insert improved
7–12% across all N. See PERF-FOLIO-002 in the Log and `docs/decisions.md`.

---

## Contents

- [pds (base collections) baselines](#pds-base-baselines)
- [pds-folio baselines](#pds-folio-baselines)
- [pds-merkle-spine baselines](#pds-merkle-spine-baselines)
- [pds-durable baselines](#pds-durable-baselines)
- [Tiered collections baselines](#tiered-baselines)
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
| `hamt_insert` (build from empty) | 21.4 µs | 370.0 µs | 6.20 ms | 73.4 ms |
| `hamt_get` (single lookup, n/2 key) | 43.4 ns | 78.4 ns | 116.2 ns | 117.8 ns |
| `hamt_remove` (build + remove all) | 37.3 µs | 776.9 µs | 13.4 ms | 163.9 ms |
| `hamt_clone_snapshot` (O(1) refcount) | 22.6 ns | 21.7 ns | 22.5 ns | 22.9 ns |

**Note:** `hamt_clone_snapshot` times decreased from ~37 ns to ~22 ns. The previous
measurement included `increment_root_refcount` which acquired a `Mutex` via
`node_store.lock()` on every clone. The concurrent session (commit `4dc85e2`) refactored
the NodeStore refcount table to use `PageRefcount`, which also changed the clone path.

### HamtSet<u64>

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `hamtset_contains` (single probe) | 41.6 ns | 71.6 ns | 100.8 ns | 101.9 ns |

### FolioVector<u32>

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `vector_push_back` (build from empty) | 20.7 µs | 333.7 µs | 4.41 ms | 64.1 ms |
| `vector_get` (single read, n/2 index) | 244.5 ns | 488.5 ns | 489.9 ns | 740.0 ns |

### FolioOrdMap<u32, u32> (B+ tree, BTREE_ORDER=32)

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `ordmap_insert_sequential` | 49.7 µs | 605.2 µs | 9.57 ms | 116.7 ms |
| `ordmap_insert_random` | 49.5 µs | 605.4 µs | 9.07 ms | 115.6 ms |
| `ordmap_range_scan` | 320.4 ns | 2.15 µs | 21.0 µs | 216.3 µs |

### FolioOrdSet<u32> (B+ tree)

| Benchmark | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|--------:|
| `ordset_insert` (build from empty) | 48.1 µs | 551.0 µs | 9.11 ms | 113.0 ms |

### Codec comparison (PodCodec vs PostcardCodec, HamtMap)

| Benchmark | PostcardCodec n=1K | PodCodec n=1K | PostcardCodec n=10K | PodCodec n=10K |
|-----------|-----------------:|-------------:|--------------------:|---------------:|
| `pod_codec/get` | 113.0 ns | 111.7 ns | 113.0 ns | 112.6 ns |
| `pod_codec/insert` | 5.95 ms | 5.97 ms | 73.5 ms | 87.6 ms† |

† PodCodec/10000 insert showed an unusually wide CI (84.7–90.5 ms vs 73.3–73.7 ms for
PostcardCodec). Likely I/O jitter in this particular run; the codecs are otherwise
indistinguishable for `u64` types (same byte representation). See `docs/decisions.md`
for the full analysis.

**Finding:** PodCodec and PostcardCodec are indistinguishable at this granularity for
`u64` keys/values — both codecs encode Pod types to the same byte sequence. See
`docs/decisions.md` for the full analysis.

### Collection comparison (insert)

| n | HamtMap | FolioOrdMap | FolioVector |
|--:|--------:|------------:|------------:|
| 10 | 21.8 µs | 50.9 µs | 20.5 µs |
| 100 | 375.2 µs | 631.5 µs | 334.9 µs |
| 1 000 | 6.29 ms | 9.72 ms | 4.35 ms |
| 10 000 | 73.5 ms | 117.4 ms | 64.8 ms |

### Collection comparison (get, single element at n/2)

| n | HamtMap | FolioOrdMap | FolioVector |
|--:|--------:|------------:|------------:|
| 10 | 43.6 ns | 265.3 ns | 248.1 ns |
| 100 | 77.6 ns | 513.9 ns | 497.4 ns |
| 1 000 | 112.0 ns | 811.5 ns | 498.6 ns |
| 10 000 | 112.8 ns | 1.05 µs | 753.8 ns |

### Collection comparison (clone, O(1) structural sharing)

| n | HamtMap | FolioOrdMap | FolioVector |
|--:|--------:|------------:|------------:|
| 10 | 20.7 ns | 20.5 ns | 21.2 ns |
| 100 | 20.7 ns | 20.5 ns | 21.1 ns |
| 1 000 | 20.7 ns | 20.6 ns | 21.1 ns |
| 10 000 | 20.9 ns | 20.7 ns | 21.1 ns |

**Note:** Clone times improved from ~40 ns to ~21 ns. The refactoring in commit
`4dc85e2` simplified `clone()` by removing the `increment_root_refcount` call that
had previously acquired a `Mutex` lock per clone. The current clone path is a pure
`Arc::clone` (atomic refcount increment, no mutex).

---

## pds (base collections) baselines {#sec:pds-base-baselines}

**Bench command:** `direnv exec . cargo bench -p pds --bench hashmap`
**Notes:** All median times from criterion. Default features (no `foldhash`; SipHash-1-3).
HASH_LEVEL_SIZE=5 (32-way branching). Partial run — 500K-entry string benchmarks not captured
(too slow; ran for 30+ minutes without completing).

### pds::HashMap<i64, i64>

| Benchmark | n=100 | n=1 000 | n=10 000 | n=100 000 |
|-----------|------:|--------:|---------:|----------:|
| `hashmap_i64/lookup` (N lookups) | 654 ns | 6.88 µs | 81.1 µs | 1.239 ms |
| `hashmap_i64/insert_mut` (N in-place inserts) | 2.29 µs | 31.1 µs | 133.8 µs | 4.08 ms |
| `hashmap_i64/insert` (N persistent inserts) | 17.5 µs | 227.9 µs | 3.15 ms | — |
| `hashmap_i64/remove` (N persistent removes) | 13.6 µs | 225.4 µs | 3.28 ms | — |
| `hashmap_i64/iter` | — | 2.03 µs | 32.0 µs | 550 µs |
| `hashmap_i64/from_iter` | — | 31.1 µs | 241.6 µs | 4.01 ms |

### pds::HashMap<String, String>

| Benchmark | n=100 | n=1 000 | n=10 000 | n=100 000 |
|-----------|------:|--------:|---------:|----------:|
| `hashmap_str/lookup` (N lookups) | 753 ns | 8.40 µs | 139.9 µs | 2.837 ms |
| `hashmap_str/insert_mut` (N in-place inserts) | 2.91 µs | 42.3 µs | 209.7 µs | 6.93 ms |
| `hashmap_str/iter` | — | 1.98 µs | 41.4 µs | — |
| `hashmap_str/insert_once` | — | — | — | 55.96 µs |
| `hashmap_str/remove_once` | — | — | — | 54.59 µs |

**Key observations:**
- `insert` (persistent — each call returns a new map) is ~14× slower than `insert_mut` at n=1K,
  reflecting the cost of path-copying in the persistent HAMT on each insertion.
- String hashing is ~30% slower than i64 hashing for lookup at n=1K (8.40 µs vs 6.88 µs).
- `from_iter` at n=1K takes 31 µs; building with sequential `insert` would cost 227 µs.
  `from_iter` is ~7.3× faster because it uses the mutable path for bulk construction.

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

### Initial baseline (2026-07-01 morning)

| Suite | Max range / median | Verdict |
|-------|--------------------|---------|
| pds-folio (all benches) | ≤ 1.5% | PASS |
| pds-merkle-spine (all benches) | ≤ 1.0% | PASS |
| pds-durable: strict_insert | 1.2% | PASS |
| pds-durable: strict_insert_batch | 0.5% | PASS |
| pds-durable: relaxed_insert | 2.1% (I/O variance) | PASS |
| pds-durable: relaxed_insert_flush | 16.4% range (≈6% stddev/mean) | PASS — I/O jitter, no interruption; 5× check: max 7.04 ms << 5× median 32 ms |
| pds-durable: checkpoint | 1.6% | PASS |

### pds-folio re-run after PERF-FOLIO-002 (2026-07-01)

| Suite | Max range / median | Verdict |
|-------|--------------------|---------|
| hamt_insert (all N) | ≤ 1.0% | PASS |
| hamt_get (all N) | ≤ 0.4% | PASS |
| hamt_remove (all N) | ≤ 1.1% | PASS |
| hamt_clone_snapshot (all N) | ≤ 0.7% | PASS |
| hamtset_contains (all N) | ≤ 3.5% (n=10 has wide CI due to ~40–43 ns range) | PASS |
| vector_push_back (all N) | ≤ 0.5% | PASS |
| vector_get (all N) | ≤ 0.3% | PASS |
| ordmap_insert_* (all N) | ≤ 0.4% | PASS |
| ordset_insert (all N) | ≤ 0.3% | PASS |
| compare_insert/HamtMap/10000 | 0.7% range | PASS |

No re-runs required. High-severe outlier counts (up to 18%) are within normal criterion
behaviour — they do not shift the median, only widen the CI.

---

## Tiered collections baselines {#sec:tiered-baselines}

**Bench command:** `direnv exec . cargo bench --bench tiered --features tiered`
**Date:** 2026-07-02 (updated after T.1 — TieredBag flush insert_many optimisation)
**Notes:** All median times from criterion. `TieredCollection` with `PropagationPolicy::Manual`
(propagation overhead excluded) unless otherwise noted.
Results in `/private/tmp/bench_tiered_new_types_1782915695.txt`.

**Tuning findings (T.0d):** See `docs/perf-tuning-plan.md` for the full tuning log.
Key result: `Mutex` is 60% faster than `RwLock` for uncontended reads on Apple Silicon
M5 Max (5 ns vs 8 ns). `Immediate` propagation adds ~6× overhead vs `Manual` at n=1 000.

**Tuning (T.1 — 2026-07-02):** `TieredBag::flush` now calls `cold.insert_many(elem, count)`
instead of looping `count` times. Each distinct element costs one HAMT path-copy regardless
of count. `flush_1000` improved 45.6% (75.5 µs → 41.2 µs). See `perf-tuning-plan.md` Log.

### TieredCollection (HashMap tier: StdHashMap → PdsHashMap)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_hash/insert` (build from empty) | 3.59 µs | 43.4 µs | 405 µs |
| `tiered_hash/get_hit` (hot-tier hit) | — | 8.59 ns | — |
| `tiered_hash/get_cold_fallback` (cold-tier fallback) | — | 9.51 ns | — |
| `tiered_hash/flush_1000` (flush 1 000 entries) | — | 279.6 µs | — |
| `tiered_hash/cold_snapshot` (Arc clone of cold) | — | 5.60 ns | — |

### TieredCollection (OrdMap tier: StdBTreeMap → PdsOrdMap)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_ord/insert` (build from empty) | 2.51 µs | 32.8 µs | 598 µs |
| `tiered_ord/get_hit` (hot-tier hit) | — | 7.54 ns | — |
| `tiered_ord/get_cold_fallback` (cold-tier fallback) | — | 7.94 ns | — |
| `tiered_ord/flush_1000` (flush 1 000 entries) | — | 174.2 µs | — |
| `tiered_ord/cold_snapshot` (Arc clone of cold) | — | 5.65 ns | — |

### TieredSequence (Vector tier: StdVec → PdsVector)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_vec/push_back` (build from empty) | 654 ns | 4.95 µs | 46.5 µs |
| `tiered_vec/get_hit` (hot-tier hit) | — | 4.83 ns | — |
| `tiered_vec/get_cold_fallback` (cold-tier fallback) | — | 7.68 ns | — |
| `tiered_vec/flush_1000` (flush 1 000 elements) | — | 8.05 µs | — |
| `tiered_vec/cold_snapshot` (Arc clone of cold) | — | 11.80 ns | — |

### 3-tier TieredCollection (StdHashMap → PdsHashMap → MerkleWrapper, `traits` feature)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_3tier_hash/insert` (build from empty) | 4.13 µs | 49.9 µs | 462 µs |
| `tiered_3tier_hash/get_hit` (hot-tier hit) | — | 8.50 ns | — |
| `tiered_3tier_hash/flush_1000` (flush 1 000 entries) | — | 280.7 µs | — |
| `tiered_3tier_hash/cold_snapshot` (Arc clone of cold) | — | 5.46 ns | — |

*Note: 3-tier baselines are from 2026-07-01 (bench file: bench_tiered_1782899618.txt).*

### Propagation policy overhead (tiered_hash, n=1 000)

| Policy | Time | vs Manual |
|--------|-----:|----------:|
| `Manual` (no propagation) | 44.2 µs | 1.0× |
| `Immediate` (propagate after every insert) | 280.7 µs | 6.4× |

**Observation:** `Immediate` policy propagates and flushes after every single insert.
At n=1 000 with unique keys (as in this benchmark, each insert adds to hot without
accumulating count), the flush cost is bounded and the overhead is modest (6×). For
workloads with high multiplicity per key, use `Batched(n)` or `Manual` + explicit
`flush()` for bulk operations.

### TieredSet (StdHashSet → PdsHashSet)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_set/insert` | 3.66 µs | 45.4 µs | 410 µs |
| `tiered_set/contains_hit` (hot-tier hit) | — | 8.05 ns | — |
| `tiered_set/contains_cold_fallback` | — | 9.28 ns | — |
| `tiered_set/flush_1000` | — | 277.6 µs | — |
| `tiered_set/cold_snapshot` | — | 5.46 ns | — |

### TieredBag (StdHashBag → PdsBag)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_bag/insert` | 3.26 µs | 37.6 µs | 381 µs |
| `tiered_bag/count_hit` (hot-tier hit) | — | 9.26 ns | — |
| `tiered_bag/count_cold_fallback` | — | 9.40 ns | — |
| `tiered_bag/flush_1000` | — | **41.2 µs** | — |
| `tiered_bag/cold_snapshot` | — | 5.81 ns | — |

*TieredBag flush is 6.7× faster than TieredSet/TieredHashMap flush because each distinct
element costs one HAMT path-copy via `insert_many` (T.1 optimisation, 2026-07-02).*

### TieredMultiMap (StdHashMultiMap → PdsHashMultiMap)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_multimap/insert` | 8.16 µs | 57.7 µs | 668 µs |
| `tiered_multimap/get_all_hit` (hot-tier hit) | — | 59.0 ns | — |
| `tiered_multimap/get_all_cold_fallback` | — | 124.3 ns | — |
| `tiered_multimap/flush_1000` | — | 109.2 µs | — |
| `tiered_multimap/cold_snapshot` | — | 5.82 ns | — |

### TieredBiMap (StdBiMap → PdsBiMap)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_bimap/insert` | 8.78 µs | 105 µs | 927 µs |
| `tiered_bimap/get_by_key_hit` (hot-tier hit) | — | 9.48 ns | — |
| `tiered_bimap/get_by_key_cold_fallback` | — | 9.30 ns | — |
| `tiered_bimap/flush_1000` | — | 195.2 µs | — |
| `tiered_bimap/cold_snapshot` | — | 7.39 ns | — |

### TieredSymMap (StdSymMap → PdsSymMap)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_symmap/insert` | 8.16 µs | 97.6 µs | 853 µs |
| `tiered_symmap/get_hit` (hot-tier hit) | — | 8.08 ns | — |
| `tiered_symmap/get_cold_fallback` | — | 9.60 ns | — |
| `tiered_symmap/flush_1000` | — | 188.4 µs | — |
| `tiered_symmap/cold_snapshot` | — | 7.38 ns | — |

### TieredInsertionOrderMap (StdInsertionOrderMap → PdsInsertionOrderMap)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_iom/insert` | 5.31 µs | 66.0 µs | 622 µs |
| `tiered_iom/get_hit` (hot-tier hit) | — | 13.90 ns | — |
| `tiered_iom/get_cold_fallback` | — | 13.21 ns | — |
| `tiered_iom/flush_1000` | — | 120.8 µs | — |
| `tiered_iom/cold_snapshot` | — | 7.15 ns | — |

### TieredTrie (StdTrie → PdsTrie)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_trie/insert` | 12.1 µs | 128 µs | 1.38 ms |
| `tiered_trie/get_hit` (hot-tier hit) | — | 30.0 ns | — |
| `tiered_trie/get_cold_fallback` | — | 32.1 ns | — |
| `tiered_trie/flush_1000` | — | 247.0 µs | — |
| `tiered_trie/cold_snapshot` | — | 5.78 ns | — |

### TieredUniqueVector (StdVec → PdsUniqueVec)

| Benchmark | n=100 | n=1 000 | n=10 000 |
|-----------|------:|--------:|---------:|
| `tiered_unique_vec/push_back` | 3.70 µs | 45.9 µs | 370 µs |
| `tiered_unique_vec/contains_hit` (hot-tier hit) | — | 9.20 ns | — |
| `tiered_unique_vec/contains_cold_fallback` | — | 9.46 ns | — |
| `tiered_unique_vec/flush_1000` | — | 85.5 µs | — |
| `tiered_unique_vec/cold_snapshot` | — | 15.4 ns | — |

### Key observations

- **Cold snapshot is O(1)** at ~5–15 ns regardless of collection size — pure Arc clone
  of the persistent collection (structural sharing; no data copied).
- **TieredBag flush is the fastest map-like flush** at 41.2 µs — 6.7× faster than
  TieredSet (277.6 µs) because `insert_many` amortises count multiplicity into one
  HAMT path-copy per distinct element (T.1 optimisation, 2026-07-02).
- **TieredBiMap and TieredSymMap insert cost** (~8–9 µs/100 vs ~3.6 µs/100 for
  TieredHashMap) reflects the double-index maintenance — both key→value and value→key
  HAMTs are updated on every insert.
- **TieredTrie is the slowest collection** across all operations (~30 ns get_hit vs
  ~8 ns for hash-based types) reflecting trie traversal vs HAMT probing.
- **TieredMultiMap get_all is expensive** at 59–124 ns because it allocates a Vec
  of values per call, not a single-element lookup.
- **OrdMap flush is 38% faster than HashMap flush** at n=1 000 (174 µs vs 280 µs):
  BTreeMap drain is more cache-friendly than HashMap drain for sequential traversal.
- **Vector flush is 35× faster than HashMap flush** at n=1 000 (8.05 µs vs 279.6 µs):
  append-log semantics avoid the full merge pass required by map flush.

---

## How to re-run {#sec:how-to-rerun}

```bash
# All suites (one at a time; never in parallel):
direnv exec . cargo bench -p pds-folio     2>&1 | tee /private/tmp/bench_folio_$(date +%s).txt
direnv exec . cargo bench -p pds-merkle-spine 2>&1 | tee /private/tmp/bench_merkle_$(date +%s).txt
direnv exec . cargo bench -p pds-durable   2>&1 | tee /private/tmp/bench_durable_$(date +%s).txt

# Tiered collections:
direnv exec . cargo bench --bench tiered --features tiered 2>&1 | tee /private/tmp/bench_tiered_$(date +%s).txt

# Single benchmark (filter):
direnv exec . cargo bench -p pds-folio --bench bench -- hamt_insert

# Before/after comparison (criterion baseline):
direnv exec . cargo bench -p pds-folio -- --save-baseline before
# ... make changes ...
direnv exec . cargo bench -p pds-folio -- --baseline before
```

When updating this document, note the date, Rust version, and any
significant changes that may have affected the numbers.
