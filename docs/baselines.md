# Performance Baselines {#sec:baselines}

Baseline measurements for build speed, test speed, and runtime benchmarks.
Re-run periodically (especially after significant changes) to detect
regressions or improvements. Compare against these numbers.

**Machine:** MacBook Pro M5 Max (18-core CPU, 128 GB unified RAM)
**Rust:** 1.95.0 (stable, via Nix rust-overlay)
**Date:** 2026-04-24

---

## Contents

- [Build speed](#build-speed)
- [Test speed](#test-speed)
- [Benchmark summary](#benchmark-summary)
- [How to re-run](#how-to-re-run)

---

## Build speed {#sec:build-speed}

All times are wall-clock. "Cold" = after `cargo clean`. "Incremental" = after
touching `src/lib.rs` with everything else cached.

| Metric | Time | Notes |
|--------|------|-------|
| Cold `cargo check` | 1.2s | Lib only, 14 crates |
| Cold `cargo test --no-run` | 5.4s | Includes 83 dev-dep crates |
| Cold `cargo bench --no-run` | 24s | Release + thin LTO |
| Incremental `cargo check` | 0.17s | After touching lib.rs |
| Incremental `cargo test --no-run` | 0.67s | After touching lib.rs |

### Critical-path crates (test build)

| Crate | Time | Notes |
|-------|------|-------|
| pds (self) | 2.75s | Bottleneck — single crate, cannot parallelise |
| serde_derive | 0.79s | Proc macro, dev-dep |
| proc-macro2 | 0.51s | Transitive |
| proptest-derive | 0.42s | Dev-dep |

### Profile settings

| Profile | LTO | Codegen units | Debug info | Notes |
|---------|-----|---------------|------------|-------|
| dev | off | default | full (split) | Fast compile, debug assertions on |
| release | thin | 16 | off | Optimised binary |
| bench | thin (inherited) | 16 (inherited) | full | Release + debug symbols for profiling |

---

## Test speed {#sec:test-speed}

Times measured with crate already compiled (execution only, not compilation).

| Configuration | Unit tests | Proptests | Wall clock | Notes |
|---------------|-----------|-----------|------------|-------|
| Default features | 122 + 120 | ~7s | 7.1s | Baseline |
| All features | 132 + 125 | ~13s | 13.2s | Serde/rayon tests added |
| small-chunks | 122 + 120 | ~13s | 13.2s | Smaller nodes trigger more edge cases |
| **Full test.sh** | — | — | **37s** | All 3 configs + clippy + doc |

### Slow tests

The `all-features` and `small-chunks` configurations take ~2x longer than
default because proptest strategies generate more edge cases when node
sizes are smaller or additional features (rayon parallelism, serde
round-trips) are exercised.

---

## Benchmark summary {#sec:benchmark-summary}

Selected results from criterion benchmarks (`cargo bench -- --quick`).
Full results are stored in `target/criterion/` by criterion automatically.

### HashMap — imbl vs std (2026-04-25)

**i64 keys:**

| Operation | 100 | 1K | 10K | 100K | vs std 10K | vs std 100K |
|-----------|-----|-----|------|------|-----------|------------|
| Lookup | 679ns | 7.2µs | 84µs | 1.24ms | 1.4x | 1.6x |
| Insert (mut) | 2.3µs | 31µs | 219µs | 3.83ms | 0.7x | 1.6x |
| Remove (mut) | 2.4µs | 25.6µs | 254µs | — | 1.9x | — |
| Iter | — | 2.1µs | 32µs | 620µs | 3.6x | **5.3x** |
| From iter | — | 29.5µs | 216µs | 4.55ms | 2.8x | **5.0x** |

**Arc<String> keys:**

| Operation | 100 | 1K | 10K | 100K | vs std 10K | vs std 100K |
|-----------|-----|-----|------|------|-----------|------------|
| Lookup | 794ns | 8.7µs | 151µs | 3.02ms | 1.6x | 1.6x |
| Insert (mut) | 2.7µs | 40µs | 346µs | 6.36ms | 0.7x | 1.2x |
| Remove (mut) | 2.6µs | 29µs | 363µs | — | 1.9x | — |
| Iter | — | 2.0µs | 32µs | 595µs | 3.6x | **5.1x** |
| From iter | — | 39.2µs | 334µs | 6.29ms | 2.1x | 2.6x |

**Performance gap analysis:**

| Priority | Gap | Cause (hypothesis) | Plan item |
|----------|-----|-------------------|-----------|
| **P1** | iter 4-5x | HAMT 3-tier enum dispatch + pointer chasing | — |
| **P1** | from_iter 3-5x | Per-node Arc::new + insert loop | 6.8 (arena) |
| **P2** | remove_mut 2-3x at small sizes | CoW overhead on small nodes | — |
| **P3** | lookup 1.5-2x at large sizes | SIMD probe + hash bits exhaustion | 4.7 (u64 hash) |
| **Win** | insert_mut 0.7x at ≤10K | imbl outperforms std at small-mid sizes | — |

### OrdMap (i64 keys)

| Operation | 100 | 1K | 10K | 100K |
|-----------|-----|-----|------|------|
| Lookup | 0.74µs | 13µs | 202µs | 3.4ms |
| Insert (mut) | — | — | — | — |

### Vector

Benchmark covers push_front, push_back, pop, split/append, sort,
focus/focus_mut access. Results stored in `target/criterion/vector/`.

---

## Optimisation notes {#sec:optimisation-notes}

### target-cpu=native (bench.sh only)

`-C target-cpu=native` is applied in `bench.sh` but NOT in default RUSTFLAGS.
This lets LLVM use M5 Max-specific instruction scheduling without affecting
check/test compile speed.

| Benchmark | Default | Native | Improvement |
|-----------|---------|--------|-------------|
| hashmap_i64/lookup_1000 | 6.8 us | 6.5 us | ~5% |
| hashmap_i64/lookup_10000 | 92 us | 80 us | ~13% |
| hashmap_str/lookup_1000 | 9.4 us | 8.5 us | ~10% |
| hashmap_i64/insert_1000 | 243 us | 230 us | ~5% |
| vector push_back | No significant difference | | |

Gains are concentrated in HashMap/OrdMap (SIMD lookup paths). Vector
operations show no measurable difference.

### lld vs ld64

lld is 42% faster for cold test builds (5.6s vs 9.7s). Incremental builds
are similar (~0.7s both). lld is configured via RUSTFLAGS in flake.nix.

### -Z threads=14 (nightly only)

Tested on nightly — actually slower for this single-crate project (1.83s
vs 1.2s) due to threading overhead. Not applied.

---

## How to re-run {#sec:how-to-rerun}

```bash
# Build speed (cold)
cargo clean && time cargo check
cargo clean && time cargo test --no-run
cargo clean && time cargo bench --no-run

# Build speed (incremental)
touch src/lib.rs && time cargo check

# Test speed
time cargo test
time cargo test --all-features
time cargo test --features small-chunks
time bash test.sh

# Benchmarks (full, not quick)
bash bench.sh                         # all suites
bash bench.sh hashmap                 # single suite
bash bench.sh -- --save-baseline v7   # named baseline for comparison

# Compare against a saved baseline
bash bench.sh -- --baseline v7
```

When updating this document, note the date, Rust version, and any
significant changes that may have affected the numbers.

---

## Memory profiling (dhat) {#sec:memory-profiling}

**Date:** 2026-04-25
**Run:** `cargo bench --bench memory`

Allocation counts and bytes per operation. Measures heap pressure, not
peak RSS. Lower is better.

### HashMap<i64, i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 246 | 131 KB |
| from_iter(10K) | 1,139 | 532 KB |
| from_iter(100K) | 29,652 | 13.9 MB |
| single insert (10K base) | 3 | 2.5 KB |
| clone + modify (10K base) | 3 | 2.5 KB |
| clone (10K) | 0 | 0 |

~0.3 allocs/element at scale — inherent to HAMT structure (one
Arc::new per node). Clone is O(1) / zero allocs. Single insert
touches O(log n) nodes.

### HashSet<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 293 | 110 KB |
| from_iter(10K) | 1,150 | 382 KB |
| from_iter(100K) | 29,692 | 9.8 MB |

Smaller per-entry footprint than HashMap (no value stored).

### OrdMap<i64, i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 68 | 37 KB |
| from_iter(10K) | 666 | 358 KB |
| from_iter(100K) | 6,641 | 3.6 MB |
| single insert (10K base) | 5 | 2.8 KB |
| clone + modify (10K base) | 4 | 2.2 KB |

B+ tree with chunk_size=32 → far fewer node allocations than HAMT.
~0.07 allocs/element at scale.

### OrdSet<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 68 | 20 KB |
| from_iter(10K) | 666 | 198 KB |
| from_iter(100K) | 6,641 | 2.0 MB |

### Vector<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| push_back(1K) | 21 | 12 KB |
| push_back(10K) | 169 | 95 KB |
| push_back(100K) | 1,619 | 906 KB |
| from_iter (same as push_back) | — | — |
| clone + push_back (10K base) | 1 | 536 B |

RRB tree has excellent allocation density: ~0.016 allocs/element.
Clone + single push_back = 1 allocation (new leaf chunk).

### Bag<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 273 | 144 KB |
| from_iter(10K) | 1,146 | 539 KB |
| from_iter(100K) | 29,653 | 13.9 MB |

Backed by HashMap — similar allocation profile.

### BiMap<i64, i64> / SymMap<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| BiMap from_iter(10K) | 2,297 | 1.1 MB |
| SymMap from_iter(10K) | 2,284 | 1.1 MB |

~2× HashMap allocations as expected (two internal maps).
