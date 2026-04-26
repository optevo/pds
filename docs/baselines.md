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
- [OrdMap vs HashMap — head-to-head](#ordmap-vs-hashmap)
- [Memory profiling (dhat)](#memory-profiling-dhat)
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

## OrdMap vs HashMap — head-to-head {#sec:ordmap-vs-hashmap}

**Date:** 2026-04-27
**Machine:** MacBook Pro M5 Max (18-core CPU, 128 GB unified RAM)
**Rust:** 1.95.0 (stable)
**Bench:** `cargo bench --bench compare --features rayon -- compare/<op>`
**Keys:** `i64`, randomised, seeded RNG (reproducible)
**Method:** Each benchmark runs one operation per iteration across the full
key set; all values are the criterion median.

### lookup (n random hits)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
|   100 |   549 ns  |   630 ns  | HashMap ×1.15 |
| 1,000 |  5.76 µs  | 10.60 µs  | HashMap ×1.84 |
| 10,000|  74.6 µs  |  157 µs   | HashMap ×2.11 |
| 100,000| 1.17 ms  |  2.27 ms  | HashMap ×1.94 |

HAMT gives O(1) amortised lookups (fixed trie depth for the key range);
OrdMap is O(log n) with a higher constant due to binary search per B+ node.

### insert_mut (build from empty, n sequential inserts, sole owner)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
|   100 |  2.20 µs  |  1.26 µs  | OrdMap ×1.74 |
| 1,000 | 30.4 µs   | 16.2 µs   | OrdMap ×1.88 |
| 10,000| 236 µs    | 230 µs    | ≈ equal       |
| 100,000| 3.97 ms  |  2.01 ms  | OrdMap ×1.98 |

OrdMap wins when the map is solely owned: copy-on-write detects the sole
reference and mutates in-place without allocating. HashMap's HAMT must
always rewrite trie nodes even under sole ownership.

### remove_mut (remove all keys in shuffled order, sole owner)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
|   100 |  2.87 µs  |  1.31 µs  | OrdMap ×2.19 |
| 1,000 | 32.7 µs   | 18.9 µs   | OrdMap ×1.73 |
| 10,000| 328 µs    |  380 µs   | HashMap ×1.16 |
| 100,000| 6.15 ms  |  7.38 ms  | HashMap ×1.20 |

Crossover near 10K: OrdMap rebalancing cost overtakes HAMT's per-remove
overhead as the tree grows. At 100K OrdMap is ~20% slower.

### iter (full iteration, all n entries)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
|   100 |   199 ns  |   145 ns  | OrdMap ×1.37 |
| 1,000 |  1.89 µs  |  1.35 µs  | OrdMap ×1.40 |
| 10,000|  33.3 µs  |  14.3 µs  | OrdMap ×2.33 |
| 100,000|  553 µs  |   155 µs  | OrdMap ×3.57 |

B+ tree leaves are contiguous arrays of up to 32 key-value pairs — cache
line friendly. HAMT nodes are sparse bitmapped arrays with irregular
sizes; traversal has worse spatial locality.

### from_iter (bulk construction from Vec of pairs)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
|   100 |  2.21 µs  |  1.18 µs  | OrdMap ×1.87 |
| 1,000 | 30.4 µs   | 17.1 µs   | OrdMap ×1.78 |
| 10,000| 246 µs    |  179 µs   | OrdMap ×1.37 |
| 100,000| 4.05 ms  |  2.05 ms  | OrdMap ×1.98 |

OrdMap `from_iter` benefits from in-place mutation of the sole owner
during bulk insert; HashMap always pays allocation cost per HAMT node.

### par_union (two maps with 50% overlap, parallel)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
| 10,000|  1.08 ms  |  267 µs   | OrdMap ×4.0 |
| 100,000| 13.0 ms  |  840 µs   | OrdMap ×15.5 |

OrdMap uses the O(m log(n/m)) parallel join algorithm (split + recurse +
concat). HashMap uses filter+reduce (collect all keys not in self, then
insert sequentially) — O(n) with a large sequential bottleneck. The gap
widens with size because the join algorithm's sequential phase is O(log n).

### par_intersection (two maps with 50% overlap, parallel)

| Size  | HashMap   | OrdMap    | Faster |
|------:|----------:|----------:|--------|
| 10,000|  929 µs   |  437 µs   | OrdMap ×2.1 |
| 100,000|  9.55 ms |  1.49 ms  | OrdMap ×6.4 |

Same join-vs-filter algorithm difference as par_union.

### Summary

| Operation | Winner | Margin |
|-----------|--------|--------|
| lookup | HashMap | 1.2–2.1× |
| insert_mut | OrdMap | 1.7–2.0× (except ~equal at 10K) |
| remove_mut | OrdMap at ≤1K; HashMap at ≥10K | up to ×2.2 / ×1.2 |
| iter | OrdMap | 1.4–3.6× |
| from_iter | OrdMap | 1.4–2.0× |
| par_union | OrdMap | 4–16× |
| par_intersection | OrdMap | 2–6× |

**When to use HashMap:** random key lookups dominate (point queries on
large maps). The HAMT's O(1) amortised lookup is ~2× faster than B+ tree
binary search at 10K–100K entries.

**When to use OrdMap:** writes, iteration, bulk construction, parallel
set operations, or when sorted order / range queries are needed. In most
workloads OrdMap is equal or faster, with a large advantage in parallel
set operations.

---

## Memory profiling (dhat) {#sec:memory-profiling}

**Date:** 2026-04-26
**Run:** `cargo bench --bench memory` (bench profile: release + debuginfo)

Allocation counts and bytes per operation. Measures heap pressure, not
peak RSS. Lower is better. Re-run with `cargo bench --bench memory` to update.

### HashMap<i64, i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 226 | 120 KB |
| from_iter(10K) | 1,134 | 528 KB |
| from_iter(100K) | 29,633 | 13.9 MB |
| single insert (10K base) | 3 | 2.5 KB |
| clone + modify (10K base) | 3 | 2.5 KB |
| clone (10K) | 0 | 0 |

~0.3 allocs/element at scale — inherent to HAMT trie structure (one
Arc per node, plus promotions through SmallSimd → LargeSimd → Hamt tiers).
Clone is O(1) / zero allocs. Single insert touches O(log n) nodes.

### HashSet<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 248 | 92 KB |
| from_iter(10K) | 1,147 | 381 KB |
| from_iter(100K) | 29,709 | 9.8 MB |

Smaller per-entry footprint than HashMap (no value stored).

### OrdMap<i64, i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 68 | 36 KB |
| from_iter(10K) | 666 | 358 KB |
| from_iter(100K) | 6,641 | 3.6 MB |
| single insert (10K base) | 5 | 2.8 KB |
| clone + modify (10K base) | 4 | 2.2 KB |

B+ tree with NODE_SIZE=16 — up to 16 key-value pairs per leaf allocation.
~0.07 allocs/element at scale (approximately n/16 leaves + branch nodes).
**4.5× fewer allocations and 3.9× fewer bytes than HashMap at 100K entries.**

### OrdSet<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| from_iter(1K) | 68 | 20 KB |
| from_iter(10K) | 666 | 198 KB |
| from_iter(100K) | 6,641 | 2.0 MB |

**4.5× fewer allocations than HashSet at 100K entries.**

### HashMap vs OrdMap — summary

| Entries | HashMap allocs | OrdMap allocs | Ratio |
|--------:|:--------------:|:-------------:|:-----:|
| 1,000   | 226            | 68            | 3.3×  |
| 10,000  | 1,134          | 666           | 1.7×  |
| 100,000 | 29,633         | 6,641         | 4.5×  |

The ratio grows with scale because HAMT trie depth increases with n
(hash bit exhaustion forces more levels), while B+ tree height grows as
log₁₆(n) with 16 entries per leaf.

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
| from_iter(1K) | 266 | 141 KB |
| from_iter(10K) | 1,142 | 535 KB |
| from_iter(100K) | 29,639 | 13.9 MB |

Backed by HashMap — same allocation profile.

### BiMap<i64, i64> / SymMap<i64>

| Operation | Allocs | Bytes |
|-----------|--------|-------|
| BiMap from_iter(10K) | 2,300 | 1.1 MB |
| SymMap from_iter(10K) | 2,283 | 1.1 MB |

~2× HashMap allocations — each type maintains two internal maps.
