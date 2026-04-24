# Project Directives — pds

Persistent data structures with structural sharing for Rust. Forked from
[imbl](https://github.com/jneem/imbl) with different design priorities.

## Contents

- [Build system](#build-system)
- [Rust conventions](#rust-conventions)
- [Testing](#testing)
- [Build outputs](#build-outputs)
- [Code documentation](#code-documentation)
- [Crate selection](#crate-selection)
- [Documentation](#documentation-docs)
- [Living documents](#living-documents)
- [Implementation plan review](#implementation-plan-review)
- [Dependency management](#dependency-management)
- [Project maintenance](#project-maintenance)

---

## Build system

This project uses **Nix** for its build environment. The `flake.nix` devShell is
the single source of truth for all build-time tooling (Rust toolchain, `sccache`,
system libraries, etc.). Entry: `direnv` with `use flake` in `.envrc`.

Two devShells are available:

| Shell | Enter via | Provides |
|-------|-----------|----------|
| `default` | `nix develop` (automatic via direnv) | Stable Rust, sccache |
| `nightly` | `nix develop .#nightly` | Nightly Rust, miri, cargo-fuzz |

### Non-negotiable rules

- **Never install tools outside Nix.** Do not use `cargo install`, `brew install`,
  `rustup`, `curl | sh`, or any other ad-hoc installation method. If a tool is
  needed, add it to the `packages` list in `flake.nix`.
- **Rust toolchain is Nix-managed.** The toolchain comes from `rust-overlay` in
  `flake.nix`, not from `rustup`. To change the Rust version or add components,
  edit `flake.nix`.
- **Never modify `PATH` or environment variables** outside of `flake.nix`'s
  `shellHook` or `.envrc`. The Nix devShell and direnv manage the environment.
- **`rust-toolchain.toml` is for documentation only.** The local dev environment
  uses the Nix-provided toolchain; `rust-toolchain.toml` records the expected
  version for environments without Nix (e.g. CI).
- **Crate dependencies via `Cargo.toml` are fine** — Cargo is a language-level
  package manager, not a system-level one. The restriction is on system tools
  and binaries, not Rust library crates.

---

## Rust conventions

### Core rules

- This is a **library crate** — no binary, no `main.rs`. All public API changes
  must consider downstream compatibility and semver implications.
- `#![deny(unsafe_code)]` is set at the crate root. Only `vector/mod.rs` has
  `#![allow(unsafe_code)]`. New unsafe code requires explicit justification,
  a `// SAFETY:` comment, and should be preceded by a miri run.
- Integration tests live in `tests/`; unit tests in `#[cfg(test)]` modules.
  Property tests use `proptest`. Fuzz targets live in `fuzz/`.
- Do not remove lints from `#![deny]` without a documented reason in
  `docs/decisions.md`.
- Write comments for a reader who knows the domain but is new to this codebase;
  highlight Rust-specific choices where they differ from the obvious approach.
- Use glossary terminology (`docs/glossary.md`) — do not introduce synonyms.

### Change discipline

Every change should be small, focused, well-tested, with a clear commit
message. Avoid coupling unrelated changes. Breaking changes are batched
into v2.0.0 (Phase 5 of the implementation plan).

### Compiler and clippy warnings

All compiler warnings and clippy lints must be addressed — never ignored or left
to accumulate. `test.sh` enforces this via `cargo clippy -- -D warnings`.

- **Use the standard clippy lint set.** The default clippy lints plus `-D warnings`
  are the baseline for all projects. Do not weaken, remove, or globally allow
  standard lints without a project-specific justification.
- **Fix the warning** whenever the fix is straightforward. Most warnings indicate
  genuinely improvable code.
- **Suppress with explanation** when the warning is a false positive or the
  flagged pattern is intentional. Use `#[allow(clippy::lint_name)]` (or
  `#[allow(unused_...)]` etc.) with an inline comment explaining why the
  suppression is acceptable. Never use a bare `#[allow(...)]` without a comment.
- **Add to the implementation plan** when fixing the warning requires non-trivial
  refactoring. Add the item to `docs/impl-plan.md` under Future with a reference
  to the specific warning, and suppress the warning in the interim with a comment
  noting the plan item.
- **Project-level lint deviations** — if a project needs to deviate from the
  standard clippy lint set (e.g. `#![allow(clippy::some_lint)]` at the crate
  root), the suppression must include a comment explaining why this project
  specifically needs the deviation. Lint deviations are project-specific decisions
  and do not propagate to other projects.

### AI-specific pitfalls

These are failure modes that survive `cargo check` and `clippy` and must be
caught by review. They are listed because they recur in AI-assisted development.

- **Unsigned integer underflow.** `len() - 1` panics on an empty collection
  because `usize` is unsigned. Use `.checked_sub(1)`, `.saturating_sub(1)`, or
  guard with an emptiness check first.
- **`expect()` semantics.** `expect()` is only for invariant violations —
  conditions that are logically impossible in correct code. If failure is a
  realistic production path, return a `Result` instead.
- **No unsolicited abstractions.** Do not introduce trait hierarchies,
  associated types, extra indirection layers, or generics not explicitly
  requested. Premature abstraction is harder to remove than to add.
- **No new `unsafe`.** NEVER introduce an `unsafe` block without explicit
  instruction. If lifetimes are difficult, find the safe abstraction; do not
  use `unsafe` to paper over borrow checker errors. Existing unsafe code
  should be removed unless there is a measured performance reason to keep
  it. When evaluating an unsafe block for removal, always write the safe
  alternative and benchmark both versions — if the safe version shows no
  meaningful regression, replace the unsafe. Do not assume unsafe is
  faster without measurement. When touching code near an unsafe block,
  check whether the safe alternative is viable — if it is, replace the
  unsafe and verify with tests. Unsafe blocks that must remain require a `// SAFETY:` comment
  documenting the invariant and a `debug_assert!` checking the
  precondition where feasible (e.g. pointer non-null, index in-bounds).
  Debug assertions compile to nothing in release builds, so they do not
  affect benchmarks or production performance. Every unsafe code path
  must have a corresponding test that exercises it under miri — these
  tests should NOT be marked `#[cfg_attr(miri, ignore)]` and should
  target boundary conditions likely to trigger UB (e.g. aliasing
  violations, dangling pointers, out-of-bounds access).
  See also Phase 3.2 (unsafe audit).
- **No undeclared dependencies.** NEVER add a crate to `Cargo.toml` that is
  not already present, without explicit approval. Consult `Cargo.toml` for
  the authoritative list of available crates and their pinned versions; do
  not assume API knowledge from training data.
- **Unnecessary clones.** AI-generated Rust defaults to `.clone()` to escape
  the borrow checker. This compiles and passes clippy but introduces hidden
  allocation and copying overhead — especially costly in hot paths and with
  heap-allocated types (`String`, `Vec`, `HashMap`). Prefer borrowing (`&T`)
  over owned values in function signatures, use `&str` instead of `String`
  for read-only access, pass slices (`&[T]`) instead of `Vec<T>`, and use
  `Cow<'_, T>` when a function sometimes needs ownership and sometimes does
  not. After completing any work item, review the changed code for clones
  that can be replaced with references. This is a regular review obligation,
  not a one-off check.
- **Collecting iterators unnecessarily.** AI materialises intermediate
  collections compulsively — `.collect::<Vec<_>>()` followed by `.iter()`
  when the iterators could just be chained. Each unnecessary collect is a
  heap allocation plus a copy that does nothing. Prefer chaining iterator
  adaptors (`.map()`, `.filter()`, `.flat_map()`) and collecting only at
  the final consumer. Also watch for `.collect()` into a `Vec` just to call
  `.len()` — use `.count()` on the iterator instead.
- **Allocating in loops.** Creating `String`, `Vec`, `HashMap`, or other
  heap types inside a loop body when they could be allocated once outside
  the loop and reused via `.clear()`. AI treats each iteration as
  independent and does not consider reuse across iterations. For string
  building, allocate a `String` before the loop and use `write!()` or
  `.push_str()` with `.clear()` between iterations.
- **Owned types in structs to avoid lifetimes.** AI avoids lifetime
  parameters on structs by making every field owned (`String` instead of
  `&str`, `Vec<T>` instead of `&[T]`, `PathBuf` instead of `&Path`). For
  long-lived or static data (configuration, CLI args, constants),
  `&'static str` or `&'a str` eliminates allocation entirely. Use owned
  types when the struct genuinely needs to own the data; use references
  when the data outlives the struct.
- **`format!()` in hot paths.** Every `format!()` allocates a new `String`.
  In logging, error construction, or display paths that run frequently,
  prefer `write!()` to a reusable buffer or `std::fmt::Display`
  implementations. Reserve `format!()` for one-off string construction
  where the allocation cost is negligible.
- **`Mutex` where `RwLock` suffices.** AI defaults to `Mutex` for any
  shared state. When reads vastly outnumber writes (the common case),
  `RwLock` allows concurrent readers and only blocks for writers. Use
  `Mutex` only when every access is a write, or when the critical section
  is so short that the overhead of `RwLock`'s reader tracking exceeds the
  benefit.
- **Missing `with_capacity`.** When the output size is known or bounded,
  `Vec::with_capacity(n)` avoids reallocations during growth. AI creates
  empty `Vec::new()` then pushes in a loop of known length. The same
  applies to `String::with_capacity()` and `HashMap::with_capacity()`.
- **`unwrap()` in non-test code.** The `expect()` rule above covers
  intent, but AI also scatters bare `.unwrap()` on `Option` and `Result`
  in production paths. These are silent panics with no diagnostic message.
  In production code, use `?` to propagate, or match/`if let` to handle.
  `.unwrap()` is acceptable only in tests and in code paths where the
  invariant is documented.

All of the above are regular review obligations — after completing any
work item, review the changed code for these patterns. Fixing them may
require reworking function signatures or data flow, so ensure the test
suite covers the affected paths before and after the change.

---

## Testing

### Plan first

Before writing tests for an implementation plan item, review the item's
rationale and dependencies in `docs/impl-plan.md`. State what needs to be
tested, why (what failure mode it guards against), and how (unit, integration,
property, fuzz, benchmark).

### Completion gate

Every work item in `docs/impl-plan.md` should define its acceptance criteria
as tests where feasible. An item moves from planned to done only when
`test.sh` passes with those tests in place. Items that cannot be meaningfully tested
(scaffolding, documentation, tooling setup) must state why in lieu of test
criteria.

### Coverage threshold

Write tests for error paths with the same attention as success paths — AI
consistently skips them.

Prefer `Result`-returning test functions over `#[should_panic]`; a `Result`
failure names what went wrong, a panic only confirms something blew up.

Do not mock internal boundaries. Mock at the system edge (external I/O, network)
only — internal mocking masks integration failures that only appear at runtime.

### Periodic code coverage analysis

Run `cargo llvm-cov --all-features --summary-only` periodically (at minimum
before and after any significant work item) to identify untested code paths.
This requires `cargo-llvm-cov` in the flake's `packages` and the
`llvm-tools-preview` component on the Rust toolchain.

Use coverage data to:
- Identify uncovered functions and dead code paths
- Prioritise test additions for critical code with low coverage
- Verify that new code is exercised by existing or new tests

Coverage is a diagnostic tool, not a target — 100% line coverage does not mean
correct code. Focus coverage efforts on code paths that matter: error handling,
edge cases, upgrade/downgrade paths, and rarely-exercised branches.

### Property-based testing

pds already uses `proptest` extensively. When adding or modifying data structure
operations, add proptest strategies that exercise the new code paths. Fuzz targets
(`fuzz/`) complement proptest for longer-running, coverage-guided exploration —
particularly important for unsafe code paths (Focus/FocusMut).

### Benchmarking

pds has criterion benchmarks in `benches/` for ordmap, hashmap, and vector.
When making performance-sensitive changes, run the relevant benchmarks before
and after. Wrap inputs in `std::hint::black_box` — without it the optimiser
may eliminate the work and the benchmark measures nothing.

New benchmarks should be added for any data structure that gains or loses a
benchmark during the improvement work (e.g. hashset, ordset benchmarks are
currently missing and should be added in Phase 0.3).

### Benchmark isolation

Benchmarks require exclusive CPU access to produce reliable results.
**Non-negotiable rules:**

- **Never run benchmarks in parallel.** Run one benchmark suite at a time —
  criterion measures are sensitive to CPU contention.
- **No CPU-intensive work during benchmarks.** Do not compile, run tests, or
  perform other heavy work while benchmarks are running. Wait for the benchmark
  to finish before starting other work.
- **No background benchmark runs.** Always run benchmarks in the foreground and
  wait for completion. Background execution risks interference from other tasks.

### Benchmark coverage

Benchmarks are a review obligation, just like tests. When adding new
functionality that has performance implications:

- **Add benchmarks for new operations** — any new public method or data
  structure operation that is expected to be called in hot paths needs a
  criterion benchmark.
- **Add benchmarks for parallel/concurrent methods** — methods that use
  parallelism (e.g. via `rayon`) need benchmarks that measure scaling
  behaviour across different collection sizes.
- **Review benchmark coverage** after completing any work item, just as you
  review test coverage. If a change affects a hot path that lacks a benchmark,
  add one.

### Profile before optimising

Never optimise based on assumptions about where time is spent. Before any
performance work:

1. **Establish a baseline** — run the relevant benchmark and record the number
2. **Profile the hot path** — use `samply record cargo bench --bench <name> -- <filter>`
   to capture a CPU profile, or `cargo flamegraph` for a flamegraph SVG
3. **Identify the bottleneck** — the profile shows where time actually goes.
   Optimise that, not what you think is slow
4. **Measure after** — re-run the same benchmark. If the number didn't move,
   the optimisation missed the real bottleneck

Both `samply` and `cargo-flamegraph` are in the Nix devShell. `samply` opens
an interactive profile viewer; `cargo flamegraph` produces an SVG file.

### Allocation profiling

For data structure libraries, heap allocation count and pattern often matter
more than CPU time. Use `dhat` (via `#[global_allocator]` in a benchmark
binary) to count allocations per operation.

When investigating `from_iter`, `collect`, or bulk construction performance,
allocation profiling reveals whether the bottleneck is allocation overhead
(many small `Arc::new` calls) vs computation (hashing, tree traversal).

### Benchmark result persistence

Benchmark results must be saved to durable files immediately upon completion.
Never hold benchmark data only in conversation context — it is lost during
context compaction and re-running benchmarks wastes 5-15 minutes per suite.

- **Always pipe output to a file:**
  `cargo bench ... 2>&1 | tee /private/tmp/bench_<name>_$(date +%s).txt`
- **Use criterion baselines** for before/after comparison:
  `bench.sh -- --save-baseline before` then `bench.sh -- --baseline before`
- **Write summary tables** to `docs/baselines.md` immediately after benchmarks
  complete — before any other work
- **Never run benchmarks in the background** without file output capture

### Benchmark regression detection

Use criterion's baseline comparison to detect regressions:

```bash
# Save a baseline before changes
bench.sh -- --save-baseline before

# Make changes, then compare
bench.sh -- --baseline before
```

When completing a work item that touches performance-sensitive code, save a
baseline before starting and compare after. Record significant changes
(>5% in either direction) in `docs/decisions.md`.

### Proptest configuration

Property tests are the primary correctness tool for data structure operations.
Configure case counts based on test criticality:

- **Default:** 256 cases (proptest default) — sufficient for most operations
- **Critical paths** (node promotion/demotion, tree rebalancing, unsafe-adjacent
  code): set `PROPTEST_CASES=1000` or use `proptest::test_runner::Config` with
  `cases: 1000`
- **Flake investigation:** temporarily increase to 10,000+ cases to reproduce
  rare failures, then add a targeted regression test for the specific input

---

## Build outputs

### Standard entry points

| Command | Profile | Purpose |
|---------|---------|---------|
| `build.sh` | Debug | Compile all targets — fast feedback, debug assertions on |
| `build.sh --release` | Release | Compile all targets — optimised, for verifying release builds |
| `test.sh` | Debug | Quality gate — tests + clippy + doc |
| `bench.sh` | Release | Run criterion benchmarks (release mode, `[profile.bench] debug = true` for profiling) |
| `bench.sh vector` | Release | Run a single benchmark suite |

Tests always run in **debug** mode (fast compile, debug assertions and overflow
checks enabled). Benchmarks always run in **release** mode (optimised, realistic
performance numbers). Do not benchmark in debug mode — the numbers are meaningless.

### Quality gate (`test.sh`)

`test.sh` is the single entry point for all quality checks. It runs, in order:

1. `cargo test` — unit, integration, and doc tests (default features)
2. `cargo test --all-features` — tests with all features enabled
3. `cargo test --features small-chunks` — small-chunks variant
4. `cargo clippy --all-features -- -D warnings` — lint enforcement
5. `cargo doc --no-deps` — catches broken doc links and missing doc comments

If any step fails, the script exits non-zero. A green `test.sh` is the proof
required by the completion gate.

### Local rustdocs

Whenever documentation is updated (doc comments, module-level docs, or
README changes), rebuild the local rustdocs:

```bash
cargo doc --no-deps --all-features --target-dir rustdocs
```

This places the generated docs in `rustdocs/doc/pds/`. The `rustdocs/`
directory must be in `.gitignore` (build artefact, not committed).

### Benchmarking (`bench.sh`)

`bench.sh` runs criterion benchmarks in release mode. Use it to measure
performance before and after any optimisation work. Criterion stores baseline
results automatically — use `bench.sh -- --save-baseline before` and
`bench.sh -- --baseline before` for explicit A/B comparisons.

---

## Code documentation

Code doc comments and `docs/` specification have complementary, non-overlapping
roles. Do not duplicate content between them — cross-reference instead.

| Layer | Answers | Lives in |
|-------|---------|----------|
| Doc comments | What this unit does; its contract, parameters, errors, panics | `///` / `//!` in source |
| Specification | Why the system is designed this way; cross-cutting flows; architecture | `docs/` |

Rules:
- The codebase currently has a ~4% comment ratio. Rather than a standalone
  documentation pass, every PR that touches internal code must document what
  it modifies: architecture decisions, invariants, algorithmic complexity, and
  `// SAFETY:` comments for any unsafe block.
- When behaviour changes, update the doc comment in the same commit.
- Use glossary terminology (`docs/glossary.md`) — do not introduce synonyms.
- Data invariants that cannot be expressed in the type system belong in the
  doc comment of the owning type.
- System-level reasoning about why a design choice was made belongs in
  `docs/decisions.md`, not in a doc comment.

---

## Crate selection

Always use the latest stable version of crates unless there is a specific,
documented reason not to (e.g. incompatibility with another dependency,
a regression in the new release, or a removed feature the project relies on).
When pinning to an older version, record the reason in `docs/decisions.md`.

pds's existing dependencies are established and well-tested. Before adding
any new crate, evaluate it against the project's goals and record the result
in `docs/decisions.md`. Criteria:

- **Correctness fit.** Does it actually solve the problem?
- **Ecosystem alignment.** Does it fit the existing dependency stack?
- **Compile-time cost.** Does the dependency tree materially affect build times?
- **Maintenance health.** Recent releases, responsive maintainers.
- **MSRV compatibility.** Does its minimum Rust version match the project's MSRV (1.85)?

---

## Documentation (`docs/`)

All project documentation lives under `docs/` as Markdown.

| File | Purpose |
|------|---------|
| `decisions.md` | Decision log — what was decided and why |
| `glossary.md` | Project terminology (data structures, internals) |
| `references.md` | Papers, implementations, external resources |
| `baselines.md` | Build speed, test speed, and benchmark baselines for periodic comparison |
| `architecture.md` | Internal architecture of core data structure modules |
| `impl-plan.md` | Phased implementation plan (Phase 0–6) with dependency tracking |

Subdirectories are permitted for images (`docs/img/`) or large topic areas.

### Formatting

- **No numbered headings.** Use `#`, `##`, `###` without manual numbers.
- **Table of contents.** Every document with more than two top-level sections
  must open with a ToC as a Markdown link list.
- **Pandoc cross-references** for all internal links:

  | Element  | Declaration                        | Reference    |
  |----------|------------------------------------|--------------|
  | Figure   | `![Caption](img/x.png){#fig:id}`   | `@fig:id`    |
  | Table    | `Table: Caption {#tbl:id}`         | `@tbl:id`    |
  | Section  | `# My Section {#sec:id}`           | `@sec:id`    |

---

## Living documents

### Implementation plan (`docs/impl-plan.md`)

`docs/impl-plan.md` is a phased implementation plan (Phase 0–6) with dependency
tracking. It serves as both a backlog and a sequencing guide. Items are numbered
by phase (e.g. 0.1, 3.2, 5.4). Dependencies between items are documented in
the dependency map at the end of the file.

### Proof-of-concept gate

Any speculative or uncertain plan item must pass a low-cost proof of concept
before committing to full implementation. The PoC answers a specific go/no-go
question — if the answer is "no", the item is killed or deprioritised without
wasting effort on the full build.

**When to require a PoC:**
- The item's value depends on an unverified assumption (e.g. "data structure X
  is faster than Y", "memory overhead is dominated by Z")
- The complexity is moderate or higher
- A failed implementation would produce throwaway code

**PoC types (cheapest first):**
1. **Measurement** — benchmark or profile the current system to validate the
   premise. If the bottleneck isn't where you think it is, the optimisation
   has no target.
2. **Micro-benchmark** — build the smallest possible standalone version of
   the proposed change and measure it in isolation.
3. **Spike** — implement the change in a branch with minimal testing, measure
   the impact, then decide whether to do it properly.

**PoC rules:**
- Define the go/no-go question before starting
- Set a time box (typically 1-2 hours for measurement, half a day for a spike)
- Record the result in `docs/decisions.md` regardless of outcome — negative
  results prevent future rework
- A PoC that passes is not the implementation — it validates the direction,
  then the real implementation follows with proper tests and documentation

### Decision log (`docs/decisions.md`)

Record every non-obvious architectural or design choice. The key field is
**why** — reasoning that will not be apparent from the code and that prevents
future changes from silently reversing deliberate decisions.

Each entry: **Context** (what prompted it) . **Decision** (what was chosen) .
**Alternatives considered** (what was rejected and why) . **Consequences**
(trade-offs introduced).

### Glossary (`docs/glossary.md`)

Define all project-specific terms. Use them consistently in identifiers, doc
comments, commit messages, and documentation. If two terms mean the same thing,
pick one; record the other as an alias.

### References (`docs/references.md`)

When consulting an external resource — paper, implementation, RFC — add it to
the relevant section. This makes the context available for future development,
including AI-assisted work on this project.

---

## Implementation plan review {#sec:plan-review}

The implementation plan (`docs/impl-plan.md`) is long and its items are
interdependent. It must be kept current as work progresses:

- **After completing any plan item**, review surrounding items for cascading
  effects. Update dependencies, sequencing, and rationale as needed.
- **When unexpected findings emerge** during research or implementation (e.g.
  an optimisation proves infeasible, a dependency turns out to be unnecessary,
  or a new prerequisite is discovered), update `docs/impl-plan.md` immediately
  — do not defer until the item is "done".
- **At the start of each new phase**, re-read the entire plan to check whether
  completed work has changed the assumptions of upcoming items.
- **Periodically review** the dependency map and parallel-tracks section to
  ensure they still reflect reality. Dependencies may dissolve or new ones
  may appear as the codebase evolves.
- **Move completed items** to a "Done" section or mark them `[x]`. Do not
  delete them — the rationale and decisions embedded in completed items are
  valuable context.
- **Record surprises** in `docs/decisions.md`. If an item's outcome differed
  significantly from what was planned, capture what changed and why.

The goal is that `docs/impl-plan.md` always reflects current understanding,
not the assumptions from the day it was written.

---

## Dependency management

### Regular review

Dependencies must be reviewed regularly — not just when something breaks.
At the start of any significant work session, or at minimum monthly:

1. **`cargo update --dry-run`** — check for available semver-compatible updates
2. **`cargo audit`** — check for known security advisories (add to CI if not
   already present)
3. **`cargo tree -d`** — check for duplicate crate versions (increases compile
   time and binary size)
4. **Review changelogs** for direct dependencies before updating — look for
   performance improvements, bug fixes, deprecations, and breaking changes
   in upcoming major versions

### Suitability review

Beyond version updates, periodically assess whether each dependency is still
the right choice. At minimum quarterly, or when starting a major new work item:

1. **Maintenance health** — check the crate's repository for recent commits,
   open issue response times, and release cadence. A crate with no releases
   in 12+ months and unaddressed issues is a candidate for replacement.
2. **Deprecation signals** — look for deprecation notices in the README,
   `docs.rs` page, or Rust community channels (URLO, Reddit, This Week in
   Rust). Some crates are superseded quietly without a formal deprecation.
3. **Better alternatives** — search crates.io and lib.rs for newer crates
   that serve the same purpose. The Rust ecosystem moves fast; a crate
   chosen 6 months ago may now have a more performant, better-maintained,
   or more ergonomic competitor.
4. **Dependency weight** — run `cargo tree` and check whether any dependency
   pulls in a disproportionate transitive tree for what it provides. A
   utility crate that adds 30 transitive dependencies may not be worth it
   when a 10-line local implementation would suffice.
5. **Feature flag audit** — check whether enabled feature flags are still
   needed. Unused features pull in unnecessary code and dependencies.

When a dependency is identified as unsuitable, record the finding and the
proposed replacement in `docs/decisions.md` before making the change.
Replacements must pass `test.sh` and should not be batched with unrelated
work.

### Safe update process

- Run `cargo update` to apply semver-compatible updates
- Run `test.sh` to verify nothing breaks
- For major version bumps: review the changelog, update call sites, record
  the decision in `docs/decisions.md`

### Test coverage enables safe updates

Dependency updates are only safe when the test suite exercises the dependency
at its integration points. When adding or evaluating a dependency:

- Ensure tests cover the code paths that use it
- If a dependency is used in unsafe code, ensure fuzz/miri coverage exists
- If a dependency affects serialisation (serde, bincode), ensure round-trip
  tests exist

### CI enforcement

`test.sh` is the quality gate for dependency updates. If `test.sh` passes
after `cargo update`, the update is safe to commit. Add `cargo audit` to
CI to catch advisories automatically.

---

## Project maintenance

### Commit on success

After each incremental success — a work item completed, `test.sh` green,
a meaningful chunk of functionality working — commit and push immediately.
Do not batch unrelated successes into a single commit.

- **Commit message:** Describe *what changed and why*, not "wip" or "updates".
  Reference the `docs/impl-plan.md` item if applicable (e.g. "0.3: add
  hashset benchmarks").
- **Push:** Push to remote after each commit. Small, frequent pushes are
  safer than large batches.
- **Report:** After committing, state what was completed so progress is
  visible in the conversation.
- **Update the plan:** Move the completed item from Current to Done in
  `docs/impl-plan.md` in the same commit or immediately after.

### General rules

- Keep `README.md` current whenever the public API, usage, dependencies, or
  architecture changes.
- `build.sh` and `test.sh` must remain runnable without arguments and exit
  non-zero on failure.
- When making changes, update both `docs/impl-plan.md` (mark items done,
  adjust dependencies) and `docs/decisions.md` (record non-obvious choices).
- CI (`.github/workflows/ci.yml`) is the upstream CI configuration. Keep it
  working but do not add local-only checks to it — those belong in `test.sh`.
