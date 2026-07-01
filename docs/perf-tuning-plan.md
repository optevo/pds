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
