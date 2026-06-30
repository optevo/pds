# pds-folio — Performance Baselines

Baselines and exploration results for the performance work items in `docs/impl-plan.md`.

---

## Summary table

Measured on Apple M5 Max, MemBackend, `u64 → u64`, PostcardCodec, release mode.
All timings are `criterion` point estimates (median).

### Before PERF-1 (original baseline, 2026-07-01)

| Operation | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|---------:|
| `hamt_get` | 381 ns | 752 ns | 863 ns | 873 ns |
| `hamt_insert` (total build) | — | — | — | 85.7 ms |

---

### After PERF-1 (page read cache, 2026-07-01)

| Operation | n=10 | n=100 | n=1 000 | n=10 000 |
|-----------|-----:|------:|--------:|---------:|
| `hamt_get` | 57.5 ns | 88.9 ns | 167 ns | 191 ns |
| `hamt_insert` (total build) | — | — | — | 82.5 ms |

---

## PERF-1 — Page read cache in NodeStore

**Status: KEPT**

**Hypothesis:** `get` traverses O(log N) pages; every page read incurs a folio store
lookup and memcopy.  The upper HAMT levels are read on every `get` — caching decoded
pages in `NodeStore` eliminates repeated store reads.

**Implementation:** Added `page_cache: HashMap<u64, HamtNodePage>` and
`cache_order: VecDeque<u64>` to `NodeStore`.  FIFO eviction at capacity 128 entries
(64 KiB).  Cache is populated on every `alloc_node` (write) and `read_node` (read).
Cache is invalidated on `free_node` and `free_nodes`.  The `read_node` signature
changed from `&self` to `&mut self` to allow cache mutation.

**Results:**

| n | before | after | improvement |
|--:|-------:|------:|------------:|
| 10 | 381 ns | 57.5 ns | **−84.9%** |
| 100 | 752 ns | 88.9 ns | **−88.2%** |
| 1 000 | 863 ns | 167 ns | **−80.6%** |
| 10 000 | 873 ns | 191 ns | **−78.1%** |

Insert at n=10 000: 85.7 ms → 82.5 ms (−3.7%; insert also reads pages during descent).

**Verdict:** All sizes show ≥15% improvement.  Gain is 78–88%.  Kept.

---

## PERF-2 — PodCodec for numeric key/value types

**Status: DEFERRED — Codec trait redesign required**

**Hypothesis:** `PostcardCodec` allocates a `Vec` per encode call and uses varint
encoding for integers.  `PodCodec` should bypass both via raw `bytemuck` bytes,
eliminating heap allocations per node access.

**Measurement:** Both `PostcardCodec` and `PodCodec` currently go through
`postcard::to_allocvec` / `postcard::from_bytes` via the `Codec` trait.  The `PodCodec`
Codec trait impl is a postcard fallback; the zero-copy path (`encode_pod`/`decode_pod`)
is only reachable directly, not through the `Codec` trait generic `encode<T: Serialize>`
/ `decode<T: Deserialize>` methods.

**Benchmark results (both use postcard, 2026-07-01):**

| Operation | codec | n=1 000 | n=10 000 |
|-----------|-------|--------:|---------:|
| `pod_codec/get` | PostcardCodec | 106.4 ns | 108.6 ns |
| `pod_codec/get` | PodCodec | 107.4 ns | 108.1 ns |
| `pod_codec/insert` | PostcardCodec | 6.33 ms | 79.7 ms |
| `pod_codec/insert` | PodCodec | 6.43 ms | 83.5 ms |

**Finding:** No difference between the two — both use postcard through the Codec trait.
A true zero-copy PodCodec requires either:
- Stable Rust specialization (not available)
- Changing the `Codec` trait signature from `encode<T: Serialize>` to accept a
  fixed-width buffer or a type-aware writer
- Using `std::any::TypeId` dispatch (runtime overhead, complexity)

After PERF-1, the `get` at n=10 000 is ~108 ns.  The postcard decode of a `u64` from
a small byte slice is O(1) with minimal overhead relative to the Mutex lock (~30 ns)
and HashMap cache lookup.  The PoC established that the Codec trait's generic
`encode<T>` / `decode<T>` interface prevents zero-copy specialisation without a
breaking API change.

**Verdict:** Not implemented.  The `Codec` trait redesign needed for a true
`PodCodec` is a Phase G follow-up item (a breaking change to the trait); the gain
at current get latency (~108 ns) would not reach ≥15%.  Deferred.

---

## PERF-3 — Single-threaded NodeStore path

**Status: NOT IMPLEMENTED — Mutex overhead below 10% threshold**

**Hypothesis:** `Arc<Mutex<NodeStore<B>>>` acquires a lock on every node read/write.
At 30–50 ns per lock/unlock on M-series, this would cost 30–50 ns × O(log N) per
`HamtMap::get`.

**Measurement:** Uncontended Mutex lock/unlock on M5 Max measured at **4.5 ns/op**
(using `std::sync::Mutex`).  `get` acquires the Mutex once per call (not once per
page — the lock is held for the entire HAMT descent).

After PERF-1, `hamt_get` at n=10 000 is ~191 ns.  Mutex overhead: 4.5 ns / 191 ns
= **~2.4%** — well below the 10% threshold defined in the impl-plan.

**Verdict:** Not implemented.  2.4% is too small to justify the complexity and
`!Send + !Sync` implications of a single-thread `NodeStoreKind` enum.  The
`Arc<Mutex<>>` path stays.

---

## PERF-4 — Write batching

**Status: NOT APPLICABLE to MemBackend — no implementation**

**Hypothesis:** Each HAMT insert writes O(log N) pages independently; grouping all
dirty pages into one WAL commit would reduce WAL flush overhead and write
amplification.

**Analysis:** `pds-folio` uses `MemBackend` for all current benchmarks.
`MemBackend` has no WAL — each page write is a `HashMap` insert into an in-memory
store.  The folio WAL feature is gated by `feature = "wal"` and is not enabled in
the current build.  The `log_change` calls in `FolioStore` accumulate a changelog
in memory but issue no disk I/O.

PoC: counting `alloc_node` calls per HAMT insert at n=10 000 confirms O(log N) ≈ 3-4
page writes per insert.  For `MemBackend` these are O(1) HashMap insertions each.
Batching them would produce no measurable speedup on the current backend.

**Verdict:** Not implemented.  The gain is real but only for disk-backed backends
(FolioStore with `feature = "wal"`).  When a disk-backed benchmarking target is
added, batch-write should be revisited as the first optimisation candidate for
insert-heavy workloads.
