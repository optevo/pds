# Project Directives — imbl

Persistent/immutable collection datatypes for Rust. Fork of
[jneem/imbl](https://github.com/jneem/imbl) with a long-term improvement plan.

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

### Upstream-first

Every change should be structured as an independent, upstreamable PR: small,
focused, well-tested, with a clear commit message. Avoid coupling unrelated
changes. Breaking changes are batched into v8.0.0 (Phase 5 of the
implementation plan).

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
  use `unsafe` to paper over borrow checker errors. Existing unsafe code should
  be audited and documented (Phase 3.2), not expanded.
- **No undeclared dependencies.** NEVER add a crate to `Cargo.toml` that is
  not already present, without explicit approval. Consult `Cargo.toml` for
  the authoritative list of available crates and their pinned versions; do
  not assume API knowledge from training data.

---

## Testing

### Plan first

Before writing tests for a TODO item, review the item's rationale and
dependencies in `TODO.md`. State what needs to be tested, why (what failure
mode it guards against), and how (unit, integration, property, fuzz, benchmark).

### Completion gate

Every work item in `TODO.md` should define its acceptance criteria as tests
where feasible. An item moves from planned to done only when `test.sh` passes
with those tests in place. Items that cannot be meaningfully tested
(scaffolding, documentation, tooling setup) must state why in lieu of test
criteria.

### Coverage threshold

Write tests for error paths with the same attention as success paths — AI
consistently skips them.

Prefer `Result`-returning test functions over `#[should_panic]`; a `Result`
failure names what went wrong, a panic only confirms something blew up.

Do not mock internal boundaries. Mock at the system edge (external I/O, network)
only — internal mocking masks integration failures that only appear at runtime.

### Property-based testing

imbl already uses `proptest` extensively. When adding or modifying data structure
operations, add proptest strategies that exercise the new code paths. Fuzz targets
(`fuzz/`) complement proptest for longer-running, coverage-guided exploration —
particularly important for unsafe code paths (Focus/FocusMut).

### Benchmarking

imbl has criterion benchmarks in `benches/` for ordmap, hashmap, and vector.
When making performance-sensitive changes, run the relevant benchmarks before
and after. Wrap inputs in `std::hint::black_box` — without it the optimiser
may eliminate the work and the benchmark measures nothing.

New benchmarks should be added for any data structure that gains or loses a
benchmark during the improvement work (e.g. hashset, ordset benchmarks are
currently missing and should be added in Phase 0.3).

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

imbl's existing dependencies are established and well-tested. Before adding
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

The implementation plan lives in `TODO.md` at the project root (not
`docs/impl-plan.md`) because of its size and central role in this fork.

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

### Implementation plan (`TODO.md`)

`TODO.md` is a phased implementation plan (Phase 0–6) with dependency tracking.
It serves as both a backlog and a sequencing guide. Items are numbered by phase
(e.g. 0.1, 3.2, 5.4). Dependencies between items are documented in the
dependency map at the end of the file.

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

The implementation plan (`TODO.md`) is long and its items are interdependent.
It must be kept current as work progresses:

- **After completing any TODO item**, review surrounding items for cascading
  effects. Update dependencies, sequencing, and rationale as needed.
- **When unexpected findings emerge** during research or implementation (e.g.
  an optimisation proves infeasible, a dependency turns out to be unnecessary,
  or a new prerequisite is discovered), update `TODO.md` immediately — do not
  defer until the item is "done".
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

The goal is that `TODO.md` always reflects current understanding, not the
assumptions from the day it was written.

---

## Project maintenance

- Keep `README.md` current whenever the public API, usage, dependencies, or
  architecture changes.
- `build.sh` and `test.sh` must remain runnable without arguments and exit
  non-zero on failure.
- When making changes, update both `TODO.md` (mark items done, adjust
  dependencies) and `docs/decisions.md` (record non-obvious choices).
- CI (`.github/workflows/ci.yml`) is the upstream CI configuration. Keep it
  working but do not add local-only checks to it — those belong in `test.sh`.
