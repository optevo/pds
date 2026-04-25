# pds — Implementation Plan

Sequenced implementation plan for pds (persistent data structures for
Rust). Forked from [imbl](https://github.com/jneem/imbl) with different
design priorities: performance over compatibility, Merkle hashing, SIMD
HAMT nodes, and no_std support.

**Current state (Apr 2026):** v1.0.0, ~12K lines of Rust, 11 collection
types (Vector, HashMap, HashSet, OrdMap, OrdSet, Bag, HashMultiMap,
InsertionOrderMap, BiMap, SymMap, Trie). SIMD HAMT, Merkle hashing,
and no_std support implemented.

---

## Principles

### Change discipline

pds is an independent fork of jneem/imbl. Changes should be small,
focused, and well-tested with clear commit messages. Breaking changes
are batched into v2.0.0 to avoid churn.

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

Items are grouped by semver impact. Non-breaking changes can ship as v1.x
point releases. Breaking changes (5.1, 5.2, 5.3, 5.4) are batched into a
single v2.0.0 release in Phase 5.

---

## Contents

- [Done](#done)
- [Current](#current)
- [Completed phases](#future) (Phases 0–6 — reference documentation)
- [Residual](#residual) — open items
- [Dependency map](#dependency-map)
- [References](#references)

---

## Done {#done}

*Newest first.*

- **[2026-04-25] 4.7 Stage 3: Identity hasher.** `IdentityHasher` +
  `IdentityBuildHasher` in `src/identity_hasher.rs`. All integer `write_*`
  methods specialised; XOR-fold fallback for bytes. Zero-sized `Copy`
  `BuildHasher`. 10 tests.

- **[2026-04-25] 6.6 extension: SSP serialisation (remaining 7 types).**
  `HashSetPool` (dedicated HAMT collector), `BagPool` (delegates to
  `HashMapPool<A, usize>`), `BiMapPool` (pools forward map only),
  `SymMapPool` (pools forward map only), `HashMultiMapPool` (flat pairs),
  `InsertionOrderMapPool` (ordered pairs), `TriePool` (flat path pairs).
  19 tests total.

- **[2026-04-25] 3.4: Parallel bulk ops.** `par_union`,
  `par_intersection`, `par_relative_complement`,
  `par_symmetric_difference` for HashMap and HashSet. Filter-map +
  fold/reduce pattern via rayon `par_iter()`. 10 tests.

- **[2026-04-25] 6.6 extension: SSP serialisation (OrdMap, OrdSet,
  Vector).** `OrdMapPool` with B+ tree node-level pooling.
  `OrdSetPool` as type alias with convenience methods. `VectorPool`
  with flat element-level serialisation. 11 tests total.

- **[2026-04-25] 6.6: SSP serialisation (HashMap).** `persist` feature
  with `HashMapPool`. Serde-based pool serialisation — writes each HAMT
  node once; shared nodes referenced by integer ID. Deserialisation
  extracts leaves and rebuilds via `FromIterator` (hasher-independent).
  Optional `InternPool` integration post-deserialisation. 8 tests.
  Design diverged from DEC-027: manual serde (not rkyv), HashMap only
  (not all 11 types), leaf extraction (not tree reconstruction).

- **[2026-04-25] 6.5: Hash consing / interning.** `hash-intern` feature
  with explicit `InternPool<A, P, H>`. HAMT nodes only. Bottom-up
  post-hoc interning (Appel's insight). Strong-reference pool with
  multi-pass `purge()` eviction. 19 tests including independently-built-
  identical-maps deduplication, COW correctness, cascading purge,
  collision node interning, stats accuracy.

- **[2026-04-25] dhat memory profiling.** `benches/memory.rs` with
  `dhat` dev-dependency measuring allocations per operation.

- **[2026-04-25] 4.7 Stage 1+2.** HashBits widened u32→u64, HashWidth
  trait threaded through entire HAMT stack.

- **[2026-04-25] 4.6: Vector Merkle hash.** Lazy per-node AtomicU64,
  O(k log n) recomputation. Positive equality in PartialEq.

- **[2026-04-25] 6.9: Persistent trie.** Derived structure wrapping
  HashMap. Full API: insert, get, remove, iter, subtrie, merge.

- **[2026-04-25] BiMap and SymMap collection types.** Added two new
  bidirectional map types: `BiMap<K, V>` (heterogeneous bijection with
  get_by_key/get_by_value, bijection invariant enforcement on insert) and
  `SymMap<A>` (symmetric bijection within a single type with `Direction`
  enum for parameterised lookups and O(1) `swap()`). Both backed by
  pairs of GenericHashMaps. Full standard trait coverage, serde support.
  Collection count: 9 → 11.

- **[2026-04-25] PBag → Bag rename.** Renamed `PBag` to `Bag` across
  all source, documentation, and serialisation. The `P` prefix was a
  stale convention from the imbl fork; all other types already use plain
  names.

- **[2026-04-25] Standard trait coverage fill.** Audited all 11
  collection types against the standard trait table in directives.md.
  Filled gaps: ConsumingIter (owned IntoIterator) for Bag, HashMultiMap,
  InsertionOrderMap; Hash for all types using order-independent XOR
  combiner; From conversions (Vec, slice, array) for all types; Add/Sum
  where applicable. All serde impls consolidated in ser.rs.

- **[2026-04-25] README comparison table.** Added side-by-side feature matrix
  comparing pds with rpds, im, and imbl — collections, backing structures,
  SIMD, Merkle hashing, no_std, and ecosystem features.

- **[2026-04-25] Remove bincode feature (DEC-025).** Deleted
  `src/bincode.rs`, removed bincode dependency, removed deprecated module
  from `lib.rs`, removed `-A deprecated` clippy allow. `cargo audit` now
  clean.

- **[2026-04-25] Remove deprecated difference aliases.** Removed
  `difference`, `difference_with`, `difference_with_key` from HashMap,
  HashSet, OrdMap, OrdSet. These were deprecated aliases for the
  `symmetric_difference*` methods inherited from imbl. No downstream users
  at v1.0.0, so removed rather than carrying the deprecation forward.

- **[2026-04-25] Rename crate from imbl to pds.** Version reset to 1.0.0.
  All internal references updated. imbl-sized-chunks dependency unchanged.

- **[2026-04-25] IntoIterator for Bag, HashMultiMap, InsertionOrderMap.**
  Added owned and borrowed IntoIterator with named ConsumingIter types.
  Bag yields (element, count), HashMultiMap yields flattened (key, value),
  InsertionOrderMap yields (key, value) in insertion order.

- **[2026-04-25] Merkle negative check for HashSet PartialEq.** HashSet
  now short-circuits equality to false when root Merkle hashes differ
  (HashMap already had this). Test deduplication: removed 3 redundant
  tests in ord/map.rs. Fixed unused import warning in hash/set.rs.

- **[2026-04-25] 4.7 Stage 1: Widen HashBits from u32 to u64.** Eliminated
  truncation of BuildHasher output. 12 usable trie levels (up from 6).
  Collision nodes virtually eliminated for collections under ~4B entries.
  Benchmarks: performance neutral at 100K–500K entries. Small keys (i32)
  pay +8 bytes/entry from alignment; larger keys pay nothing.

- **[2026-04-25] Docs review, coverage tests, and trait audit.**
  Fixed stale doc comments across crate (branching factor, OrdSet "map"→"set",
  broken links, missing features in README). Added ~100 coverage tests for
  ord/map.rs, ord/set.rs, hash/set.rs. Crate-wide coverage: 90.1% lines,
  86.4% functions (up from ~79%/76%). Trait audit: core 5 types complete;
  Bag/HashMultiMap/InsertionOrderMap lack IntoIterator (needs named iterators).

- **[2026-04-25] kv_merkle_hash for HashMap (DEC-021).** Added V: Hash
  key+value Merkle hash for O(1) positive equality. Two-tier API: public
  insert/remove maintain hash incrementally; internal helpers invalidate.
  19 unit tests + 2 proptests. Guard: positive equality only when hash
  width ≥ 64 bits (DEC-023).

- **[2026-04-25] Vector per-node lazy Merkle (DEC-022).** Two-level scheme:
  AtomicU64 per RRB node (lazy, Relaxed ordering) + merkle_hash/merkle_valid
  on GenericVector. O(k log n) recomputation where k = modified nodes.
  Positive equality in PartialEq with hash-width guard (DEC-023).
  27 unit tests + 2 proptests.

- **[2026-04-25] Merkle hash width guard (DEC-023).** Added
  MERKLE_HASH_BITS / MERKLE_POSITIVE_EQ_MIN_BITS constants in config.rs.
  All three positive equality sites (HashMap, HashSet, Vector) guarded.
  Compile-time elimination when both are 64.

- **[2026-04-25] Demotion edge case regression tests.** Added 12 tests in
  hash/map.rs covering all HAMT node upgrade/demotion paths using LolHasher
  for deterministic hash control. Guards against the proptest flake root cause
  (non-Value entry demotion).

- **[2026-04-25] CHAMP PoC artefacts removed (DEC-020).** Deleted
  `src/champ.rs`, `src/champ_v2.rs`, `src/nodes/champ_node.rs`, and
  `benches/champ.rs` (3,406 lines total). Three independent PoC attempts
  to replace/improve the HAMT all failed their gates (DEC-007, DEC-015,
  DEC-019). Dead code accrues maintenance cost; the analysis and lessons
  are preserved in decisions.md.

- **[2026-04-25] 6.8: Arena batch construction — KILLED (DEC-019).**
  Three approaches tried; all failed ≥15% improvement gate. The from_iter
  gap vs std is inherent to HAMT structure (~0.3 node allocs per element).

- **[2026-04-25] 6.3: ThinArc for node pointers — KILLED (DEC-018).**
  Premise invalid. `SharedPointer<T, ArcTK>` is already 8 bytes — archery's
  ArcTK backend wraps `triomphe::Arc<()>` with zero size overhead. No memory
  to save.

- **[2026-04-25] 6.7: Hybrid SIMD-CHAMP — KILLED (DEC-015).** Full prototype
  built and benchmarked. CHAMP v2 is 2-79% slower for lookups, 5-64% slower
  for mutations. Root cause: HAMT inline SIMD nodes avoid pointer indirection
  that CHAMP leaf nodes behind SharedPointer cannot match.

- **[2026-04-25] OrdMap B+ tree node size tuning (DEC-017).**
  `ORD_CHUNK_SIZE` increased from 16 to 32 based on Apple Silicon benchmarks.
  Lookup 8-21% faster, mutable ops 10-37% faster, iteration 10-12% faster.

- **[2026-04-24] 5.4: no_std support.**
  `#![cfg_attr(not(feature = "std"), no_std)]` with `extern crate alloc`.
  Replaced `std::` imports with `core::`/`alloc::` across ~30 files. Gated
  `RandomState`-dependent type aliases behind `#[cfg(feature = "std")]`.
  Generic variants always available. SpinMutex fallback for FocusMut.
  See DEC-012.

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
  large for a general-purpose library. The SIMD HAMT remains.
  Prototype removed (DEC-020).

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
  implementation which lacked front buffers entirely — pds already has
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
  `Bag<A>` (persistent multiset backed by HashMap<A, usize>),
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
  will be removed entirely in v2.0.0.

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

All phases complete. All Phase 6 research items resolved (5 killed, 1
deprioritised, 3 done). 11 collection types. See [Residual](#residual)
for the few remaining open items.

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

**Why:** HashMap and OrdMap have no fuzz targets. Focus/FocusMut are the
highest-risk unsafe code and are exercised by the unsafe audit (3.2).
Without fuzz coverage, subtle bugs in node manipulation or pointer
arithmetic will not be caught.

**Complexity:** Low. Existing targets provide templates.

**Prerequisite for:** 3.2 (unsafe audit).

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
fixes that benefit pds directly.

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
  academic papers.
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

### ~~1.3 Deprecate bincode feature~~ — DONE

Removed entirely at v1.0.0 (DEC-025). See Done section.

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

**Why:** This is the most powerful missing API in pds. It subsumes
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
single largest ergonomic gap in pds's map API. `try_map_values` (Haskell's
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

**What:** Add new collection types built on existing pds primitives,
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

4. **`Bag<A>` (Multiset)** — persistent unordered collection with
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
`Bag`; `arc-swap` crate (docs.rs/arc-swap); `indexmap` crate.

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

**Why:** pds is used in production by security-sensitive projects (Matrix
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

### 4.3 CHAMP integration — KILLED

**Status:** Permanently closed. Both 4.2 (basic CHAMP, DEC-007) and 6.7
(hybrid SIMD-CHAMP, DEC-015) showed that CHAMP does not outperform
imbl's SIMD HAMT in Rust. The HAMT's inline SIMD nodes and efficient
enum dispatch are structurally superior to CHAMP's pointer-chased leaf
nodes and dual-bitmap indexing. The existing HAMT is retained.

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

### 4.6 Vector Merkle hash caching

**What:** Maintain an incremental `u64` hash per RRB tree node, analogous
to 4.4 (HAMT Merkle hash). When comparing two Vectors, if root hashes
differ → definitely not equal → return `false` in O(1) without element
traversal.

**PoC gate:** Benchmark the per-mutation overhead (hash maintenance cost)
vs the equality fast-path gain. Go/no-go question: does the overhead pay
for itself when Vectors are frequently compared but rarely equal?

#### Design

**Key difference from HAMT:** The HAMT Merkle hash is commutative
(addition-based, order-independent) because hash maps are unordered.
Vector hashes must be **order-sensitive** — `[a, b]` and `[b, a]` must
produce different hashes. Use position-dependent mixing:
`hash(chunk) = XOR of fmix64(global_index ^ hash(element))`.

**Node structure change** (`src/nodes/rrb.rs`):

```rust
// Current: Node { children: Entry<A, P> }
// Proposed:
pub(crate) struct Node<A, P: SharedPointerKind> {
    children: Entry<A, P>,
    merkle_hash: u64,       // subtree hash, 0 if not computed
}
```

**Hash computation:**
- Leaf chunks (`Entry::Values`): `XOR of fmix64(global_offset + i) ^ hash(element[i])`
  for each element. Global offset is passed down from the parent during
  construction/mutation.
- Internal nodes (`Entry::Nodes`): `XOR of child.merkle_hash` values.
- Empty/Inline vectors: hash = 0 (sentinel, not used for comparison).

**Incremental maintenance:** Each mutation operation already does
path-copy (copy-on-write via `SharedPointer::make_mut`). The hash update
piggybacks on this path — after mutating a child, recompute the affected
leaf hash and propagate up. Cost: O(log n) hash recomputations per
mutation, same as the structural update.

**PartialEq fast path** (`src/vector/mod.rs` line 2123):

```rust
fn eq(&self, other: &Self) -> bool {
    self.ptr_eq(other)                                   // O(1) positive
    || (self.len() == other.len()
        && self.root_hashes_match(other)                 // O(1) negative
        && self.iter().eq(other.iter()))                  // O(n) fallback
}
```

Where `root_hashes_match` returns `true` if either hash is 0 (unknown)
or both hashes are equal. Only returns `false` (triggering the fast
negative) when both hashes are non-zero and differ.

**Constraint:** `A: Hash` is needed to compute element hashes. Since
`Vector<A>` does not require `Hash`, the hash is computed opportunistically:
- The field exists on all Nodes (8 bytes overhead)
- Hash is 0 (sentinel) unless explicitly set
- `Vector::with_merkle_hash()` constructor computes the hash once
- `push_back`/`set` etc. maintain the hash if the source had one
- `PartialEq` only uses the fast path when both sides have non-zero hashes

This avoids adding `A: Hash` to the type signature while still enabling
the fast path for types that do implement Hash.

#### Implementation steps

**Step 1: Add `merkle_hash` field to `Node`** (`src/nodes/rrb.rs`)
- Add `merkle_hash: u64` field to the `Node` struct (line 231)
- Initialize to 0 in all `Node` constructors: `new()`, `from_chunk()`,
  `single_parent()`, `join_dense()`, `join_branches()`, `parent()`
- Update `Clone`, `Debug`, and any derived impls
- **Files:** `src/nodes/rrb.rs`

**Step 2: Add hash computation for leaf chunks**
- Add `fn compute_leaf_hash(chunk: &Chunk<A>, global_offset: usize) -> u64`
  where `A: Hash`
- Uses `fmix64` from `src/nodes/hamt.rs` (make it `pub(crate)`)
- Position-dependent: `xor_fold(fmix64(offset + i) ^ std_hash(element))`
- **Files:** `src/nodes/rrb.rs`, `src/nodes/hamt.rs` (export fmix64)

**Step 3: Propagate hash through mutation operations**
- `Node::push_chunk` (line 644): after pushing, update parent merkle
- `Node::pop_chunk` (line 786): after popping, update parent merkle
- `Node::split` (line 847): recompute hash for both halves
- `Node::merge` / `concat_rebalance` (lines 966, 1153): recompute
- `Node::index_mut` (line 596): invalidate hash to 0 (can't recompute
  without `A: Hash` bound on `index_mut`)
- `RRB::push_back` / `push_front`: update outer buffer hash if maintained
- **Key insight:** `index_mut` cannot maintain the hash because it doesn't
  have `A: Hash` bound. Solution: invalidate to 0 on mutable access.
  The hash is most valuable for cloned-then-compared patterns where
  mutable access is infrequent.
- **Files:** `src/nodes/rrb.rs`, `src/vector/mod.rs`

**Step 4: Hash for the 4-buffer structure**
- RRB has 4 chunk buffers outside the tree (`outer_f`, `inner_f`,
  `inner_b`, `outer_b`). The top-level hash must incorporate these.
- Option A: Push buffers into tree before comparison (expensive)
- Option B: Store a separate `u64` for each buffer, combine at comparison
- Option C: Only use tree-level merkle for the `middle` node, combine
  with buffer hashes lazily
- **Recommendation:** Option B — 32 bytes of additional fields on RRB,
  but avoids any structural changes for comparison.
- **Files:** `src/vector/mod.rs` (RRB struct)

**Step 5: PartialEq fast path**
- Modify `GenericVector::eq()` to check root hashes before element
  comparison (only when both are non-zero)
- **Files:** `src/vector/mod.rs`

**Step 6: Public API**
- Add `Vector::merkle_hash() -> Option<u64>` — returns `Some` if hash
  is computed, `None` if invalidated (0)
- Add `Vector::compute_merkle_hash(&mut self)` where `A: Hash` —
  forces hash computation from scratch
- **Files:** `src/vector/mod.rs`

**Step 7: Benchmarks and PoC gate**
- Benchmark `push_back` overhead with merkle hash (before/after)
- Benchmark `PartialEq` for structurally-shared vectors with hash
- Go/no-go: if per-mutation overhead exceeds 10% on push_back, kill
- **Files:** `benches/vector.rs`

#### Test plan

- **Correctness:** Hash of `[a, b]` ≠ hash of `[b, a]`
- **Incremental:** Hash after `push_back(x)` matches full recompute
- **Invalidation:** Hash becomes 0 after `index_mut` access
- **PartialEq:** Fast negative on different-hash vectors
- **PartialEq:** No false negatives (same content, both with hash)
- **Proptest:** For `A: Hash`, `v1 == v2` iff element-by-element equal

**Affects:** `Vector<A>`.

**Prerequisites:** 0.1 ✓ (CI), 0.3 ✓ (Vector benchmarks), 0.5 ✓ (RRB
architecture docs). Benefits from 4.4 ✓ (HAMT Merkle hash — established
pattern).

**References:** Merkle trees (Merkle, 1987); 4.4 (HAMT Merkle hash —
implementation pattern and overhead analysis).

---

### 4.7 Pluggable hash width and fast-path hashing

**What:** Abstract the HAMT's internal hash representation to support
wider hashes and provide convenience hashers for well-distributed key
types.

**Current limitation:** `HashBits` is `u32` — `hash_key()` truncates
the `u64` output of `BuildHasher::hash_one()` to 32 bits. With 5 bits
per trie level, this gives 6.4 usable levels before hash exhaustion
triggers collision nodes. 32 bits of entropy means collision probability
reaches ~50% at ~65K entries (birthday bound) — collision nodes are hit
earlier and more often than the branching factor suggests.

**Design — three stages:**

1. **Widen HashBits to u64 (non-breaking).** Change `HashBits` from
   `u32` to `u64`. This eliminates a truncation that discards half the
   entropy `BuildHasher::hash_one()` already computes. 12.8 trie levels
   before exhaustion. Cost: +4 bytes per SIMD node entry. Go/no-go:
   benchmark the per-entry storage increase vs collision reduction at
   large collection sizes (100K+).

2. **Abstract hash width (breaking — v2.0.0).** Replace the concrete
   `HashBits` type with an associated type on a `HashWidth` trait:
   ```rust
   trait HashWidth {
       type Bits: Copy + Eq + ...;
       fn mask(hash: Self::Bits, shift: usize) -> usize;
       fn ctrl_hash(hash: Self::Bits) -> u8;
   }
   ```
   Default implementation uses `u64`. A `Wide` implementation uses
   `u128`. This is the const-generic-free path to configurable hash
   width — avoids the `generic_const_exprs` blocker that killed 5.3.

3. **Identity hasher for u128/UUID keys.** Provide an `IdentityHasher`
   (or integrate with `foldhash`'s passthrough path) that returns the
   key bytes directly as the hash value. For u128 keys that are already
   well-distributed (UUID v4/v7), this eliminates hash computation
   entirely. Combined with `HashWidth::Wide`, gives 25.6 trie levels
   from 128 bits of native key entropy — virtually zero collisions.

**Motivation:** Systems whose keys are inherently well-distributed
(UUIDs, cryptographic hashes, content-addressed identifiers) pay hash
computation overhead for no benefit. The HAMT then discards most of the
computed entropy via truncation. Both costs are avoidable. Azoth's data
model uses u128 Ids as all Map keys — the identity-hash + wide-hash
combination would eliminate both the hash computation and the collision
overhead.

**Scope:** Stage 1 (widen to u64) is a self-contained, non-breaking
change that can ship as v7.x. Stage 2 (trait abstraction) is breaking
and belongs in v2.0.0. Stage 3 (identity hasher) is a convenience
addition that can land with either stage.

#### Stage 1 implementation plan (widen to u64)

**Go/no-go question:** Does the +4 bytes per SIMD entry overhead pay for
itself via collision reduction at large collection sizes?

**Step 1: Change `HashBits` type** (`src/nodes/hamt.rs` line 21)
- Change: `pub(crate) type HashBits = u64;` (was derived from
  `BitsImpl<HASH_WIDTH>` which resolved to `u32`)
- Remove the `BitsImpl`/`Bits` machinery if it was only used for this
- The `fmix64` Merkle mixer already operates on `u64` — no change needed
- **Files:** `src/nodes/hamt.rs`

**Step 2: Remove truncation in `hash_key`** (line 49-50)
- Current: `bh.hash_one(key) as HashBits` — truncates u64 → u32
- After: `bh.hash_one(key)` — identity, since HashBits is now u64
- **Files:** `src/nodes/hamt.rs`

**Step 3: Update SIMD node entry storage**
- `GenericSimdNode` stores `SparseChunk<(A, HashBits), WIDTH>` — each
  entry grows by 4 bytes (HashBits u32 → u64)
- SmallSimdNode entry: was `(A, u32)`, now `(A, u64)` — alignment may
  pad this further depending on `A`
- LargeSimdNode: same growth pattern
- **Measure:** SmallSimdNode size increase (currently 224 bytes),
  LargeSimdNode (currently 432 bytes)
- **Files:** `src/nodes/hamt.rs`

**Step 4: Update hash bit extraction** (line 368)
- `let mask = (HASH_WIDTH - 1) as HashBits;` — mask is 31 (0x1F) for
  5-bit extraction. This works the same with u64.
- `(hash >> shift) & mask` — shift and mask operate identically on u64
- Verify all shift arithmetic is correct for 12.8 levels (max shift =
  60, which fits in u64)
- **Files:** `src/nodes/hamt.rs`

**Step 5: Update SIMD control hash**
- The SIMD control byte (`ctrl_hash`) uses the high bits of the hash to
  create a 7-bit fingerprint for parallel probing. With u64, we have
  more bits to work with — use bits 57-63 instead of bits 25-31
- Search for `ctrl_hash` or control byte computation and update
- **Files:** `src/nodes/hamt.rs`

**Step 6: Update collision threshold**
- Collision nodes are created when hash exhaustion occurs
  (shift + HASH_SHIFT >= HASH_WIDTH). With u32 (HASH_WIDTH=32) and
  HASH_SHIFT=5, this happened at shift=30 (6 levels). With u64
  (HASH_WIDTH=64), this happens at shift=60 (12 levels) — collisions
  become extremely rare
- No code change needed — the threshold is computed from constants
- **Benefit:** Virtually eliminates collision nodes for collections
  under ~4 billion entries

**Step 7: Update rayon module**
- `src/hash/rayon.rs` imports HASH_SHIFT and HASH_WIDTH — verify
  these still work correctly for work-splitting
- **Files:** `src/hash/rayon.rs`

**Step 8: Benchmark**
- Compare all hashmap operations before/after at 100, 1K, 10K, 100K
- Measure memory overhead: `std::mem::size_of` for SmallSimdNode,
  LargeSimdNode, HamtNode before and after
- Key metric: does collision reduction at 100K+ offset the per-entry
  storage increase?
- **Files:** `benches/hashmap.rs`

#### Stage 2 — DONE (2026-04-25)

HashWidth trait implemented and threaded through the entire HAMT stack.
The trait is defined in `src/hash_width.rs`:

```rust
pub trait HashWidth: Copy + Eq + Hash + Default + Debug + Send + Sync + 'static {
    fn from_hash64(hash: u64) -> Self;
    fn trie_index(&self, shift: usize) -> usize;
    fn ctrl_byte(&self) -> u8;
    fn ctrl_group(&self) -> u64;
    fn to_u64(&self) -> u64;
}
```

Impls for u64 (12 levels, default) and u128 (25 levels, wide). The `H`
parameter is added with `H: HashWidth = u64` default to:
- `GenericHashMap<K, V, S, P, H>`, `GenericHashSet<A, S, P, H>`
- `GenericHashMultiMap<K, V, S, P, H>`, `GenericInsertionOrderMap<K, V, S, P, H>`
- All HAMT node types, entry types, iterator types
- Serde Serialize/Deserialize impls

Merkle hashing always uses u64 via `H::to_u64()`. Rayon parallel
iterators use the u64 default (u128 rayon support deferred).

Files touched: 9 (hash_width.rs new, hamt.rs, map.rs, set.rs, rayon.rs,
hash_multimap.rs, insertion_order_map.rs, ser.rs, lib.rs).

#### Stage 3: Identity hasher

~50 lines. Provide `IdentityHasher` that passes through the key bytes
directly. For u64/u128 keys that are already well-distributed (UUIDs,
content hashes), this eliminates hash computation entirely.

```rust
pub struct IdentityHasher;
impl BuildHasher for IdentityHasher {
    type Hasher = IdentityHasherState;
    // ...
}
```

**Complexity:** Low. ~50 lines for the hasher implementation.

**Affects:** `HashMap<K, V>`, `HashSet<A>`.

**Prerequisites:** 0.1 ✓ (CI), 0.3 ✓ (HashMap benchmarks), 0.5 ✓
(HAMT architecture docs). Stage 1 is independent. Stage 2 should follow
stage 1 benchmarks. Stage 3 is independent of both.

**References:** foldhash crate (passthrough hashing for integer keys);
hashbrown `FixedState`; Swiss Tables hash representation (7-bit ctrl +
full hash); Steindorfer/Vinju OOPSLA 2015 (CHAMP uses full 32-bit hash,
5 bits per level).

---

## Phase 5 — Breaking API changes (v2.0.0) {#phase-5}

All items in this phase are breaking changes. They must be batched into a
single major version bump to minimise disruption for downstream users.
Ship as v2.0.0 when all are ready.

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

### 5.3 Configurable branching factor (issue [#145](https://github.com/jneem/imbl/issues/145)) — DEFERRED (nightly-gate path identified)

**Status:** Deferred. See DEC-011.

**Blocker:** Stable Rust cannot compute derived constants from const generic
parameters (`generic_const_exprs` is unstable, tracking issue
rust-lang/rust#76560). The HAMT's SIMD node hierarchy requires
`SparseChunk<..., 2^HASH_LEVEL_SIZE>` — this is a computed const generic
argument, which is not supported. Vector and OrdMap const generics are
feasible but the scope (~140 type reference sites, ~80 impl blocks) is
disproportionate to the marginal benefit over the existing `small-chunks`
feature flag.

**Nightly-gate approach (Apr 2026):** Add an opt-in `nightly-branching`
feature flag that enables `#![feature(generic_const_exprs)]`. The branching
factor constants in `config.rs` (`HASH_LEVEL_SIZE`, `VECTOR_CHUNK_SIZE`,
`ORD_CHUNK_SIZE`) become const generic parameters on the collection types.
This is a large refactor (~140 type sites, ~80 impl blocks) so it remains
deferred — but the nightly-gated approach is the path forward when
`generic_const_exprs` stabilises or when nightly-only usage is acceptable
for specific consumers.

**References:** imbl issue #145; PR #155; immer `BL` template parameter.

---

### 5.4 `no_std` support — DONE (DEC-012)

**What:** Make imbl usable in `no_std + alloc` environments.

**Implemented:** `#![cfg_attr(not(feature = "std"), no_std)]` with
`extern crate alloc`. Replaced `std::` imports with `core::`/`alloc::`
equivalents across ~30 source files. Gated `RandomState`-dependent
convenience type aliases and methods behind `#[cfg(feature = "std")]`.
Generic variants (`GenericHashMap` etc.) available in no_std — users
supply their own `BuildHasher`. Wrote `SpinMutex` fallback for
`FocusMut` interior mutability in no_std. Feature `std` is default-on.

**References:** imbl PR #149; hashbrown crate; DEC-012.

---

## Phase 6 — Research & speculative {#phase-6}

High-complexity items with uncertain payoff. Each requires a prototype and
benchmark before committing to integration.

### 6.1 Persistent Adaptive Radix Tree for OrdMap — DEPRIORITISED

**What:** Replace OrdMap's B+ tree with a persistent ART.

**Research outcome (DEC-014):** Not recommended. ART requires byte-
encodable keys (`K: ByteEncodable`), not generic `K: Ord` — a breaking
API change affecting ~280 downstream crates. No production persistent
ART for generic keys exists. Encoding overhead erodes ART's advantage
for small collections. DuckDB confirms range-scan limitations vs B-trees.
Better investments: tune `ORD_CHUNK_SIZE` ✓ (DEC-017, increased to 32),
branch-free intra-node search, bulk operations.

**Status:** Research complete. Deprioritised. See DEC-014.

---

### 6.2 HHAMT inline storage — KILLED (via 6.7)

**What:** Store small values inline in HAMT nodes instead of behind Arc
pointers.

**Research outcome (DEC-014):** Steindorfer's measurements show 55%
median memory reduction (maps), 78% (sets). Implemented in Scala 2.13,
Kotlin, Swift Collections 1.1. However, all use CHAMP as the base.
imbl's three-tier architecture already captures the spirit of inline
specialisation. Was merged into hybrid SIMD-CHAMP redesign (6.7).

**Kill reason:** Parent item 6.7 killed (DEC-015). The HAMT's inline
SIMD nodes already provide the performance benefits that HHAMT targets
in other implementations. No viable integration path remains.

---

### 6.3 ThinArc for node pointers — KILLED (DEC-018)

**What:** Use `triomphe::ThinArc` for internal nodes (header + variable-
length array behind a single thin pointer). Claimed to save 8 bytes per pointer.

**Kill reason (DEC-018):** Premise invalid. `SharedPointer<T, ArcTK>` is
already 8 bytes — archery's ArcTK backend wraps `triomphe::Arc<()>` with zero
size overhead. Measured: all pointer types (HamtNode, SmallSimdNode,
CollisionNode) are 8 bytes. No memory to save.

---

### 6.4 `dupe::Dupe` trait support (issue [#113](https://github.com/jneem/imbl/issues/113)) — KILLED

**What:** Implement Meta's `Dupe` trait. Mechanical — delegates to `clone()`.

**Research outcome (DEC-014):** dupe ecosystem is narrow (Meta-internal).
`light_clone` crate (Feb 2026) already provides `LightClone` for imbl
types externally. If proceeding: optional feature flag, 5 impl blocks.

**Kill reason:** Meta-internal ecosystem with negligible external adoption.
`light_clone` crate already provides the functionality externally without
requiring a feature flag in pds. Not worth maintaining even 5 lines of
delegation for a trait ecosystem that has no traction outside Meta.

**Complexity:** Trivial.

**Affects:** All collection types.

---

### 6.5 Hash consing / interning (compile-time feature) — DONE

**What:** Opt-in `hash-intern` feature with explicit `InternPool<A, P, H>`.
HAMT nodes only. Bottom-up post-hoc interning (Appel's insight).

**Implementation:**
- `src/intern.rs` — `InternPool` struct with `intern_hamt`, `intern_small`,
  `intern_large`, `intern_collision` methods. Strong-reference pool with
  multi-pass `purge()` eviction (loops until stable to handle parent→child
  chains). `InternStats` for hit/miss/eviction tracking.
- `src/nodes/hamt.rs` — `Entry::intern()` recursive method (children before
  parents). Structural equality checks use `ptr_eq` fast path for interned
  children.
- `src/hash/map.rs` — `GenericHashMap::intern(&mut pool)` public API.
- `src/hash/set.rs` — `GenericHashSet::intern(&mut pool)` public API.
  `HashSetInternPool` type alias hides the internal `Value<A>` wrapper.

**Key design decisions (vs research plan):**
- Explicit pool (not global/thread-local) — Rust can't have generic statics.
  Matches `hashconsing` crate's approach.
- Strong references with `purge()` (not weak references) — `triomphe::Arc`
  doesn't support weak refs. Purge loops until stable to handle cascading
  eviction.
- Deduplication by Merkle hash + structural equality (not just Merkle) —
  guards against hash collisions.

**Tests (19):** independently-built-identical-maps ptr_eq, COW correctness,
re-intern after mutation, cascading purge, collision node interning, stats
accuracy, idempotent re-intern, many overlapping maps, HashSet interning.

**Affects:** HAMT-backed types (HashMap, HashSet, and by extension
HashMultiMap, InsertionOrderMap, BiMap, SymMap, Trie).

**References:** Filliâtre & Conchon (2006); `hashconsing` crate;
Appel (1993).

---

### 6.6 Structural-sharing-preserving serialisation — DONE (HashMap only)

**What:** Pool-based serde serialisation that writes each HAMT node once
and references shared nodes by integer ID.

**Implementation:** `src/persist.rs` — `HashMapPool<K, V, H>` struct with
manual `Serialize`/`Deserialize` impls. Feature flag `persist` (requires
`std`, `serde_core`, `hash-intern`).

- **Serialise (`HashMapPool::from_maps`):** `PoolCollector` walks HAMT
  tree post-order, deduplicates by pointer address, assigns integer IDs.
  Tagged node format: `{"h": ...}` for HamtNode, `{"s": ...}` for
  SmallSimd, `{"l": ...}` for LargeSimd, `{"c": ...}` for Collision.
- **Deserialise (`HashMapPool::to_maps`):** Extracts all (K,V) leaf pairs
  from the pool tree, then rebuilds via `FromIterator` with the default
  hasher. This is hasher-independent — HAMT tree structure depends on the
  hasher, so reconstructing the original tree layout is impossible with a
  different `RandomState`. The leaf-extraction approach avoids this.
- **InternPool integration:** Post-deserialisation, users can call
  `map.intern(&mut pool)` to deduplicate across maps. Not automatic
  during deserialisation (unlike the original DEC-027 design).

**Design divergences from DEC-027:**
- Uses manual serde (not rkyv, not derive macros — `serde_core` doesn't
  re-export derive macros).
- Scope: HashMap only (not all 11 types yet). B+ tree and RRB tree nodes
  would need separate pool types.
- No `PoolBuilder`/`PoolReader` — simpler `HashMapPool::from_maps` /
  `to_maps` API.
- Leaf extraction instead of tree reconstruction on deserialisation.

**Tests (8):** roundtrip single/large maps, `get()` correctness after
roundtrip, shared-node deduplication in pool, two-map roundtrip, empty
map, intern-after-deserialise deduplication.

**Affects:** HashMap, HashSet (via wrapper). Other types future work.

**References:** immer `persist.hpp`; DEC-027.

---

### 6.7 Hybrid SIMD-CHAMP prototype — KILLED (DEC-015)

**Status:** PoC gate failed. Full prototype built and benchmarked; CHAMP v2
with SIMD leaf probing is 2-79% slower for lookups and 5-64% slower for
mutations compared to the existing HAMT. See DEC-015 for full analysis.

**Root cause:** The HAMT stores SIMD nodes (SmallSimdNode, LargeSimdNode)
inline within the Entry enum — zero pointer indirection. CHAMP stores Leaf
nodes behind SharedPointer, adding an extra pointer chase and cache miss at
every bottom-level access. Two-bitmap indexing is not cheaper than enum
dispatch in Rust (branch prediction handles the 5-way match efficiently).

**Key lesson:** The JVM-centric CHAMP design (Steindorfer/Vinju OOPSLA 2015)
does not translate to a Rust performance advantage because: (1) JVM already
pays pointer indirection for all objects, while Rust can store data inline
in enums; (2) Rust's enum discriminant match compiles efficiently with
branch prediction; (3) CHAMP's contiguous-array advantage (from the Java
version) is lost when using SparseChunk with the same allocation pattern.

**Prototype removed:** `src/nodes/champ_node.rs`, `src/champ_v2.rs`,
`src/champ.rs`, and `benches/champ.rs` deleted. All benchmark data and
analysis preserved in DEC-007, DEC-015, and DEC-020. See DEC-020 for
removal rationale.

---

### 6.8 Arena-backed batch construction — KILLED (DEC-019)

**Status:** PoC gate failed. Three approaches tried (Vec-of-Vecs
partitioning, pre-allocated partitioning, in-place American Flag sort);
all failed the ≥15% improvement gate. The from_iter gap vs std is
inherent to HAMT structure (~0.3 node allocations per element via
Arc::new). See DEC-019 for full analysis and profiling data.

---

### 6.9 Persistent trie — DONE

**What:** A purpose-built persistent trie (prefix tree) data structure
with structural sharing at every prefix node. Keys are sequences of
segments (`K: Clone + Eq + Hash`); values are stored at interior and/or
leaf positions.

**Motivation:** Hierarchical namespaces, path-based routing, locale
resolution, and symbol tables are natural trie workloads. The derived
approach — recursive `HashMap<K, TrieNode<K, V>>` — works but carries
HAMT overhead per trie node (SIMD groups, SparseChunk, hash storage)
that is disproportionate for nodes with 1–3 children, which are the
common case in most trie workloads. A native trie can right-size each
node to its actual fanout and support trie-specific operations
(longest prefix match, prefix collection, path compression) without
mapping them onto hash-map semantics.

Azoth's Naming Subsystem (`SYS_NS_TRIE_INDEX`) is the primary
motivating use case: Id-segment paths, shallow depth (2–5 levels),
mixed fanout (some nodes wide, many narrow), heavy prefix queries.
The derived `HashMap`-per-level approach is the recommended starting
point for Azoth; this item explores whether a native trie justifies
its implementation cost for the general case.

**Open questions (research required before design):**

1. **Node representation.** Flat sorted array for small fanout
   (≤8 children), HAMT-like bitmap node for medium fanout (9–32),
   full HashMap delegation for wide fanout (33+)? Or a single
   adaptive node type that grows? The HAMT's 3-tier hierarchy
   (SmallSimdNode → LargeSimdNode → HamtNode) is a reference
   point for fanout-adaptive nodes.
2. **Path compression.** Patricia/compressed trie merges chains of
   single-child nodes into one node with a multi-segment key. Worth
   the complexity? Depends on expected key distribution — high value
   for file-path-like keys with long shared prefixes, low value for
   short fixed-depth keys.
3. **Structural sharing model.** Each trie node behind `SharedPointer`
   (same as HashMap/OrdMap)? Or arena-backed with CoW at the subtree
   level? The former integrates with imbl's existing infrastructure;
   the latter may be more memory-efficient for large tries.
4. **Trait bounds.** Segments need `Eq` for matching. `Hash` enables
   HAMT-style child lookup for wide nodes. `Ord` enables sorted
   iteration and range-prefix queries. Minimum bound: `Eq + Clone`.
   Recommended: `Eq + Hash + Clone` for performance.
5. **API surface.** What operations beyond insert/get/remove? Prefix
   iteration (`iter_prefix`), longest prefix match
   (`longest_prefix`), subtree extraction (`subtrie`), structural
   merge. Which of these justify a native type vs being achievable
   on the derived `HashMap` approach?
6. **Benchmark target.** What workload demonstrates the native trie
   outperforming the derived `HashMap<K, TrieNode>` approach by
   enough to justify the implementation? Memory usage on narrow
   tries (many 1–3 child nodes) is the likeliest win.

**PoC gate:** Build a standalone prototype with the minimal API
(insert, get, remove, iter_prefix, longest_prefix) and benchmark
against the derived HashMap approach at representative workloads:
narrow tries (file paths), wide tries (DNS labels), mixed tries
(namespace hierarchies). Go/no-go on memory usage and prefix query
performance.

**Prior art:**
- `patricia_tree` crate — persistent Patricia trie for bit-string keys
- `sequence_trie` crate — generic sequence trie (not persistent)
- Clojure's `PersistentHashMap` — HAMT, not a trie, but the structural
  sharing model is the reference
- Haskell `Data.Trie` — bytestring trie with Patricia compression
- Scala `TrieMap` — concurrent trie map (different problem, but
  adaptive node sizing is relevant)
- Erlang/OTP `gb_trees` — general balanced trees used for prefix
  matching in routing tables

**Complexity:** High. New data structure module, new node types, full
test/bench/fuzz/proptest coverage. Reuses SharedPointer infrastructure.

**Affects:** New type. No changes to existing collections.

**Prerequisites:** 0.1 ✓ (CI), 0.3 ✓ (benchmarks for comparison
target).

---

## Residual {#residual}

Open items not yet completed or killed. All prerequisites met.

### 3.4: Parallel bulk operations — DONE

**What:** Parallel `union`, `intersection`, `difference`,
`symmetric_difference` for HashMap/HashSet via rayon.

**Status:** DONE. All parallel operations implemented:
- `par_union`, `par_intersection`, `par_relative_complement`,
  `par_symmetric_difference` for both HashMap and HashSet.
- Uses filter_map + fold/reduce pattern with rayon's `par_iter()`.
- `par_symmetric_difference` uses `rayon::join` for two-way parallelism.
- 10 tests (8 operations + empty + disjoint edge cases).
- Avoids `V: Hash` requirement by using `insert_invalidate_kv`.

---

### 4.7 Stage 3: Identity hasher — DONE

**What:** `IdentityHasher` and `IdentityBuildHasher` in `src/identity_hasher.rs`.
Passes key bits directly as the hash value for integer keys.

**Status:** DONE [2026-04-25]. `IdentityHasher` with specialised `write_*` methods
for all integer types (u8–u128, usize, all signed variants). XOR-fold fallback for
byte slices. `IdentityBuildHasher` is zero-sized and `Copy`. 10 tests covering
identity property, map/set integration at 1000 entries. Exposed as
`pds::identity_hasher::{IdentityHasher, IdentityBuildHasher}`.

**Dependencies:** None (4.7 stages 1+2 done).

---

### 5.3: Configurable branching factor — DEFERRED

**Status:** Blocked on `generic_const_exprs` stabilisation (tracking
issue rust-lang/rust#76560). Nightly-gate approach identified but the
scope (~140 type sites, ~80 impl blocks) is disproportionate to the
benefit over the existing `small-chunks` feature flag. See DEC-011.

**Dependencies:** `generic_const_exprs` stabilisation.

---

### 6.1: Persistent ART for OrdMap — DEPRIORITISED

**Status:** Research complete (DEC-014). Not recommended — ART requires
`K: ByteEncodable`, not `K: Ord`. No production persistent ART for
generic keys exists. Better OrdMap investments: tune chunk size (done,
DEC-017), branch-free intra-node search, bulk operations.

**Dependencies:** None, but questionable ROI.

---

### 6.6 extension: SSP serialisation for remaining types — DONE

**What:** Extend pool-based serialisation from HashMap to all 11 collection types.

**Status:** DONE [2026-04-25]. All 11 types covered:

Deep HAMT pooling (full SSP — shared nodes deduplicated by pointer address):
- `HashSetPool<A, H>` — dedicated pool collector for `HamtNode<Value<A>>`,
  unwraps `Value` wrapper. 4 tests.
- `BagPool<A>` — delegates to `HashMapPool<A, usize>` via `bag.map`. 2 tests.
- `BiMapPool<K, V, H>` — pools forward `HashMap<K, V>`, rebuilds backward
  map on deserialisation. 2 tests.
- `SymMapPool<A, H>` — pools forward `HashMap<A, A>`, rebuilds backward
  map on deserialisation. 2 tests.

Flat serialisation (correct, compact; no deep HAMT pooling):
- `HashMultiMapPool<K, V>` — flat `(K, V)` pairs per container. 2 tests.
- `InsertionOrderMapPool<K, V>` — ordered `(K, V)` pairs; insertion order
  preserved, internal indices compacted to 0…n on deserialisation. 2 tests.
- `TriePool<K, V>` — flat `(Vec<K>, V)` path pairs per container. 3 tests.

Previously done: `OrdMapPool<K, V>`, `OrdSetPool<A>`, `VectorPool<A>`,
`HashMapPool<K, V, H>`.

**Dependencies:** 6.6 ✓ (HashMap implementation as template).

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
  1.3 bincode removal    ✓ DONE    │                                      │
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
  4.2 CHAMP prototype ◄── 0.3, 0.5  ✓ DONE (DEC-007: HAMT retained)      │
  4.3 CHAMP integration ◄── 4.2  ✗ KILLED (DEC-007/015: HAMT retained)   │
  4.4 Merkle hash caching ◄── 0.3, 0.5  ✓ DONE                           │
  4.5 SharedPointer hasher PoC ◄── 5.2  ✓ DONE                            │
  4.6 Vector Merkle hash ◄── 0.3 ✓, 0.5 ✓ (benefits from 4.4 ✓ pattern)  │
  4.7 Pluggable hash width ◄── 0.3 ✓, 0.5 ✓ (stage 2 → v2.0.0)          │
                                   │                                      │
Phase 5 (breaking — v2.0.0)        │                                      │
  5.1 triomphe default ◄── 0.3, 0.4  ✓ DONE (DEC-010)                     │
  5.2 remove Clone bounds ◄── 3.1  ✓ DONE                                │
  5.3 const generic branching ◄── 4.3  ✗ DEFERRED (DEC-011: stable Rust blocker) │
  5.4 no_std ◄── 4.3 (if proceeding)  ✓ DONE (DEC-012)                    │
                                   │                                      │
Phase 6 (research)                 │                                      │
  6.1 ART for OrdMap ◄── 0.2, 0.3  ✗ DEPRIORITISED (DEC-014)             │
  6.2 HHAMT inline ◄── 4.3  ✗ KILLED (via 6.7 — DEC-015)                  │
  6.3 ThinArc ◄── 5.1 ✓  ✗ KILLED (DEC-018: pointers already 8 bytes)      │
  6.4 Dupe trait ◄── (none)  ✗ KILLED (Meta-internal, light_clone exists)     │
  6.5 hash consing/interning ◄── 4.4 ✓  ✓ DONE                             │
  6.6 sharing-preserving serialisation ◄── 0.5, 6.5 ✓  ✓ DONE (HashMap, OrdMap, OrdSet, Vector) │
  6.7 hybrid SIMD-CHAMP ◄── 0.3, 0.5  ✗ KILLED (DEC-015: PoC failed)     │
  6.8 arena batch construction ◄── (none)  ✗ KILLED (DEC-019: PoC failed)  │
  6.9 persistent trie ◄── 0.3 ✓  ✓ DONE (derived HashMap wrapper)                          │
```

### Parallel tracks — status

All major tracks complete. Remaining open items listed in [Residual](#residual).

1. **Vector track:** ✓ COMPLETE (2.1, 4.1, 4.6 all done)
2. **Hash track:** ✓ COMPLETE (4.2→4.3✗→6.7✗→6.8✗; 4.7 stage 1+2 done;
   stage 3 identity hasher is residual)
3. **Mutation track:** ✓ COMPLETE (3.1→3.2→3.3→5.2→4.5 all done)
4. **Parallel track:** ✓ COMPLETE (3.4 par_iter/par_iter_mut/par_sort ✓;
   parallel bulk ops — par_union/par_intersection/par_relative_complement/
   par_symmetric_difference for HashMap+HashSet ✓)
5. **Diff track:** ✓ COMPLETE (2.4→2.5→2.6→3.6, 3.5 all done)
6. **Map API track:** ✓ COMPLETE (2.7, 2.8, 2.9, 2.10, 2.11 all done)
7. **Hash integrity track:** ✓ COMPLETE (4.4→6.5→6.6 all done)
8. **Serialisation track:** ✓ COMPLETE (6.6 done: HashMap, HashSet via
   HashMapPool; OrdMap, OrdSet via OrdMapPool; Vector via VectorPool)
9. **Trie track:** ✓ COMPLETE (6.9 done)

---

## References {#references}

See `docs/references.md` for the full bibliography — papers, implementations,
and Rust crates referenced by plan items above.
