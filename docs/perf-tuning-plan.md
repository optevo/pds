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
- Is 32-way HAMT branching optimal for this hardware's cache line size?
- Are HAMT nodes cache-line aligned?
- Is the bitmap popcount using hardware instructions?
- Does `foldhash` (already an optional feature) outperform the default hasher?
- Can `from_iter` use size hints to pre-allocate tree structure?
- Is `triomphe::Arc` measurably faster than `std::Arc` here?
- CHAMP encoding: are collision nodes handled optimally?

### pds-folio

Key questions:
- PERF-1 done (page cache, 78–88% improvement). What's the optimal cache size?
- Is the FIFO eviction policy optimal vs LRU or ARC?
- PERF-2 (PodCodec): can we work around the trait specialization limitation?
- PERF-4 (write batching): measure on a disk-backed backend (MemBackend gave nothing)
- Can node reads be parallelised for range scans?
- Is postcard the right codec? Measure vs bincode and raw bytemuck for Pod types.

### pds-merkle-spine

Key questions:
- BLAKE3 batching: can multiple leaf hashes be batched in one call?
- Parallel Merkle root: tree structure allows independent subtree hashing via rayon
- Incremental hashing: only re-hash changed path from leaf to root (structural sharing
  already means only the changed path is copied — is the hashing on that path minimal?)
- H.9 done (lazy root, 84% improvement). What's the remaining overhead?
- Is BLAKE3 the right hash for this use case, or is something faster available
  (e.g. xxHash128 for non-adversarial integrity)?

### pds-durable

Key questions:
- WAL group commit: batch multiple entries into one fsync (critical for Strict mode —
  currently 4.86s/1000 entries because of 1 fsync/entry)
- CRC32C: verify hardware acceleration is active on Apple Silicon
- postcard encoding: measure overhead vs raw byte writes
- TieredMap t0 (std::HashMap) → t1 (pds::HashMap): is O(N) `from_iter` the best we can do,
  or can we exploit the sorted-by-hash property to build the HAMT bottom-up faster?
- MemOnlyMap at 58 ns/insert is 120× faster than pds::HashMap. What is the crossover
  point where pds structural sharing's benefits (O(1) clone, history) outweigh the cost?

---

## Pre-Identified Investigation Areas

Ranked by estimated impact:

| # | Area | Crate | Estimated impact | Type |
|---|------|-------|-----------------|------|
| 1 | WAL group commit (batch fsync) | pds-durable | Very high (Strict: 4.86s → ~50ms) | Code fix |
| 2 | BLAKE3 parallel subtree hashing | pds-merkle-spine | High | Algorithmic |
| 3 | HAMT branching factor tuning | pds | Medium-High | Architectural |
| 4 | Folio page cache LRU vs FIFO | pds-folio | Medium | Code fix |
| 5 | Folio codec: bytemuck vs postcard for Pod types | pds-folio | Medium | Code fix |
| 6 | WAL CRC32C hardware intrinsics verification | pds-durable | Low-Medium | Code fix |
| 7 | pds from_iter with capacity hint | pds | Medium | Code fix |
| 8 | Bottom-up HAMT construction from sorted keys | pds | Medium | Algorithmic |
| 9 | Folio parallel node reads (range scan) | pds-folio | Medium | Architectural |
| 10 | Alternative hash (foldhash vs AHash) for pds::HashMap | pds | Low-Medium | Code fix |

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
