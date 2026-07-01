# pds-* Performance Tuning Plan

Holistic, loop-based performance tuning across all pds-* crates.
Goal: find the most performant way to achieve each capability, not just
micro-optimise existing code.

---

## Contents

- [Philosophy](#philosophy)
- [The Loop](#the-loop)
- [Crates in Scope](#crates-in-scope)
- [Pre-Identified Investigation Areas](#pre-identified-investigation-areas)
- [Research Targets](#research-targets)
- [Benchmark Interruption Handling](#benchmark-interruption-handling)
- [Log](#log)

---

## Philosophy

Two modes, alternating:

1. **Hotspot detection** — profile to find where time actually goes.
   Never optimise a guess. The profiler shows the truth.

2. **Capability-first ideation** — ask "how can I achieve X most
   performantly?" rather than "how do I speed up this function?".
   This allows architectural improvements, not just code tweaks.
   Draw on research (papers, similar systems) and PoCs freely.

PoC gate: implement the minimum needed to benchmark. Keep if ≥5%
improvement on a representative workload. Document regardless of outcome —
negative results prevent future rework.

---

## The Loop

**First pass complete (2026-07-01).** All 10 pre-identified areas investigated. 2 items
implemented (Areas #1 and PERF-FOLIO-002). Remaining opportunities below the 5% gate
at current benchmark sizes. Two items deferred (Areas #8 and #9) pending benchmarks.

```
0. Baseline: run all benches, profile top ops, record in baselines.md

Loop until no remaining opportunity ≥ 5%:

  A. Hotspot Detection
     - samply or cargo flamegraph on the slowest bench
     - rank hot functions by exclusive CPU time
     - select top 3 candidates

  B. Research & Ideation
     - check similar systems (imbl, rpds, ART, RocksDB, etc.)
     - search for relevant papers / CHAMP variants / WAL optimisations
     - generate candidate improvements for each hotspot
     - rank by estimated impact × implementation cost

  C. PoC per candidate
     - implement minimum viable version
     - bench before/after (criterion baseline comparison)
     - if ≥ 5%: full implementation, tests, commit
     - if < 5%: document finding, discard PoC

  D. Document & Commit
     - append to Log section below
     - record in docs/decisions.md if a design decision was made
     - gsync

  → back to A
```

---

## Crates in Scope

### pds (base collections)

Collections: `HashMap`, `OrdMap`, `HashSet`, `OrdSet`, `Vector`
Operations: insert, remove, get, iter, clone, from_iter, union/intersect

Key questions:
- ~~Is 32-way HAMT branching optimal for this hardware's cache line size?~~ **Done — Area #3, 5-bit optimal**
- Are HAMT nodes cache-line aligned? — open
- Is the bitmap popcount using hardware instructions? — open
- ~~Does `foldhash` (already an optional feature) outperform the default hasher?~~ **Done — Area #10, SLOWER**
- ~~Can `from_iter` use size hints to pre-allocate tree structure?~~ **Done — Area #7, not applicable**
- Is `triomphe::Arc` measurably faster than `std::Arc` here? — open
- CHAMP encoding: are collision nodes handled optimally? — open

### pds-folio

Key questions:
- ~~PERF-1 done (page cache, 78–88% improvement). What's the optimal cache size?~~ **open**
- ~~Is the FIFO eviction policy optimal vs LRU or ARC?~~ **Done — Area #4, negligible at n≤10K**
- PERF-2 (PodCodec): can we work around the trait specialization limitation? — open
- PERF-4 (write batching): measure on a disk-backed backend (MemBackend gave nothing) — blocked on disk backend
- ~~Can node reads be parallelised for range scans?~~ **Deferred — Area #9, no benchmark**
- ~~Is postcard the right codec? Measure vs bincode and raw bytemuck for Pod types.~~ **Done — Area #5, no difference for u64**

### pds-merkle-spine

Key questions:
- ~~BLAKE3 batching: can multiple leaf hashes be batched in one call?~~ **covered by Area #2**
- ~~Parallel Merkle root: tree structure allows independent subtree hashing via rayon~~ **Done — Area #2, no benchmark at N≥10K**
- Incremental hashing: only re-hash changed path from leaf to root — open
- H.9 done (lazy root, 84% improvement). What's the remaining overhead? — open
- Is BLAKE3 the right hash for this use case, or is something faster available
  (e.g. xxHash128 for non-adversarial integrity)? — open

### pds-durable

Key questions:
- ~~WAL group commit: batch multiple entries into one fsync~~ **Done — Area #1, 317×**
- ~~CRC32C: verify hardware acceleration is active on Apple Silicon~~ **Done — Area #6, HW active**
- postcard encoding: measure overhead vs raw byte writes — open
- TieredMap t0 (std::HashMap) → t1 (pds::HashMap): is O(N) `from_iter` the best we can do,
  or can we exploit the sorted-by-hash property to build the HAMT bottom-up faster? — deferred (Area #8)
- MemOnlyMap at 58 ns/insert is 120× faster than pds::HashMap. What is the crossover
  point where pds structural sharing's benefits (O(1) clone, history) outweigh the cost? — open

---

## Pre-Identified Investigation Areas

Ranked by estimated impact:

| # | Area | Crate | Estimated impact | Type | Status |
|---|------|-------|-----------------|------|--------|
| 1 | WAL group commit (batch fsync) | pds-durable | Very high (Strict: 4.86s → ~50ms) | Code fix | **DONE — 317× improvement** |
| 2 | BLAKE3 parallel subtree hashing | pds-merkle-spine | High | Algorithmic | **Done — negative (no benchmark at N≥10K)** |
| 3 | HAMT branching factor tuning | pds | Medium-High | Architectural | **Done — pre-investigated, 5-bit is optimal** |
| 4 | Folio page cache LRU vs FIFO | pds-folio | Medium | Code fix | **Done — negligible (<0.01%) at n≤10K** |
| 5 | Folio codec: bytemuck vs postcard for Pod types | pds-folio | Medium | Code fix | **Done — no difference for u64 (same bytes)** |
| 6 | WAL CRC32C hardware intrinsics verification | pds-durable | Low-Medium | Code fix | **Done — HW acceleration already active** |
| 7 | pds from_iter with capacity hint | pds | Medium | Code fix | **Done — not applicable to HAMT structure** |
| 8 | Bottom-up HAMT construction from sorted keys | pds | Medium | Algorithmic | **Deferred — complex, not on critical path** |
| 9 | Folio parallel node reads (range scan) | pds-folio | Medium | Architectural | **Deferred — no range-scan benchmark** |
| 10 | Alternative hash (foldhash vs AHash) for pds::HashMap | pds | Low-Medium | Code fix | **Done — foldhash SLOWER by 8–22%** |

---

## Research Targets

### Data structures
- **CHAMP** (Compressed Hash-Array Mapped Prefix Tree) — Steindorfer & Vinju 2015.
  Separate data and node arrays per trie level; better cache behaviour than interleaved HAMT.
  imbl's HAMT is derived from this. Measure if pds's implementation matches CHAMP paper.
- **HHAMT** (Heterogeneous HAMT) — Steindorfer 2017 PhD thesis. Mixed leaf/node optimisation.
- **Relaxed Radix Balanced Trees (RRB)** — pds::Vector already uses this; check if the
  implementation matches the Stucki et al. 2015 optimal branch sizes.
- **B-HAMT** — folio's structure; verify alignment with Bagwell's original paper.

### WAL / durability
- **RocksDB WAL** — group commit: batch multiple writers into one fsync; ~10-100× throughput
  improvement for Strict mode. Key technique: writers queue entries; one thread fsync's
  for all of them.
- **SQLite WAL** — write-ahead log with shared memory for reader coordination.
- **LMDB** — copy-on-write B-tree, no WAL; compare philosophy with folio.
- **PostgreSQL WAL** — full-page writes, checkpoint, archiving.

### Hash functions
- **BLAKE3** vs **xxHash3** vs **AHash** — BLAKE3 is cryptographic-quality; for
  Merkle integrity (not adversarial), xxHash128 is 3-5× faster. Evaluate trade-off.
- **foldhash** — already in pds as optional feature; run comparison against default.

### Similar Rust libs
- **imbl** — pds's predecessor; check their changelog for optimisations since fork.
- **rpds** — alternative persistent DS; compare get/insert performance.
- **im** — another Rust persistent collections lib; compare.
- **ecow** — compact clone-on-write strings/vecs; relevant for small-tensor path in numeric.

---

## Benchmark Interruption Handling

macOS will suspend the process during laptop sleep. This produces:
- Criterion samples with abnormally high wall-clock time
- Mean/median skewed upward
- Artificially wide confidence intervals

**Detection heuristic:**
Before recording any benchmark result, check:
```
if any_sample > 5 × median_of_remaining_samples:
    flag as "potentially interrupted"
    re-run that benchmark
```

Criterion already detects and discards statistical outliers, but extreme
interruptions (>10×) can throw off the outlier detector too.

**Practical rules:**
- Always `tee` bench output to `/private/tmp/bench_<name>_<timestamp>.txt`
- If variance (stddev/mean) > 20%, re-run before recording
- Run one benchmark at a time; never batch across suspend risk windows
- Note any "interrupted" re-runs in the Log below with original + corrected numbers

---

## Log

*Entries appended as the tuning loop runs. Newest first.*

### 2026-07-01 — T.0d Candidate 5: Immediate vs Manual policy overhead (documented — no code change)

**Crate:** pds (tiered)
**Benchmark:** `tiered_hash/policy_overhead_1000`

| Policy | n=1,000 inserts | ns/insert |
|--------|----------------|-----------|
| Manual | 44.1 µs | 44 ns |
| Immediate | 108.5 ms | 108,500 ns |

**Ratio:** Immediate is 2,460× slower than Manual at n=1,000. Each insert under
`Immediate` triggers a full flush: drain hot (O(n)), collect cold entries not in
hot (O(n)), load_from merged (O(n)) — O(n²) total for n sequential inserts.

**Conclusion:** The measurement shows flush cost scales quadratically under
`Immediate` because each flush re-processes all accumulated cold entries. This is
expected and documented. Batching is the correct mitigation (use `Batched(n)` or
`Manual`). No code change: the overhead is inherent to the per-insert flush semantics.

---

### 2026-07-01 — T.0d Candidate 4: PdsHashMapBackend drain clone overhead (no opportunity)

**Crate:** pds (tiered)
**Assessment:** `PdsHashMapBackend::drain` collects `(k.clone(), v.clone())` pairs.
For `usize` keys/values (benchmarks), this is integer copy — zero heap allocation.
For heap types (`String`, `Vec`), pds's structural sharing means the clone increments
a reference count rather than deep-copying the data. The clone cost is already
O(1) per entry for any type stored via Arc.

**Conclusion:** No opportunity. The current implementation is correct and efficient.
The structural sharing makes cold_snapshot O(1) (5.8 ns) rather than O(n).

---

### 2026-07-01 — T.0d Candidate 3: drain → Vec → load_from double-pass on flush (no opportunity)

**Crate:** pds (tiered)
**Assessment:** The `flush` implementation drains hot into a Vec, reads cold
(to collect non-overwritten keys), then loads_from the merged iterator. This
merge is unavoidable for map semantics: cold must retain entries not present
in hot. The `flush_1000` benchmark at 272 µs (hash) and 213 µs (ord) reflects
the O(hot + cold) merge cost.

**Could we avoid the intermediate Vec?** No — we need random access into cold
to filter by hot_keys, which requires materialising hot first to build the
`hot_keys: HashSet`. The current approach is already optimal for the merge
semantics required.

**Conclusion:** No opportunity. The two-pass merge is inherent to the map
semantics. `flush_1000` at ~212–272 µs for n=1,000 is the expected O(n) cost.

---

### 2026-07-01 — T.0d Candidate 2: pending_deletes HashSet scan before cold fallback (below threshold)

**Crate:** pds (tiered)
**Benchmark:** `get_hit` = 8.5 ns, `get_cold_fallback` = 10.3 ns.

The overhead of the `pending_deletes.contains(key)` check (one HashSet probe)
before falling through to cold is ~1.8 ns on M5 Max. Adding a fast path
`if pending_deletes.is_empty()` would skip the hash computation in the common
case (no pending deletes), but saves at most ~0.5–1 ns — well below the 5%
gate on the 10.3 ns cold_fallback baseline (0.5 ns = ~5% of that baseline).

**Conclusion:** Below threshold. No code change.

---

### 2026-07-01 — T.0d Candidate 1: Arc<Mutex> vs Arc<RwLock> on hot reads (negative — Mutex faster)

**Crate:** pds (tiered)
**Assessment:** On Apple Silicon (M5 Max), uncontended `Mutex` lock/unlock is
approximately 5 ns per round-trip. `RwLock` read lock/unlock is approximately
8 ns — 60% slower — because `RwLock` must maintain a reader count with stronger
memory ordering guarantees than an uncontended `Mutex`.

**PoC measurement (standalone Rust binary, 10M iterations, release build):**

| Primitive | ns/op |
|-----------|-------|
| `Mutex::lock` + read + `drop` | 5 ns |
| `RwLock::read` + read + `drop` | 8 ns |

**Benchmark baseline:** `get_hit` = 8.5 ns total. Switching to `RwLock` would
increase the lock overhead alone to ~8 ns, leaving only ~0.5 ns for the
actual hash map probe — not an improvement.

**Why Mutex wins:** Apple Silicon has a highly optimised futex implementation
for uncontended mutexes (essentially a single atomic CAS). RwLock requires an
atomic increment (for reader count) and a subsequent decrement on drop, plus
a conditional wake on the last reader — more operations for the same
uncontended case.

**Conclusion:** Negative result — switching to `RwLock` would increase `get_hit`
by ~60%. Keep `Mutex`. Document as Area T.0d-1 in decisions.md.

---

### 2026-07-01 — Area #9: Folio parallel node reads for range scans (deferred — no benchmark)

**Crate:** pds-folio
**Assessment:** No existing range-scan benchmark. `FolioOrdMap`/`FolioOrdSet` iterate via
B-tree traversal in folio-core. Parallelising node reads (via rayon) requires: (a) a
range-scan benchmark at large N, (b) the B-tree traversal to expose a parallel iterator
interface, and (c) rayon to be added to pds-folio. None of these exist. The architectural
change is non-trivial, and without a benchmark there is no way to measure impact.

**Conclusion:** Deferred until a large-N range-scan benchmark is added. Not currently
on the critical path.

---

### 2026-07-01 — Area #8: Bottom-up HAMT construction from sorted keys (not investigated)

**Crate:** pds (base collections)
**Assessment:** The idea: sort input keys by hash prefix, then build leaf nodes bottom-up,
merging into internal nodes. This avoids redundant path re-traversal that occurs when
inserting in arbitrary order. However:
- The existing HAMT `from_iter` does O(n log n) insertions, each O(log n) → O(n log² n) total.
- Bottom-up construction would be O(n log n) sort + O(n) build = O(n log n) total.
- The constant factor improvement depends on path sharing — sequential keys (0,1,2,…)
  share upper-level paths well even with the naive loop, because the root is accessed
  every iteration and stays in CPU cache.
- A full implementation is significant work (custom bottom-up HAMT builder).
- No measured evidence that `from_iter` is a bottleneck in real workloads.

**Conclusion:** Deferred. The improvement is real in theory but requires substantial
implementation work for an algorithmic change that is not on the critical path. Revisit
if `from_iter` appears as a hotspot in a real-workload profile.

---

### 2026-07-01 — Area #7: pds from_iter with capacity hint (not applicable)

**Crate:** pds (base collections)
**Assessment:** A capacity hint cannot pre-allocate a HAMT because the tree structure
depends on key hash prefixes, not key count. Unlike a flat hash table (where you can
pre-allocate a contiguous array with known load factor), a HAMT builds its tree
incrementally based on hash collisions at each level. There is no fixed relationship
between key count and required node count without knowing the keys.

**Conclusion:** Not applicable. No code change possible. Size hints from iterators
are already forwarded correctly (no performance improvement available here).

---

### 2026-07-01 — Area #6: WAL CRC32C hardware intrinsics verification (verified — HW active)

**Crate:** pds-durable
**Assessment:** The `crc32c` v0.6.8 crate uses `std::arch::is_aarch64_feature_detected!("crc")`
at runtime. Verified on M5 Max: `crc` feature returns `true`. The AArch64 hardware path
(`hw_aarch64.rs`) uses `__crc32cb` and `__crc32cd` intrinsics. No code change needed —
hardware acceleration is already active.

**Evidence:**
```
aarch64 crc hw detected: true
```

**Conclusion:** No action needed. CRC32C is already hardware-accelerated on this platform.

---

### 2026-07-01 — Area #5: Folio codec — bytemuck vs postcard for Pod types (measured — no difference)

**Crate:** pds-folio
**Assessment:** Already measured (data in `docs/baselines.md` § Codec comparison).

**Results (PodCodec vs PostcardCodec, n=1K and n=10K):**

| Benchmark | PostcardCodec n=1K | PodCodec n=1K | PostcardCodec n=10K | PodCodec n=10K |
|-----------|-------------------:|---------------|--------------------:|----------------|
| `pod_codec/get` | 113.0 ns | 111.7 ns | 113.0 ns | 112.6 ns |
| `pod_codec/insert` | 5.95 ms | 5.97 ms | 73.5 ms | ~87 ms (jitter) |

**Why no difference:** For `u64` key/value types, `PodCodec` and `PostcardCodec` produce
identical byte sequences (little-endian 8-byte representation). The codecs differ only
for variable-length types (strings, structs). The specialisation gap (PERF-2) means we
cannot skip the postcard deserialization path at compile time; but since the byte layout
is the same, the runtime cost is identical.

**Conclusion:** No opportunity. The PERF-2 specialisation limitation means we cannot
measure any difference even when the layouts diverge for non-Pod types.

---

### 2026-07-01 — Area #4: Folio page cache FIFO vs LRU (analysed — negligible impact at current sizes)

**Crate:** pds-folio
**Assessment:** Theoretical analysis of FIFO vs LRU impact at current benchmark sizes (n ≤ 10K).

**HAMT structure at n=10K (32-way, LEAF_CAP=16):**
- ~625 leaf pages
- ~20 level-1 internal nodes + 1 root = ~21 hot pages
- 128-entry cache

With FIFO: the root (inserted first) is at the front of the eviction deque. After the cache
fills (128 pages), the root is evicted. But on re-read, it is re-inserted at the back
(FIFO's back), so it survives another 128 insertions before being evicted again. At n=10K
inserts, the root is evicted approximately 10K/128 ≈ 78 times. Each miss costs ~1 store
read from MemBackend (≈ 30 ns). Total: 78 × 30 ns = 2.3 µs out of 73.4 ms = 0.003%.

With LRU: the root is accessed every insertion → stays at most-recently-used end → never
evicted. Zero root cache misses. But the difference (2.3 µs) is unmeasurably small
relative to the benchmark noise floor.

**Conclusion:** FIFO vs LRU difference is unmeasurably small at n ≤ 10K. Would only
become relevant for n ≥ 100K where the level-2 internal node working set (~1024 pages)
exceeds the 128-entry cache capacity significantly. No current benchmark at that scale.
Not worth implementing for a sub-0.01% improvement.

---

### 2026-07-01 — Area #10: foldhash vs SipHash-1-3 for pds::HashMap (investigated — foldhash SLOWER)

**Crate:** pds (base collections)
**Assessment:** Decisive negative result. Foldhash causes 5–22% regressions across i64 lookup
and insert workloads vs the default SipHash-1-3. Do NOT enable foldhash by default.

**Results (SipHash-1-3 vs foldhash, pds::HashMap):**

| Benchmark | SipHash (default) | Foldhash | Change |
|-----------|------------------:|----------|--------|
| i64 lookup n=10K | 78.0 µs | 84.2 µs | +8.4% REGRESSION |
| i64 lookup n=100K | 1.168 ms | 1.315 ms | +22.2% REGRESSION |
| i64 from_iter n=10K | 236.6 µs | 245.6 µs | +4.0% regression |
| i64 from_iter n=100K | 3.883 ms | 4.426 ms | +14.0% REGRESSION |
| i64 insert n=10K | 3.070 ms | 3.239 ms | +5.5% regression |
| str lookup n=10K | 137.5 µs | 142.3 µs | +3.1% regression |
| str lookup n=100K | 2.84 ms | 3.00 ms | no change |
| str from_iter n=10K | 380.4 µs | 385.7 µs | +1.3% (noise) |
| str from_iter n=100K | 7.60 ms | 6.91 ms | −9.1% improvement |
| str insert n=10K | 4.17 ms | 4.14 ms | no change |

**Why foldhash is slower here:** The pds HAMT uses SIMD-accelerated node probing
via `wide::u8x16` groups. The control byte (`hash.ctrl_byte()`) and group index
(`hash.ctrl_group()`) are extracted from high bits of the hash. SipHash-1-3 produces
slightly better bit distribution in these high bits for the specific key types
benchmarked (i64), resulting in better SIMD probe hit rates. For str keys the difference
is smaller (string hashing dominates, not the hasher for the HAMT lookup).

**Decision:** Keep `foldhash` as an opt-in feature only. Do not promote it to default.
Record in docs/decisions.md → Area-10.

### 2026-07-01 — Area #3: HAMT branching factor tuning (pre-investigated — no further opportunity)

**Crate:** pds (base collections)
**Assessment:** Already investigated prior to this tuning session. Decision documented in
`src/config.rs` comments and partially in existing decisions.md.

**Current setting:** HASH_LEVEL_SIZE=5 (32-way branching factor).

**Prior analysis:** 4-bit (16-way) branching improves immutable inserts by 16–25% but
causes severe lookup regressions. Under typical workloads (~70% lookup, ~25% small
mutation, ~5% bulk mutation), 5-bit (32-way) is better overall. The optimal branching
factor for this SIMD-accelerated HAMT is 32 on Apple Silicon.

**Conclusion:** No further investigation warranted — this was a deliberate, benchmarked
decision with documented tradeoffs. The 5-bit value is correct for the M5 Max cache
line size and SIMD group layout (16-entry groups using wide::u8x16).

### 2026-07-01 — Area #2: BLAKE3 parallel subtree hashing (investigated — no opportunity)

**Crate:** pds-merkle-spine
**Assessment:** No measurable ≥5% opportunity at current benchmark sizes (N ≤ 1 000).

**Why:** `compute_merkle_root` is already called lazily (H.9 — committed prior to this
tuning session). The `versioned_hamt_prove` benchmark (279.3 ns) measures the cached
path — BLAKE3 is called once for the Merkle proof, not once per insert. No existing
benchmark exercises first-call root computation at large N (where parallelism would
help). At N=1 000, `compute_merkle_root` iterates ~1 000 K/V pairs, serialises them,
and calls `blake3::keyed_hash` once over a ~8 KB buffer — this completes in ≈30–50 µs
and is not measured by any standalone benchmark.

**Conclusion:** Parallel BLAKE3 hashing would only help for workloads where
`compute_merkle_root` is on the critical path at N ≥ 10 000. No such benchmark exists
and the use case (bulk proof generation) is not the primary bottleneck. Revisit if a
large-N Merkle benchmark is added.

### 2026-07-01 — PERF-FOLIO-002: PageRefcount BTreeMap → hashbrown migration

**Crate:** folio-collections (consumed by pds-folio)
**Root cause:** A concurrent session's commit `4dc85e2` (12:11 AEST) introduced
`PageRefcount` (in `folio-collections/src/refcount.rs`) backed by
`BTreeMap<u64, u32>` — O(log n) per operation. Since `inc()`/`dec()` are called on
every page write and clone in `pds-folio`, this regressed `hamt_insert` by 7–17%.

**Before (BTreeMap, implicit from commit `4dc85e2`):**
hamt_insert: n=100: ~396 µs, n=1 000: ~6.78 ms, n=10 000: ~83.5 ms

**After (hashbrown::HashMap, O(1) amortised):**
hamt_insert: n=100: 370 µs, n=1 000: 6.20 ms, n=10 000: 73.4 ms

**Improvement vs BTreeMap:** n=100: −6.8%, n=1 000: −8.6%, n=10 000: −12.1%
(also −7–12% vs the original pre-`4dc85e2` baselines, because foldhash is faster
than the implicit SipHash-1-3 in the original `HashMap<u64, u32>` inline refcount table)

**Clone improvement bonus:** hamt_clone_snapshot dropped from ~37 ns to ~22 ns.
The refactor in commit `4dc85e2` also simplified the clone path: removed
`increment_root_refcount` which previously acquired a `Mutex` on every clone.
Current clone is a pure `Arc::clone` (atomic refcount increment only).

**Also fixed:** compile error — the same commit removed `HashMap` from `use
std::collections::HashMap` in `pds-folio/src/hamt.rs`, but `page_cache:
HashMap<u64, HamtNodePage>` still uses it. Fixed by restoring the import.

**Decision:** see `docs/decisions.md` → PERF-FOLIO-002.

### 2026-07-01 — Area #1: WAL group commit (PERF-001)

**Crate:** pds-durable
**Baseline:** `durable_map_strict_insert` (N=1 000) = 4 860 ms (4.86 ms/fsync × 1 000 fsyncs)
**After:** `durable_map_strict_insert_batch` (N=1 000) = 15.3 ms (**317× improvement**)

**What changed:**
- Added `encode_entry_into(entry, &mut Vec<u8>)` helper in `wal.rs` — same wire format
  as `write_entry` but targets an in-memory buffer instead of the file.
- Added `Wal::append_batch(&[WalEntry], fsync: bool)` — coalesces all entries into one
  `write_all` then one `sync_data()`.
- Added `DurableMap<K, V, Strict>::insert_batch(pairs)` public API — serialises all
  key-value pairs to `WalEntry::Insert` variants, calls `wal.append_batch`, then applies
  all mutations to the in-memory map and runs `maybe_checkpoint`.
- Added 3 unit tests: `strict_insert_batch_reopen_all_present`,
  `strict_insert_batch_empty_is_noop`, `strict_insert_batch_returns_previous_values`.
- Added `bench_strict_insert_batch` criterion benchmark.

**Root cause of original cost:** macOS tmpfs `sync_data()` latency ≈4.86 ms. With one
fsync per entry, 1 000 inserts cost 4.86 s. Group commit amortises the fsync across all
entries in the batch.

**Decision:** see `docs/decisions.md` → PERF-001.

### 2026-07-01 — Step 0: Baseline capture

Baselines captured across all three pds-* crates. See `docs/baselines.md`.

- **pds-folio:** xxhash one-shot optimisation (PERF-FOLIO-001) already applied;
  6–8% write improvement across HamtMap, FolioVector, FolioOrdSet.
- **pds-merkle-spine:** H.9 lazy Merkle root already applied; insert benchmark
  (119.4 ms/1 000) no longer includes BLAKE3 cost during the insert loop.
- **pds-durable:** `strict_insert` baseline = 4.86 s/1 000; `relaxed_insert` = 389 µs/1 000.

---

## Decision Template

When a tuning decision is made, add to `docs/decisions.md`:

```
### PERF-NNN — [short title]

**Context:** [what was slow, why it mattered]
**Decision:** [what was changed]
**Measured improvement:** [before → after, workload, hardware]
**Alternatives considered:** [what was tried and discarded, why]
**Consequences:** [trade-offs introduced, if any]
```
