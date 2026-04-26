# Project Directives — pds

<!-- Persistent data structures with structural sharing for Rust. Forked from imbl with different design priorities. -->

@../rust-template/directives.md

---

## Build system — nightly devShell

*(Extends "Build system" in `rust-template/directives.md`)*

Two devShells are available:

| Shell | Enter via | Provides |
|-------|-----------|----------|
| `default` | `nix develop` (automatic via direnv) | Stable Rust, sccache |
| `nightly` | `nix develop .#nightly` | Nightly Rust, miri, cargo-fuzz |

`rust-toolchain.toml` records the expected versions for both shells.

---

## Rust conventions — core rules

**Overrides:** "Core rules" in `rust-template/directives.md` — pds is a library crate
with no binary and a targeted unsafe exception in `vector/mod.rs`.

- This is a **library crate** — no binary, no `main.rs`. All public API changes must
  consider downstream compatibility and semver implications.
- `#![deny(unsafe_code)]` is set at the crate root. Only `vector/mod.rs` has
  `#![allow(unsafe_code)]`. New unsafe code requires explicit justification,
  a `// SAFETY:` comment, and a miri run.
- Integration tests live in `tests/`; unit tests in `#[cfg(test)]` modules.
  Property tests use `proptest`. Fuzz targets live in `fuzz/`.
- Do not remove lints from `#![deny]` without a documented reason in `docs/decisions.md`.
- Write comments for a reader who knows the domain but is new to this codebase;
  highlight Rust-specific choices where they differ from the obvious approach.
- Use glossary terminology (`docs/glossary.md`) — do not introduce synonyms.

---

## Standard trait coverage

*(pds-specific — no equivalent in rust-template)*

Every public collection type must implement the standard trait set below.
When adding a new collection type or modifying an existing one, audit its
trait coverage against this table and fill any gaps.

| Trait | Map types | Set types | Vector | Bag types | Notes |
|-------|-----------|-----------|--------|-----------|-------|
| `Clone` | Y | Y | Y | Y | Manual impl to avoid spurious `P: Clone` bound |
| `Debug` | Y | Y | Y | Y | |
| `PartialEq` / `Eq` | Y | Y | Y | Y | |
| `PartialOrd` / `Ord` | Ordered only | Ordered only | Y | n/a | Only for types with deterministic iteration order |
| `Hash` | Y | Y | Y | Y | Order-independent (XOR-combine) for unordered types |
| `Default` | Y | Y | Y | Y | |
| `Send` / `Sync` | auto | auto | auto | auto | Derived from contents; verify with static assertions |
| `FromIterator` | Y | Y | Y | Y | |
| `IntoIterator` (owned) | Y | Y | Y | Y | Named `ConsumingIter` type |
| `IntoIterator` (`&`) | Y | Y | Y | Y | |
| `Extend` | Y | Y | Y | Y | |
| `Index` / `IndexMut` | Keyed types | n/a | Y | n/a | |
| `Serialize` / `Deserialize` | Y | Y | Y | Y | Behind `serde` feature gate |
| `From` conversions | Y | Y | Y | Y | From slice, Vec, array `[T; N]`, std equivalents |

**Rules:**
- When a trait impl is added, add a corresponding test in the module's
  `#[cfg(test)]` block.
- `Hash` for unordered collections must use an order-independent combiner
  (XOR of individually hashed entries) — never hash iteration order.
- `From<[T; N]>` uses const generics with a reasonable upper bound.
- Serde impls go in `src/ser.rs`, not in the collection module.
- This table is the obligation — if a cell says Y, the impl must exist.
  Missing impls are bugs.
- **Do not implement `Add`, `Mul`, or `Sum` for collection types.** Use named
  methods instead: `union()`, `intersection()`, `difference()`,
  `symmetric_difference()`. The exception is `Vector`, where `Add` is
  concatenation — analogous to `String + &str`.

---

## Set operation naming

*(pds-specific — no equivalent in rust-template)*

All set-like operations must use the canonical names below, consistently
across every collection type that supports them:

| Operation | Method name |
|-----------|-------------|
| All elements from both | `union()` |
| Elements in `self` not in `other` | `difference()` |
| Elements in both | `intersection()` |
| Elements in exactly one | `symmetric_difference()` |

**Rules:**
- Never use `sum`, `subtract`, `minus`, or other synonyms. The names above are
  the only permitted names.
- A collection type that logically supports a set operation must use these names —
  do not invent per-type variants.
- When adding a new collection type, audit which of the four operations are
  applicable and implement them with the canonical names.

---

### Change discipline

*(pds-specific — no equivalent in rust-template)*

Every change should be small, focused, well-tested, with a clear commit message.
Avoid coupling unrelated changes. Breaking changes are batched into v2.0.0
(Phase 5 of the implementation plan).

---

## Build outputs

**Overrides:** "Build outputs" in `rust-template/directives.md` — pds is a library
crate with no binary and has a `bench.sh` entry point; the test.sh quality gate runs
five steps rather than three.

### Standard entry points

| Command | Profile | Purpose |
|---------|---------|---------|
| `build.sh` | Debug | Compile all targets — fast feedback, debug assertions on |
| `build.sh --release` | Release | Compile all targets — optimised, for verifying release builds |
| `test.sh` | Debug | Quality gate — tests + clippy + doc |
| `bench.sh` | Release | Run criterion benchmarks (release mode, `[profile.bench] debug = true` for profiling) |
| `bench.sh vector` | Release | Run a single benchmark suite |

Tests always run in **debug** mode (fast compile, debug assertions and overflow checks
enabled). Benchmarks always run in **release** mode. Do not benchmark in debug mode.

### Quality gate (`test.sh`)

**Overrides:** "Quality gate (test.sh)" in `rust-template/directives.md` — pds runs
three cargo-test invocations to cover default features, all features, and the
small-chunks variant.

`test.sh` runs, in order:

1. `cargo test` — unit, integration, and doc tests (default features)
2. `cargo test --all-features` — tests with all features enabled
3. `cargo test --features small-chunks` — small-chunks variant
4. `cargo clippy --all-features -- -D warnings` — lint enforcement
5. `cargo doc --no-deps` — catches broken doc links and missing doc comments

### Local rustdocs

```bash
rm -rf rustdocs && cargo doc --no-deps --all-features --target-dir rustdocs
```

This places the generated docs in `rustdocs/doc/pds/`.

### Benchmarking (`bench.sh`)

`bench.sh` runs criterion benchmarks in release mode. pds has criterion benchmarks
in `benches/` for ordmap, hashmap, and vector. Use `bench.sh -- --save-baseline before`
and `bench.sh -- --baseline before` for explicit A/B comparisons.

---

## Testing — property-based testing

**Overrides:** "Property-based testing" in `rust-template/directives.md` — pds uses
proptest extensively; fuzz targets complement proptest for unsafe code paths.

pds uses `proptest` extensively. When adding or modifying data structure operations,
add proptest strategies that exercise the new code paths. Fuzz targets (`fuzz/`)
complement proptest for longer-running, coverage-guided exploration — particularly
important for unsafe code paths (Focus/FocusMut).

---

## Testing — benchmark coverage

**Overrides:** "Benchmark coverage" in `rust-template/directives.md` — pds has
specific missing benchmarks to add as part of Phase 0.3.

Same rules as rust-template, plus: new benchmarks should be added for any data
structure that gains or loses a benchmark during improvement work (e.g. hashset,
ordset benchmarks are currently missing and should be added in Phase 0.3).

---

## Testing — benchmark result persistence

**Overrides:** "Benchmark result persistence" in `rust-template/directives.md` — pds
writes summary tables to `docs/baselines.md`, not `bench_results.md`/`bench_results.json`.

- **Always pipe output to a file:**
  `cargo bench ... 2>&1 | tee /private/tmp/bench_<name>_$(date +%s).txt`
- **Use criterion baselines** for before/after comparison:
  `bench.sh -- --save-baseline before` then `bench.sh -- --baseline before`
- **Write summary tables** to `docs/baselines.md` immediately after benchmarks
  complete — before any other work
- **Never run benchmarks in the background** without file output capture

---

## Testing — proptest configuration

**Overrides:** "Proptest configuration" in `rust-template/directives.md` — critical
paths are pds-specific (node promotion/demotion, tree rebalancing).

- **Default:** 256 cases — sufficient for most operations
- **Critical paths** (node promotion/demotion, tree rebalancing, unsafe-adjacent
  code): set `PROPTEST_CASES=1000` or use `proptest::test_runner::Config { cases: 1000 }`
- **Flake investigation:** temporarily increase to 10,000+ cases to reproduce
  rare failures, then add a targeted regression test for the specific input

---

## Documentation (`docs/`)

**Overrides:** docs/ table in `rust-template/directives.md` — pds has `baselines.md`
and `architecture.md` instead of `spec.md`; `impl-plan.md` covers Phases 0–6.

| File | Purpose |
|------|---------|
| `decisions.md` | Decision log — what was decided and why |
| `glossary.md` | Project terminology (data structures, internals) |
| `references.md` | Papers, implementations, external resources |
| `baselines.md` | Build speed, test speed, and benchmark baselines for periodic comparison |
| `architecture.md` | Internal architecture of core data structure modules |
| `impl-plan.md` | Phased implementation plan (Phase 0–6) with dependency tracking |

Subdirectories are permitted for images (`docs/img/`) or large topic areas.

---

## Crate selection

**Overrides:** "Crate selection" in `rust-template/directives.md` — pds is a library
crate; MSRV is 1.85; the existing dependency set is established and well-tested.

Always use the latest stable version of crates unless there is a specific, documented
reason not to. When pinning to an older version, record the reason in `docs/decisions.md`.

pds's existing dependencies are established and well-tested. Before adding any new crate,
evaluate it against the project's goals and record the result in `docs/decisions.md`.
Criteria:

- **Correctness fit.** Does it actually solve the problem?
- **Ecosystem alignment.** Does it fit the existing dependency stack?
- **Compile-time cost.** Does the dependency tree materially affect build times?
- **Maintenance health.** Recent releases, responsive maintainers.
- **MSRV compatibility.** Does its minimum Rust version match the project's MSRV (1.85)?

---

## Implementation plan review

*(pds-specific — no equivalent in rust-template)*

The implementation plan (`docs/impl-plan.md`) is long and its items are
interdependent. It must be kept current as work progresses:

- **After completing any plan item**, review surrounding items for cascading
  effects. Update dependencies, sequencing, and rationale as needed.
- **When unexpected findings emerge** during research or implementation (e.g.
  an optimisation proves infeasible, a dependency turns out to be unnecessary,
  or a new prerequisite is discovered), update `docs/impl-plan.md` immediately —
  do not defer until the item is "done".
- **At the start of each new phase**, re-read the entire plan to check whether
  completed work has changed the assumptions of upcoming items.
- **Periodically review** the dependency map and parallel-tracks section to
  ensure they still reflect reality.
- **Move completed items** to a "Done" section or mark them `[x]`. Do not
  delete them — the rationale embedded in completed items is valuable context.
- **Record surprises** in `docs/decisions.md`. If an item's outcome differed
  significantly from what was planned, capture what changed and why.

The goal is that `docs/impl-plan.md` always reflects current understanding,
not the assumptions from the day it was written.
