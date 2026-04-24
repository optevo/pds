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
| wide | 0.7 | 1.3 | SIMD for HAMT (CHAMP killed — DEC-007/015/020) |
| criterion | 0.7 | 0.8 | Dev-dep, benchmarks |
| proptest-derive | 0.6 | 0.8 | Dev-dep, test macros |
| bincode | 2.0.1 | 3.0.0 | RUSTSEC-2025-0141 unmaintained |

Duplicate crate: `getrandom` v0.3/v0.4 (transitive from different rand_core
versions — harmless, resolves with rand update).

**Decision:**
- Do not update breaking dependencies now. All are non-urgent.
- **bincode**: deprecation tracked in item 1.3 — remove in v8.0.0, not update.
- **wide 0.7 → 1.3**: CHAMP evaluation complete (DEC-007, DEC-015) —
  HAMT retained, `wide` stays. Update to 1.3 can proceed when convenient.
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
Breaking updates are deferred to natural integration points (v8.0.0
for bincode removal). The semver-compatible deps are all current and
audit-clean except the known bincode advisory.

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

---

## DEC-007: 4.2 CHAMP prototype — mixed results, defer integration

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Plan item 4.2 required a standalone CHAMP (Compressed Hash-Array Mapped
Prefix-tree) prototype benchmarked against the current SIMD HAMT to make
a go/no-go decision on item 4.3 (CHAMP integration). The prototype
(`src/champ.rs`) implements the full OOPSLA 2015 design: two-bitmap
encoding (datamap + nodemap), contiguous value/child arrays, canonical
deletion, and `Arc`-based structural sharing. Benchmarked with criterion,
`target-cpu=native`, on Apple M5 Max.

**Benchmark results (median, 100 samples):**

| Operation | Size | CHAMP | HAMT | Delta |
|-----------|------|-------|------|-------|
| i64 lookup | 10K | 138 µs | 84 µs | +64% slower |
| i64 lookup | 100K | 1720 µs | 1534 µs | +12% slower |
| str lookup | 10K | 176 µs | 149 µs | +18% slower |
| str lookup | 100K | 3345 µs | 3051 µs | +10% slower |
| i64 insert (persistent) | 10K | 2700 µs | 3841 µs | -30% faster |
| i64 remove (persistent) | 10K | 2258 µs | 3811 µs | -41% faster |
| i64 iter | 100K | 511 µs | 909 µs | -44% faster |
| str insert (persistent) | 10K | 3241 µs | 4353 µs | -26% faster |
| str remove (persistent) | 10K | 2855 µs | 4521 µs | -37% faster |
| str iter | 100K | 543 µs | 853 µs | -36% faster |

**Decision:**
Do not proceed to 4.3 (full CHAMP integration) at this time. The
results are mixed — CHAMP is dramatically faster for persistent
mutations (26-41%) and iteration (36-44%), but significantly slower for
lookups (10-64%). The SIMD HAMT's parallel probe is genuinely effective
for lookups and cannot be replicated with CHAMP's popcount-based
indexing.

**Rationale:**
- The 64% i64 lookup regression at 10K is too large to accept for a
  general-purpose collection library. Lookups are the most common
  operation in typical map usage.
- The mutation and iteration wins are impressive but insufficient to
  offset the lookup penalty for most workloads.
- A hybrid approach (CHAMP node layout with SIMD probing) might capture
  both benefits, but that requires further research — it's not the
  standard CHAMP design from the paper.

**Alternatives considered:**
- Accept the lookup regression and adopt CHAMP for its mutation/iteration
  wins — rejected because lookup is the dominant operation for most map
  users.
- Abandon CHAMP entirely — rejected because the prototype demonstrates
  significant structural advantages (contiguous layout, canonical form)
  that could be exploited in future work.
- Hybrid SIMD-CHAMP — promising but speculative; deferred to Phase 6
  as a research item.

**Consequences:**
- 4.3 (CHAMP integration) is deferred indefinitely. The current SIMD
  HAMT remains.
- The prototype (`src/champ.rs`) was removed in DEC-020.
- The CHAMP iteration advantage (36-44%) motivates investigating whether
  the SIMD HAMT's iteration can be improved independently (the current
  3-tier node hierarchy fragments iteration across node types).
- Future work: the hybrid SIMD-CHAMP idea was prototyped and failed its
  PoC gate (DEC-015). CHAMP integration is fully abandoned (DEC-020).

---

## DEC-008: 3.3 Transient/Builder API — already handled by &mut self methods

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Plan item 3.3 proposed a `Builder<T>` wrapper that holds sole ownership of
nodes, ensuring `SharedPointer::get_mut` always succeeds (no fallback
cloning). The motivating scenario was batch mutations where the persistent
API clones nodes unnecessarily because the collection is still referenced.

**Decision:**
Mark 3.3 as already handled. The existing `&mut self` methods on all five
collection types already provide the builder pattern's core benefit: when
the caller holds the only reference, `Arc::make_mut` detects refcount == 1
and returns `&mut T` without cloning (DEC-004). Benchmarked at 100K i64
inserts: `&mut self` methods are 8-14× faster than persistent methods,
achieving throughput comparable to `std::collections::HashMap`.

The only remaining overhead a builder could eliminate is the per-node
atomic compare-exchange in `Arc::make_mut` (~20-30% potential saving).
However, implementing a builder requires duplicating the entire node
hierarchy for each collection type (~5000 lines of parallel builder node
types across the five collections), plus maintaining two code paths for
every mutation. The complexity cost vastly exceeds the marginal gain.

**Alternatives considered:**
- Full builder implementation — rejected due to massive code duplication
  (~5000 lines) for a marginal 20-30% improvement over `&mut self` which
  is already 8-14× faster than persistent ops.
- `UnsafeCell`-based builder that avoids `Arc` overhead — rejected because
  it bypasses Rust's ownership guarantees and conflicts with the crate's
  `#![deny(unsafe_code)]` policy.
- Transient flag on existing nodes (Clojure-style `AtomicBoolean`) —
  rejected because Rust's ownership system already provides the same
  guarantee at compile time via `&mut self`.

**Consequences:**
3.3 is closed. The idiomatic Rust pattern for batch mutation is:
```rust
let mut map = map;  // take ownership (refcount == 1)
map.insert(k1, v1); // &mut self — no cloning
map.insert(k2, v2);
// map is now the persistent result
```
Documentation should guide users toward this pattern. No code changes
needed for 3.3.

---

## DEC-009: 4.4 Merkle hash caching — accepted with incremental maintenance

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
Plan item 4.4 proposed adding a Merkle hash fingerprint to HAMT nodes for
O(1) negative equality checks (maps with different root hashes are definitely
unequal, skipping element-by-element comparison). The implementation is
HAMT-only — B+ tree and RRB tree nodes would require additional `Hash`
bounds on values.

**Design:**
Each HAMT node (GenericSimdNode, HamtNode) stores a `u64` merkle_hash field.
The hash is maintained incrementally during mutations:
- Leaf entries contribute `fmix64(key_hash as u64)` (MurmurHash3 finaliser)
- Child nodes contribute their `merkle_hash` directly (no outer mixing)
- Combination is commutative addition (`wrapping_add`/`wrapping_sub`)
- On insert/remove, compute `old_contrib` and `new_contrib` for the changed
  entry, then: `merkle_hash = merkle_hash - old_contrib + new_contrib`

The root merkle_hash is effectively the sum of `fmix64(key_hash)` across all
entries — independent of tree structure. This is correct because HAMT tree
structure is fully determined by key hashes (same keys → same structure).

**Equality check integration:**
In `test_eq`, after the existing `ptr_eq` fast path, a Merkle negative check
compares root hashes. The check is only valid when both maps share the same
hasher instance (detected via type-erased pointer comparison), which is the
common case for maps cloned from a common ancestor.

**Benchmark results (criterion, Apple M5 Max, 10K i64 keys):**

| Approach | lookup | insert_mut | remove_mut |
|----------|--------|-----------|------------|
| Full recompute (v1) | +0.7% | +348% | +290% |
| Incremental + inner fmix64 (v2) | +2.4% | +3.9% | +14.6% |
| Incremental, no inner fmix64 (v3) | +1.6% | ~0% | +7.7% |
| Inline old_m capture (v4) | +1.5% | ~0% | +4.9% |
| Wyhash wide-mul mixer (v5, final) | **-1.7%** | **-8.7%** | **+1.4%** |

The wyhash-style wide-multiply mixer (128-bit multiply + fold-XOR, 2 ops
vs fmix64's 6 ops) eliminated effectively all overhead. On Apple Silicon
the UMULH instruction makes 64×64→128 multiply single-cycle.

**Decision:**
Accept the implementation as always-on (no feature flag). The final overhead
is effectively zero: insert is faster than pre-merkle (better codegen from
simpler mixer), lookup unchanged, remove within noise threshold at +1.4%.
All benchmarks are for i64 keys — string keys show even less relative impact.

The benefit is O(1) negative equality rejection for maps that differ, which
is a substantial win for workloads that frequently compare or diff maps.

**Alternatives considered:**
- Feature flag (opt-in or opt-out) — rejected because conditional compilation
  throughout the HAMT code adds complexity disproportionate to a <5% overhead.
- u32 merkle_hash — saves 4 bytes per node but same arithmetic speed on 64-bit
  hardware. Could be revisited if memory footprint becomes critical.
- XOR-based combination instead of addition — rejected because XOR has higher
  collision risk (a ⊕ a = 0 for any a, losing information on duplicates).
- Lazy computation (compute merkle_hash only on first equality check) — rejected
  because it adds branching on every mutation and equality check, and the
  incremental approach is already near-free.
- Full recompute on mutation — rejected due to catastrophic O(n) per-level cost.

**Consequences:**
All HAMT nodes gain a `u64` merkle_hash field (+8 bytes per node). The hash
is maintained incrementally during insert/remove. Equality checks gain a fast
path that rejects unequal maps in O(1). The hash only covers keys (via their
existing HashBits), not values — value-only changes are not detected by the
Merkle hash. This means the Merkle check cannot be used for diff optimisation
(where value changes matter), only for equality.

---

## DEC-010: 5.1 Default to triomphe::Arc

**Date:** 2026-04-24
**Status:** Accepted

**Context:**
The `triomphe` feature flag already existed, switching the internal
`DefaultSharedPtr` from `std::sync::Arc` (via archery's `ArcK`) to
`triomphe::Arc` (via `ArcTK`). `triomphe::Arc` omits the weak reference
count, saving 8 bytes per allocation and eliminating one atomic RMW per
clone/drop. The feature was opt-in; this decision makes it the default.

**Decision:**
Add `triomphe` to the default features in `Cargo.toml`. All collections
now use `triomphe::Arc` internally by default. Users who need
`Arc::downgrade` (weak references) can opt out with
`default-features = false`.

**Benchmark results (triomphe vs std::Arc):**

| Benchmark | Change |
|-----------|--------|
| hashmap_i64/lookup_10000 | -0.8% |
| hashmap_i64/insert_mut_10000 | +4.8% (noise — 100K shows -4.2%) |
| hashmap_i64/remove_mut_10000 | -5.3% |
| hashmap_str/lookup_10000 | -2.0% |
| hashmap_str/insert_mut_10000 | -6.8% |
| hashmap_str/remove_mut_10000 | -8.6% |

String-key operations improve 2-9% (allocation-heavy paths benefit most).
Integer-key operations show mixed results at 10K but consistent improvement
at 100K. No significant regressions.

**Alternatives considered:**
- Keep as opt-in — rejected because the performance improvement is
  consistent and the trade-off (no weak references) is acceptable for
  persistent collections that never use `Arc::downgrade` internally.
- Remove std::Arc support entirely — rejected because downstream users
  may have legitimate reasons (weak references, interop with other crates
  that require `std::Arc`).

**Consequences:**
Breaking change: the concrete pointer type changes for all users not
already enabling the `triomphe` feature. Batched into v8.0.0. Users who
extract or inspect internal pointer types, or who rely on `Arc::downgrade`,
must opt out of the default feature. The `triomphe` crate becomes a
required dependency (previously optional).

---

## DEC-011: 5.3 Const generic branching factor — deferred

**Date:** 2026-04-24
**Status:** Deferred

**Context:**
The plan proposed replacing the compile-time `small-chunks` feature flag
with const generic parameters on all collection types, allowing users to
specify branching factor at the type level (e.g. `Vector<A, 4>`).

**Decision:**
Defer indefinitely. The cost is disproportionate to the benefit.

**Blockers identified:**
1. **HashMap/HashSet: blocked by stable Rust.** The HAMT's SIMD node
   hierarchy derives constants from HASH_LEVEL_SIZE: HASH_WIDTH = 2^B,
   SMALL_NODE_WIDTH = HASH_WIDTH/2. Using computed values as const generic
   arguments (e.g. `SparseChunk<..., {2_usize.pow(B)}>`) requires the
   unstable `generic_const_exprs` feature. No workaround on stable Rust.
2. **Vector + OrdMap/OrdSet: feasible but massive.** ~140+ type reference
   sites across ~80 impl blocks, plus iterators, Focus/FocusMut, rayon,
   serde, diff, and proptest implementations. Purely mechanical but
   high-risk for compilation cascades.
3. **Marginal practical benefit.** The `small-chunks` feature flag already
   provides the only real use case (testing with small nodes). No
   production use case for custom branching factors exists.

**Alternatives considered:**
- Trait-based config (e.g. `HashConfig` trait with associated constants) —
  same `generic_const_exprs` blocker for SparseChunk parameterisation.
- Partial implementation (Vector + OrdMap only, not HashMap) — inconsistent
  API, confusing for users.
- Internal const generics without public exposure — provides no benefit
  over the feature flag approach.

**Consequences:**
The `small-chunks` feature flag remains the mechanism for testing with
small node sizes. Revisit when `generic_const_exprs` stabilises (tracking
issue rust-lang/rust#76560).

## DEC-012: 5.4 no_std support — accepted

**Date:** 2026-04-24

**Context:** imbl depended on `std` for `fmt`, `hash`, `mem`, `ops`, `collections`,
`sync::Mutex`, and `collections::hash_map::RandomState`. Most of these have
`core` or `alloc` equivalents. Embedded and WASM users cannot link `std`.

**Decision:** Add `#![cfg_attr(not(feature = "std"), no_std)]` with `extern crate alloc`.
Replace `std::fmt/hash/mem/ops/iter/cmp/borrow/marker/ptr` with `core::` equivalents.
Replace `std::collections::{BTreeMap,BTreeSet,VecDeque}` with `alloc::collections::`.
Add explicit `use alloc::{vec, vec::Vec, borrow::ToOwned}` imports where needed (these
are in the std prelude but not the core prelude). Gate `RandomState`-dependent type
aliases (`HashMap`, `HashSet`, `PBag`, `HashMultiMap`, `InsertionOrderMap`),
convenience `new()` methods, `Default` impls, and `From<std::collections::*>` impls
behind `#[cfg(feature = "std")]`. Generic variants (`GenericHashMap` etc.) remain
available in no_std. Write a spin-lock fallback (`SpinMutex`) for `FocusMut`'s
interior mutability since `std::sync::Mutex` is unavailable in no_std.
Feature `std` is on by default.

**Alternatives considered:**
- `core`-only (no `alloc`) — impossible, imbl fundamentally requires heap allocation
  for its tree nodes (Arc, Vec, Box).
- Conditional compilation with `cfg` on every `std` use site — fragile and hard to
  maintain; the module-level import replacement is cleaner.
- Third-party no_std mutex (e.g. `spin` crate) — unnecessary dependency for a single
  internal use site; the SpinMutex is ~30 lines.

**Consequences:**
- `default-features = false` gives no_std + alloc support.
- Users must provide their own `BuildHasher` in no_std (no `RandomState`).
- The `atom` feature requires `std` (depends on `arc-swap` which needs `std`).
- SpinMutex is only used by `FocusMut` which needs interior mutability for its
  tree reference; the lock is very short-held so spin is acceptable.

## DEC-013: foldhash as optional no_std hasher — accepted

**Date:** 2026-04-24

**Context:** With no_std support (DEC-012), users must supply their own
`BuildHasher` because `RandomState` requires `std`. Evaluated foldhash
as an optional built-in default for no_std environments.

**Decision:** Add foldhash as an optional dependency (`dep:foldhash`,
default-features = false). When the `foldhash` feature is enabled without
`std`, provide convenience type aliases (`HashMap`, `HashSet`, `PBag`,
`HashMultiMap`, `InsertionOrderMap`) using `foldhash::fast::RandomState`.
When `std` is enabled, `std::collections::hash_map::RandomState` remains
the default regardless of the `foldhash` feature.

**Evaluation summary:**
- **Zero runtime dependencies** — critical for a library crate.
- **no_std: core-only** — does not even require `alloc`. Uses ASLR-derived
  entropy via atomic fallback, no `getrandom` needed.
- **Performance:** Top-tier, ranks #1-2 across diverse workloads. 40-byte
  HashMap overhead (vs ahash's 64).
- **API:** `foldhash::fast::RandomState` implements `Default + Clone +
  BuildHasher + Send + Sync` — exact match for imbl's bounds.
- **MSRV:** 1.60 (imbl is 1.85).
- **Adoption:** ~37M downloads/month, 179 direct dependents.
- **Security:** Minimal DoS resistance (ASLR seeds). Acceptable for
  persistent data structures; security-sensitive users supply their own hasher.
- **Maintenance:** Single maintainer (Orson Peters), but algorithmically
  stable and zero-dependency.

**Alternatives considered:**
- ahash — strong DoS resistance but 3-4 transitive dependencies (cfg-if,
  zerocopy, once_cell, optionally getrandom). Too heavy for a library crate.
- rustc-hash / fxhash — no random seeding by default, not DoS-resistant.
- wyhash — requires manual seeding, no `BuildHasher` + `Default` impl.
- No built-in hasher — forces all no_std users to bring their own. Workable
  but poor DX. foldhash's zero-dep nature makes this unnecessary.

**Consequences:**
- New optional feature `foldhash` (not in `default`).
- Three-tier type alias resolution: std → RandomState; !std + foldhash →
  foldhash::fast::RandomState; !std + !foldhash → no alias (use Generic*).

## DEC-014: Phase 6 research outcomes — April 2026

**Date:** 2026-04-24

**Context:** Comprehensive research sweep across all Phase 6 items and
broader state-of-the-art persistent data structures (2022–2026). Four
parallel research threads covering: (1) persistent ART for OrdMap, (2)
HAMT/CHAMP optimisations and inline storage, (3) Dupe trait / hash
consing / sharing-preserving serialisation, (4) novel techniques from
other languages and recent papers. Included detailed comparison of
lookup implementations across Clojure, Scala CHAMP, Haskell, Lean 4,
rpds, hashbrown, and imbl.

### 6.1 ART for OrdMap — **deprioritised (not recommended)**

ART fundamentally operates on byte sequences, not abstract `Ord`
comparisons. Replacing the B+ tree would require changing the key bound
from `K: Ord + Clone` to `K: ByteEncodable + Clone` — a breaking API
change affecting ~280 downstream crates. No production-quality persistent
ART for generic keys exists (PART, VART, rart all target database-style
byte-string or fixed-integer keys). DuckDB's experience confirms ART has
range-scan limitations vs B-trees even in a database context.

Better investments for OrdMap: tuning `ORD_CHUNK_SIZE` (experiment with
24 or 32), branch-free intra-node binary search, and bulk operations
exploiting sorted structure.

### 6.2 HAMT inline storage — **validated, but CHAMP path is closed**

Steindorfer's GPCE 2014 paper measured 55% median memory reduction for
maps, 78% for sets. Implemented in Capsule (Java), Scala 2.13, Kotlin,
and Swift Collections 1.1 (PR #31 by Steindorfer). However, these all
use CHAMP as the base structure, which was evaluated and rejected for
imbl (DEC-007, DEC-015, DEC-020). imbl's current three-tier architecture
(SmallSimdNode → LargeSimdNode → HamtNode) already captures the spirit
of inline specialisation. Any future memory optimisation would need to
work within the existing SIMD HAMT architecture.

### 6.3 ThinArc — **KILLED (DEC-018)**

`triomphe::ThinArc` was evaluated to save 8 bytes per child pointer.
However, `SharedPointer<T>` is already 8 bytes (single pointer, no
vtable) — there is nothing to save. See DEC-018 for full analysis.

### 6.4 Dupe trait — **low priority, trivial if wanted**

The `dupe` crate's ecosystem is narrow (almost entirely Meta-internal:
Buck2, Starlark-Rust). A newer `light_clone` crate (Feb 2026) already
provides `LightClone` for imbl types from the outside without imbl
needing to do anything. If proceeding: optional `dupe` feature flag, 5
`impl Dupe for X` blocks that call `clone()`. Zero impact on default
builds.

### 6.5 Hash consing — **design validated, medium priority**

Research confirms the plan's design (thread-local
`HashMap<u64, Weak<Node>>`, optional sharded global table) is well-aligned
with Filliâtre & Conchon (2006) and the `hashconsing` Rust crate. The
10–30ns per-lookup overhead estimate is reasonable. The `weak-table`
crate provides `WeakHashSet` that handles stale entry cleanup. Appel's
insight: only intern nodes that survive initial creation (not ephemeral
intermediates during bulk operations). 64-bit Merkle hash collision risk
is ~1 in 2^32 at 2^32 nodes (birthday bound) — acceptable.

A 2025 symbolic computation study measured 2.5x initial memory overhead
but 5–100x downstream speedups for repeated traversals.

### 6.6 Sharing-preserving serialisation — **rkyv is the path**

serde's data model is tree-structured — it cannot preserve sharing
natively (confirmed: issues #194, #1073 closed without resolution).
rkyv's `Sharing`/`Pooling` traits already solve pointer deduplication
and are the architecturally closest match. Apache Fury (now Fory) also
provides automatic reference identity preservation. Cap'n Proto and
FlatBuffers do not support DAG serialisation. immer's `persist.hpp`
(pool-based approach) is a good design reference but JSON-only.

Recommended approach: pool-based like immer, but use rkyv for the binary
format and a custom serde wrapper for the JSON-compatible format. If
Merkle hashing (4.4, done) is used for node IDs, deduplication works
even across separate serialisation sessions.

### Lookup comparison — key finding

The 5-variant Entry enum match on every HamtNode visit is imbl's main
structural disadvantage vs other implementations:

| Implementation | Type discrimination per level |
|---|---|
| CHAMP (Scala/Java) | 2 bitmap tests (no dispatch) |
| Clojure HAMT | 1 null-sentinel check |
| imbl HAMT | 5-way enum match |
| Haskell | 5-way constructor tag |

CHAMP's two-bitmap design (`dataMap` + `nodemap`) eliminates type
discrimination entirely. A bit in `dataMap` means "inline data, compare
key"; a bit in `nodemap` means "child node, recurse." No enum dispatch.

imbl's SIMD leaf nodes are unique among persistent HAMTs (no other
implementation does SIMD at the leaf level). The cold-function workaround
(`get_terminal`) is pragmatic but CHAMP solves the problem structurally.

### Highest-priority new finding: hybrid SIMD-CHAMP

The CHAMP prototype (DEC-007) showed 26–41% faster insert/remove and
36–44% faster iteration, but 10–64% slower lookups. The lookup regression
was the reason for retaining the SIMD HAMT.

**A hybrid approach has not been explored and is the single highest-
potential structural improvement:** use CHAMP's two-bitmap trie structure
(eliminating the 5-way enum match) with SIMD probing retained at the
leaf level for dense nodes. This combines CHAMP's lean traversal with
imbl's unique SIMD filtering.

This should be added as a new research/prototype item.

### Other notable findings

- **Lean 4** uses "Counting Immutable Beans" (Ullrich & de Moura, 2019)
  for automatic destructive update when refcount == 1. This is what
  `Arc::make_mut` already provides in imbl. Validates our approach.
- **Swift Collections 1.1** uses CHAMP with a dual-end buffer layout
  (children from start, values from end of single buffer). Worth
  benchmarking if CHAMP is revisited.
- **Haskell's `Full` node** specialisation eliminates popcount for dense
  (all-32-slots-filled) nodes. Low-complexity optimisation imbl could
  adopt.
- **Arena-backed batch construction** (new item): allocating nodes from a
  bump arena during `FromIterator`/`collect()` eliminates per-node Arc
  overhead during construction.
- **Apple Silicon note:** cache lines are 128 bytes (not 64). Node
  alignment via `#[repr(align(128))]` could improve cache utilisation.

**Alternatives considered:** See individual subsections above.

**Consequences:**
- 6.1 deprioritised. B+ tree retained for OrdMap.
- 6.2 merged into hybrid CHAMP research track.
- 6.3 ThinArc validated, can proceed (prerequisite 5.1 already done).
- 6.4 Dupe low-priority; `light_clone` crate covers the need externally.
- 6.5 hash consing design validated; awaits prioritisation.
- 6.6 rkyv identified as primary path; immer's pool design as reference.
- New item: hybrid SIMD-CHAMP prototype (highest-potential structural
  improvement).
- New item: arena-backed batch construction.
- References updated in `docs/references.md`.

---

## DEC-015: Hybrid SIMD-CHAMP — PoC gate failed, not integrating {#sec:dec-015}

**Date:** 2026-04-24
**Status:** Accepted (supersedes hybrid approach from DEC-014)

**Context:**
DEC-014 identified a hybrid SIMD-CHAMP as the "single highest-potential
structural improvement": CHAMP's two-bitmap trie structure (eliminating the
5-way Entry enum match) with SIMD probing at the leaf level. DEC-007 had
shown the basic CHAMP prototype was 10-64% slower for lookups but 26-44%
faster for mutations/iteration. The hybrid was expected to resolve the lookup
regression by adding SIMD leaves.

A full prototype was built in `src/nodes/champ_node.rs` with:
- Two-bitmap InnerNode (datamap/nodemap via SparseChunk)
- SIMD LeafNode with `wide::u8x16` parallel probing, 32-entry capacity
  (2 SIMD groups of 16), control-byte hashing
- Leaf expansion to InnerNode when full
- Canonical deletion (inline single-entry children)
- Incremental Merkle hash (wyhash-style mixer)
- Mutable and persistent insert/remove paths

LEAF_WIDTH was initially 16, then increased to 32 (matching BRANCH_FACTOR)
to reduce trie depth — at 1000 entries with 32-way branching, each root
position has ~31 entries, overflowing a 16-entry leaf.

**PoC gate question:** Does the hybrid SIMD-CHAMP achieve ≥20% lookup
improvement over the existing HAMT, with no insert/remove regression?

**Benchmark results (criterion, Apple M5 Max, i64 keys):**

| Operation | Size | CHAMP v2 | HAMT | Ratio |
|-----------|------|----------|------|-------|
| lookup | 100 | 700 ns | 688 ns | 1.02x slower |
| lookup | 1000 | 8.37 µs | 7.27 µs | 1.15x slower |
| lookup | 10000 | 92.6 µs | 85.8 µs | 1.08x slower |
| lookup | 100000 | 2.42 ms | 1.35 ms | 1.79x slower |
| insert_mut | 100 | 2.82 µs | 2.32 µs | 1.22x slower |
| insert_mut | 1000 | 33.0 µs | 31.6 µs | 1.05x slower |
| insert_mut | 10000 | 273 µs | 228 µs | 1.20x slower |
| insert_mut | 100000 | 6.11 ms | 3.93 ms | 1.55x slower |
| remove_mut | 100 | 3.93 µs | 2.39 µs | 1.64x slower |
| remove_mut | 1000 | 39.7 µs | 26.4 µs | 1.50x slower |
| remove_mut | 10000 | 370 µs | 258 µs | 1.43x slower |
| iter | 10000 | 27.2 µs | 33.8 µs | **0.80x faster** |
| insert_once | 100000 | 55.6 µs | 43.9 µs | 1.27x slower |
| remove_once | 100000 | 55.8 µs | 44.9 µs | 1.24x slower |

**PoC gate: FAILED.** Lookup target was ≥20% improvement; actual is 2-79%
regression. Insert/remove also regressed 5-64% (unlike the basic CHAMP
prototype from DEC-007 which was faster for mutations).

**Decision:**
Do not integrate the hybrid SIMD-CHAMP. The existing SIMD HAMT is
structurally superior for this Rust implementation. Kill Item 1 from the
Phase 6 plan.

**Root cause analysis:**

1. **Inline SIMD nodes vs pointer-chased Leaf nodes.** The HAMT stores
   SmallSimdNode (16 entries) and LargeSimdNode (32 entries) inline within
   the Entry enum — zero extra pointer indirection. The CHAMP puts Leaf
   nodes behind `SharedPointer`, adding an extra pointer chase and cache
   miss at every bottom-level access. At 100K entries (deep tries), this
   compounds to 79% lookup regression.

2. **Two-bitmap indexing is not cheaper than enum dispatch in Rust.** The
   CHAMP paper's theoretical advantage (two bitmap tests vs type dispatch)
   does not materialise in Rust: the Entry enum's discriminant is a single
   byte, branch prediction handles it efficiently, and the compiler can
   optimise the match into a computed jump. Two popcount + mask operations
   are not faster.

3. **Mutation regression.** The basic CHAMP prototype (DEC-007) was 26-41%
   faster for mutations because its contiguous arrays reduced allocation
   overhead. The hybrid version's SparseChunk-based layout eliminates that
   advantage — SparseChunk has the same allocation characteristics as the
   HAMT's SparseChunk.

4. **LEAF_WIDTH=32 helped but was insufficient.** Increasing from 16 to 32
   improved 1000-entry lookup from 43% slower to 15% slower, confirming the
   depth hypothesis. But the pointer indirection penalty at leaf level
   cannot be resolved without fundamentally changing the node layout to
   inline leaf data (which would essentially recreate the HAMT's Entry
   approach).

**Alternatives considered:**
- **LEAF_WIDTH=64 or larger** — would reduce depth further but increase
  SIMD probe cost per leaf (4+ groups) and waste memory for sparse leaves.
  Does not address the pointer indirection problem.
- **Inline leaf data in InnerNode** (three-bitmap: datamap + leafmap +
  nodemap) — would eliminate the pointer chase but requires variable-size
  inline arrays, essentially recreating the HAMT's Entry enum with extra
  complexity.
- **ThinArc for leaf pointers** — saves 8 bytes per pointer but does not
  eliminate the pointer chase that causes the cache miss penalty.

**Consequences:**
- Item 1 (hybrid SIMD-CHAMP) is killed.
- The existing SIMD HAMT (`src/nodes/hamt.rs`) remains the production
  implementation.
- `src/nodes/champ_node.rs`, `src/champ_v2.rs`, `src/champ.rs`, and
  `benches/champ.rs` were removed in DEC-020.
- Items 2 (ThinArc) and 4 (Arena) proceed targeting the existing HAMT.
- The `wide` crate dependency decision (DEC-003) stands — it remains
  needed for the HAMT's SIMD nodes.
- Key lesson: in Rust, storing variant data inline (enum) with branch
  prediction is more cache-friendly than storing it behind shared pointers
  with bitmap indexing. The JVM-centric CHAMP design does not translate
  to a Rust performance advantage because JVM already pays pointer
  indirection for all objects.

---

## DEC-017: OrdMap B+ tree node size — 32 {#sec:dec-017}

**Date:** 2026-04-24
**Status:** Accepted (supersedes prior size-16 choice)

**Context:**
`ORD_CHUNK_SIZE` controls B+ tree node capacity. Was 16, chosen without
Apple Silicon benchmarks. Apple Silicon has 128-byte cache lines (vs 64 on
x86), potentially favouring larger nodes.

**Decision:**
Increase `ORD_CHUNK_SIZE` from 16 to 32. Benchmarked sizes 16, 24, 32, 48
on Apple Silicon M5 Max with i64 keys/values. Size 32 provides the best
overall profile: large lookup and mutable-op improvements outweigh the
persistent single-op regression in all workloads where lookups exceed
~6-30× inserts (nearly all real workloads).

**Benchmark summary** (i64 keys, % change vs size 16):

| Operation (N=10K) | Size 24 | Size 32 | Size 48 |
|--------------------|---------|---------|---------|
| lookup             | +11.5%  | **-7.6%**  | **-12.8%** |
| lookup (N=100K)    | +3.0%   | **-21.0%** | **-10.4%** |
| insert_mut         | -18.3%  | **-27.2%** | **-33.5%** |
| insert_mut (N=100K)| -19.3%  | **-36.9%** | **-38.4%** |
| remove_mut         | -11.0%  | **-13.4%** | -12.9%  |
| iter               | -9.6%   | **-11.6%** | -10.9%  |
| range_iter         | -6.0%   | **-11.0%** | -11.1%  |
| insert_once        | +12.9%  | +23.0%  | +40.5%  |
| remove_once        | +15.3%  | +24.5%  | +39.9%  |

Negative = faster, positive = slower. Bold = best or near-best.

**Breakeven analysis:** At N=100K, a persistent insert costs ~254 ns (size 32)
vs ~220 ns (size 16) — a 34 ns penalty. A lookup saves ~6.1 ns (22.7 vs 28.8).
Breakeven: 34/6.1 ≈ 5.6 lookups per insert. At N=10K: ~30 lookups per insert.

**Alternatives considered:**
- **Keep 16** — better for persistent-mutation-dominated workloads, but those
  are rare in practice and the lookup regression at large sizes is severe.
- **24** — inconsistent results; lookup at 1K/10K regressed vs 16 despite better
  at 100. Likely unfavourable cache alignment.
- **48** — diminishing lookup returns with accelerating persistent-op regression
  (35-52% slower). Bulk immutable build also regressed 16-27%.
- **64** — not tested; 48 already showed clear diminishing returns.

**Consequences:**
- `src/config.rs` updated to `ORD_CHUNK_SIZE = 32`.
- All three test configurations pass (default, all-features, small-chunks).
- Derived constants: `MEDIAN = 16`, `THIRD = 10`, `NUM_CHILDREN = 33`.
- Leaf nodes: `Chunk<(K,V), 32>` — 512 bytes for `(i64, i64)`.
- Branch nodes: 32 keys + 33 children — fits in ~4 Apple Silicon cache lines.

## DEC-018: ThinArc for HAMT pointers — killed, premise invalid {#sec:dec-018}

**Date:** 2026-04-25
**Status:** Killed

**Context:**
Item 2 of the Phase 6 performance plan proposed replacing archery's
`SharedPointer<T, P>` with `triomphe::ThinArc` to reduce child pointers from
a claimed 16 bytes to 8 bytes. The premise was that `SharedPointerKind`'s type
erasure added vtable/metadata overhead making pointers fat.

**Decision:**
Kill the item. The premise is wrong — `SharedPointer<T, ArcTK>` is already
8 bytes. archery's `ArcTK` backend wraps `triomphe::Arc<()>` via type erasure
(`ManuallyDrop<UntypedArc>`) with zero size overhead. Measured sizes:

| Type | Size |
|------|------|
| `SharedPointer<HamtNode<(i32,i32)>>` | 8 bytes |
| `SharedPointer<SmallSimdNode<(i32,i32)>>` | 8 bytes |
| `SharedPointer<CollisionNode<(i32,i32)>>` | 8 bytes |
| `Entry<(i32,i32)>` | 16 bytes |
| `std::sync::Arc<u64>` | 8 bytes |

**Alternatives considered:**
- **Remove P parameter from HashMap/HashSet** — still valid as an API
  simplification, but provides no performance benefit. Can be done as part
  of the v8.0.0 API cleanup (Phase 5) rather than as a performance item.
- **Use triomphe::Arc directly** — archery already delegates to triomphe::Arc
  when the `triomphe` feature is enabled (which is the default). The ManuallyDrop
  transmute in archery is zero-cost.

**Consequences:**
- Item 2 removed from the performance plan.
- Item 4 (Arena) no longer blocked on Item 2.
- The `P: SharedPointerKind` parameter remains on all collection types.

## DEC-019: Arena/bulk batch construction for from_iter — killed {#sec:dec-019}

**Date:** 2026-04-25
**Status:** Killed

**Context:**
Item 6.8 proposed arena-backed batch construction for `HashMap::from_iter` to
close the 3-5x gap vs `std::HashMap`. Allocation profiling showed imbl makes
~30K heap allocations for 100K elements vs 1 for std.

**Decision:**
Kill the item. Three approaches were prototyped and all failed the PoC gate
(≥15% improvement on `from_iter`):

1. **Vec-of-Vecs partitioning** — partition items into 32 bucket Vecs per level,
   build bottom-up. Created ~60K temporary Vec allocations (worse than the 30K
   node allocations it replaced). 2x slower than original at all sizes.

2. **Pre-allocated partition Vecs** — same approach with `Vec::with_capacity`.
   Reduced to ~60K allocations but still 2x slower due to partition overhead.

3. **In-place American Flag sort + slice-based build** — zero-allocation
   partitioning via swap-based radix sort, clone items from slices at leaf
   level. Same allocation count as original but 10-25% slower due to
   partitioning + cloning overhead.

Allocation profiling data (100K i64 pairs):

| Approach | Allocs | Time | vs Original |
|----------|--------|------|-------------|
| Original (insert loop) | 29,657 | 3.3ms | baseline |
| Bulk v1 (Vec-of-Vecs) | 71,718 | 2.8ms | -15% time but 2.4x allocs |
| Bulk v2 (with_capacity) | 59,847 | 2.5ms | similar |
| Bulk v3 (in-place partition) | 27,575 | 3.0ms | same allocs, 10% slower |
| std::HashMap | 1 | 0.7ms | — |

For string keys, all bulk approaches are significantly worse because cloning
strings from slices doubles heap allocations (each string clone = separate alloc).

**Root cause:** The from_iter gap is inherent to HAMT structure, not to
construction algorithm. HAMT requires ~0.3 node allocations per element
(SmallSimdNode, HamtNode pointers), each via Arc::new. The insert-one-at-a-time
path is already well-optimised: Arc::get_mut always succeeds (unique ownership
during from_iter), SIMD lookups are fast, and tree depth is only 3-4 levels.

**Alternatives considered:**
- **Sort-then-insert** — sort elements by hash to improve cache locality during
  insert. Comparison sort cost (~2ms for 100K) exceeds the entire current
  from_iter time. Not viable.
- **True arena allocator** — pre-allocate a single memory block for all nodes,
  bump-allocate during construction, promote to Arc at the end. This would
  reduce per-allocation overhead but requires deep changes to SharedPointer/Arc
  and a promotion pass. The profiling shows allocation overhead is only ~25% of
  total cost — the majority is tree traversal and SIMD operations. Maximum
  benefit: ~25% improvement, at high complexity cost.

**Consequences:**
- Item 6.8 (Arena batch construction) killed.
- The from_iter 3-5x gap vs std is accepted as inherent to HAMT.
- Focus shifts to other optimisation targets (4.7 hash width, iteration speed).

Profiling data preserved at:
- `/private/tmp/bench_alloc_profile_*.txt` — allocation counts + timings
- `/private/tmp/bench_alloc_bulk_*.txt` — bulk construction variants

## DEC-020: Remove CHAMP PoC artefacts {#sec:dec-020}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
Three CHAMP-related PoC items have been conclusively killed:
- DEC-007: Basic CHAMP prototype — lookup regression too large (10-64%)
- DEC-015: Hybrid SIMD-CHAMP — PoC gate failed (2-79% slower lookups)
- DEC-019: Arena batch construction — PoC gate failed (inherent HAMT gap)

The prototype files totalled 3,406 lines across four files:
- `src/champ.rs` (1,039 lines) — basic CHAMP from DEC-007
- `src/nodes/champ_node.rs` (1,749 lines) — hybrid SIMD-CHAMP from DEC-015
- `src/champ_v2.rs` (205 lines) — public wrapper for champ_node benchmarking
- `benches/champ.rs` (413 lines) — CHAMP benchmark harness

These were originally retained "for reference and benchmark comparison" but
no future plan item depends on them. The HAMT is the permanent production
structure (confirmed by three independent failed attempts to replace it).

**Decision:**
Delete all four files and their module declarations. The benchmark data,
analysis, and lessons learned are fully captured in DEC-007, DEC-014,
DEC-015, and DEC-019.

**Why remove rather than keep for reference:**
1. **Dead code accumulates maintenance cost.** Module declarations, feature
   gates, and Cargo.toml entries must be kept compiling across refactors.
   The 4.7 (hash width) plan included a step to update champ_node.rs —
   unnecessary work for dead code.
2. **Decisions are the reference, not code.** The lessons (inline enum >
   pointer-chased leaves in Rust; HAMT's SIMD probing is non-replicable
   in CHAMP; from_iter gap is structural) are recorded in decisions.md
   where they prevent future rework. The code itself adds nothing over
   the written analysis.
3. **3,406 lines removed** from the build and search surface. Reduces
   noise in grep results, IDE navigation, and crate-level documentation.

**Alternatives considered:**
- **Keep behind a `champ` feature flag** — still compiles, still needs
  maintenance, still appears in searches. No benefit over git history.
- **Move to `examples/`** — the code depends on crate internals
  (`nodes::champ_node`, `SparseChunk`, `SharedPointer`) and cannot
  be an independent example.

**Consequences:**
- `src/champ.rs`, `src/champ_v2.rs`, `src/nodes/champ_node.rs`, and
  `benches/champ.rs` deleted.
- Module declarations removed from `src/lib.rs`, `src/nodes/mod.rs`.
- Bench entry removed from `Cargo.toml`.
- `docs/impl-plan.md` updated: 6.7 "prototype retained" → "prototype
  removed", 4.7 step referencing champ files removed, 6.8 and 6.9
  cross-references to dead items cleaned up, dependency map updated.
- Code remains accessible via git history if ever needed.
