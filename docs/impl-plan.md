# imbl — Implementation Plan

Sequenced implementation plan for improvements to the
[imbl](https://github.com/jneem/imbl) Rust crate (persistent/immutable
collections with structural sharing).

**Current state (Apr 2026):** v7.0.0, ~12K lines of Rust, 5 core types
(Vector, HashMap, HashSet, OrdMap, OrdSet). Maintained reactively by jneem —
explicitly welcoming PRs but not driving a roadmap. Used in production by
Matrix SDK, Fedimint, ~280 downstream crates.

---

## Principles

### Upstream-first

This is a fork of jneem/imbl. Every change should be structured as an
independent, upstreamable PR: small, focused, well-tested, with a clear
commit message. Avoid coupling unrelated changes. Breaking changes need
strong justification. Batch breaking changes into a single major version
bump (v8.0.0) to avoid churn for downstream users.

### Document as you go

The codebase has a ~4% comment ratio. Rather than a standalone documentation
pass, every PR that touches internal code must document what it modifies:
architecture decisions, invariants, algorithmic complexity, and `// SAFETY:`
comments for any unsafe block. By the time all phases are complete, the
documentation will be comprehensive as a natural byproduct.

### Measure before and after

No optimisation lands without benchmarks proving the improvement and no
structural change lands without fuzz/miri validation. Preparation steps
(benchmarks, fuzz targets, miri) are first-class work items, not afterthoughts.

### Semver discipline

Items are grouped by semver impact. Non-breaking changes can ship as v7.x
point releases. Breaking changes (5.1, 5.2, 5.3, 5.4) are batched into a
single v8.0.0 release in Phase 5.

---

## Contents

- [Done](#done)
- [Current](#current)
- [Future](#future)
  - [Phase 0 — Foundations](#phase-0)
  - [Phase 1 — Housekeeping](#phase-1)
  - [Phase 2 — Correctness fixes & quick API wins](#phase-2)
  - [Phase 3 — Mutation & parallel performance](#phase-3)
  - [Phase 4 — Data structure internals](#phase-4)
  - [Phase 5 — Breaking API changes (v8.0.0)](#phase-5)
  - [Phase 6 — Research & speculative](#phase-6)
- [Dependency map](#dependency-map)
- [References](#references)

---

## Done {#done}

*Newest first.*

- **[2026-04-24] 5.1: Default to triomphe::Arc.**
  Added `triomphe` to default features. All collections now use
  `triomphe::Arc` (no weak reference count) internally — saves 8 bytes
  per node, eliminates one atomic RMW per clone/drop. String-key hashmap
  ops improved 2-9%, integer-key ops mixed at 10K but consistent
  improvement at 100K. Users needing `Arc::downgrade` can opt out with
  `default-features = false`. See DEC-010.

- **[2026-04-24] 4.4: Merkle hash caching — accepted, always-on.**
  Each HAMT node stores a u64 merkle_hash maintained incrementally during
  mutations. Root hash is the sum of mixer(key_hash) across all entries
  (wyhash wide-multiply mixer). Equality check gains O(1) negative fast
  path (different root hashes → definitely unequal). Final overhead:
  effectively zero (-1.7% lookup, -8.7% insert_mut, +1.4% remove_mut vs
  pre-merkle baseline — all within noise or improved). Always-on, no
  feature flag. See DEC-009.

- **[2026-04-24] 3.3: Transient/builder API — resolved as already handled.**
  Existing `&mut self` methods already provide the builder pattern's core
  benefit: `Arc::make_mut` detects refcount == 1 and mutates in place
  without cloning (8-14× faster than persistent methods at 100K elements).
  A dedicated builder would only eliminate per-node atomic CAS overhead
  (~20-30%) but requires ~5000 lines of parallel node types. The Rust
  idiom of taking ownership (`let mut map = map; map.insert(...)`) is the
  correct pattern. See DEC-008.

- **[2026-04-24] 4.2: CHAMP prototype benchmark.** Built a standalone
  CHAMP implementation (`src/champ.rs`): two-bitmap encoding
  (datamap + nodemap), contiguous value/child arrays, canonical deletion,
  Arc-based structural sharing. Benchmarked against the SIMD HAMT.
  Results: CHAMP is 26-41% faster for persistent insert/remove and
  36-44% faster for iteration (contiguous value arrays), but 10-64%
  slower for lookups (popcount vs SIMD parallel probe). Decision
  (DEC-007): do not proceed to 4.3 — the lookup regression is too
  large for a general-purpose library. The SIMD HAMT remains. The
  prototype is retained for future hybrid-SIMD-CHAMP research.

- **[2026-04-24] 4.5: SharedPointer-wrapped hasher.** Wrapped the hasher
  in `SharedPointer<S, P>` in both `GenericHashMap` and `GenericHashSet`.
  Cloning the map now bumps a refcount instead of cloning the hasher,
  eliminating `S: Clone` from the entire HashMap/HashSet API (~50 bounds
  removed). Benchmark results: 3-5% regression on i64 lookups (where hash
  time ~2ns makes the pointer deref proportionally visible), 0-2% for
  string keys and mutations (hash time dominates). Decision: keep the
  change — the regression is confined to the narrowest case, the API
  simplification cascades to all downstream consumers, and sharing the
  hasher aligns with the library's structural sharing philosophy.

- **[2026-04-24] 5.2: Remove unnecessary Clone bounds.** Audited Clone
  dependencies across HashMap, HashSet, OrdMap, and OrdSet. Split impl
  blocks by actual Clone requirements. HashMap: removed `S: Clone` from
  30+ methods that never clone the hasher — read-only block (`get`,
  `contains_key`, `is_submap`, `diff`, etc.), mutating-no-S-clone block
  (`insert`, `remove`, `retain`, `iter_mut`, `get_mut`), `FromIterator`,
  `PartialEq`/`Eq`, disjoint. Methods that genuinely clone self/hasher
  (`update`, `without`, `entry`, `union`, `intersection`, etc.) retain
  `S: Clone`. HashSet: same split — `insert`, `remove`, `retain`,
  `partition`, `union`, `unions`, `symmetric_difference`,
  `relative_complement` no longer need `S: Clone`. OrdMap: moved
  `partition_map` from `K+V: Clone` to `K: Clone` block (only borrows V);
  `map_values`, `map_values_with_key`, `try_map_values`, `map_accum`
  moved to `K: Clone` block; `map_keys`, `map_keys_monotonic` moved to
  `V: Clone` block. Remaining `S: Clone` on HashMap persistent methods
  is structural — the hasher is stored bare and `self.clone()` clones it.
  See 4.5 for PoC to eliminate this.

- **[2026-04-24] 3.4 (partial): HashMap par_iter_mut + Vector par_sort.**
  Added `IntoParallelRefMutIterator` for `GenericHashMap`, enabling parallel
  mutable value iteration via `map.par_iter_mut()`. Implementation uses
  `SharedPointer::make_mut` at the root and lazily at each HAMT node during
  DFS traversal (same CoW semantics as sequential `iter_mut`). Work
  splitting follows the same `UnindexedProducer` pattern as `par_iter`,
  expanding single-child HamtNode entries for deeper parallelism. Added
  `par_sort()` and `par_sort_by()` for Vector — collects to contiguous
  buffer, sorts in parallel via rayon's `par_sort_unstable_by`, rebuilds
  the vector. Remaining 3.4 item: parallel bulk ops
  (union/intersection/difference on HashMap/HashSet).

- **[2026-04-24] 2.1: Fix RRB tree concatenation (issue #35).** Replaced
  Stucki's concatenation algorithm with L'orange's bounded-height approach.
  Key changes: `merge` now returns `(Node, level)` instead of always
  wrapping at level+1; new `concat_rebalance` collects children from the
  merge boundary, redistributes undersized nodes (flattening and repacking
  children below a minimum-size threshold based on L'orange's invariant
  m - floor(m/4)), and only increases tree height when children genuinely
  overflow NODE_SIZE. Removed dead `Entry::values()` and `Entry::nodes()`
  methods. Added `concat_depth_bounded` and `concat_depth_equal_sized`
  regression tests verifying O(log n) height for repeated concatenation.

- **[2026-04-24] 4.1: Vector prefix buffer — already implemented.**
  Investigation revealed the 4-buffer RRB structure (outer_f, inner_f,
  middle, inner_b, outer_b) already provides symmetric O(1) amortised
  push_front and push_back. Benchmarked at 100K elements: push_front
  444µs vs push_back 432µs (~3% difference). The plan description was
  based on an incorrect assumption that front buffers were absent or
  asymmetric. Scala 2.13's improvement was relative to their old
  implementation which lacked front buffers entirely — imbl already has
  them. No code changes needed.

- **[2026-04-24] 3.6: Pointer-aware subtree skipping in diff.**
  Rewrote HashMap and HashSet `DiffIter` from iterate-and-lookup to
  simultaneous HAMT tree walk. At each node, `Entry::ptr_eq` checks
  `SharedPointer` identity — shared subtrees are skipped in O(1).
  Complexity: O(changes × tree_depth) for maps sharing structure, O(n+m)
  fallback for independently-constructed maps with incompatible hashers
  (detected via sentinel probe). Added `Entry::ptr_eq()` and
  `Entry::collect_values()` to hamt.rs, made `HASH_WIDTH` pub(crate).
  `DiffIter` now implements `ExactSizeIterator`. OrdMap already has
  `advance_skipping_shared` upstream. Vector `DiffIter` rewritten from
  element-by-element `Iter` to chunk-level `Focus` comparison — at each
  position, `chunk_at` retrieves the leaf chunk and `std::ptr::eq`
  compares slice pointers to detect shared Arc-managed leaf data.
  Pointer-equal chunks are skipped in O(1) per chunk, falling back to
  element comparison within non-equal chunks. Complexity:
  O(changes × tree_depth) for structurally-shared vectors, O(n) fallback.

- **[2026-04-24] 3.2: Unsafe code audit.** Audited all unsafe sites across
  4 files. Removed 3 unsafe operations: 2 in hamt.rs (ptr::read/ptr::write →
  safe SparseChunk::remove/insert) and 1 in vector/mod.rs (ptr::swap in
  Vector::swap → safe clone-and-replace, fixing a real UB detected by miri
  where copy-on-write invalidated a held pointer). Documented 16 remaining
  unsafe sites with `// SAFETY:` comments — all retained for borrow checker
  limitations (lending iterators, get_many_mut, loop reborrow) or performance
  (branchless binary search, zero-copy node construction). Added debug_assert!
  precondition checks to Focus/FocusMut pointer dereferences. Added 25
  miri-targeted tests exercising unsafe edge cases.

- **[2026-04-24] 3.1: Arc::get_mut — already handled.** Investigation
  found that `Arc::make_mut` already checks refcount == 1 internally and
  skips cloning when sole owner. Adding `get_mut` pre-checks to 110 call
  sites would be redundant. The real sole-owner performance win requires
  explicit ownership transfer (item 3.3 Transient/Builder). See DEC-004.

- **[2026-04-24] 3.4 (partial): Parallel iterators for all four hash/ord
  types.** Added `IntoParallelRefIterator`, `FromParallelIterator`, and
  `ParallelExtend` for HashMap, HashSet, OrdMap, and OrdSet. HashMap/HashSet
  use HAMT-based `UnindexedProducer` with up to 32-way root splitting.
  OrdMap/OrdSet use B+ tree leaf flattening for work distribution. Also
  fixed all pre-existing clippy warnings (arbitrary deprecation, btree
  collapsible match, hamt unnecessary cast and enum variant names).
  Remaining 3.4 items: `par_iter_mut`, parallel bulk ops, parallel sort
  for Vector.

- **[2026-04-24] 2.11: Companion collection types.** Added four new types:
  `PBag<A>` (persistent multiset backed by HashMap<A, usize>),
  `Atom<T>` (thread-safe atomic state holder wrapping arc-swap, behind
  `atom` feature flag), `HashMultiMap<K, V>` (persistent multimap backed
  by HashMap<K, HashSet<V>>), `InsertionOrderMap<K, V>` (insertion-ordered
  map backed by HashMap<K, usize> + OrdMap<usize, (K, V)>).

- **[2026-04-24] 2.7: General merge.** Added `merge_with` to OrdMap and
  HashMap. Takes three closures (left-only, both, right-only) each
  returning `Option<V3>` — subsumes union_with, intersection_with,
  difference_with as special cases. Supports different value types on
  left and right maps. OrdMap uses sorted merge of iterators (O(n+m));
  HashMap uses iterate-left-probe-right then iterate-right-for-unseen.

- **[2026-04-24] 2.10: Vector convenience operations.** Added five methods:
  `adjust` (apply function at index returning new vector), `chunked` (split
  into fixed-size non-overlapping chunks), `patch` (replace a slice with
  another vector), `scan_left` (prefix accumulation producing n+1 elements),
  `sliding` (overlapping windows with configurable size and step). All use
  existing O(log n) operations (`set`, `split_at`, `skip`, `take`, `append`).

- **[2026-04-24] 2.8/2.9: Map/set API completeness.** Added to all relevant
  collection types: `map_values`, `map_values_with_key`, `try_map_values`,
  `map_keys` (OrdMap gets `map_keys_monotonic` with debug_assert for order
  preservation), `retain` (OrdMap/OrdSet — closes parity gap with HashMap),
  `partition`, `disjoint` (O(n+m) sorted traversal for Ord types, O(n)
  iterate-smaller-probe-larger for Hash types), `restrict_keys`/`without_keys`
  (maps), `restrict` (sets, complement to existing `difference`),
  `partition_map` (partition + transform into two differently-typed maps),
  `relative_complement_with` (asymmetric diff with per-entry resolver),
  `map_accum` (threaded accumulator through traversal with value transform).

- **[2026-04-24] 2.6: Patch/apply from diff.** Added `apply_diff()` to all
  five collection types: OrdMap, OrdSet, HashMap, HashSet, Vector. Each
  method takes any `IntoIterator<Item = DiffItem>` and produces a new
  collection with the diff applied — `Add`/`Update` insert entries,
  `Remove` removes entries. For Vector, `Remove` truncates at the given
  index (matching the diff output order where removes are always at the
  tail). All methods return a new collection (`&self -> Self`), preserving
  the original via structural sharing. Tests cover roundtrip (diff then
  apply recovers the modified collection), empty diffs, from/to empty,
  and original-preservation.

- **[2026-04-24] 2.4: HashMap/HashSet diff.** Added `diff()` to HashMap and
  HashSet, producing `DiffIter` iterators that yield `DiffItem` variants.
  HashMap diff yields `Add/Update/Remove` (matching OrdMap's API); HashSet
  diff yields `Add/Remove` (matching OrdSet's API). Implementation uses a
  two-phase iterate-and-lookup approach: phase 0 iterates the old collection
  finding Remove/Update items via lookup in the new collection, phase 1
  iterates the new collection finding Add items via lookup in the old
  collection. Includes `ptr_eq` fast path. Implements FusedIterator. The
  simultaneous trie walk with subtree skipping is deferred to 3.6.

- **[2026-04-24] 2.5: Vector diff.** Added `diff()` to Vector, producing a
  `DiffIter` that yields positional `DiffItem::{Add, Update, Remove}` items.
  Compares elements at each index; excess elements in the longer vector are
  Add or Remove. Includes `ptr_eq` fast path (shared-structure vectors
  produce empty diff in O(1)). Implements FusedIterator. Tests cover
  identical, single update, additions, removals, mixed changes, empty
  vectors, from/to empty, and fused behaviour.

- **[2026-04-24] 3.5: PartialEq ptr_eq fast paths.** Added O(1) early-exit
  to `PartialEq` for HashMap, HashSet, and Vector when two collections share
  the same root pointer. Vector uses its existing `ptr_eq()`. HashMap and
  HashSet use type-erased data pointer comparison in `test_eq()` — works
  across different `SharedPointerKind` type parameters (different pointer
  kinds can never share an allocation, so comparison correctly returns false).
  Also short-circuits on `(None, None)` roots to avoid `HashSet` allocation
  for empty collections. OrdMap and OrdSet already had this via `diff()`.

- **[2026-04-24] 2.3: OrdMap iter_mut.** Added `iter_mut()` to OrdMap,
  yielding `(&K, &mut V)` pairs in key order. Implementation uses a
  stack-based depth-first traversal with `SharedPointer::make_mut` at
  each node (copy-on-write, same pattern as HashMap's IterMut). Three
  stack item variants: LeafEntries (slice iter over kv-pairs),
  LeafChildren (slice iter over leaf pointers), BranchChildren (slice
  iter over branch pointers). Implements ExactSizeIterator and
  FusedIterator. Tests cover basic mutation, empty maps, ordering,
  shared-structure isolation, and exact size tracking. Addresses
  issue #156.

- **[2026-04-24] 2.2: get_next_exclusive / get_prev_exclusive.** Added
  `get_prev_exclusive`, `get_next_exclusive`, `get_prev_exclusive_mut`,
  `get_next_exclusive_mut` to OrdMap and `get_prev_exclusive`,
  `get_next_exclusive` to OrdSet. Uses `Bound::Excluded` instead of
  `Bound::Included` for strictly-less / strictly-greater semantics.
  Unit tests cover key present/absent, boundaries, empty collections.
  Addresses issue #157.

- **[2026-04-24] 1.4: Edition 2021 migration.** Updated `Cargo.toml`
  edition from 2018 to 2021. `cargo fix --edition` found no code changes
  needed. Edition 2021 enables resolver v2 (fewer unnecessary features on
  transitive deps).

- **[2026-04-24] 1.3: Deprecate bincode feature.** Added `#[deprecated]`
  attribute on the `bincode` module in `lib.rs` with deprecation notice
  pointing to serde. Updated feature table in lib.rs doc comment. Feature
  will be removed entirely in v8.0.0.

- **[2026-04-24] 1.2: Remove dead pool code.** Deleted `src/fakepool.rs`
  (no-op stub, orphaned — no `mod` declaration) and `src/vector/pool.rs`
  (referenced non-existent `POOL_SIZE` and `util::Pool`, also orphaned).
  Removed Pool/RRBPool glossary entry. No code in the module tree
  referenced either file.

- **[2026-04-24] 0.5: Architecture documentation.** Documented internal
  architecture of all core modules in `docs/architecture.md`: HAMT 3-tier
  SIMD hybrid (SmallSimdNode/LargeSimdNode/HamtNode), RRB tree 4-buffer
  structure and VectorInner representation, B+ tree node types and Cursor
  navigation, Focus/FocusMut unsafe invariants and caching strategy,
  SharedPointer abstraction. Full unsafe inventory (22 sites across 4 files).

- **[2026-04-24] 0.4: Dependency audit.** All semver-compatible deps current.
  No security vulnerabilities. Breaking updates (rand 0.10, wide 1.3,
  criterion 0.8, proptest-derive 0.8) deferred to natural integration points.
  bincode unmaintained advisory tracked in item 1.3. cargo-audit added to
  Nix devShell. See DEC-003.

- **[2026-04-24] 0.3: Complete benchmark coverage.** Added `benches/hashset.rs`
  (HashSet vs std, i64 + string keys, set operations: union/intersection/
  difference) and `benches/ordset.rs` (OrdSet vs BTreeSet, i64 + string keys,
  remove_min/remove_max). Registered in Cargo.toml.

- **[2026-04-24] 0.2: Complete fuzz coverage.** Added `fuzz/fuzz_targets/hashmap.rs`
  (insert, remove, get, union, symmetric_difference, intersection vs std HashMap)
  and `fuzz/fuzz_targets/ordmap.rs` (insert, remove, get, range iteration with
  bidirectional traversal vs BTreeMap). Extended `fuzz/fuzz_targets/vector.rs`
  with `FocusGet` and `FocusMutSet` actions exercising Focus/FocusMut cursors.

- **[2026-04-24] 0.1: CI pipeline.** Updated `.github/workflows/ci.yml`:
  actions/checkout v4, added miri job (nightly), added `small-chunks` testing,
  clippy with `-D warnings` + upstream lint allowances, cargo doc with
  `-D warnings`, cargo audit via rustsec/audit-check, modernised fuzz job.

- **[2026-04-24] Project infrastructure setup.** Nix devShells (stable +
  nightly), build.sh, test.sh, bench.sh, directives.md, CLAUDE.md, docs/
  (decisions, glossary, references, baselines). Dependency update
  (`cargo update`), dead `version_check` build-dep removed, Cargo profiles
  tuned (split-debuginfo, LTO, codegen-units), `target-cpu=native` for
  benchmarks.

---

## Current {#current}

Phases 0–4 complete. Phase 5 in progress — 5.1 done (triomphe default,
DEC-010), 5.2 done (Clone bounds). Remaining Phase 5: 5.3 (const generic
branching), 5.4 (no_std). Both unblocked since 4.3 (CHAMP) was deferred.

Phase 3 status: All resolved. 3.4 partially complete (par_iter done for
all types; par_iter_mut for HashMap; par_sort for Vector; parallel bulk
ops deferred to Phase 6).
Phase 4: All resolved. SIMD HAMT retained (DEC-007). Merkle caching
accepted (DEC-009). SharedPointer-wrapped hasher done.
Phase 5: 5.1 done (DEC-010). 5.2 done.

---

## Future {#future}

---

## Phase 0 — Foundations {#phase-0}

Everything in this phase must land before any structural work begins. The
goal is to make the project safe to change: CI catches regressions,
benchmarks quantify impact, fuzz targets catch edge cases, miri catches UB,
and architecture documentation ensures changes are made with understanding.

### 0.1 CI pipeline, test.sh, build.sh

**What:** Set up GitHub Actions and standard project entry points.

**Scope:**
- GitHub Actions workflow: stable matrix (`cargo test`, `cargo clippy`,
  `cargo test --all-features`) and nightly matrix (`cargo +nightly miri test`)
- `build.sh` at project root (standard entry point per workspace conventions)
- `test.sh` at project root wrapping `cargo test --all-features`
- Run fuzz targets in CI for a short duration (60s each) as a smoke check
- Run `cargo test --features small-chunks` — this feature exists specifically
  to improve test coverage by forcing smaller node sizes that trigger edge
  cases (node splitting, merging, rebalancing)

**Why:** There is no CI at all. Miri is essential given the unsafe code in
Focus/FocusMut (vector/focus.rs) and nodes/hamt.rs. The `small-chunks`
feature is designed for testing but there's no evidence it's regularly run.

**Complexity:** Low.

**Prerequisite for:** Everything in Phases 2–6.

---

### 0.2 Complete fuzz coverage

**What:** Add missing fuzz targets and extend existing ones to cover
unsafe-heavy code paths.

**Scope:**
- **New:** `fuzz/fuzz_targets/hashmap.rs` — random sequences of insert,
  remove, get, iter, union, difference, intersection against
  `std::collections::HashMap` reference. Modelled on existing `hashset.rs`.
- **New:** `fuzz/fuzz_targets/ordmap.rs` — same pattern against
  `std::collections::BTreeMap` reference. Modelled on existing `ordset.rs`.
- **Extend:** `fuzz/fuzz_targets/vector.rs` — add `Focus` and `FocusMut`
  actions to the existing `Action` enum: create Focus/FocusMut, random
  indexed reads/writes, interleave with structural mutations (push, split,
  join). Focus and FocusMut (vector/focus.rs) contain the most complex
  unsafe code (raw pointers, manual Send/Sync impls, AtomicPtr) and have
  zero fuzz coverage today.

**Why:** HashMap and OrdMap have no fuzz targets. The CHAMP rewrite (4.3)
replaces the entire HAMT node layout — the most invasive change in this plan.
Focus/FocusMut are the highest-risk unsafe code and are exercised by the
unsafe audit (3.2). Without fuzz coverage, subtle bugs in node manipulation
or pointer arithmetic will not be caught.

**Complexity:** Low. Existing targets provide templates.

**Prerequisite for:** 3.2 (unsafe audit), 4.3 (CHAMP integration), 6.1 (ART).

---

### 0.3 Complete benchmark coverage

**What:** Fill gaps in the benchmark suite and add measurement types that
don't currently exist.

**Scope:**
- **New:** `benches/hashset.rs` — insert, remove, lookup, iteration, union,
  intersection, difference. Compare against `std::collections::HashSet` and
  `rpds`.
- **New:** `benches/ordset.rs` — same pattern against
  `std::collections::BTreeSet` and `rpds`.
- **Extend `benches/vector.rs`:**
  - Sole-owner mutation benchmarks: sequential insert chains where the old
    binding is immediately dropped, bulk construction via repeated push,
    update-in-place loops (baseline for 3.1).
  - Concat-depth benchmarks: iteration speed on concat-built vs push-built
    vectors of equal size (baseline for 2.1).
  - Prepend benchmarks: push_front chains at 1K/10K/100K elements
    (baseline for 4.1).
- **Extend `benches/hashmap.rs` and `benches/ordmap.rs`:**
  - Sole-owner mutation benchmarks (baseline for 3.1).
- **New: memory profiling benchmarks.** Several items claim memory savings
  (5.1, 4.3, 6.3) but there is no way to measure memory usage. Add benchmarks
  using `std::alloc::GlobalAlloc` tracking (or the `dhat` crate) to measure
  peak heap allocation for constructing collections of 1K/10K/100K/1M
  elements.

**Why:** HashSet and OrdSet have zero benchmarks. The sole-owner and
concat-depth benchmarks provide before/after baselines for the highest-impact
changes. Memory profiling is essential for items that target memory reduction.

**Complexity:** Low-moderate. Criterion boilerplate for throughput; `dhat` or
custom allocator for memory.

**Prerequisite for:** 2.1 (concat fix), 3.1 (Arc::get_mut), 4.1 (prefix
buffer), 4.2 (CHAMP prototype), 5.1 (triomphe default).

---

### 0.4 Dependency audit

**What:** Full review of all dependencies in `Cargo.toml` — both direct and
transitive — for security, performance, staleness, and compatibility issues.

**Scope:**
- **Direct deps audit:** Review each dependency for:
  - Available updates (semver-compatible and breaking)
  - Known security advisories (`cargo audit`)
  - Performance-relevant changes in newer versions
  - MSRV compatibility with the project's Rust 1.85 minimum
  - Whether the dep is still needed (e.g. `version_check` was a dead
    build-dep — already removed)
- **Transitive dep review:** Check for duplicate versions of the same crate
  in the dependency tree (`cargo tree -d`) — these increase compile time
  and binary size
- **Feature flag review:** Ensure optional deps use `default-features = false`
  where appropriate and that feature combinations are tested
- **Dev-dep review:** Ensure benchmark comparison targets (rpds) and test
  tooling (proptest, criterion) are current
- **Add `cargo audit` to CI** — automated security advisory checking

**Why:** The project had stale deps (5 unmerged dependabot PRs, a dead
build-dependency). Keeping deps current prevents security debt from
accumulating and ensures compatibility with the evolving Rust ecosystem.
Updates to core deps like `archery` and `triomphe` may include performance
fixes that benefit imbl directly.

**Complexity:** Low.

**Prerequisite for:** 1.1 (dependabot PR triage), 5.1 (triomphe default).

---

### 0.5 Architecture documentation

**What:** Document the current internal architecture of each data structure
module before modifying it. This is a prerequisite for making safe changes,
not a polish step.

**Scope:**
- **RRB tree (nodes/rrb.rs, vector/mod.rs):** Document the `Entry` enum
  (`Values` / `Nodes`), `Size` tracking (dense vs size table), the 4-buffer
  structure (`outer_f`, `inner_f`, `inner_b`, `outer_b`), the `middle` tree,
  and how `push_middle`/`pop_middle`/`prune` maintain invariants. Document
  the concatenation algorithm (currently Stucki's, to be replaced in 2.1).
- **HAMT (nodes/hamt.rs):** Document the SIMD-accelerated hybrid
  architecture. The current implementation is NOT a standard bitmap HAMT —
  it uses a 3-tier node hierarchy: `SmallSimdNode` (16 slots, 1×u8x16 SIMD
  group for parallel probe), `LargeSimdNode` (32 slots, 2×u8x16 SIMD
  groups), and `HamtNode` (classic bitmap-indexed, 32-slot SparseChunk).
  Nodes promote: Small→Large→Hamt as they fill. The `Entry` enum has 5
  variants: `Value`, `SmallSimdNode`, `LargeSimdNode`, `HamtNode`,
  `Collision`. This is significantly more complex than described in the
  academic papers and must be understood before the CHAMP rewrite (4.2/4.3).
- **B+ tree (nodes/btree.rs, ord/map.rs):** Document the node structure
  (rewritten in v6.0), split/merge/rebalance logic, and the `Cursor` type.
  Needed before `iter_mut` (2.3) and any future OrdMap work.
- **Focus/FocusMut (vector/focus.rs):** Document the unsafe invariants —
  raw `target_ptr`, `AtomicPtr` in FocusMut, `Send`/`Sync` impls, the
  interaction between focus cursors and tree modification. These have zero
  documentation and contain the densest unsafe code.
- **SharedPointer abstraction (shared_ptr.rs, archery):** Document how the
  `DefaultSharedPtr` type alias works, what `archery::SharedPointerKind`
  provides (`get_mut`, `make_mut`, `strong_count`), and how the `triomphe`
  feature flag switches the default.

**Why:** The codebase has ~4% comment ratio. Contributors in upstream issues
describe the RRB implementation as "severely under-documented." Every
subsequent phase modifies these internals — without documentation, changes
are made blind and review is impossible. This also fulfils the user's request
to include documentation review as preparation.

**Complexity:** Moderate. Requires reading and understanding ~5K lines of
core implementation. Produces no functional changes.

**Prerequisite for:** 2.1 (concat fix), 3.1 (Arc::get_mut), 3.2 (unsafe
audit), 4.1 (prefix buffer), 4.2 (CHAMP prototype).

---

## Phase 1 — Housekeeping {#phase-1}

Low-risk cleanup that can proceed in parallel with Phase 0 or immediately
after. Each item is an independent PR.

### 1.1 Merge or close stale dependabot PRs

**What:** Five dependabot PRs (#142, #132, #126, #125, #124) bumping rayon,
rand, rpds, criterion, and half have sat unmerged for 6-12 months.

**Why:** Stale PRs signal an unmaintained project. Dependency updates often
contain security fixes.

**Complexity:** Trivial.

---

### 1.2 Remove dead pool code

**What:** Remove `fakepool.rs`, `vector/pool.rs`, and all references to
`POOL_SIZE` (which doesn't exist in `config.rs` — the code referencing it
in `vector/pool.rs` cannot compile if that code path is reached). Remove
phantom pool references from documentation.

**Why:** `fakepool.rs` is a no-op stub. `vector/pool.rs` defines `RRBPool`
types that reference `crate::config::POOL_SIZE` which doesn't exist. The
pool was an Rc-only optimisation in the original `im` crate; imbl dropped
Rc support. Dead code and phantom feature flags confuse users.

**Complexity:** Low.

**References:** imbl issue #52.

---

### 1.3 Deprecate bincode feature

**What:** The optional `bincode` feature depends on bincode 2.x (not 1.x as
previously noted — the imports use `bincode::{Decode, Encode}` which is the
2.x API). The bincode 2.x crate had ownership issues and is not considered
well-maintained.

**Approach:** Deprecate the feature with a `#[deprecated]` attribute on the
bincode-specific impls. Remove entirely in v8.0.0. Users can implement
bincode serialisation externally via serde.

**Complexity:** Low.

**References:** imbl issue #146.

---

### 1.4 Edition 2021 migration

**What:** The crate uses `edition = "2018"` despite MSRV 1.85 (which
supports edition 2021). Migrate to edition 2021.

**Why:** Edition 2021 provides cleaner closure captures, `IntoIterator` for
arrays, and other ergonomic improvements. The MSRV already supports it.
Doing this early avoids it becoming a nuisance in later PRs.

**Complexity:** Trivial. Run `cargo fix --edition` and update `Cargo.toml`.

---

## Phase 2 — Correctness fixes & quick API wins {#phase-2}

Non-breaking changes that fix bugs or add missing API surface. Each is an
independent PR suitable for upstream submission. These can start once the
relevant Phase 0 items have landed.

### 2.1 Fix RRB tree concatenation (issue [#35](https://github.com/jneem/imbl/issues/35))

**What:** Vector concatenation produces excessively deep trees. With
branching factor 64, height 3 should accommodate ~200K elements, but vectors
of ~40K elements reach height 7 after repeated concatenation. The root
cause: imbl implements Stucki's concatenation algorithm, which bounds height
at O(log(N × C)) where C is the concatenation count.

**Fix:** Implement L'orange's RRB concatenation algorithm. L'orange's
algorithm maintains proper tree balance by redistributing nodes during
concatenation. The `librrb` C reference implementation and his 2014 master's
thesis document it thoroughly.

**Validation:**
- The concat-depth regression test (from 0.3) must pass — assert that a
  vector of N elements produced by repeated concat does not exceed the
  expected height
- The concat-depth benchmark (from 0.3) must show improvement in iteration
  speed on concat-built vectors
- All existing Vector tests and the fuzz target must continue to pass
- Run under miri

**Complexity:** Moderate-high. The algorithm is more complex than Stucki's.
The issue has been open since October 2021.

**Affects:** `Vector<A>`.

**Prerequisites:** 0.1 (CI/miri), 0.3 (concat-depth benchmarks), 0.5
(RRB architecture docs).

**References:** L'orange, "Improving RRB-Tree Performance through
Transience" (master's thesis, 2014); L'orange, `librrb` C implementation
(github.com/hyPiRion/c-rrb); Stucki et al., "RRB Vector: A Practical
General Purpose Immutable Sequence" (ICFP 2015); imbl issue #35.

---

### 2.2 `get_next_exclusive` / `get_prev_exclusive` (issue [#157](https://github.com/jneem/imbl/issues/157))

**What:** `OrdMap::get_next(key)` uses `Bound::Included(key)`, so it returns
the entry for `key` itself if it exists. Add `get_next_exclusive` (using
`Bound::Excluded`) and `get_prev_exclusive` for strictly-greater /
strictly-less semantics.

**Why:** The current semantics surprise users. A `get_next_exclusive` aligns
with `BTreeMap::range((Excluded(k), Unbounded))`. The maintainer agrees.
This is a pure addition — no existing API changes.

**Complexity:** Trivial. Single comparison change per method.

**Affects:** `OrdMap<K, V>`, `OrdSet<A>`.

**Prerequisites:** 0.1 (CI).

**References:** imbl issue #157.

---

### 2.3 OrdMap `iter_mut` (issue [#156](https://github.com/jneem/imbl/issues/156))

**What:** Add a mutable iterator to `OrdMap` and `OrdSet`. HashMap already
has `iter_mut` (via `NodeIterMut` in hamt.rs), but btree.rs has zero mutable
iteration infrastructure — this must be built from scratch.

**Design:** The iterator walks the B+ tree and yields `(&K, &mut V)` pairs.
Each node on the path must be made exclusive via `SharedPointer::make_mut`
(copy-on-write at the node level). This is the same pattern HashMap uses.
No new unsafe code should be needed — the B+ tree node operations are all
safe Rust.

**Why:** `BTreeMap` provides `iter_mut()`. Its absence is a friction point
for anyone migrating from std. The maintainer has agreed and would accept a
PR.

**Complexity:** Low-moderate. The B+ tree internals (rewritten in v6.0)
support mutable access to leaf nodes, but the iterator scaffolding (tracking
position across nodes, yielding references) needs to be written.

**Affects:** `OrdMap<K, V>`, `OrdSet<A>`.

**Prerequisites:** 0.1 (CI), 0.5 (B+ tree architecture docs).

**References:** imbl issue #156.

---

### 2.4 HashMap/HashSet diff

**What:** Add `diff()` to HashMap and HashSet, producing a `DiffIter` that
yields `DiffItem::{Add, Update, Remove}` — matching the existing
OrdMap/OrdSet diff API.

**Why:** HashMap is the most widely used collection type in the library.
Any system that uses persistent HashMaps for version control, change
tracking, or incremental computation needs efficient differencing to
detect what changed between two versions. OrdMap and OrdSet already provide
`diff()`, but HashMap and HashSet — despite being more commonly used — do
not. This is the most significant API gap in the library.

**Design:** Walk both HAMT tries simultaneously, descending into subtrees
that differ. At leaf level, emit Add/Update/Remove items. The HAMT's
hash-prefix structure provides a natural alignment for parallel tree
traversal (analogous to how OrdMap's sorted keys provide alignment for
its cursor-based diff).

**Complexity:** Moderate. The HAMT's 3-tier node hierarchy
(SmallSimdNode/LargeSimdNode/HamtNode) adds implementation complexity
compared to a standard bitmap HAMT diff.

**Affects:** `HashMap<K, V>`, `HashSet<A>`.

**Prerequisites:** 0.1 (CI), 0.3 (benchmarks for performance validation).

**References:** Steindorfer and Vinju, OOPSLA 2015 (includes diff algorithm
for bitmap tries); Clojure `clojure.data/diff`.

---

### 2.5 Vector diff

**What:** Add `diff()` to Vector, producing a `DiffIter` that yields
positional `DiffItem::{Add(index, value), Update{index, old, new},
Remove(index, value)}` items.

**Why:** Version-controlled Vector data needs differencing for the same
reasons as Map data. Without it, consumers must fall back to O(n)
element-by-element comparison with no way to leverage structural sharing.

**Design:** Positional diff — compare elements at each index. If lengths
differ, excess elements are Add (longer) or Remove (shorter). This is
the right abstraction for indexed collections where position is the key
(unlike content-based diff algorithms like Myers, which suit
text/sequences where content identity matters more than position).

**Complexity:** Low-moderate. Simpler than HashMap diff since Vector
indices provide trivial alignment.

**Affects:** `Vector<A>`.

**Prerequisites:** 0.1 (CI).

---

### 2.6 Patch/apply from diff

**What:** Add an `apply_diff()` method that takes a DiffIter (or any
iterator of `DiffItem`) and produces a new collection with the diff
applied. Completes the diff-merge-patch cycle.

**Why:** Diff alone is only half the story. A version-controlled system
needs: diff(base, version_a), diff(base, version_b), resolve conflicts,
then apply the resolved diff to produce the merged result. Without apply,
consumers must manually reconstruct the merged collection entry by entry.

**Design:** For each DiffItem, apply the corresponding mutation.
`Add(k, v)` → insert, `Remove(k, _)` → remove, `Update{key, new, ..}`
→ update. The method should accept any `IntoIterator<Item = DiffItem>`,
not just the library's own `DiffIter` — this allows consumers to filter,
transform, or merge diff streams before applying.

**Complexity:** Low. Uses existing insert/remove/update operations
internally.

**Affects:** All collection types that have diff: HashMap, HashSet,
OrdMap, OrdSet, Vector.

**Prerequisites:** 2.4 (HashMap diff), 2.5 (Vector diff). The
OrdMap/OrdSet implementations can land earlier since those diffs already
exist.

---

### 2.7 General merge

**What:** Add a general-purpose `merge_with` that combines two maps in a
single traversal, handling all three partitions: keys only in the left
map, keys in both maps, keys only in the right map. Each partition gets
its own closure.

**Signature (Rust):**
```rust
fn merge_with<V2, V3>(
    &self,
    other: &Map<K, V2>,
    both: impl FnMut(&K, &V, &V2) -> Option<V3>,
    left_only: impl FnMut(&K, &V) -> Option<V3>,
    right_only: impl FnMut(&K, &V2) -> Option<V3>,
) -> Map<K, V3>
```

**Why:** This is the most powerful missing API in imbl. It subsumes
`union_with`, `intersection_with`, `difference_with`, and
`symmetric_difference_with` as special cases — each is just a specific
combination of closures. More importantly, it handles mixed strategies
(e.g. "keep left-only entries unchanged, merge both-entries with a
custom resolver, discard right-only entries") that currently require
multiple passes. Haskell's `Data.Map.mergeWithKey` and the
`Data.Map.Merge.Strict` module provide equivalent functionality and are
among the most-used map combinators in the Haskell ecosystem.

**Complexity:** Moderate. Requires a simultaneous traversal of two trees
(HAMT or B+ tree), dispatching to the appropriate closure at each node.
The OrdMap implementation can reuse the cursor-based diff machinery.
The HashMap implementation requires a parallel HAMT walk.

**Affects:** `HashMap<K, V>`, `OrdMap<K, V>`.

**Prerequisites:** 0.1 (CI). Benefits from 2.4 (HashMap diff) since both
require parallel HAMT traversal — shared infrastructure.

**References:** Haskell `Data.Map.mergeWithKey`; Haskell
`Data.Map.Merge.Strict` (merge tactics API); Scala `merged`.

---

### 2.8 Map value and key transformations

**What:** Add a family of map transformation methods that produce new
maps with transformed values or keys. Currently, all such transforms
require `iter().map().collect()`, which rebuilds the tree from scratch
and loses structural sharing.

**Methods:**
- `map_values(&self, f: impl FnMut(&V) -> V2) -> Map<K, V2>` — transform
  all values
- `map_values_with_key(&self, f: impl FnMut(&K, &V) -> V2) -> Map<K, V2>`
  — transform values with key access
- `map_keys<K2>(&self, f: impl FnMut(&K) -> K2) -> Map<K2, V>` — transform
  keys (may merge entries if `f` is not injective)
- `map_keys_monotonic<K2>(&self, f: impl FnMut(&K) -> K2) -> OrdMap<K2, V>`
  — transform keys preserving order (OrdMap only; can reuse tree structure
  since relative ordering is unchanged)
- `try_map_values(&self, f: impl FnMut(&K, &V) -> Result<V2, E>) -> Result<Map<K, V2>, E>`
  — fallible value transformation with early exit on first error
- `map_accum<S, V2>(&self, init: S, f: impl FnMut(S, &K, &V) -> (S, V2)) -> (S, Map<K, V2>)`
  — thread an accumulator through key-order traversal while transforming
  values

**Why:** `map_values` is one of the most commonly needed operations on
maps across every language ecosystem (Haskell `fmap`/`mapWithKey`, Scala
`transform`/`mapValues`, Clojure `update-vals`). Its absence is the
single largest ergonomic gap in imbl's map API. `try_map_values` (Haskell's
`traverseWithKey`) fills a critical niche for validation and parsing
pipelines. `map_keys_monotonic` enables efficient key type conversions
on ordered maps without rebuilding.

**Complexity:** Low-moderate per method. `map_values` and
`map_values_with_key` are straightforward tree walks. `map_keys` needs
to rebuild (keys affect structure). `map_keys_monotonic` can reuse tree
nodes since order is preserved. `try_map_values` adds early-exit logic.
`map_accum` threads state through an in-order traversal.

**Affects:** `HashMap<K, V>`, `OrdMap<K, V>`.

**Prerequisites:** 0.1 (CI).

**References:** Haskell `Data.Map.mapWithKey`, `Data.Map.mapKeys`,
`Data.Map.mapKeysMonotonic`, `Data.Map.traverseWithKey`,
`Data.Map.mapAccumWithKey`; Scala `transform`, `mapValues`.

---

### 2.9 Map/set partitioning and bulk filtering

**What:** Add partitioning and bulk key-set filtering operations to maps
and sets.

**Methods:**
- `partition(&self, f: impl FnMut(&K, &V) -> bool) -> (Self, Self)` —
  split into entries that satisfy the predicate and entries that do not
- `partition_map<V1, V2>(&self, f: impl FnMut(&K, &V) -> Result<V1, V2>) -> (Map<K, V1>, Map<K, V2>)`
  — partition + transform into two differently-typed maps (Haskell's
  `mapEither`)
- `restrict_keys(&self, keys: &Set<K>) -> Self` — keep only entries
  whose keys are in the given set
- `without_keys(&self, keys: &Set<K>) -> Self` — remove all entries
  whose keys are in the given set
- `disjoint(&self, other: &Self) -> bool` — check whether two maps/sets
  share no keys, with O(1) early exit on first shared key
- `relative_complement_with<F>(&self, other: &Self, f: F) -> Self where F: FnMut(&K, &V, &V) -> Option<V>`
  — asymmetric difference where `f` decides per-entry whether to keep,
  modify, or discard
- `retain` for OrdMap/OrdSet — HashMap already has `retain`, but OrdMap
  and OrdSet do not

**Why:** These are the standard vocabulary of set-theoretic operations
on maps. `partition` appears in Haskell, Scala, and Clojure. `restrict_keys`
/ `without_keys` enable bulk key-set operations in O(m+n) via simultaneous
traversal (vs O(m log n) for iterating the key set and calling `remove`
individually). `disjoint` enables O(m+n) conflict detection with early
exit, replacing the current approach of building a full `intersection`
and checking `is_empty`. OrdMap's missing `retain` is a straightforward
parity gap with HashMap.

**Complexity:** Low-moderate per method. Most are tree-walk-and-collect
patterns. `restrict_keys`/`without_keys` can be optimised with
simultaneous traversal on OrdMap.

**Affects:** `HashMap<K, V>`, `HashSet<A>`, `OrdMap<K, V>`, `OrdSet<A>`.

**Prerequisites:** 0.1 (CI).

**References:** Haskell `Data.Map.partition`, `Data.Map.restrictKeys`,
`Data.Map.withoutKeys`, `Data.Map.disjoint`, `Data.Map.mapEither`,
`Data.Map.differenceWith`; Scala `partition`.

---

### 2.10 Vector convenience operations

**What:** Add commonly-needed Vector operations found in Scala and
Haskell's sequence libraries.

**Methods:**
- `chunked(n: usize) -> Vec<Vector<A>>` — split into non-overlapping
  fixed-size chunks (last chunk may be smaller). Uses `split_at`
  internally.
- `adjust<F>(&self, index: usize, f: F) -> Self where F: FnOnce(&A) -> A`
  — apply a function at an index, returning a new vector. Avoids the
  `get` → transform → `set` pattern.
- `scan_left<S>(&self, init: S, f: impl FnMut(&S, &A) -> S) -> Vector<S>`
  — cumulative fold producing a vector of intermediate results (prefix
  sums, running totals, state machine traces)
- `patch(&self, from: usize, replacement: &Vector<A>, replaced: usize) -> Self`
  — replace `replaced` elements starting at `from` with the contents of
  `replacement`. Single operation vs `split_at` + `skip` + `append`.
- `sliding(size: usize, step: usize) -> Vec<Vector<A>>` — overlapping
  windows of a given size, advancing by `step`

**Why:** These are the most commonly-needed vector operations identified
across Scala 2.13 (`grouped`, `sliding`, `patch`, `scanLeft`) and
Haskell (`adjust'`, `scanl`, `chunksOf`). Each is currently achievable
via combinations of existing methods but requires verbose multi-step
code. `adjust` in particular eliminates the get-modify-set pattern that
is a frequent source of off-by-one errors and unnecessary allocations.

**Complexity:** Low. All build on existing operations (`split_at`,
`append`, `set`, iteration). `sliding` needs care to avoid O(n) per
window (use `skip`/`take` which are O(log n) on RRB trees).

**Affects:** `Vector<A>`.

**Prerequisites:** 0.1 (CI). `chunked` and `patch` benefit from 2.1
(RRB concat fix) for efficient split/append, but are not blocked by it.

**References:** Scala `Vector.grouped`, `Vector.sliding`,
`Vector.patch`, `Vector.scanLeft`; Haskell `Data.Sequence.adjust'`,
`Data.Sequence.scanl`, `Data.Sequence.chunksOf`.

---

### 2.11 Companion collection types

**What:** Add new collection types built on existing imbl primitives,
filling common patterns that currently require manual composition.

**Types:**

1. **`Atom<T>`** — thread-safe atomic state holder for persistent
   collections. Wraps `arc-swap` to provide `load() -> Arc<T>`,
   `store(T)`, and `update(f: impl FnOnce(&T) -> T)` with CAS-loop
   retry. This is the canonical way to share persistent data structures
   across threads: readers get consistent snapshots via `load()` without
   locking; writers apply pure functions via `update()`.

   Completes the concurrency story for persistent collections. Without
   it, users must reinvent the pattern using `ArcSwap` or
   `RwLock<Arc<T>>` — every project does this slightly differently.
   Clojure's `atom` and immer's `immer::atom<T>` fill the same role in
   their ecosystems. Minimal implementation (~50 lines wrapping
   `arc-swap`).

2. **`HashMultiMap<K, V>`** — persistent multimap (key → set of values).
   Backed by `HashMap<K, HashSet<V>>` internally. Provides `insert(k, v)`
   (add value to key's set), `remove(k, v)` (remove single value),
   `remove_all(k)` (remove all values for key), `get(k) -> &HashSet<V>`,
   `contains(k, v)`, plus set operations (`union`, `intersection`).

   Multimap is an extremely common pattern (tags-to-items, graph
   adjacency lists, inverted indices). Currently requires manual inner-set
   management for every operation. Capsule (CHAMP reference
   implementation) provides `SetMultimap` as a first-class type.

3. **`InsertionOrderMap<K, V>`** — persistent map preserving insertion
   order. Backed by `HashMap<K, usize>` (key → insertion index) plus
   `OrdMap<usize, (K, V)>` (index → entry). Iterates in insertion order.
   Provides the same API as HashMap plus ordered iteration.

   No persistent insertion-ordered map exists in Rust. The `indexmap`
   crate fills this niche for mutable maps. Common for JSON object
   representation, configuration files, and API responses where key order
   matters. PCollections (Java) provides `OrderedPMap`.

4. **`PBag<A>` (Multiset)** — persistent unordered collection with
   duplicates, backed by `HashMap<A, usize>` (element → count). Provides
   `insert(a)` (increment count), `remove(a)` (decrement), `count(a)`,
   `total_count()`, plus multiset operations (sum, intersection,
   difference). Trivial wrapper — ~100 lines.

**Complexity:** Low per type. All delegate to existing collection
implementations. `Atom<T>` adds `arc-swap` as an optional dependency
behind a feature flag.

**Affects:** New types; no changes to existing collections.

**Prerequisites:** 0.1 (CI). `Atom<T>` requires `arc-swap` crate
approval (see dependency evaluation process in directives).

**References:** Clojure `atom`; immer `immer::atom<T>`;
Capsule `SetMultimap`, `BinaryRelation`; PCollections `OrderedPMap`,
`PBag`; `arc-swap` crate (docs.rs/arc-swap); `indexmap` crate.

---

## Phase 3 — Mutation & parallel performance {#phase-3}

The core performance track. 3.1 is the foundation, 3.2 validates safety,
3.3 builds the user-facing API on top, 3.4 extends parallelism across
all collection types, and 3.5–3.6 optimise equality and diff operations
for structurally-shared collections.

### 3.1 `Arc::get_mut` in-place mutation

**What:** When a node's `SharedPointer` refcount is 1, mutate it in place
instead of clone-on-write. Replace calls to `SharedPointer::make_mut` (which
always clones if refcount > 1) with a `SharedPointer::get_mut` check
(which returns `Some(&mut T)` if sole owner) followed by `make_mut` as
fallback.

**Key finding:** `archery::SharedPointer` already exposes `get_mut()` — the
method exists in the trait and works through both `ArcK` (std::Arc) and
`ArcTK` (triomphe::Arc). There are 105 `make_mut` call sites across the
codebase. The change is mechanically replacing each with a get_mut check +
make_mut fallback, but care is needed to ensure the semantics are identical
(the old collection must actually be dropped before the refcount reaches 1).

**Why:** The pattern `let mut map = map.insert(k, v)` clones O(tree_depth)
nodes unnecessarily because the refcount is 1 by the time the clone happens.
Clojure measured ~2× speedup for bulk construction with this optimisation.

**Note:** This does NOT require new unsafe code. `SharedPointer::get_mut` is
safe Rust. The subtlety is logical: ensuring that sole-owner detection
happens at the right point in the call sequence. The crate-root
`#[deny(unsafe_code)]` will enforce this.

**Validation:**
- Sole-owner mutation benchmarks (from 0.3) must show improvement
- All existing tests, proptests, and fuzz targets must pass
- Run under miri

**Complexity:** Low-moderate. Mechanically straightforward but must be
threaded through all mutation paths consistently.

**Affects:** All five collection types.

**Prerequisites:** 0.1 (CI/miri), 0.3 (sole-owner benchmarks), 0.5
(architecture docs for all three data structures).

**References:** Clojure transients — Rich Hickey; immer memory policy —
Bolívar Puente, "Persistence for the Masses" (CppCon 2017); Bifurcan —
Zach Tellman.

---

### 3.2 Unsafe code audit (issue [#27](https://github.com/jneem/imbl/issues/27))

**What:** Audit, document, and where possible eliminate `unsafe` blocks. The
crate uses `#[deny(unsafe_code)]` at the crate root (lib.rs:321) with
`#[allow(unsafe_code)]` only in `vector/mod.rs`. Unsafe also exists in
`nodes/hamt.rs` (inline `#[allow]` blocks) and `nodes/btree.rs`.

**Current unsafe inventory:**
- `vector/mod.rs`: 12 occurrences — mostly self-referential pointer casts
  for Focus/FocusMut iterator lifetimes
- `vector/focus.rs`: 6 occurrences — raw `target_ptr`, `AtomicPtr`, manual
  `Send`/`Sync` impls
- `nodes/hamt.rs`: 8 occurrences — `node_with` uses `UnsafeCell` +
  `transmute_copy` for zero-copy node construction; `ptr::read`/`ptr::write`
  for in-place entry replacement
- `nodes/btree.rs`: 4 occurrences — `get_unchecked` for binary search

**Approach:**
1. Run `cargo +nightly miri test` — fix any existing UB before proceeding
2. For every `unsafe` block, add a `// SAFETY:` comment documenting the
   invariant and what would break it
3. Identify blocks replaceable with safe alternatives:
   - The `get_unchecked` calls in btree.rs can likely become safe indexing
     with negligible cost
   - The Focus/FocusMut pointer casts may be replaceable with GATs or
     lifetime tricks (needs investigation)
4. For blocks that must remain, ensure the fuzz targets (0.2) exercise the
   code path — the combination of fuzzing + miri gives high confidence
5. Enable `unsafe_op_in_unsafe_fn` lint to tighten granularity

**Why:** imbl is used in production by security-sensitive projects (Matrix
SDK, Fedimint). Undocumented unsafe invariants are a credibility and safety
liability. Issue open since August 2021.

**Affects:** Primarily `Vector<A>` (Focus/FocusMut), also nodes/hamt.rs
and nodes/btree.rs.

**Prerequisites:** 0.1 (CI/miri), 0.2 (Focus/FocusMut fuzz coverage), 0.5
(Focus/FocusMut architecture docs).

**References:** imbl issue #27; Rust unsafe code guidelines.

---

### 3.3 Transient / builder API — DONE

**Status:** Resolved — already handled. See Done section and DEC-008.

The existing `&mut self` methods already provide the builder pattern's
core benefit via `Arc::make_mut`'s refcount-1 fast path (8-14× faster
than persistent ops). A dedicated builder would only save ~20-30% on
atomic CAS overhead but requires ~5000 lines of parallel node types.

---

### 3.4 Parallel iterators and bulk operations (rayon)

**What:** Extend rayon support beyond Vector to all collection types.
Currently only `Vector` has `par_iter()` and `par_iter_mut()`. HashMap,
HashSet, OrdMap, and OrdSet have no parallel support despite being
naturally parallelisable tree structures.

**Scope:**

1. **HashMap/HashSet `par_iter()`** — The HAMT is a tree of independent
   subtrees. The 32-way branching factor at the root lets rayon split into
   up to 32 parallel tasks. Implement `IntoParallelRefIterator`,
   `IntoParallelRefMutIterator` (HashMap only), and `ParallelExtend`.
   Highest-impact addition for multi-core machines.

2. **OrdMap/OrdSet `par_iter()`** — The B+ tree structure allows splitting
   at internal nodes. Less natural than HAMT (no random-access split) but
   the tree depth provides log(n) split points. Implement
   `IntoParallelRefIterator` and `IntoParallelRefMutIterator` (OrdMap only).

3. **Parallel `FromIterator` / `collect()`** — Construct collections from
   parallel iterators via rayon's `FromParallelIterator`. Persistent data
   structures support this naturally: build subtrees in parallel, merge at
   the end. For HashMap/HashSet, parallel subtree construction is
   straightforward since hash partitioning is embarrassingly parallel.

4. **Parallel bulk operations** — `union`, `intersection`, `difference`,
   `symmetric_difference` on HashMap/HashSet can process independent HAMT
   subtrees in parallel. The hash-prefix partitioning means subtrees at the
   same position can be merged independently.

5. **Parallel sort for Vector** — Replace the sequential `sort()` with a
   parallel merge-sort that exploits RRB tree split/concat. Split into
   chunks, sort in parallel, concat results. The O(log n) concat makes
   the merge phase efficient.

**Why:** Persistent data structures are naturally suited to parallelism
because subtrees are immutable and independently traversable. On an
18-core M5 Max, HashMap operations with 32-way root branching can
theoretically saturate all cores. The current `rayon` feature flag exists
but only covers Vector — extending it to all types is a high-value,
moderate-effort improvement.

**Validation:**
- Benchmark each parallel operation against its sequential counterpart
  at 1K/10K/100K/1M elements
- Measure scaling efficiency: how many cores are actually utilised
- Ensure `par_iter()` produces identical results to `iter()` (proptest)
- Test with `--features small-chunks` (smaller branching = more splits)

**Complexity:** Moderate per collection type. Vector's existing rayon.rs
provides the template. HashMap/HashSet are the highest priority (natural
HAMT parallelism). OrdMap/OrdSet are lower priority (less natural split).

**Affects:** All five collection types.

**Prerequisites:** 0.1 (CI), 0.3 (benchmarks for before/after comparison).
Items 3.4.3–3.4.5 benefit from but do not require 3.1 (Arc::get_mut,
resolved DEC-004) and 3.3 (resolved DEC-008 — `&mut self` is sufficient).

**References:** rayon crate (docs.rs/rayon); Vector's existing
`src/vector/rayon.rs`; Scala parallel collections
(docs.scala-lang.org/overviews/parallel-collections).

---

### 3.5 PartialEq ptr_eq fast paths

**What:** Add a `ptr_eq` early-exit check to the `PartialEq`
implementation for HashMap, HashSet, and Vector. If two collections share
the same root pointer (one was cloned from the other and neither has been
modified), return `true` immediately in O(1) without traversing elements.

**Why:** Collections that share structure via cloning are the fundamental
pattern in persistent data structure usage. When checking whether a value
has changed (for incremental recomputation, cache invalidation, or
change detection), the common case is that it *hasn't* — and the pointer
check confirms this in O(1). Current state:
- HashMap: O(n) always, plus allocates a `std::HashSet` for tracking
- HashSet: O(n) always, same allocation
- Vector: O(n) always (`iter().eq()`)
- OrdMap: already O(1) for pointer-equal maps (via `diff()` which checks
  `ptr_eq`) ✓
- OrdSet: already O(1) (delegates to OrdMap) ✓

**Complexity:** Trivial. Single `ptr_eq` check at the top of each `eq()`
method.

**Affects:** `HashMap<K, V>`, `HashSet<A>`, `Vector<A>`.

**Prerequisites:** 0.1 (CI).

---

### 3.6 Pointer-aware subtree skipping in diff

**What:** When diffing two collections that share structure, skip entire
subtrees where `Arc::ptr_eq` confirms the subtree is physically
identical. This reduces diff complexity from O(n) to O(changes) for
collections derived from a common ancestor.

**Why:** The primary use case for persistent collections is
fork-modify-diff-merge. After forking, most of the tree is shared. A diff
that walks the entire tree misses the core performance advantage of
structural sharing. For large collections where only a few entries
changed, the difference is orders of magnitude (e.g. O(10) vs O(10M) for
a 10M-entry collection with 10 changes).

**Current state:**
- HashMap: `HashedValue::ptr_eq` returns `false` unconditionally
  (`hash/map.rs:122-124`) — the plumbing exists in the trait but is
  stubbed out. The HAMT's `HamtNode` entries could compare child pointers
  but currently do not.
- OrdMap: root-level `ptr_eq` check exists (`ord/map.rs:305`), but the
  B+ tree cursor does not check `Node::ptr_eq` during traversal — it
  visits every element even in shared subtrees. `Node::ptr_eq` already
  exists (`btree.rs:91-96`) but is unused by diff.
- Vector: depends on 2.5 (Vector diff) existing first.

**Design:** At each internal node during diff traversal, check `ptr_eq`
on child pointers. If equal, skip the entire subtree (emit no diff
items). If unequal, descend. This is a tree-walk optimisation, not a new
algorithm — it layers onto existing diff implementations.

**Complexity:** Moderate. Requires modifying the diff traversal for each
data structure type. The HAMT's 3-tier node hierarchy adds complexity for
HashMap.

**Affects:** HashMap (via 2.4), OrdMap (existing diff), Vector (via 2.5).

**Prerequisites:** 2.4 (HashMap diff — must exist before it can be
optimised), 0.5 (architecture docs for understanding node structure).

---

## Phase 4 — Data structure internals {#phase-4}

Structural changes to individual data structures. Each is a significant
body of work. Items within this phase are independent of each other and
can proceed in parallel.

### 4.1 Vector prefix buffer

**What:** Add a prefix (head) buffer to complement the existing tail
buffer. The current RRB structure has 4 buffers (`outer_f`, `inner_f`,
`inner_b`, `outer_b`) flanking a `middle` tree. Despite having front
buffers, prepend still requires tree modification in many cases. A true
prefix buffer would give O(1) amortised prepend symmetric with append.

**Why:** Scala 2.13 measured 2-3× faster sequential prepend and 35-40×
faster alternating append/prepend with their radix-balanced finger tree
rewrite.

**Ordering rationale:** Must follow 2.1 (concat fix) because both modify
the RRB tree structure. The concat fix should land on the current
representation before the prefix buffer changes it.

**Complexity:** Low-moderate. The tail buffer mechanism exists; the prefix
buffer is symmetric. Interaction with concat, split, and indexed access
needs care.

**Affects:** `Vector<A>`.

**Prerequisites:** 2.1 (concat fix), 0.3 (prepend benchmarks).

**References:** Scala 2.13 `Vector` — Zeiger, "The New Collections
Implementation"; Hinze and Paterson, "Finger Trees" (JFP 2006).

---

### 4.2 CHAMP prototype benchmark — DONE

**Status:** Complete. See Done section for details and DEC-007.

**Important context:** The current HAMT is NOT a textbook bitmap trie. It
is a SIMD-accelerated hybrid with a 3-tier node hierarchy:
1. `SmallSimdNode` — 16 slots, 1×u8x16 SIMD control group for parallel
   probe. Used for small/leaf nodes.
2. `LargeSimdNode` — 32 slots, 2×u8x16 SIMD groups. Promoted from Small
   when full.
3. `HamtNode` — classic bitmap-indexed SparseChunk, 32 slots. Promoted
   from Large when full. This is the only level that has child pointers.

The `Entry` enum has 5 variants: `Value`, `SmallSimdNode`, `LargeSimdNode`,
`HamtNode`, `Collision`. This architecture was introduced in v6.1 (Sep 2025)
and v7.0 (Jan 2026) with explicit performance tuning.

**Why CHAMP may still win:** CHAMP's benefits are architectural (canonical
form, contiguous data layout enabling O(1) equality short-circuit and cache-
friendly iteration). The current SIMD approach optimises lookup latency but
doesn't address memory density or canonical form. However, the SIMD probing
may partially offset CHAMP's iteration advantage. **This is uncertain and
must be benchmarked before committing.**

**What:** Build a standalone CHAMP implementation (two-bitmap encoding,
canonical deletion) and benchmark it against the current SIMD HAMT across
all operations (insert, remove, lookup, iteration, equality, memory usage)
at sizes 100, 1K, 10K, 100K, 1M. This is a go/no-go gate for 4.3.

**Decision rule:** Only proceed to 4.3 if CHAMP shows material improvement
in at least one dimension without regression in others.

**Complexity:** Moderate. Standalone prototype, not yet integrated.

**Affects:** `HashMap<K, V>`, `HashSet<A>`.

**Prerequisites:** 0.3 (HashMap benchmarks + memory profiling), 0.5 (HAMT
architecture docs).

**References:** Steindorfer and Vinju, "Optimizing Hash-Array Mapped Tries
for Fast and Lean Immutable JVM Collections" (OOPSLA 2015); Scala 2.13
`scala.collection.immutable.HashMap`; Bagwell, "Ideal Hash Trees" (2001);
Capsule reference implementation (github.com/usethesource/capsule); imbl
issue #154.

---

### 4.3 CHAMP integration

**What:** If 4.2 benchmarks justify it, replace the SIMD HAMT with CHAMP.
This includes both the two-bitmap encoding (OOPSLA 2015, Section 3) and
canonical deletion (OOPSLA 2015, §4.2). These are inseparable —
canonical form is a key benefit of CHAMP and only works with the two-bitmap
layout.

**Complexity:** High. Replaces the entire node layer for HashMap/HashSet.
The current SIMD architecture is ~1100 lines with extensive optimisation.

**Affects:** `HashMap<K, V>`, `HashSet<A>`.

**Prerequisites:** 4.2 (prototype must show improvement), 0.1 (CI/miri),
0.2 (HashMap fuzz target).

**Ordering note:** Must land BEFORE 5.3 (const generic branching) and 5.4
(no_std), because both would need to accommodate whatever node architecture
exists. If CHAMP lands first, 5.3 parameterises the CHAMP nodes. If 5.3
landed first, CHAMP would need to be generic from day one.

**References:** Same as 4.2.

---

### 4.4 Merkle hash caching — DONE

**Status:** Complete. See Done section for details and DEC-009.

**What was done:** Added a `u64` merkle_hash field to each HAMT node
(GenericSimdNode, HamtNode), maintained incrementally during mutations
using commutative addition of fmix64(key_hash) values. Equality check
gains O(1) negative fast path. HAMT-only — B+ tree and RRB tree would
need additional Hash bounds on values. Final overhead: ~0% insert, ~5%
remove_mut (i64 keys). Always-on, no feature flag.

**Design evolution:**
1. Full recompute: iterating all entries per level → +348% insert (rejected)
2. Incremental with fmix64 at all levels → +14.6% remove
3. Remove inner fmix64 (root hash = flat sum of leaf hashes) → +7.7% remove
4. Inline old_m capture (eliminate upfront lookup) → +4.9% remove (accepted)

**Scope limitation:** The Merkle hash covers keys only (via existing
HashBits), not values. This means it cannot be used for diff optimisation
(where value changes matter), only for equality.

**References:** Merkle trees (Merkle, 1987); MurmurHash3 fmix64 finaliser.

---

### 4.5 SharedPointer-wrapped hasher PoC — DONE

**Status:** Complete. See Done section for details.

**What was done:** Wrapped the hasher in `SharedPointer<S, P>` in both
`GenericHashMap` and `GenericHashSet`. Eliminated `S: Clone` from the
entire HashMap/HashSet API (~50 bounds). Benchmark showed 3-5% i64
lookup regression (acceptable — hash time ~2ns makes pointer deref
proportionally visible), 0-2% for string keys and mutations. Decision:
keep — API simplification cascades to all downstream consumers and
aligns with structural sharing philosophy.

**Affects:** `HashMap<K, V, S>`, `HashSet<A, S>`.

**Prerequisites:** 5.2 (Clone bounds audit — completed).

---

## Phase 5 — Breaking API changes (v8.0.0) {#phase-5}

All items in this phase are breaking changes. They must be batched into a
single major version bump to minimise disruption for downstream users.
Ship as v8.0.0 when all are ready.

### 5.1 Default to triomphe::Arc — DONE

**Status:** Complete. See Done section for details and DEC-010.

**What was done:** Added `triomphe` to default features in Cargo.toml.
`DefaultSharedPtr` now resolves to `ArcTK` (triomphe::Arc) by default.
String-key hashmap ops improved 2-9%, no significant regressions.
Users can opt out with `default-features = false`.

**References:** triomphe (docs.rs/triomphe); archery (docs.rs/archery).

---

### 5.2 Remove unnecessary Clone bounds (issue [#72](https://github.com/jneem/imbl/issues/72)) — DONE

**Status:** Complete. See Done section for details.

**What was done:** Full Clone dependency audit across HashMap, HashSet,
OrdMap. Traced every Clone bound to its actual usage — `self.clone()`,
`self.new_from()` (clones hasher), `SharedPointer::make_mut` (clones
node contents), or `hash_key()` (only borrows, no Clone needed). Split
impl blocks by actual requirements. Removed `S: Clone` from all methods
that only borrow or mutate the hasher (read-only ops, `insert`, `remove`,
`retain`, `FromIterator`, `PartialEq`/`Eq`). Moved `partition_map` and
value/key transform methods to correctly-bounded blocks.

**Remaining S: Clone:** Persistent methods that call `self.clone()`
(`update`, `without`, `union`, `intersection`, `entry`, etc.) still need
`S: Clone` because the hasher is stored inline. Item 4.5 proposes
eliminating this by wrapping the hasher in `SharedPointer`.

**References:** imbl issue #72.

---

### 5.3 Configurable branching factor (issue [#145](https://github.com/jneem/imbl/issues/145)) — DEFERRED

**Status:** Deferred. See DEC-011.

**Blocker:** Stable Rust cannot compute derived constants from const generic
parameters (`generic_const_exprs` is unstable, tracking issue
rust-lang/rust#76560). The HAMT's SIMD node hierarchy requires
`SparseChunk<..., 2^HASH_LEVEL_SIZE>` — this is a computed const generic
argument, which is not supported. Vector and OrdMap const generics are
feasible but the scope (~140 type reference sites, ~80 impl blocks) is
disproportionate to the marginal benefit over the existing `small-chunks`
feature flag.

**References:** imbl issue #145; PR #155; immer `BL` template parameter.

---

### 5.4 `no_std` support (PR [#149](https://github.com/jneem/imbl/pull/149))

**What:** Make imbl usable in `no_std + alloc` environments.

**Breaking because:** `RandomState` (the default hasher for HashMap/HashSet)
comes from `std`. A `no_std` build needs a different default hasher or a
mandatory type parameter. Either changes the public API.

**Design:** Follow `hashbrown`'s approach: default hasher is a no_std-
compatible hasher (e.g. `ahash`) when `std` feature is disabled, and
`RandomState` when `std` is enabled. The `std` feature is on by default.

**Ordering rationale:** Must land AFTER 4.3 (CHAMP integration) if CHAMP
proceeds. The CHAMP implementation should be designed with `no_std` in mind
from the start rather than retrofitted.

**Complexity:** Moderate. Core data structures are pure; the blocker is
the hasher default.

**Affects:** All five collection types (HashMap/HashSet most directly).

**Prerequisites:** 4.3 (CHAMP integration, if proceeding — otherwise
independent).

**References:** imbl PR #149; hashbrown crate; rpds `no_std` support.

---

## Phase 6 — Research & speculative {#phase-6}

High-complexity items with uncertain payoff. Each requires a prototype and
benchmark before committing to integration.

### 6.1 Persistent Adaptive Radix Tree for OrdMap

**What:** Replace OrdMap's B+ tree with a persistent ART.

**Caveats:**
- ART works best with byte-string keys. Arbitrary `Ord` types need encoding.
- The B+ tree was rewritten in v6.0 with significant improvements.
- ART is less proven in production for general-purpose ordered maps.

**Approach:** Prototype as a standalone crate, benchmark against current
OrdMap across all operations and key types.

**Complexity:** High.

**Affects:** `OrdMap<K, V>`, `OrdSet<A>`.

**Prerequisites:** 0.2 (OrdMap fuzz target), 0.3 (OrdMap/OrdSet benchmarks).

**References:** Ankur Dave, "PART"; Leis et al., "The Adaptive Radix Tree"
(ICDE 2013).

---

### 6.2 HHAMT inline storage

**What:** Store small values inline in HAMT nodes instead of behind Arc
pointers.

**Caveats:** Rust lacks specialisation (nightly-only). A size-threshold
approach via const generics is possible but awkward.

**Complexity:** High. Requires reworking node memory layout.

**Affects:** `HashMap<K, V>`, `HashSet<A>`.

**Prerequisites:** 4.3 (CHAMP integration — builds on whatever node layout
exists).

**References:** Steindorfer, "Efficient Immutable Collections" (PhD thesis,
2017), Chapter 5.

---

### 6.3 ThinArc for node pointers

**What:** Use `triomphe::ThinArc` for internal nodes (header + variable-
length array behind a single thin pointer). Saves 8 bytes per pointer.

**Complexity:** Moderate. All node pointer types change.

**Affects:** All five collection types.

**Prerequisites:** 5.1 (triomphe default — ThinArc is triomphe-specific).

**References:** triomphe `ThinArc` (docs.rs/triomphe).

---

### 6.4 `dupe::Dupe` trait support (issue [#113](https://github.com/jneem/imbl/issues/113))

**What:** Implement Meta's `Dupe` trait. Mechanical — delegates to `clone()`.

**Complexity:** Trivial.

**Affects:** All five collection types.

**References:** imbl issue #113; `dupe` crate.

---

### 6.5 Hash consing / interning (compile-time feature)

**What:** An opt-in compile-time feature (`hash-intern`) that adds a
global intern table for tree nodes. When creating a new node, look up
its Merkle hash (from 4.4) in the table — if a live node with the same
hash exists, return the existing `Arc` instead of allocating. This makes
independently-constructed subtrees with identical content pointer-equal
by construction.

**Design:**
- Gated by `#[cfg(feature = "hash-intern")]` — when disabled, zero
  overhead (no hash field, no table, no code generated)
- Intern table: `HashMap<u64, Weak<Node>>` — `Weak` references ensure
  unused nodes are still collected by normal `Arc` drop
- Thread-local tables by default (zero contention). Optional sharded
  global table (`DashMap`-style) behind a sub-feature for cross-thread
  deduplication
- On node creation: compute Merkle hash (reuses 4.4 infrastructure),
  check table, return existing `Arc` or insert new entry
- All `ptr_eq` checks (3.5, 3.6) automatically benefit because interning
  makes independently-equal subtrees the same pointer

**Why:** Completes the equality/deduplication story. Pointer equality
(3.5, 3.6) handles shared-lineage subtrees. Merkle hashing (4.4) handles
independently-equal subtrees via hash comparison. Interning goes further:
it physically deduplicates them so all future operations (diff, equality,
memory) benefit permanently. This is how git and content-addressable
storage work. Particularly valuable for workloads with many similar
collections (version history, branch-heavy workflows, caches of derived
data).

**Trade-offs:**
- Intern table lookup on every node creation (~10-30ns per lookup with
  a good hash table)
- `Weak` reference overhead per interned node
- Thread-local tables don't deduplicate across threads
- Hash collisions (vanishingly rare with 64-bit hash but non-zero risk)

**Complexity:** Moderate. The intern table and `intern_or_alloc()` wrapper
are straightforward. The complexity is in threading it through all node
creation paths and ensuring the `Weak` cleanup is correct.

**Affects:** All five collection types (internal node allocation).

**Prerequisites:** 4.4 (Merkle hash caching — provides the hash
infrastructure that interning builds on).

**References:** Hash consing (Goto, 1974; Filliâtre and Conchon, 2006);
git content-addressable object store; immer memory policies.

---

### 6.6 Structural-sharing-preserving serialisation

**What:** A serialisation format that preserves the internal tree
topology, so that two collections sharing structure are serialised
without duplicating the shared nodes.

**Design:** Use a "pool" approach (inspired by immer's `persist` module):
1. **Serialise:** walk the tree, assign each unique node an ID,
   serialise each node once with references to child IDs. Shared nodes
   (same `Arc` pointer) naturally get the same ID.
2. **Deserialise:** reconstruct nodes from the pool, restoring `Arc`
   sharing by reusing the same node for all references to the same ID.
3. Custom serde layer or standalone serialisation module (the standard
   serde `Serialize`/`Deserialize` model does not support shared
   references).

**Why:** Without this, serialising two `HashMap`s that share 99% of
their structure writes the full data twice. With it, shared nodes are
written once. Critical for:
- Checkpointing application state (undo/redo, save/load)
- Distributing persistent collections over the network
- Persisting version history where successive versions share structure
- Any application where serialised size matters and collections share
  lineage

**Complexity:** High. Requires exposing internal node identity, building
a deduplication table during serialisation, and reconstructing the
sharing graph on deserialisation. The serde model (sequential
serialize/deserialize) does not naturally support this — needs a custom
serialisation layer or a wrapper that manages node pools.

**Affects:** All five collection types.

**Prerequisites:** 0.5 (architecture docs for understanding node
structure and pointer layout).

**References:** immer `extra/persist.hpp` (pool-based serialisation);
Cap'n Proto (shared subobject references); FlatBuffers (DAG
serialisation).

---

## Dependency map {#dependency-map}

```
Phase 0 (foundations)
  0.1 CI/miri ─────────────────────┬──────────────────────────────────────┐
  0.2 fuzz coverage ───────────────┤                                      │
  0.3 benchmark coverage ──────────┤                                      │
  0.4 dependency audit ────────────┤                                      │
  0.5 architecture docs ───────────┤                                      │
                                   │                                      │
Phase 1 (housekeeping)             │ (parallel with Phase 0)              │
  1.1 dependabot PRs ◄── 0.4      │                                      │
  1.2 dead pool code               │                                      │
  1.3 bincode deprecation          │                                      │
  1.4 edition 2021                 │                                      │
                                   ▼                                      │
Phase 2 (correctness + API)                                               │
  2.1 RRB concat fix ◄── 0.1, 0.3, 0.5                                   │
  2.2 get_next_exclusive ◄── 0.1                                          │
  2.3 OrdMap iter_mut ◄── 0.1, 0.5                                        │
  2.4 HashMap/HashSet diff ◄── 0.1, 0.3                                   │
  2.5 Vector diff ◄── 0.1                                                 │
  2.6 patch/apply ◄── 2.4, 2.5                                            │
  2.7 general merge ◄── 0.1                                               │
  2.8 map value/key transforms ◄── 0.1                                    │
  2.9 partitioning + bulk filter ◄── 0.1                                  │
  2.10 vector convenience ops ◄── 0.1                                     │
  2.11 companion types ◄── 0.1                                            │
                                   │                                      │
Phase 3 (mutation + parallel perf)  │                                      │
  3.1 Arc::get_mut ◄── 0.1, 0.3, 0.5                                     │
  3.2 unsafe audit ◄── 0.1, 0.2, 0.5                                     │
  3.3 transient/builder ◄── 3.1                          ✓ DONE (DEC-008) │
  3.4 parallel iterators ◄── 0.1, 0.3                                     │
  3.5 PartialEq ptr_eq fast paths ◄── 0.1                                 │
  3.6 subtree-aware diff ◄── 2.4, 0.5                                     │
                                   │                                      │
Phase 4 (internals)                │                                      │
  4.1 prefix buffer ◄── 2.1                                               │
  4.2 CHAMP prototype ◄── 0.3, 0.5  ✓ DONE (DEC-007: HAMT retained)                                        │
  4.3 CHAMP integration ◄── 4.2, 0.1, 0.2 (only if benchmarks justify)   │
  4.4 Merkle hash caching ◄── 0.3, 0.5  ✓ DONE                            │
  4.5 SharedPointer hasher PoC ◄── 5.2  ✓ DONE                                    │
                                   │                                      │
Phase 5 (breaking — v8.0.0)        │                                      │
  5.1 triomphe default ◄── 0.3, 0.4  ✓ DONE (DEC-010)                     │
  5.2 remove Clone bounds ◄── 3.1  ✓ DONE                                │
  5.3 const generic branching ◄── 4.3  ✗ DEFERRED (DEC-011: stable Rust blocker) │
  5.4 no_std ◄── 4.3 (if proceeding)                                      │
                                   │                                      │
Phase 6 (research)                 │                                      │
  6.1 ART for OrdMap ◄── 0.2, 0.3                                         │
  6.2 HHAMT inline ◄── 4.3                                                │
  6.3 ThinArc ◄── 5.1                                                     │
  6.4 Dupe trait ◄── (none)                                                │
  6.5 hash consing/interning ◄── 4.4                                      │
  6.6 sharing-preserving serialisation ◄── 0.5                             │
```

### Parallel tracks

Once Phase 0 is complete, eight independent tracks can proceed in
parallel:

1. **Vector track:** 2.1 → 4.1
2. **Hash track:** 4.2 → (4.3 if justified) → 5.3, 5.4
3. **Mutation track:** 3.1 ✓ → 3.2 ✓, 3.3 ✓ → 5.2 ✓ → 4.5 ✓
4. **Parallel track:** 3.4 (HashMap/HashSet par_iter first, then
   OrdMap/OrdSet, then bulk ops and parallel sort). Benefits from but
   does not block on 3.1/3.3.
5. **Diff track:** 2.4, 2.5 (independent of each other) → 2.6 → 3.6.
   Item 3.5 (PartialEq fast paths) is independent and can land at any
   time after 0.1.
6. **Map API track:** 2.7, 2.8, 2.9 (independent of each other and of
   all other tracks). 2.7 (general merge) shares HAMT traversal
   infrastructure with 2.4 (HashMap diff) — co-development is efficient
   but not required.
7. **Hash integrity track:** 4.4 ✓ (Merkle hash caching) → 6.5 (hash
   consing/interning). 4.4 complete — 6.5 can now proceed.
8. **Serialisation track:** 6.6 (sharing-preserving serialisation).
   Independent but benefits from 4.4 (Merkle hashes enable
   content-addressed node pools).

Items 2.2, 2.3, 2.10, 2.11, 1.x, and 6.4 are independent and can be
done at any time after their prerequisites.

---

## References {#references}

See `docs/references.md` for the full bibliography — papers, implementations,
and Rust crates referenced by plan items above.
