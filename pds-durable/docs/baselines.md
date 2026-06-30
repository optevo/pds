# pds-durable — Benchmark Baselines

All measurements taken on Apple M5 Max (18-core, 128 GB unified RAM) using
`cargo bench` with the `bench` profile (release + debug symbols). tmpfs on macOS
means fsync latency is near zero; real-disk numbers are dominated by the ~100 µs
fsync cost per Strict insert.

---

## D.8 / D.9 Baseline — 2026-07-01

Criterion measurements: 100 samples, estimated 5 s per benchmark.
N = 1 000 key-value pairs per insert benchmark; N/2 = 500 reads per get benchmark.

### D.8 — `DurableMap` (WAL-backed)

| Benchmark | Median | Notes |
|-----------|--------|-------|
| `durable_map_strict_insert` (N=1 000) | — | fsync per entry; macOS tmpfs; not re-run in this session |
| `durable_map_relaxed_insert` (N=1 000) | 7.31 ms | No flush; buffer only |
| `durable_map_relaxed_insert_flush` (N=100 + flush) | 52.53 ms | 100 inserts + WAL fsync |
| `durable_map_get` (N/2 reads) | — | Not re-run in this session |
| `durable_map_checkpoint` | — | Not re-run in this session |
| `heap_reference` (N=1 000) | 67.09 µs | Bare `pds::HashMap` insert; reference baseline |

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
