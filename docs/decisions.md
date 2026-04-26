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
well-tested. Batch breaking changes into a single major version bump (v2.0.0).
Maintain the fork as a parallel track, contributing fixes upstream where possible.

**Alternatives considered:**
- Hard fork with no upstream intent — diverges quickly, duplicates maintenance.
- Only upstream PRs, no fork — blocks on upstream review timelines, cannot
  experiment freely.

**Consequences:**
Each change must be self-contained and tested in isolation. Breaking changes
must be held until v2.0.0 batch. Some experimental items (Phase 6) may
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
| bincode | — | — | Removed (DEC-025) |

Duplicate crate: `getrandom` v0.3/v0.4 (transitive from different rand_core
versions — harmless, resolves with rand update).

**Decision:**
- Do not update breaking dependencies now. All are non-urgent.
- **bincode**: removed entirely (DEC-025).
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
Breaking updates are deferred to natural integration points.
The semver-compatible deps are all current and audit-clean.

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
already enabling the `triomphe` feature. Batched into v2.0.0. Users who
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
aliases (`HashMap`, `HashSet`, `Bag`, `HashMultiMap`, `InsertionOrderMap`),
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
`std`, provide convenience type aliases (`HashMap`, `HashSet`, `Bag`,
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

### DEC-017 Addendum: join-algorithm re-validation (R.15) — 2026-04-26

**Question:** Does the join algorithm (R.11 — `par_union`, `par_intersection`,
`par_difference`, `par_symmetric_difference`) favour a different node size than
the single-tree benchmark used to select 32?

**Method:** Re-benchmarked sizes 16, 24, 32, 48 with `--quick --features rayon`,
adding two new benchmark groups: `ordmap_parallel` (join ops, 10K and 100K,
overlap and disjoint inputs) and `ordmap_i64` (single-tree ops for comparison).
All numbers vs size 32 baseline.

**Single-tree ops (negative = faster than size 32):**

| Op (10K / 100K) | size 16 | size 24 | size 32 | size 48 |
|----------------|---------|---------|---------|---------|
| lookup 10K | +10.7% | +32.3% | **baseline** | +7.3% |
| lookup 100K | +27.3% | +43.8% | **baseline** | +18.5% |
| insert_mut 10K | +41.0% | +27.1% | **baseline** | +2.4% |
| insert_mut 100K | +44.0% | +34.4% | **baseline** | +9.7% |
| iter 10K | +13.9% | +9.6% | **baseline** | +3.6% |

**Parallel join ops — overlap inputs (50% shared keys):**

| Op | size 16 | size 24 | size 32 | size 48 |
|----|---------|---------|---------|---------|
| par_union 10K | +21.7% | +3.5% | **185µs** | +19.4% |
| par_union 100K | +20.0% | +273%† | **592µs** | +68.8% |
| par_intersection 10K | −10.8% | −2.2% | **349µs** | −22.7% |
| par_intersection 100K | +9.0% | +230%† | **1.35ms** | +57.1% |
| par_difference 10K | +20.9% | +1.7% | **203µs** | +13.4% |
| par_difference 100K | +14.6% | +129%† | **935µs** | +21.2% |
| seq_union 10K | +36.5% | +34.3% | **484µs** | −6.3% |
| seq_union 100K | +63.3% | +49.4% | **5.47ms** | +15.2% |

†Size 24 at 100K shows extreme regressions (+129–708%) consistent with a tree
structural pathology at that fill-factor ratio (THIRD=8, MEDIAN=12); the numbers
confirm it is not a viable candidate regardless.

**Conclusion:** Size 32 is confirmed as optimal for both single-tree and join-based
parallel operations. The join algorithm does not change the selection. Key finding:
- Sizes 16 and 24 are 10–44% slower on single-tree ops and offer no parallel advantage.
- Size 48 is only marginally slower on single-tree ops (+2–19%) but 20–69% slower on
  parallel join at 100K, where the dual-tree cache pressure and per-split copy cost of
  larger nodes outweigh the shallower tree depth.
- **`ORD_CHUNK_SIZE = 32` is confirmed. No change to `src/config.rs`.**

**R.15 closed.**

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
  of the v2.0.0 API cleanup (Phase 5) rather than as a performance item.
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

## DEC-021: kv_merkle_hash — O(1) positive equality for HashMap {#sec:dec-021}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
HashMap already had per-node `merkle_hash` on HAMT nodes (key-only), but
comparing maps still required O(n) element-wise iteration. Adding a
value-aware Merkle hash enables O(1) positive equality — if two maps have
the same kv_merkle_hash, they are equal without checking elements.

**Decision:**
Add `kv_merkle_hash: u64` to `GenericHashMap`. Maintained incrementally on
`insert`/`remove` when `V: Hash`. Contribution per entry:
`fmix64(key_hash.wrapping_add(value_hash))`, accumulated via commutative
addition (order-independent). Two-tier internal API: public methods require
`V: Hash` and maintain the hash; internal `_invalidate_kv` helpers don't
require `V: Hash` and mark the hash invalid.

**Alternatives considered:**
- **Key-only Merkle** — already existed. Cannot detect value changes, so
  cannot skip element-wise comparison.
- **Require V: Hash everywhere** — too viral. Would propagate to `partition`,
  `merge_with`, serialisation, and all internal callers. The two-tier
  approach confines `V: Hash` to the public insert/remove surface.

**Consequences:**
- `V: Hash` added to public `insert`, `remove`, `From<Vec>`, `From<BTreeMap>`,
  `FromIterator`, `Extend`, serde `Deserialize`, quickcheck `Arbitrary`,
  `arbitrary::Arbitrary`, proptest `Arbitrary`, rayon `FromParallelIterator`
  and `ParallelExtend`.
- Internal callers (set operations, `hash_multimap`) use invalidating
  helpers — no `V: Hash` required.
- Positive equality check in `PartialEq::eq` only fires when both maps have
  valid kv_merkle and the same hasher instance (pointer equality on
  `RandomState`).

## DEC-022: Vector per-node lazy Merkle hashing {#sec:dec-022}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
Vector equality was always O(n) element-wise comparison. For large vectors
that share most of their structure (common with persistent data structures),
this is wasteful — subtrees that haven't been modified don't need comparison.

**Decision:**
Two-level lazy Merkle scheme:

1. **Node level** (`nodes/rrb.rs`): Each `Node<A, P>` gets `merkle: AtomicU64`.
   Sentinel `u64::MAX` = "not computed". Computed lazily on first request;
   cached until mutation. `AtomicU64` with `Relaxed` ordering — concurrent
   computations are benign (deterministic hash). Hash is position-sensitive:
   `h = h * PRIME + child_hash` so `[a,b] ≠ [b,a]`.

2. **Vector level** (`vector/mod.rs`): `merkle_hash: u64` + `merkle_valid: bool`
   on `GenericVector`. Invalidated by any `&mut self` method. `recompute_merkle()`
   combines all five RRB segments (outer_f, inner_f, middle tree, inner_b,
   outer_b). The middle tree's hash is lazy — only modified subtrees are
   recomputed, giving O(k log n) cost where k is modified nodes.

Positive equality in `PartialEq::eq`: if both vectors have valid Merkle and
matching hashes (and same length), return `true` without element comparison.

**Alternatives considered:**
- **Eager hash maintenance** — would require `A: Hash` on every mutation
  method. Too viral for a library type. Lazy is strictly better: only paid
  when explicitly requested.
- **Vector-level only (no per-node)** — loses the key benefit. Invalidation
  would require recomputing the entire O(n) hash, no subtree skipping.
- **Separate Merkle wrapper type** — adds API complexity. The `merkle_valid`
  bool + lazy recompute is zero-cost when not used.

**Consequences:**
- 8 bytes (`AtomicU64`) added per RRB node. For a 10k-element vector with
  ~160 internal nodes, this is ~1.3 KB overhead.
- 9 bytes (`u64` + `bool`) added per `GenericVector` instance.
- No `A: Hash` requirement on any existing method. Hash bound only on
  `recompute_merkle()` and `merkle_hash()`.
- Positive equality is probabilistic (64-bit hash). See DEC-023 for the
  threshold policy.

## DEC-023: Merkle positive equality requires ≥64-bit hash width {#sec:dec-023}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
Merkle-based positive equality assumes hash collisions are astronomically
unlikely. At 64 bits this holds (~1/2^64 per comparison). If the HashWidth
trait (plan item 4.7) introduces smaller hash widths (e.g. 32-bit for
memory savings), the birthday-bound collision probability becomes
non-negligible (~65k entries for 50% collision chance at 32 bits).

**Decision:**
Positive equality shortcuts (in `PartialEq::eq` for HashMap, HashSet, and
Vector) must only fire when the effective Merkle hash width is ≥ 64 bits.
Currently all Merkle hashes are `u64`, so the check is always satisfied.
When HashWidth is implemented, add a compile-time or runtime guard.

**Alternatives considered:**
- **Always allow positive equality regardless of width** — unacceptable
  false-positive risk at 32 bits.
- **Only support 64-bit Merkle** — overly restrictive. 32-bit nodes can
  still benefit from Merkle for *negative* checks (different hash ⇒
  definitely different) and diff acceleration, just not positive equality.

**Consequences:**
- HashWidth implementation must expose a mechanism to query hash width.
- Positive equality guards must be updated when HashWidth lands.

---

## DEC-024: Unwrap SharedPointer-wrapped hasher {#sec:dec-024}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
DEC-006 wrapped the hasher in `SharedPointer<S, P>` to eliminate ~50
`S: Clone` bounds from the HashMap/HashSet API. This incurs a 3-5%
regression on i64 lookups (pointer indirection on every hash call).

Since then, DEC-021 (kv_merkle_hash) added `V: Hash` to all mutating
HashMap methods. The user observed that requiring `V: Hash` for Merkle
weakens the original motivation for avoiding `S: Clone`, since
`S: Clone` is a much lighter bound than `V: Hash`.

**Decision:**
Unwrap the hasher — store `hasher: S` directly instead of
`hasher: SharedPointer<S, P>`. Re-add `S: Clone` to impl blocks that
clone the map/set (persistent operations, Add for references, Sum).

Hasher identity for Merkle equality gating (previously via pointer
equality on SharedPointer) is replaced by a `hasher_id: u64` field
backed by a global `AtomicU64` counter. Maps/sets cloned from the same
ancestor share the same `hasher_id`; independently constructed instances
get unique IDs.

**Alternatives considered:**
- Keep wrapped — maintains fewer bounds but pays pointer indirection on
  every hash call. The practical burden of `S: Clone` is negligible since
  all standard hashers (`RandomState`, `foldhash::fast::RandomState`)
  implement `Clone` cheaply (just a couple of seed integers).

**Consequences:**
- ~3-5% improvement on i64 hash lookups (removes pointer chase)
- `S: Clone` added to persistent-operation impl blocks (update, without,
  alter, union, intersection, restrict, apply_diff, etc.) and operator
  impls (Add for &Map, Sum)
- Simpler internal code — no SharedPointer deref on hash paths
- `hasher_id` field adds 8 bytes to map/set structs (same size as the
  pointer that was removed from SharedPointer overhead)

---

## DEC-025: Remove deprecated bincode feature {#sec:dec-025}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
The `bincode` feature was deprecated in imbl with a message "will be
removed in v8.0.0". Now that the crate is renamed to pds v1.0.0,
there are no downstream users. The `bincode` crate itself is unmaintained
(RUSTSEC-2025-0141). Users should use serde for serialisation instead.

**Decision:**
Removed entirely at v1.0.0 — deleted `src/bincode.rs`, removed the
`bincode` dependency from `Cargo.toml`, removed the deprecated module
from `lib.rs`, and removed the `-A deprecated` clippy allow from `test.sh`.

**Alternatives considered:**
- Keep deprecated — no benefit, adds a security advisory to `cargo audit`

**Consequences:**
`cargo audit` is now clean. Users needing binary serialisation should use
serde with any binary format crate (e.g. `postcard`, `bitcode`).

## DEC-026: HashWidth trait design — value-level trait, not associated type {#sec:dec-026}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
4.7 Stage 2 requires abstracting hash width so users can choose between
u64 (default, 12 trie levels) and u128 (wide, 25 trie levels). The impl
plan's original sketch used an associated type (`type Bits`). The actual
implementation uses a value-level trait where `H` itself is the hash value
type (u64 or u128 implements HashWidth directly).

**Decision:**
`HashWidth` is implemented as a trait on the hash value type itself:
```rust
pub trait HashWidth: Copy + Eq + Hash + Default + Debug + Send + Sync + 'static {
    fn from_hash64(hash: u64) -> Self;
    fn trie_index(&self, shift: usize) -> usize;
    fn ctrl_byte(&self) -> u8;
    fn ctrl_group(&self) -> u64;
    fn to_u64(&self) -> u64;
}
```
Implemented for `u64` and `u128`. Generic parameter is `H: HashWidth = u64`
on all hash-based types.

**Alternatives considered:**
- Associated type on trait (`type Bits`) — adds indirection, requires
  `<W as HashWidth>::Bits` everywhere. The value-level approach is simpler:
  `H` is both the trait bound and the concrete hash type.
- Const generics for bit width — blocked by `generic_const_exprs` instability
  (see DEC-011).

**Consequences:**
- All hash-based types gain an `H` parameter (breaking in v2.0.0 batch)
- Existing code using `HashMap<K, V>` continues to work via `H = u64` default
- Merkle hashing always uses u64 via `H::to_u64()` — Merkle equality is
  hash-width-independent
- Rayon parallel iterators are only implemented for `H = u64` (can be
  generalised later if needed)

---

## DEC-027: Structural-sharing-preserving serialisation — serde pool design {#sec:dec-027}

**Date:** 2026-04-25
**Status:** Accepted (implemented — HashMap, OrdMap, OrdSet, Vector; see DEC-029 for extension)

**Context:**
Current serde impls serialise collections as flat sequences/maps, discarding
internal tree structure. Two HashMaps sharing 99% of their nodes via structural
sharing serialise as two independent maps — doubling size on disk and losing
sharing on deserialisation. Item 6.6 in the impl plan.

Research investigated three approaches: rkyv (zero-copy framework with built-in
Arc dedup), immer's `persist.hpp` (pool-based, C++), and IPLD/DAG-CBOR
(content-addressed).

**Decision:**
Serde-based pool serialisation with InternPool integration on deserialisation.

### Architecture

**New feature flag:** `persist` (requires `std`, `serde`, `hash-intern`)

**Two serialisation modes per collection:**

| Mode | Serialise as | Use case |
|------|-------------|----------|
| Flat (existing) | `[k, v, k, v, ...]` | Interop, human-readable, small collections |
| Pool (new) | Node pool + root ID | Preserving sharing, checkpointing, undo history |

**Scope:** All 11 collection types.

### Serialisation (write path)

Walk the node graph depth-first. Maintain a pointer-identity registry
(`HashMap<usize, NodeId>` keyed by `SharedPointer::as_ptr() as usize`).

1. For each node encountered, check the registry by pointer address.
2. If already seen → emit the existing `NodeId` as a reference.
3. If new → assign the next sequential `NodeId`, serialise the node's
   content (with child references as `NodeId`s), add to registry.
4. Output: a flat array of serialised nodes (the "pool") plus container
   metadata (root `NodeId`, size, hasher state).

The InternPool is NOT needed during serialisation — pointer identity is
sufficient and cheaper than Merkle-hash lookup.

**Per-type node pools** (following immer): all collections of the same
backing type share a pool. A `PoolBuilder` accumulates nodes from
multiple collections before emitting.

### Deserialisation (read path) — hash consing on the fly

Reconstruct nodes bottom-up (leaves first, then parents). As each node
is reconstructed:

1. Compute its Merkle hash (inherent — nodes already maintain
   `merkle_hash`).
2. Check the `InternPool` for a node with matching Merkle hash +
   structural equality.
3. If found → discard the newly constructed node, use the existing
   `SharedPointer` from the pool. This is hash consing during
   deserialisation.
4. If not found → store in the InternPool and use the new pointer.

This gives **cross-session deduplication for free**: if the same subtree
was already in memory (from a previously deserialised or constructed
collection), the deserialised version shares the existing allocation.

### Format (serde)

```json
{
  "pool": {
    "hamt": [
      {"id": 0, "merkle": 12345, "entries": [
        {"value": [key, val], "hash": 67890},
        {"node": 1},
        {"simd_small": 3}
      ]},
      ...
    ],
    "simd_small": [...],
    "simd_large": [...],
    "collision": [...],
    "btree_branch": [...],
    "btree_leaf": [...],
    "rrb_inner": [...],
    "rrb_leaf": [...]
  },
  "containers": [
    {"type": "hashmap", "root": 0, "size": 1000, "hasher_id": 42}
  ]
}
```

Node type pools are flat arrays. Child references are `NodeId` integers
pointing into the same pool. Serde handles the actual format encoding
(JSON, bincode, MessagePack, etc.).

### Collection-specific details

| Backing structure | Node types in pool | Merkle hash available | Notes |
|-------------------|-------------------|----------------------|-------|
| HAMT (HashMap, HashSet, HashMultiMap, InsertionOrderMap, BiMap, SymMap, Trie) | HamtNode, SmallSimdNode, LargeSimdNode, CollisionNode | Yes (eagerly maintained) | Full interning on deserialise |
| B+ tree (OrdMap, OrdSet) | Branch, Leaf | No | Reconstruct without interning; structural sharing still preserved within a single pool |
| RRB tree (Vector) | Inner, Leaf chunk | Yes (lazy AtomicU64) | Intern on deserialise where Merkle is computed |

**B+ tree note:** OrdMap/OrdSet nodes lack Merkle hashes, so cross-session
dedup via InternPool is not available. Within a single pool, pointer-based
dedup during serialisation still preserves sharing. Merkle hashes could be
added to B+ tree nodes in future if interning proves valuable.

**Compound types:** Bag wraps HashMap (serialise inner map with pool).
BiMap/SymMap contain two HashMaps (both feed into the same HAMT pool —
shared subtrees between forward/backward maps are deduplicated).
InsertionOrderMap contains a HashMap + OrdMap (separate pools per backing
type). Trie is recursive HashMap (single HAMT pool).

### API surface

```rust
// Pool builder — accumulates nodes from multiple collections
pub struct PoolBuilder<P: SharedPointerKind = DefaultSharedPtr, H: HashWidth = u64> {
    // Internal: per-type node registries keyed by pointer address
}

impl<P, H> PoolBuilder<P, H> {
    pub fn new() -> Self;
    pub fn add_hashmap<K, V, S>(&mut self, map: &GenericHashMap<K, V, S, P, H>);
    pub fn add_hashset<A, S>(&mut self, set: &GenericHashSet<A, S, P, H>);
    pub fn add_ordmap<K, V>(&mut self, map: &GenericOrdMap<K, V, P>);
    pub fn add_ordset<A>(&mut self, set: &GenericOrdSet<A, P>);
    pub fn add_vector<A>(&mut self, vec: &GenericVector<A, P>);
    // Convenience: add any collection
    pub fn add<T: Persist<P, H>>(&mut self, collection: &T);
}

// Serialise pool + containers
impl<P, H> Serialize for PoolBuilder<P, H> { ... }

// Pool reader — deserialises with interning
pub struct PoolReader<P: SharedPointerKind = DefaultSharedPtr, H: HashWidth = u64> {
    pool: InternPool<...>,  // used for cross-session dedup
    // Internal: deserialised node arrays
}

impl<P, H> PoolReader<P, H> {
    pub fn from_pool(pool: InternPool<...>) -> Self;
    pub fn read_hashmap<K, V, S>(&mut self) -> GenericHashMap<K, V, S, P, H>;
    pub fn read_vector<A>(&mut self) -> GenericVector<A, P>;
    // etc.
}
```

### Why not rkyv

rkyv (v0.8) is a zero-copy deserialisation framework with built-in Arc
dedup via `Sharing`/`Pooling` traits. It was investigated but rejected:

- **Separate ecosystem.** rkyv uses its own derive macros (`Archive`,
  `rkyv::Serialize`, `rkyv::Deserialize`) incompatible with serde.
  Supporting both would double the serialisation surface area.
- **Orphan rule.** Cannot impl `Archive` for `SharedPointer<T, P>`
  directly — both types are foreign. Requires newtype wrappers or
  rkyv's `With` adapter, adding friction throughout the node types.
- **Dedup is address-based only.** rkyv's `Share` strategy deduplicates
  by pointer address within a single serialisation pass. It does not do
  content-based dedup. Cross-session dedup (the main value of combining
  with InternPool) still requires the same Merkle-hash-based interning
  we'd build anyway.
- **serde already covers pds's needs.** All 11 types have serde impls.
  The pool format can be expressed in any serde-compatible format (JSON,
  bincode, CBOR, MessagePack). Adding rkyv brings marginal benefit
  (zero-copy reads) at high ecosystem cost.

rkyv remains a future option if zero-copy access to archived collections
becomes a requirement. The pool design is format-agnostic — the node ID
approach works with any serialisation backend.

### Why not content-addressed (IPLD/DAG-CBOR)

Content-addressed serialisation (CID = hash of serialised bytes) gives
global dedup and integrity verification but:

- **36+ bytes per reference** (CID) vs 4 bytes (integer NodeId)
- **Requires canonical serialisation** (same content must produce same
  bytes) — the HAMT layout depends on hasher state, which is
  session-local with `RandomState`
- **Cryptographic hashing cost** at every node
- **Overkill for local persistence** — cross-session dedup via Merkle
  hash in the InternPool is sufficient without full content-addressing

Content-addressed serialisation is the right choice for distributed
systems. pds targets local persistence and checkpointing.

**Consequences:**
- New `persist` feature flag (depends on `std`, `serde`, `hash-intern`)
- New module `src/persist.rs` (or `src/persist/` directory)
- PoolBuilder/PoolReader API — explicit, not automatic
- Flat serde impls remain the default; pool serialisation is opt-in
- B+ tree types get pool serialisation (preserving within-session sharing)
  but not cross-session interning (no Merkle hashes)
- InternPool gains a clear second use case beyond manual `map.intern()`

---

## DEC-028: Parallel bulk ops — filter+fold pattern, not tree-level merge {#sec:dec-028}

**Date:** 2026-04-25
**Status:** Accepted (implemented)

**Context:**
Parallel union/intersection/difference/symmetric_difference for HashMap and
HashSet (item 3.4 residual). Two design approaches: (a) tree-level HAMT merge
operating on subtrees in parallel, or (b) element-level filter_map + fold/reduce
over rayon's `par_iter()`.

**Decision:**
Element-level filter_map + fold/reduce.

Each parallel op filters one collection's elements against the other (using
`contains_key`/`contains`), folds matching elements into thread-local partial
maps, then reduces via sequential `union`. `par_symmetric_difference` uses
`rayon::join` to run both halves (self−other and other−self) concurrently.

HashMap methods use `insert_invalidate_kv` (pub(crate)) to avoid requiring
`V: Hash` — the kv_merkle hash is invalidated but not recomputed, keeping
trait bounds consistent with the sequential API. The collection-level Merkle
hash is rebuilt lazily on next access.

**Alternatives considered:**
- Tree-level HAMT merge — rejected: complex (needs subtree-parallel recursive
  descent into node types), difficult to test incrementally, and the element-
  level approach already saturates cores for collections above ~10K elements
  where parallelism matters.

**Consequences:**
- 8 new public methods (4 on HashMap, 4 on HashSet)
- Trait bounds require `Send + Sync` on key/value types (standard for rayon)
- Performance: parallel overhead makes these slower than sequential below ~5K
  elements; above ~50K they scale well. Users should benchmark their workloads.
- `insert_invalidate_kv` remains pub(crate) — it's an internal efficiency
  mechanism, not a public API commitment.

---

## DEC-029: SSP serialisation extension — OrdMap node pooling, Vector flat {#sec:dec-029}

**Date:** 2026-04-25
**Status:** Accepted (implemented)

**Context:**
DEC-027 designed pool-based serialisation for HashMap. Extending to OrdMap,
OrdSet, and Vector (item 6.6 extension).

**Decision:**

**OrdMap/OrdSet:** Full B+ tree node-level pooling. `OrdMapPool<K, V>` walks
the B+ tree depth-first, deduplicating nodes by pointer address (same pattern
as `HashMapPool`). Three node variants: `BranchLeaves` (branch whose children
are leaves), `BranchBranches` (branch whose children are branches), and `Leaf`.
Deserialisation extracts all leaf key-value pairs and rebuilds via
`FromIterator`, which produces a balanced B+ tree. Structural sharing within
a pool is preserved during serialisation; cross-session interning is not
available (B+ tree nodes lack Merkle hashes, as noted in DEC-027).

`OrdSetPool<A>` is a type alias for `OrdMapPool<A, ()>` with convenience
methods (`from_sets`, `to_sets`), avoiding a separate struct.

**Vector:** Flat element-level serialisation. `VectorPool<A>` stores each
vector as a `Vec<A>` of its elements. RRB node-level pooling was investigated
but deferred: `VectorInner` and `RRB` fields are private to `vector/mod.rs`,
so accessing them from `persist.rs` would require making them `pub(crate)` or
adding accessor methods — a larger refactor than justified for v1.

**Alternatives considered:**
- RRB node pooling for Vector — deferred due to visibility constraints (see
  above). Can be added later if Vector sharing preservation becomes important.
- Separate `OrdSetPool` struct — rejected: would duplicate `OrdMapPool` logic
  since OrdSet is backed by `OrdMap<A, ()>`.

**Consequences:**
- Three new public types: `OrdMapPool<K, V>`, `OrdSetPool<A>` (alias),
  `VectorPool<A>`
- All behind `persist` feature flag
- Vector pools do not preserve structural sharing — they round-trip through
  flat element arrays. This is functionally correct but loses RRB tree sharing.
- OrdMap pools preserve within-pool sharing but not cross-session (no Merkle
  hashes on B+ tree nodes)

---

## DEC-030: `IndexMut` omitted for BiMap, SymMap, HashMultiMap {#sec:dec-030}

**Date:** 2026-04-25
**Status:** Accepted

**Context:**
The standard trait table in `directives.md` requires `Index`/`IndexMut` for all keyed
map types. All five types that were missing `Index` have now had it added. However,
`IndexMut` cannot be safely provided for three of them without breaking internal
invariants.

**Decision:**
`IndexMut` is not implemented for `BiMap`, `SymMap`, or `HashMultiMap`.

- **BiMap**: stores a forward map (K→V) and a backward map (V→K). Returning `&mut V`
  would let callers change the value without updating the backward map, silently
  invalidating the bijection invariant.
- **SymMap**: same structural argument — forward and backward maps must stay in sync;
  mutating one side via a raw `&mut A` would break symmetry.
- **HashMultiMap**: internally stores `HashMap<K, HashSet<V>>` and a `total: usize`
  element count. Returning `&mut HashSet<V>` would let callers add or remove elements
  from the set without updating `total`, silently corrupting `len()`.

**Alternatives considered:**
- Implement `IndexMut` and document the footgun — rejected because silently-inconsistent
  data structures are worse than a missing impl. Callers that need mutation can use the
  provided `insert`/`remove` methods which maintain invariants.
- Add a `entry()`-style API that validates the mutation — future work; not blocked on this.

**Consequences:**
Minor deviation from the `Y` cell in the directive table. The `Index` (read-only) impl
is present for all three types. The omission is documented here and in the doc comment
on each `Index` impl.

---

## DEC-031: Bag implements `Add` and `Sum` despite directive table marking them n/a {#sec:dec-031}

**Date:** 2026-04-25
**Status:** Superseded by DEC-032

**Context:**
The standard trait table marks `Add` (union) and `Sum` as "n/a" for Bag types. The
existing `Bag` implementation provides both.

**Decision:**
Keep `Add` and `Sum` on `Bag`. The "n/a" designation was a conservative default based
on the original directive not anticipating multiset-union semantics. `Bag::add` performs
multiset union (counts are summed per element), which is a well-defined, useful, and
mathematically sound operation. `Sum` is the natural fold over `Add`.

**Alternatives considered:**
- Remove the impls to comply strictly with the directive table — rejected because the
  implementations are correct and useful; removing them would be a breaking change with
  no benefit.

**Consequences:**
`Bag<T>` is slightly richer than the trait table prescribes. The directive table should
be read as a minimum obligation, not a ceiling.

---

## DEC-032 — Remove Add/Mul/Sum from all collection types (except Vector)

**Status:** Accepted

**Context:**
All collection types inherited `Add` (for union), `Mul` (for intersection on sets), and
`Sum` from imbl. These use arithmetic operator overloading to express set operations:
`a + b` for union, `a * b` for intersection.

**Decision:**
Remove `Add`, `Mul`, and `Sum` from all collection types. Keep `Add` on `Vector`
(concatenation, analogous to `String + &str`).

**Alternatives considered:**
- Keep the operators as a convenience shorthand — rejected because `+` for union is not
  idiomatic Rust. The standard library's `HashMap`, `HashSet`, `BTreeMap`, and
  `BTreeSet` do not implement these operators. Users expect arithmetic operators to mean
  arithmetic operations. Using them for set semantics is surprising and misleading.

**Consequences:**
- `a + b` no longer compiles for map/set/bag types. Use `a.union(b)` instead.
- `Iterator::sum()` over a collection of maps/sets no longer works. Use
  `.reduce(|a, b| a.union(b))` or `fold(Default::default(), |a, b| a.union(b))`.
- All collections retain their named `union()`, `difference()`,
  `intersection()`, and (where applicable) `symmetric_difference()` methods.

---

## DEC-033: Rayon scope for newer collection types {#sec:dec-033}

**Date:** 2026-04-26
**Status:** Accepted (R.2)

**Context:**
R.2 added rayon support to Bag, HashMultiMap, BiMap, SymMap, InsertionOrderMap, and
InsertionOrderSet. Several non-obvious design choices were required.

**Decision:**

1. **InsertionOrderMap / InsertionOrderSet — read-only `par_iter` only.**
   `FromParallelIterator` and `ParallelExtend` are intentionally omitted: parallel
   collection fans out across threads with no ordering guarantee, so the resulting
   collection would have an arbitrary insertion order. The sequential `FromIterator`
   and `Extend` impls must be used when insertion order matters.

2. **Trie excluded from rayon entirely.**
   The trie's branching factor and depth are key/path-dependent, making uniform
   work distribution difficult. No rayon impl added; not advertised in feature docs.

3. **BiMap / SymMap / HashMultiMap — `par_iter` delegates to the underlying
   `GenericHashMap` forward map.** These types store their data in a
   `GenericHashMap<K, V, S, P>` (default `H = u64`). The existing `hash/rayon.rs`
   only covers the default `H = u64` case (see DEC-024 for why full H-threading
   is deferred). Consequently, `par_iter`, `FromParallelIterator`, and
   `ParallelExtend` for these types only work with the default H. All user-facing
   type aliases (`BiMap`, `SymMap`, `HashMultiMap`) use this default, so no
   practical limitation exists for downstream consumers.

4. **Bag provides two parallel iterators.**
   `par_iter()` → `(&A, usize)` pairs (matches sequential `iter()` signature).
   `par_elements()` → flat expansion: each element is yielded once per occurrence
   (equivalent to sequential `elements()`). Implemented via `repeat_n(a, count)`.

**Alternatives considered:**
- Full H-threading in `hash/rayon.rs` for BiMap/SymMap/HashMultiMap: rejected —
  requires threading H through 12 internal HAMT iteration types (Entry, Node,
  MapIterFrame, SetIterFrame variants). Significant refactoring for zero practical
  benefit since all public APIs use the default H = u64.

**Consequences:**
- Users who construct `GenericBiMap<K, V, S, P, MyHashWidth>` with a non-default H
  will not get `par_iter`. Acceptable given the target audience uses type aliases.

---

## DEC-034: Parallel transform operations (par_filter, par_map_values) use iterator interface, not direct tree manipulation {#sec:dec-034}

**Date:** 2026-04-26
**Status:** Partially superseded by DEC-035 — `par_map_values` / `par_map_values_with_key` on `HashMap` and `OrdMap` have been upgraded to tree-native implementations. `par_filter` remains collect-based (topology changes).

**Context:**
R.2 added `par_filter`, `par_map_values`, and `par_map_values_with_key` to
`HashMap`, `OrdMap`, `HashSet`, and `OrdSet`. These are convenience wrappers
around `par_iter().filter/map().collect()`.

**Decision:**
Implement via the iterator interface rather than direct tree manipulation.

The current implementations are essentially:

```rust
// par_filter on HashMap
self.par_iter().filter(|(k,v)| f(*k,*v)).map(|(k,v)| (k.clone(),v.clone())).collect()
// par_map_values on HashMap
self.par_iter().map(|(k,v)| (k.clone(), f(v))).collect()
```

This is identical to what a user could write themselves. The methods provide
convenience and an optimization seam for future improvement.

**Why not tree-aware parallel reconstruction now?**
A tree-aware version of `par_map_values` could split the HAMT root into N
subtrees, rebuild each subtree in parallel (trivial for values-only changes
since tree topology is preserved), then reassemble. This would be O(n/p) end-
to-end vs the current O(n/p + n log n) (parallel scan + sequential
FromParallelIterator rebuild, which inserts one key at a time).

This optimization is deferred because:
1. The collect-based rebuild (`FromParallelIterator`) requires touching
   all n entries regardless — the bottleneck is allocation, not compute
2. Implementing a parallel HAMT walker that produces new nodes without going
   through the insert path is non-trivial and requires HAMT internals exposure
3. The methods can be transparently optimized later without API changes

**Alternatives considered:**
- HAMT-native parallel map: valid future optimization, especially for
  `par_map_values` where tree shape is preserved. Filed as a future item
  in `docs/impl-plan.md`.
- Omit the methods entirely: rejected — the API is useful and the seam
  allows optimization later.

**Consequences:**
- `par_filter` and `par_map_values` offer no algorithmic advantage over
  `par_iter().filter().collect()` for advanced users. Documented in the
  method doc comments.
- The consistent naming across all collection types is itself valuable
  for discoverability.

---

## DEC-035: Tree-native par_map_values on HashMap and OrdMap {#sec:dec-035}

**Date:** 2026-04-26
**Status:** Accepted

**Context:**
R.10 identified that `par_map_values` / `par_map_values_with_key` on `HashMap`
and `OrdMap` could be significantly faster by walking the internal tree directly
rather than going through `par_iter().map().collect()`. The collect path inserts
each entry one by one — O(n log n) total — even though key order and tree shape
are completely unchanged by a value-only transformation.

**Decision:**
Implement tree-native `par_map_values` and `par_map_values_with_key` for both
`HashMap` (HAMT) and `OrdMap` (B+ tree) that walk and reconstruct the tree without
going through the insert path.

For **HashMap / HAMT**:
- Added `map_values_hamt_node_par`, `map_values_hamt_node_seq`, `map_values_entry`,
  `map_values_simd`, and `map_values_collision` helpers in `src/hash/rayon.rs`.
- The root HAMT node's `SparseChunk` entries are processed in parallel via rayon,
  with each entry's position (`SparseChunk::entries()`) preserved verbatim.
- Key-hash Merkle values (`merkle_hash` in `GenericSimdNode`) are copied unchanged —
  they depend only on key hashes, not values.
- The KV Merkle (`kv_merkle_valid`) is marked stale since value hashes may differ.
- Required adding `GenericSimdNode::map_values()` (gated by `cfg(any(test, feature="rayon"))`)
  to access the private `control` field internally.
- Output `GenericHashMap` is constructed directly (bypassing insert machinery), with
  `hasher_id` refreshed via `next_hasher_id()` to invalidate any cached derivations.

For **OrdMap / B+ tree**:
- Added `par_map_values_ord_node` helper in `src/ord/rayon.rs`.
- Branch separator keys are cloned unchanged; leaf `(K, V)` pairs at each
  `Children::Leaves` or `Children::Branches` level are processed in parallel.
- Rayon forks at the top-level children only; deeper recursion is sequential
  (tree depth is typically 2–4, so the fork overhead outweighs the gain at depth).
- Required adding `Branch::map_values`, `Leaf::map_values`, and `Node::map_values`
  (all gated by `cfg(any(test, feature="rayon"))`) in `src/nodes/btree.rs`.
- Output `GenericOrdMap` is constructed directly (`{ root, size }`).

**Unification:** All helpers use `F: Fn(&K, &V) -> V2` uniformly. `par_map_values`
adapts at the call site via `|_k, v| f(v)` to avoid duplicating the helper tree.

**Alternatives considered:**
- Keep collect-based approach (DEC-034): O(n/p + n log n). Rejected — O(n log n)
  rebuild is unnecessary overhead for value-only transforms on large maps.
- Gate the new helpers behind a cfg flag: rejected — the `any(test, feature="rayon")`
  gate already ensures they're absent in non-rayon, non-test builds.

**Consequences:**
- `par_map_values` and `par_map_values_with_key` are now **O(n/p)** on `HashMap`
  and `OrdMap` rather than O(n/p + n log n).
- `par_filter` remains collect-based (topology changes require re-insertion).
- The lib.rs `## Parallel operations` section now distinguishes "implementation-
  optimised" from "convenience" methods.
- `V: Hash` bound is NOT required on the optimised `par_map_values` impl block
  (unlike `par_filter` which needs it for `FromParallelIterator`).

---

## DEC-036: `ord-hash` content hash for OrdMap/OrdSet — default-on {#sec:dec-036}

**Date:** 2026-04-26
**Status:** Accepted

**Context:**
R.14 — add a cached content hash to `GenericOrdMap`/`GenericOrdSet` to enable O(1)
`PartialEq` and a `Hash` impl (when `K: Hash, V: Hash`). The last meaningful reason
to prefer `HashMap` over `OrdMap` — the HAMT's `kv_merkle_hash` O(1) equality — is
now matched.

**Design:**

- `content_hash_cache: AtomicU64` added to `GenericOrdMap`. Sentinel `0` = not cached;
  computed hash of `0` stored as `1`. `AtomicU64` with `Relaxed` ordering chosen over
  `Cell<u64>` (the original plan's design) because `Cell<T>` is `!Sync`, which would
  have broken rayon's `par_iter()`. On arm64, `Relaxed` atomics are single LDR/STR
  instructions — identical cost to a plain load/store.

- **Invalidation:** every `&mut self` mutation site calls `invalidate_hash_cache()`
  (sets cache to 0): `clear()`, `insert_key_value()`, `remove_with_key()`, `get_mut()`,
  `iter_mut()`. Clone preserves the cached value.

- **Hash scheme:** XOR of `DefaultHasher::new()` applied to `(k, v)` per entry.
  XOR is order-independent; `DefaultHasher` uses deterministic fixed keys
  (`k0 = 0, k1 = 0`), so identical-content maps produce identical hashes in the
  same binary.

- **`PartialEq` — positive and negative fast-path:** when both caches are populated
  (non-zero), `eq` returns directly: `h1 == h2` (positive equality O(1)) or `false`
  (negative). Falls through to `diff()` O(n) scan only when either cache is cold.

- **`Hash` impl:** gated on `K: Hash + V: Hash`; delegates to `content_hash()`.

**Benchmark results (criterion, Apple M5 Max, single suite, default features):**

| Workload | i64 100 | i64 1K | i64 10K | str all sizes |
|----------|---------|--------|---------|---------------|
| insert_mut overhead | +7.7% | ~+5% | asymptoting to 0 | ≤ noise |
| lookup | 0% | 0% | 0% | 0% |
| remove_mut | similar to insert_mut | | | |

The +7.7% at 100-entry i64 maps represents ~0.8 ns per insert — one `STR` instruction.
For str-key workloads the overhead is within measurement noise at all sizes. Overhead
asymptotes to zero as map size grows (amortised over O(log n) work per insert).

**Decision:**
Default-on. Feature flag `ord-hash` added to `default = [...]` in `Cargo.toml`. The
overhead is unmeasurable for typical workloads; the benefit (O(1) equality when caches
are warm, `Hash` impl) is substantial.

**Alternatives considered:**
- `Cell<u64>` + `Cell<bool>` (original plan design) — rejected because `Cell<T>` is
  `!Sync`, breaking rayon `par_iter()` which requires `Self: Sync`. `AtomicU64` solves
  this with identical performance on arm64.
- Negative-only fast-path (current plan design) — superseded: positive equality is also
  safe because `DefaultHasher` is deterministic (fixed keys). We get full O(1) PartialEq
  when both caches are warm, not just negative short-circuit.
- u128 hash — `AtomicU128` is not available in `std`. The 2^-64 collision probability
  (~5×10^-20 per comparison) is already astronomically negligible. Not needed.
- Per-node hash caching — would enable O(log n) hash on partial updates but requires
  structural changes to `Branch`/`Leaf` nodes; deferred to a future item.

**Consequences:**
- One `AtomicU64` added to `GenericOrdMap` (root struct, not to B+ tree nodes).
  `ORD_CHUNK_SIZE` (DEC-017) is unaffected — it governs `Branch`/`Leaf` sizing only.
- `OrdMap<K, V>` / `OrdSet<A>` are now usable as `HashMap` keys when `K: Hash + Ord`.
- `PartialEq` is O(1) when both maps have cached hashes; O(n) otherwise (same as before).
- Feature can be disabled: `default-features = false` or explicit `--no-default-features`.

---

## DEC-037: Performance review — April 2026 (implemented and rejected changes)

**Date:** 2026-04-27
**Status:** Accepted

**Context:**
Detailed performance review of `nodes/hamt.rs`, `nodes/btree.rs`, `hash/map.rs`,
`nodes/rrb.rs`, `vector/mod.rs`, and `ord/map.rs`. Two optimisations were implemented
and four candidates were evaluated and rejected.

### Implemented

**1. `OrdMap::PartialEq` — `ptr_eq` fast path**

`GenericOrdMap::PartialEq::eq` was missing the `ptr_eq` check that `HashMap` already
had. Fresh clones have `content_hash_cache = 0` (uncached), so the hash fast-path was
cold and the comparison fell through to O(n) `diff()` scan.

Fix: check `self.ptr_eq(other)` before the content hash check. Structurally-shared
clones compare in O(1) regardless of cache state.

Benchmark (`eq_clone` in `benches/ordmap.rs`): 1.15–1.16 ns flat from 1K to 100K
entries (was O(n) before). Recorded in `docs/baselines.md`.

**2. `HashMap::insert` — deferred `value_hash` computation**

`insert` was computing `value_hash = self.hasher.hash_one(&v)` unconditionally at the
start of the function, before moving `v` into `root.insert`. The hash was only used
inside `if self.kv_merkle_valid` branches. The wasted computation on every insert when
`kv_merkle_valid = false` was the cost.

Fix: restructured `insert` into two branches: when `kv_merkle_valid` is true, compute
`value_hash` before the move and use it; when false, skip the hash entirely. The
pre-move requirement (v is moved into root.insert) forced the restructuring.

### Rejected

**3. SIMD control byte `to_array()` + scalar replace + `SimdGroup::from()`**

Pattern: `ctrl_array = control.to_array(); ctrl_array[offset] = new_byte; control =
SimdGroup::from(ctrl_array)`. Looked expensive but compiles to 3 cheap instructions
on arm64 (UMOV, MOV byte into register, INS). The `wide::u8x16` API has no
`replace_lane()` method. No improvement possible without changing the SIMD library.

**4. `Cursor<Vec<...>>` stack allocation per iteration**

The B+ tree `Cursor` uses a `Vec` for its stack of `(index, branch_ref)` pairs.
The alloc is amortised over the entire bulk iteration (one `Vec` per traversal, not
per element) and is negligible compared to B+ tree node access cost. Converting to
a fixed-size inline array would risk stack overflow for very deep trees and require
a const-generic bound on tree depth. Not worth the complexity.

**5. `ConsumingIter` `make_mut` per leaf**

`ConsumingIter` calls `SharedPointer::make_mut` to get mutable access to each leaf
before draining it. On arm64, when refcount == 1 (the common case for a consuming
iter), `Arc::make_mut` is a single relaxed load + branch-not-taken (no clone). The
cost is effectively free. Adding unsafe `Arc::get_mut`-first logic would only help
when refcount > 1, which is the rare case in a consuming iterator.

**6. `LargeSimdNode` upgrade: batch control byte initialisation**

When `SmallSimdNode` promotes to `LargeSimdNode`, the second SIMD group's control
bytes are initialised to zero in a loop. This is a cold promotion path (one per
node upgrade, amortised over many inserts) and the initialisation is a single
`SimdGroup::default()` assignment — one instruction. Not measurable.

**Alternatives considered:**
- Unsafe `replace_lane` for the SIMD control byte — `wide` has no such API; would
  require forking the `wide` crate. Not worth it for 3 already-cheap instructions.
- Inline array for Cursor stack — risk of stack overflow for deep trees; const-generic
  depth bound adds API complexity disproportionate to the gain.

**Consequences:**
- `OrdMap::PartialEq` is now O(1) for structurally-shared clones.
- `HashMap::insert` skips `hash_one` when `kv_merkle_valid = false`.
- All other performance review candidates are confirmed not worth changing.
