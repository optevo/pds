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
- The prototype (`src/champ.rs`) is retained for future reference and
  benchmarking.
- The CHAMP iteration advantage (36-44%) motivates investigating whether
  the SIMD HAMT's iteration can be improved independently (the current
  3-tier node hierarchy fragments iteration across node types).
- Future work: explore a hybrid SIMD-CHAMP that uses CHAMP's two-bitmap
  layout for memory density and iteration but adds SIMD control groups
  for lookup acceleration.

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
- The `champ` module is gated behind `std` (uses `RandomState` internally).
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
- The `champ` module could be unblocked for no_std by accepting generic
  `BuildHasher` (future work).
