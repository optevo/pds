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
| imbl (self) | 2.75s | Bottleneck — single crate, cannot parallelise |
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
| All features | 132 + 125 | ~13s | 13.2s | Serde/bincode/rayon tests added |
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

### HashMap (i64 keys)

| Operation | 100 | 1K | 10K | 100K |
|-----------|-----|-----|------|------|
| Lookup | 0.65 us | 7.9 us | 95 us | 796 us |
| Insert (mut) | 20 us | 270 us | 3.7 ms | 25 ms |
| Remove (mut) | 15 us | 273 us | 4.1 ms | 30 ms |
| Iter | — | 2.4 us | 38 us | 678 us |

### OrdMap (i64 keys)

| Operation | 100 | 1K | 10K | 100K |
|-----------|-----|-----|------|------|
| Lookup | 0.74 us | 13 us | 202 us | 3.4 ms |
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
