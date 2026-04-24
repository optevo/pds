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

---

## DEC-004: 3.1 Arc::get_mut — already handled by Arc::make_mut

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Plan item 3.1 proposed replacing `SharedPointer::make_mut` calls with a
`SharedPointer::get_mut` check (returns `Some(&mut T)` when refcount == 1)
followed by `make_mut` as fallback, to avoid unnecessary cloning when the
caller is the sole owner.

**Decision:**
Mark 3.1 as already handled. `std::sync::Arc::make_mut` (which archery's
`SharedPointer::make_mut` delegates to) already performs an atomic
compare-exchange on the strong count (sync.rs:2503): if strong == 1 and
weak == 0, it provides `&mut T` directly without cloning. Adding a
`get_mut` pre-check would be redundant — it performs the same refcount
test that `make_mut` already does internally.

The performance scenario described in the plan ("let mut map =
map.insert(k, v) clones unnecessarily") is about binding semantics, not
`make_mut` behaviour: during `insert`, the original binding still holds
a strong reference, so refcount is genuinely >1 and cloning is correct.
Avoiding that clone requires _moving_ ownership before mutating, which is
item 3.3 (Transient/Builder API), not 3.1.

**Alternatives considered:**
- Proceed with mechanical replacement of 110 `make_mut` call sites —
  would produce identical runtime behaviour with added code complexity.
- Introduce a helper `fn make_mut_or_get` — same issue, `make_mut`
  already does this.

**Consequences:**
3.1 is closed. The sole-owner mutation performance win is redirected to
3.3 (Transient/Builder), which takes explicit ownership before bulk
mutation. No code changes needed for 3.1.

---

## DEC-005: 4.1 Vector prefix buffer — already implemented

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Plan item 4.1 proposed adding a prefix (head) buffer to the RRB tree to
give O(1) amortised push_front symmetric with push_back. The description
stated "prepend still requires tree modification in many cases" and cited
Scala 2.13's 2-3× improvement from their finger tree rewrite.

**Decision:**
Mark 4.1 as already implemented. The RRB tree's 4-buffer structure
(outer_f, inner_f, middle, inner_b, outer_b) already provides symmetric
front and back buffers. push_front works identically to push_back: fill
outer buffer, swap to inner buffer, push old inner to the middle tree
once every CHUNK_SIZE operations. Benchmarked at 100K elements:
push_front 444µs vs push_back 432µs (~3% difference, within noise).

The plan's description was based on an incorrect assumption about the
existing architecture. Scala 2.13's improvement was relative to their
old Vector which had only a tail buffer — imbl already has the
equivalent of Scala 2.13's improved structure.

**Alternatives considered:**
- Implement a different prefix buffer scheme (e.g. Hinze-Paterson
  finger tree style) — unnecessary since existing buffers already work.
- Optimise the minor left-side push_chunk asymmetry (size table
  conversion for non-full left-edge nodes) — too invasive for a ~3%
  difference.

**Consequences:**
4.1 is closed. No code changes needed. The existing 4-buffer structure
is documented in docs/architecture.md.

---

## DEC-006: 4.5 SharedPointer-wrapped hasher — keep despite i64 regression

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
After 5.2 (Clone bounds cleanup), `S: Clone` remained on ~20 persistent
methods (`update`, `without`, `union`, `intersection`, `entry`, etc.)
because the hasher was stored bare in `GenericHashMap` and `self.clone()`
clones it. Item 4.5 proposed wrapping the hasher in `SharedPointer<S, P>`
so cloning the map bumps a refcount instead of cloning the hasher.

Benchmark results (criterion, `target-cpu=native`, exclusive CPU):
- i64 lookups: 3-5% regression (hash time ~2ns, pointer deref ~1ns is
  proportionally significant)
- String lookups: 0-2% (hash time dominates, deref is noise)
- Mutations (insert/remove): 0-2% (mutation cost dominates)
- Iteration: no measurable change

**Decision:**
Keep the change. The regression is confined to the narrowest case
(tiny keys where hash computation is ~2ns). Three factors justify it:

1. **API simplification.** ~50 `S: Clone` bounds removed from the
   HashMap/HashSet API. This cascades to all downstream consumers —
   any generic struct wrapping these collections no longer needs
   `S: Clone` in its own `Clone` impl.
2. **Philosophy alignment.** imbl is a structural sharing library.
   Every other component is already behind a shared pointer. Storing
   the hasher bare was the odd one out.
3. **Downstream ergonomics.** Users with custom hashers that have
   non-trivial clone costs get a real performance win. Users with
   `RandomState` (the common case) see negligible impact outside the
   i64 micro-benchmark.

**Alternatives considered:**
- Revert and accept `S: Clone` on persistent methods — rejected because
  the bound propagation burden on downstream code is the larger cost.
- Use `Cow<'static, S>` or similar to avoid the allocation — rejected
  because `SharedPointer` already provides the right abstraction and
  `Cow` doesn't support runtime-constructed hashers.
- Specialise: inline for `RandomState`, shared for custom hashers —
  rejected as over-engineered; Rust lacks specialisation on stable.

**Consequences:**
`S: Clone` is completely eliminated from the HashMap/HashSet API.
The hasher field is `SharedPointer<S, P>` in both `GenericHashMap` and
`GenericHashSet`. Internal hasher access uses `&*self.hasher` (explicit
deref) for generic function calls and `&self.hasher` (auto-deref) for
return types. The `Clone` impl for both types no longer requires
`K: Clone`, `V: Clone`, or `S: Clone` — only `P: SharedPointerKind`.
