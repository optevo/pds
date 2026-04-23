# Decision Log {#sec:decisions}

## Contents

- [Format](#format)
- [Decisions](#decisions)

---

## Format {#sec:format}

Each entry records what was decided, **why** (the reasoning that will not be
obvious later), what alternatives were rejected and why, and what constraints
the decision introduces. This prevents future changes — including AI-assisted
ones — from silently undoing deliberate choices.

```
## DEC-NNN: Short title

**Date:** YYYY-MM-DD
**Status:** Accepted | Superseded by DEC-NNN

**Context:**
What situation or requirement prompted this decision.

**Decision:**
What was decided.

**Alternatives considered:**
- Alternative A — why rejected
- Alternative B — why rejected

**Consequences:**
Trade-offs introduced. Constraints that now apply. What becomes easier or harder.
```

Add a new entry whenever a non-obvious architectural or design choice is made.
If a decision is later reversed, mark it `Superseded by DEC-NNN` and add the
new entry explaining why the earlier reasoning no longer holds.

---

## Decisions {#sec:decision-entries}

## DEC-001: Fork maintenance strategy — upstream-first

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
imbl is a fork of jneem/imbl. The upstream maintainer is responsive to PRs but
not driving a roadmap. This fork has an extensive improvement plan
(`docs/impl-plan.md`).

**Decision:**
Structure every change as an independent, upstreamable PR: small, focused,
well-tested. Batch breaking changes into a single major version bump (v8.0.0).
Maintain the fork as a parallel track, contributing fixes upstream where possible.

**Alternatives considered:**
- Hard fork with no upstream intent — diverges quickly, duplicates maintenance.
- Only upstream PRs, no fork — blocks on upstream review timelines, cannot
  experiment freely.

**Consequences:**
Each change must be self-contained and tested in isolation. Breaking changes
must be held until v8.0.0 batch. Some experimental items (Phase 6) may
diverge from upstream if they involve fundamental structural changes.

## DEC-002: Nix devShell with stable + nightly toolchains

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Normal development requires stable Rust, but miri and cargo-fuzz require
nightly. The project previously had no Nix devShell.

**Decision:**
Provide two Nix devShells: `default` (stable, sccache) for everyday work, and
`nightly` (nightly, cargo-fuzz) for miri and fuzzing. Entered via `nix develop`
and `nix develop .#nightly` respectively.

**Alternatives considered:**
- Single nightly shell — forces nightly for everything, risks instability.
- rustup-managed toolchains — violates Nix-managed toolchain principle.

**Consequences:**
Miri and fuzzing workflows require explicitly entering the nightly shell.
CI continues to use its own toolchain installation (dtolnay/rust-toolchain).

## DEC-003: Dependency audit — defer breaking updates

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Phase 0.4 dependency audit. All semver-compatible dependencies are current
(cargo update is a no-op). Several breaking updates are available:

| Dep | Current | Available | Impact |
|-----|---------|-----------|--------|
| rand/rand_core/rand_xoshiro | 0.9.x | 0.10.x | Coordinated ecosystem bump |
| wide | 0.7 | 1.3 | SIMD for HAMT; may be removed by CHAMP (4.3) |
| criterion | 0.7 | 0.8 | Dev-dep, benchmarks |
| proptest-derive | 0.6 | 0.8 | Dev-dep, test macros |
| bincode | 2.0.1 | 3.0.0 | RUSTSEC-2025-0141 unmaintained |

Duplicate crate: `getrandom` v0.3/v0.4 (transitive from different rand_core
versions — harmless, resolves with rand update).

**Decision:**
- Do not update breaking dependencies now. All are non-urgent.
- **bincode**: deprecation tracked in item 1.3 — remove in v8.0.0, not update.
- **wide 0.7 → 1.3**: defer until after CHAMP evaluation (4.2). If CHAMP
  replaces the SIMD HAMT, `wide` is removed entirely.
- **rand ecosystem 0.9 → 0.10**: defer until the ecosystem stabilises.
  imbl only uses rand_core for hash seeding and rand_xoshiro for PRNG.
  The API surface consumed is minimal.
- **criterion 0.7 → 0.8**: evaluate when starting benchmark-heavy work.
  Current version is functional.
- **proptest-derive 0.6 → 0.8**: evaluate alongside proptest updates.
- **cargo-audit** added to default Nix devShell for local use.
  CI already has it via `rustsec/audit-check`.

**Alternatives considered:**
- Update everything now — risk breakage across multiple subsystems
  simultaneously with no functional benefit.

**Consequences:**
Breaking updates are deferred to natural integration points (CHAMP work
for wide, v8.0.0 for bincode removal). The semver-compatible deps are
all current and audit-clean except the known bincode advisory.
