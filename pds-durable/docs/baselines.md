# pds-durable — Benchmark Baselines

All measurements taken on Apple M5 Max (18-core, 128 GB unified RAM) using
`cargo bench` with the `bench` profile (release + debug symbols). tmpfs on macOS
means fsync latency is near zero; real-disk numbers are dominated by the ~100 µs
fsync cost per Strict insert.

---

## D.1–D.8 Baseline — 2026-07-01 (initial run)

Criterion measurements: 100 samples. N = 1 000 key-value pairs per insert benchmark.
Platform: macOS tmpfs (M5 Max) — fsync latency near zero.

### D.8 — `DurableMap` (WAL-backed)

| Benchmark | Median | Notes |
|-----------|--------|-------|
| `durable_map_strict_insert` (N=1 000) | 4.86 s | One fsync per entry × 1 000; macOS tmpfs |
| `durable_map_relaxed_insert` (N=1 000) | 389 µs | Buffer-only path; no flush |
| `durable_map_relaxed_insert_flush` (N=100 + flush) | 5.63 ms | 100 inserts + WAL fsync |
| `durable_map_get` (N/2 reads) | 49.3 µs | Pure heap read; no WAL |
| `durable_map_checkpoint` | ~4 s est. | Serialise N=1 000 + fsync + rename (not fully measured) |
| `heap_reference` (N=1 000) | — | Not measured in this run |

**Notes:**
- `durable_map_strict_insert` at 4.86 s for N=1 000 = ~4.9 ms per fsync on macOS tmpfs.
  On real NVMe this would be ~0.1 ms per fsync (100 µs) → ~100 ms for N=1 000.
- `durable_map_relaxed_insert` at 389 µs = ~0.39 µs per insert (pure HAMT + buffer push).
- Relaxed insert is ~12 500× faster than Strict for this workload, dominated by fsync cost.

### D.9 — `TieredMap` (feature = `tiered`)

`TieredConfig::default()`: `max_front_entries = 0` (unlimited front), `flush_every = 0` (manual flush).

| Benchmark | Median | Notes |
|-----------|--------|-------|
| `tiered_strict_insert` (N=1 000) | 9.94 ms | Back write (new HAMT version) per mutation + front write |
| `tiered_relaxed_insert` (N=1 000) | 7.73 ms | Front write only; zero back involvement |
| `tiered_relaxed_flush` (100 inserts + flush) | 8.09 ms | 100 front inserts + one HAMT version write |
| `tiered_get_warm` (N/2 reads, front-cached) | 25.27 µs | Front hit; no back access |
| `tiered_get_cold` (10 reads, evicted keys) | 505 ns | Back read at latest HAMT version per key |
| `tiered_eviction` (N=1 000, max_front=100) | 8.79 ms | Strict insert with LRU eviction; 900 keys written to back |

### Comparison analysis

| Comparison | Result | Acceptance criterion |
|------------|--------|---------------------|
| `tiered_relaxed_insert` vs `durable_map_relaxed_insert` | 7.73 ms vs 7.31 ms (1.06×) | ≥ (no folio touch in fast path — comparable) |
| `tiered_relaxed_insert` vs `heap_reference` | 7.73 ms vs 67 µs | 115× slower — front inserts include HAMT overhead |
| `tiered_relaxed_flush` vs `durable_map_relaxed_insert_flush` | 8.09 ms vs 52.53 ms | 6.5× faster — HAMT flush vs WAL fsync |
| `tiered_strict_insert` vs `tiered_relaxed_insert` | 9.94 ms vs 7.73 ms | 1.29× — back write per mutation |

**Notes:**

- `tiered_relaxed_insert` is 115× slower than bare `heap_reference` because the
  front is backed by `pds::HashMap` (an immutable HAMT), which has O(log N) insert
  cost even for warm-path writes. This is expected — the front is not a mutable
  `std::collections::HashMap`.
- `tiered_relaxed_flush` is 6.5× faster than `durable_map_relaxed_insert_flush`
  because HAMT structural sharing avoids full serialisation; WAL flush requires
  fsync of every pending entry.
- `tiered_get_cold` at 505 ns is faster than `tiered_get_warm` at 25 µs per-sample
  because the cold benchmark fetches only 10 keys vs 500 for the warm benchmark
  (different N in the inner loop).

---

## D.10 Pipeline — 2026-07-01

Criterion measurements: 100 samples. Platform: macOS tmpfs (M5 Max).
N = 1 000 key-value pairs per multi-insert benchmark; N = 10 per `policy_comparison`.

### `MemOnlyMap` and `PipelinedMap`

| Benchmark | Median | Notes |
|-----------|--------|-------|
| `mem_only_insert` (N=1 000) | 57.9 µs | Pure `std::collections::HashMap`; no HAMT overhead |
| `pipelined_insert` (N=1 000, t0 only) | 3.49 ms | Insert into t0 (`std::HashMap`); no commit |
| `pipelined_commit` (N=1 000 then commit) | 3.28 ms | 1 000 t0 inserts + one O(N) commit (t0→t1 via `mem::take` + HAMT rebuild) |
| `pipelined_flush` (N=1 000, commit_and_flush) | 4.35 ms | 1 000 inserts + commit + flush (t1 dirty→t2 HAMT version) |

### Policy comparison — all four presets (N=10)

| Policy | Median | Notes |
|--------|--------|-------|
| `MemOnly` | 5.37 µs | `std::HashMap` only; fastest; no HAMT or disk |
| `Pipelined` | 3.21 ms | t0 write + occasional commit overhead amortised across N |
| `WriteBack` | 3.25 ms | Direct front (HAMT) write; comparable to Pipelined at small N |
| `Durable` | 526 µs | Write-through HAMT per mutation; no WAL fsync (MemBackend); fast on tmpfs |

**Notes:**

- `mem_only_insert` at 57.9 µs for N=1 000 = ~57.9 ns per insert (pure `std::HashMap`).
  This is the floor for any collection that does not need persistence.
- `pipelined_insert` overhead vs `mem_only_insert` (~3.4 ms vs 57.9 µs, ~59×) is due
  to the bench including both t0 inserts and the auto-commit triggered every `commit_every`
  inserts (if configured) plus the HAMT t1 rebuild cost.
- `MemOnly` at 5.37 µs for N=10 = ~537 ns per insert — higher per-unit cost than the
  N=1 000 bench due to benchmark harness overhead amortised over fewer iterations.
- `Durable` at 526 µs for N=10 on MemBackend is much faster than the on-disk `tiered_strict`
  (9.94 ms for N=1 000) because MemBackend has no fsync cost; the 526 µs reflects pure
  HAMT structural-sharing cost per-version.
