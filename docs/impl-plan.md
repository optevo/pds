# pds — Implementation Plan

Sequenced implementation plan for pds (persistent data structures for
Rust). Forked from [imbl](https://github.com/jneem/imbl) with different
design priorities: performance over compatibility, Merkle hashing, SIMD
HAMT nodes, and no_std support.

**Current state (Apr 2026):** v1.0.0, ~12K lines of Rust, 20 collection
types (Vector, HashMap, HashSet, OrdMap, OrdSet, Bag, OrdBag, HashMultiMap,
OrdMultiMap, InsertionOrderMap, OrdInsertionOrderMap, InsertionOrderSet,
OrdInsertionOrderSet, BiMap, OrdBiMap, SymMap, OrdSymMap, Trie, OrdTrie,
UniqueVector).
SIMD HAMT, Merkle hashing, and no_std support implemented.

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

- **[2026-07-01] Phase T.0 — Core tiered write-behind infrastructure.**
  New `tiered` feature in the pds root crate. No folio or merkle-spine dependency.

  Deliverables:

  1. `src/tiered/backend.rs` — `CollectionBackend<K, V>` trait: `get`, `insert`,
     `remove`, `len`, `is_empty`, `load_from`, `drain`, `snapshot` (default).
     `Send + 'static` required; `drain` leaves backend empty; `load_from` clears
     before inserting.

  2. `src/tiered/backends.rs` — three concrete backends:
     - `StdHashMapBackend<K, V>` — wraps `std::collections::HashMap`. O(1) amortised
       ops; O(n) clone. Recommended hot-tier backend.
     - `PdsHashMapBackend<K, V>` — wraps `pds::HashMap` (HAMT). Functional API:
       each insert/remove stores a new map in place. O(1) clone via structural sharing.
     - `MerkleWrapperBackend<K, V>` — wraps `MerkleWrapper<pds::HashMap<K,V>>`. BLAKE3
       Merkle root changes with every mutation. Gated on `#[cfg(feature = "traits")]`.

  3. `src/tiered/policy.rs` — `PropagationPolicy` enum: `Immediate`, `Batched(usize)`,
     `Timed(Duration)`, `Manual`.

  4. `src/tiered/mod.rs` — `TieredCollection<K, V, Hot, Cold>`:
     - Internal state in `Arc<Mutex<TieredState<…>>>` — O(1) clone, `Send + Sync`.
     - `pending_deletes: HashSet<K>` masks cold-tier values for recently deleted keys
       until next flush.
     - Public API: `new`, `insert`, `get`, `remove`, `flush`, `cold_snapshot`,
       `hot_snapshot`, `len` (approximate), `is_empty`, `start_background_propagation`.
     - `PropagationHandle` with `Drop` impl (stop signal + join).
     - `CollectionBackend<K, V>` impl on `TieredCollection` — enables three-tier
       recursive composition.

  5. `src/tiered/tests.rs` — 14 tests: hot-only get, flush-to-cold, delete-before-flush,
     delete-after-propagation, flush-clears-deletion-mask, Batched(3) auto-flush,
     Immediate always-current, Manual never-auto-flushes, concurrent inserts (2 threads
     × 50 each), three-tier composition with MerkleWrapper root change (gated on
     `traits`), cold_snapshot independence, re-insert deleted key, is_empty both tiers,
     `CollectionBackend` drain/load, Timed policy background propagation.

  Cargo.toml: `tiered = ["std"]` feature. No new external deps.
  `lib.rs`: `pub mod tiered`, re-exports for `TieredCollection`, `PropagationPolicy`,
  `PropagationHandle`, `StdHashMapBackend`, `PdsHashMapBackend`, `MerkleWrapperBackend`.

  All three test variants green: `--features tiered` (425 tests), `--features "tiered traits"`
  (425 tests), `--all-features` (441 tests). `cargo clippy --all-features -- -D warnings`
  clean. `cargo doc --no-deps --all-features` no warnings.

- **[2026-07-01] Architectural improvements — MerkleWrapper, decision log, trait boundary note, selection guidance.**
  Four improvements from the post-H.8 architectural review:

  1. `MerkleWrapper<C, K, V>` (`src/merkle_wrapper.rs`, `traits` feature) — content-addressed
     Merkle identity over any `PersistentMap<K, V>`. In-memory alternative to
     `pds-merkle-spine::VersionedHamt`; no folio dependency. Implements
     `PersistentCollection`, `PersistentMap`, `VersionedPersistentMap`, and
     `MerklePersistentMap`. `VersionId = [u8; 32]` (the BLAKE3 Merkle root). Root hash
     computed from sorted leaf hashes via a binary Merkle tree (postcard serialisation +
     BLAKE3). Cached in `OnceLock`; not propagated through `Clone` (hash is cheap to
     recompute and deterministic). `prove_inclusion` / `verify_proof` use sibling-hash
     proof path. New optional deps: `blake3 = "1"`, `postcard = "1"` (both behind
     `traits` feature gate).

  2. `docs/decisions.md` additions:
     - DEC-DURABLE-1: role boundary between `pds-durable` (explicit checkpoint/fsync)
       and `pds-folio` disk backend (transparent page-level persistence). Deferred
       deprecation evaluation until folio disk backend lands.
     - DEC-ARCH-MERKLE: two-tier Merkle capability (`MerkleWrapper` for in-memory
       identity; `VersionedHamt` for persistent versioned history).

  3. Trait boundary note in `src/traits.rs` — explains why `pds-durable` types do not
     implement `PersistentCollection` (durability is orthogonal to structural sharing).

  4. Backend selection table in `pds-folio/src/lib.rs` — crate-level guidance on
     when to use pds vs pds-folio vs MerkleWrapper vs pds-merkle-spine vs pds-durable.

  Tests: 20 unit tests in `src/merkle_wrapper.rs` covering root hash properties,
  round-trip stability, clone behaviour, get_at/checkout version identity, proof
  generation and verification, tampered-value rejection, diff semantics, and
  PersistentMap delegation. All tests green; `test.sh` passes.

- **[2026-07-01] pds-merkle-spine H.0–H.8 — `VersionedHamt` full implementation.**
  New workspace member `pds-merkle-spine` (crate `pds-merkle-spine`): thin facade
  combining `pds-folio`'s `HamtMap<K,V,C,B>` with `merkle-spine`'s BLAKE3 hash primitives.

  Key types:
  - `VersionId { seq: u64, root_hash: [u8; 32] }` — stable, cheaply-copied version handle.
  - `VersionedHamt<K, V, C, B>` — persistent, versioned, Merkle-verified hash map.
  - `MerkleProof { root_hash, key_hash, value_hash, siblings }` — inclusion proof.
  - `DiffEntry<K, V>` enum — `Inserted`, `Removed`, `Updated` entries from `diff()`.

  Architecture: `VersionHistory<K,V,C,B>` (shared via `Arc<Mutex<…>>`) stores full
  `HamtMap` clones per version entry, keeping folio refcounts permanently alive.
  Storing page IDs alone was tried and discarded: pages are freed when the owning
  `HamtMap` is dropped, so historical `get_at` calls would silently return `None`.
  Cloning a `HamtMap` is O(1) — it only increments a root page refcount.

  Public API:
  - `new`, `len`, `is_empty`, `iter`, `version`, `root_hash`, `root_hash_at`
  - `insert`, `remove`, `get`, `contains_key`
  - `get_at` — O(log N) historical point lookup without materialising the full version
  - `checkout` — O(1) historical version branch (clone the stored snapshot)
  - `diff(from, to)` — O(changed × log N) structural diff between any two versions
  - `prove_inclusion`, `prove_inclusion_at`, `verify_proof` — O(log N) Merkle proofs

  Trait impls: `PersistentCollection`, `PersistentMap`, `VersionedPersistentMap`,
  `MerklePersistentMap` from the `pds::traits` feature.

  Merkle root: iterate all K/V pairs, sort by serialised key, concatenate
  `(key_len LE32 || key_bytes || val_len LE32 || val_bytes)`, hash with BLAKE3
  `ms:hamt-node-v1` domain key.

  Tests:
  - 30 unit tests (`src/versioned_hamt.rs`) covering all API methods, trait impls, edge cases.
  - 11 integration tests (`tests/versioned_hamt_integration.rs`): large insert/remove
    sequences, structural diff, snapshot isolation, Merkle proof round-trips, cross-crate
    trait usage.
  - 2 proptest property tests (20 cases each): historical value correctness, diff
    inverse-of-mutations.
  - 41/41 tests green; `cargo fmt --check` clean; `cargo clippy -D warnings` clean;
    full workspace `test.sh` (9 steps) passes.

  Removed `root_page_id` and `snapshot_at_root` from `pds-folio`'s `HamtMap` — added
  for the first (page-ID-based) VersionHistory design but unused by the final design.

- **[2026-07-01] pds-folio G.2 — `HamtMap` CRUD.**
  `pds-folio/src/hamt.rs`: `HamtMap<K, V, C, B>` with path-copy insert/remove/get.
  `NodeStore<B>` wraps `FolioStore<B>` for typed `HamtNodePage` page I/O; shared
  via `Arc<Mutex<…>>` across snapshots. Leaf split via `build_trie_from_entries`
  partitions entries by 5-bit hash slices into a subtrie. 10 unit tests + 1 doctest;
  38 tests total in pds-folio; all green; clippy clean; full workspace `test.sh` passes.

- **[2026-07-01] pds-folio G.1 — Core node types and slab layout.**
  `src/node.rs` in `pds-folio`: `HamtNodePage([u8; 512])` Pod slab slot type;
  fixed-header leaf layout (2 + 128 + 34 + 348 bytes = discriminant + hashes +
  offsets + data) with `LeafBuilder` / `LeafReader`; 5-bit bitmap internal node
  with `build_internal` / `InternalReader`. `LEAF_CAP = 16`, `BRANCH_BITS = 5`.
  13 tests: size checks, Pod round-trip, PostcardCodec and PodCodec leaf round-trips,
  overflow rejection, all-32-children internal round-trip. All green, clippy clean,
  full workspace `test.sh` passes.

- **[2026-06-30] pds-folio G.0 — Scaffold.**
  Created `pds-folio` as a Cargo workspace member of the `pds` repo.
  `pds-folio/Cargo.toml` with deps: `folio-core` (path), `folio-collections` (path),
  `pds` (workspace, traits feature), `serde`, `postcard`, `bytemuck`, `thiserror`.
  `pds-folio/src/lib.rs` with `#![deny(unsafe_code)]` and module declarations.
  `pds-folio/src/codec.rs`: `Codec` trait, `PodCodec` (raw bytes + postcard fallback),
  `PostcardCodec` — 9 unit tests + 5 doctests, all green.
  `pds-folio/docs/impl-plan.md` (G.1–G.15 items in Future).
  Full workspace `test.sh` gate (9 steps including workspace smoke check and audit) passes.
  pds-folio doctests pass as `Doc-tests pds_folio` under the workspace run.

- **[2026-06-30] Phase W.0 + W.1 — Workspace consolidation.**
  Added `[workspace]` table to root `Cargo.toml` with `members = []`,
  `resolver = "2"`, and a `[workspace.dependencies]` stub for `pds` itself.
  `cargo metadata` confirms workspace root at `/Users/rd/projects/pds`.
  `build.sh` updated to pass `--workspace`; `test.sh` adds a
  `cargo test --workspace` smoke-check step. CI (`ci.yml`) adds
  `cargo test --workspace` to the test matrix and switches clippy to
  `--workspace`. `docs/architecture.md` gains a Workspace layout section with
  the three-crate dependency diagram. `test.sh` green with all nine steps
  (fmt, test ×3, check, clippy, doc, workspace smoke, audit).

- **[2026-06-30] Phase F.0 + F.1 — Cross-variant trait layer (`src/traits.rs`).**
  Defined the portable trait hierarchy covering all five in-memory pds collection
  types. New `traits` Cargo feature (requires `std`) gates the module.

  Traits defined:
  - `PersistentCollection` — marker; O(1)-clone guarantee
  - `PersistentMap<K, V>` — hash-keyed map; `get_cloned`, `insert`, `remove`, `len`, `contains_key`
  - `PersistentSet<A>` — hash set; `contains`, `insert`, `remove`, `len`
  - `PersistentVector<A>` — RRB sequence; `get`, `push_back`, `push_front`, `update`,
    `pop_back`, `pop_front`, `concat`, `split_at`, `len`
  - `PersistentOrdMap<K, V>` — sorted map; adds `first`, `last`, `range`
  - `PersistentOrdSet<A>` — sorted set; adds `first`, `last`, `range`
  - `DiffEntry<K, V>` — diff enum (`Inserted`, `Removed`, `Updated`)
  - `VersionedPersistentMap<K, V>` — versioned map with `version`, `get_at`, `checkout`, `diff`
  - `MerklePersistentMap<K, V>` — BLAKE3 Merkle identity with `root_hash`, proofs

  Impls provided for:
  - `GenericHashMap<K, V, S, P, H>` — PersistentCollection + PersistentMap (needs `V: Hash`)
  - `GenericHashSet<A, S, P, H>` — PersistentCollection + PersistentSet
  - `GenericVector<A, P>` — PersistentCollection + PersistentVector
  - `GenericOrdMap<K, V, P>` — PersistentCollection + PersistentOrdMap
  - `GenericOrdSet<A, P>` — PersistentCollection + PersistentOrdSet

  Design note: `PersistentMap` impls on HashMap require `V: Hash` (not just `V: Clone`) because
  the HAMT implementation hashes values for Merkle node hashes. This is an inherent
  constraint of the HAMT structure, not a limitation of the trait design.

  Tests: 9 tests in `src/traits.rs` covering all five trait families plus DiffEntry and
  the PersistentCollection marker. All 1709 library tests + doc tests green.
  Edge case review: boundary conditions (empty, single-element, absent keys) covered
  by generic helper functions. Recursive method call ambiguity avoided by calling
  `crate::vector::GenericVector::pop_back` / `pop_front` explicitly.

  Phase F.1 (VersionedPersistentMap + MerklePersistentMap trait definitions) is included
  in this same file; the traits compile without concrete implementations. F.1 is
  accepted as part of this commit — folio-backed and merkle-spine impls are Phase G/H.

- **[2026-04-27] Range view API — OrdMap/OrdSet split and materialisation refactor.**
  Completed three related improvements across `OrdMap`, `OrdSet`, and `Vector`:

  (1) **O(log n) `OrdMapRange::to_map()` (rayon fast path).**
  Added a conditional fast path in `OrdMapRange::to_map()`: when the `rayon` feature (or
  test mode) is active, clones the full map in O(1) (Arc refcount), then trims to the
  view's bounds with two O(log n) `split_at_key_consuming` calls. Reinsertion of boundary
  keys for `Included` bounds uses `update`, which is also O(log n). Without the `rayon`
  feature, the previous O(k) `from_sorted_iter` path is preserved. The btree split
  machinery (`split_node`, `count_entries`, and helpers) retains its
  `#[cfg(any(test, feature = "rayon"))]` gate — no unconditional compile cost.
  `OrdSetRange::to_set()` gains the same benefit by delegating to `to_map()`.

  (2) **`split_at_key` as the primary split API on `OrdMap` and `OrdSet`.**
  Renamed `split_at_key_view` → `split_at_key` on `GenericOrdMap`, `GenericOrdSet`,
  `OrdMapRange`, and `OrdSetRange`. Demoted `split_at_key_consuming` from `pub` to
  `pub(crate)` (used internally by rayon parallel ops and the `to_map()` fast path).
  Updated all doc comments: the split methods now note they return borrowed views and
  describe when to call `to_map()`/`to_set()` to materialise. 12 split-view tests
  renamed accordingly (`split_at_key_view_*` → `split_at_key_*`).

  (3) **`split_at` as the primary split API on `Vector` and `VectorRange`.**
  Replaced the old consuming `GenericVector::split_at(self, index)` (which just called
  `split_off`) with the view-based implementation. Renamed `split_at_view` → `split_at`
  on both `GenericVector` and `VectorRange`. Updated `chunked()` to use `split_off`
  directly (cleaner loop: `split_off` the right half, push the left, repeat). Updated
  `patch()` similarly (two `split_off` calls on a mutable clone). 6 split-view tests
  renamed accordingly.

  All three steps: `test.sh` green (fmt, cargo test × 3 variants, clippy -D warnings,
  cargo doc, cargo audit). 1277 unit tests + 425 doc tests pass.

- **[2026-04-27] OrdMapRange construction: O(k) → O(log n) + lazy cached_len.**
  `OrdMapRange::len` field changed from `usize` (eagerly computed) to `AtomicUsize`
  with sentinel `LEN_UNCOMPUTED = usize::MAX`. Construction uses a new
  `ord_map_range_endpoints` function: one `next()` + one `next_back()` on
  `RangedIter` to find first/last in O(log n), deferring element count to first
  `len()` call. Both `submap()` constructors updated. `is_empty()` now checks
  `self.first.is_none()` (O(1)). `Clone` impl propagates the cached len.
  Benchmarked: 13–150× faster construction for small/medium ranges. Results in
  `docs/baselines.md` § "OrdMapRange — lazy construction optimisation".

- **[2026-04-27] DEC-038 perf improvements: ptr_eq fast-paths, OrdTrie merge-walk, InsertionOrderMap bulk load.**
  Three performance improvements from the DEC-038 investigation:
  (1) `Trie` (`src/trie.rs`): added `ptr_eq` fast-paths to all four set operations
  (`union`, `difference`, `intersection`, `symmetric_difference`). O(1) short-circuit
  for structurally-shared tries. Root `value` handled separately from children in each
  fast-path (ptr_eq checks only the children pointer).
  (2) `OrdTrie` (`src/ord_trie.rs`): replaced flatten-and-rebuild with full merge-walk
  in all four set operations. Recursive descent through both OrdTrie structures
  simultaneously; ptr_eq short-circuit at each level; empty-node pruning after
  difference/intersection. symmetric_difference uses a two-pass approach with sorted
  `in_both: Vec<K>` and binary_search to find "only in other" keys without a second
  mutable borrow.
  (3) `InsertionOrderMap::from_iter` (`src/insertion_order_map.rs`): replaced O(n log n)
  sequential insert loop with O(n avg) two-pass bulk load. Pass 1 deduplicates via HAMT
  index (sequential counter assignment). Pass 2 builds the `entries` OrdMap via
  `GenericOrdMap::from_sorted_iter` — a new O(n) bottom-up B+ tree constructor
  (`build_sorted` in `src/nodes/btree.rs`).
  (4) `InsertionOrderSet::from_iter` (`src/insertion_order_set.rs`): found during
  subsequent audit to have its own sequential insert loop. Fixed to delegate via
  `.map(|a| (a, ())).collect()`, routing through `InsertionOrderMap::from_iter` and
  gaining the same O(n avg) bulk-load benefit. All `test.sh` checks pass (fmt, cargo test
  × 3 feature variants, clippy -D warnings, cargo doc, cargo audit).
  Benchmarked (1) and (2) in `benches/trie.rs` head-to-head against the old implementations.
  OrdTrie merge-walk: 4–18× faster for overlapping tries, 44–443× for disjoint, thousands-fold
  for identical (ptr_eq). Trie ptr_eq check: zero measurable overhead in the non-matching case,
  thousands-fold speedup when it fires. Full results in `docs/baselines.md` § OrdTrie/Trie set ops
  and `docs/decisions.md` DEC-038 Investigation C.

- **[2026-04-27] Documentation and coverage pass.**
  Completed `# Examples` doc blocks across all 14 derived collection types. Audited
  `# Panics` sections, `#[must_use]` annotations, and iterator trait coverage
  (`FusedIterator`, `ExactSizeIterator`). Added `ExactSizeIterator` to `HashMultiMap`
  and `OrdMultiMap` `ConsumingIter` with `remaining` counter. Ran `cargo llvm-cov` and
  added targeted tests to `vector/focus.rs` (coverage 82.8% → 96.2%), covering
  `FocusMut::Single/Full` paths, `pair()`, `triplet()`, `unmut()`, all panic branches,
  and all unsafe raw pointer paths. Fixed `FocusMut::narrow` bounds-check bug
  (`r.start > self.len()` → `r.end > self.len()`). Added Miri-targeted tests for all
  unsafe code paths in focus.rs — none marked `#[cfg_attr(miri, ignore)]`. Added
  `Send`/`Sync` static assertions for all 20 collection types. Fixed README feature
  flag descriptions for `proptest`/`quickcheck`/`arbitrary` (were listing only 12 types
  as "all collection types"). Extended proptest/quickcheck/arbitrary coverage to all
  20 collection types — added Ord-backed derived types (OrdBag, OrdMultiMap, OrdBiMap,
  OrdSymMap, OrdTrie, OrdInsertionOrderMap, OrdInsertionOrderSet) and UniqueVector to
  all three feature modules. Updated lib.rs and README feature tables accordingly.
  1126 tests pass.

- **[2026-04-27] API gap fill + content hash rationalisation + doc/test consistency pass.**
  Added `HashSet::get`, `HashSet::extract`, `OrdSet::extract`, `OrdMap::remove_min`,
  `OrdMap::remove_max`, `Vector::partition`. Added `ptr_eq` to all 14 derived types
  (Bag, HashMultiMap, BiMap, SymMap, Trie, OrdBag, OrdMultiMap, OrdBiMap, OrdTrie,
  InsertionOrderMap, InsertionOrderSet, OrdInsertionOrderMap, OrdInsertionOrderSet,
  UniqueVector). Rationalised content hash API across all five primary types: renamed
  `merkle_hash` → `content_hash`, `merkle_valid`→`content_hash_valid` on Vector;
  renamed `kv_merkle_valid` → `content_hash_valid` on HashMap; added `HashSet::content_hash`
  / `content_hash_valid` (AtomicU64 cache using stored HAMT hashes); added
  `OrdMap::content_hash_valid`; updated `OrdSet::content_hash_valid` to call delegate.
  Doc pass: fixed two stale `recompute_kv_merkle` links in HashMap struct doc; updated
  `intern_and_seal` doc; added `content_hash` API note to HashSet struct doc; renamed all
  `kv_merkle_*` test functions to `content_hash_*`. 1126 tests pass.

- **[2026-04-26] Perf review: OrdMap ptr_eq fast path + HashMap insert guard.**
  Detailed performance review of `nodes/hamt.rs`, `nodes/btree.rs`, `hash/map.rs`,
  `nodes/rrb.rs`, `vector/mod.rs`, `ord/map.rs`. Two optimisations implemented:
  (1) `OrdMap::PartialEq` now checks `ptr_eq` before the content_hash_cache check —
  structurally-shared clones compare in O(1) (~1.15 ns) rather than O(n) diff traversal.
  Benchmarked: `eq_clone_100000` = 1.15 ns (flat across 1K/10K/100K).
  (2) `HashMap::insert` defers `value_hash` computation to inside the
  `kv_merkle_valid=true` branch, avoiding wasted hash work when the cache is
  invalidated. README parallel support section corrected: Ord-backed derived types
  have no rayon support; tables updated with all Ord/Hash variants.
  Added `bench_eq_clone` to `benches/ordmap.rs`.

- **[2026-04-27] R.17 — Head-to-head OrdMap vs HashMap criterion benchmarks.**
  Added `benches/compare.rs` with 7 benchmark groups (lookup, insert_mut,
  remove_mut, iter, from_iter, par_union, par_intersection) across sizes
  100/1K/10K/100K for scalar ops and 10K/100K for parallel ops. Results in
  `docs/baselines.md` § "OrdMap vs HashMap — head-to-head". Key findings:
  OrdMap faster for all write/bulk/iteration workloads (1.4–2× scalar,
  4–16× parallel set ops); HashMap ~2× faster only for random point lookups.
  `bench.sh compare` and `bench.sh compare -- --features rayon` run clean.

- **[2026-04-26] UniqueVector — persistent sequence with uniqueness guarantee.**
  New collection type `UniqueVector<A>` backed by `GenericVector<A, P>` +
  `GenericHashSet<A, S, P, H>`. Provides push_back/push_front (dedup), pop_front/
  pop_back (FIFO/LIFO), get(i) indexed access, remove(i), remove_by_value, contains,
  front/back, and full set operations (union, difference, intersection,
  symmetric_difference). Hash-sensitive `PartialEq`/`Ord`/`Hash` — order counts.
  All standard trait impls: Clone, Debug, Default, FromIterator, Extend,
  IntoIterator (owned + &), Index<usize>, From<Vec/&Vec/[A;N]/&[A]>.
  28 tests. Type alias `UniqueVector<A>` (std/foldhash feature gate).
  lib.rs, README.md, impl-plan.md updated.

- **[2026-04-26] Queue/deque ops on InsertionOrderSet and OrdInsertionOrderSet.**
  Added `front()`, `back()`, `pop_front()`, `pop_back()` to
  `GenericInsertionOrderMap`, `GenericInsertionOrderSet`,
  `GenericOrdInsertionOrderMap`, `GenericOrdInsertionOrderSet`.
  All four ops are O(log n) via `OrdMap::get_min/get_max`. This enables
  use of `InsertionOrderSet` as a persistent deduplicating FIFO queue —
  push via `insert()`, dequeue via `pop_front()`. Re-inserting a key that
  is already queued is a no-op (standard `insert` behaviour). 11 new tests.

- **[2026-04-26] R.12 Option A — Document deterministic hashing pattern.**
  Zero implementation cost. Added a "Cross-session consistency" section to
  `src/identity_hasher.rs` explaining how `IdentityBuildHasher` enables
  cross-session `InternPool` merging, reproducible Merkle hashes, and
  deterministic test snapshots. Added a "Deterministic hashing" section to
  `src/lib.rs` crate-level docs covering the full set of use cases (integer keys
  → `IdentityBuildHasher`, string/composite keys → fixed-seed AHash/FxHash),
  noting that `Ord`-backed collections are always deterministic without
  configuration. Hash DoS caveat documented in both locations. README and lib.rs
  collection tables updated for all 19 types; README comparison table updated
  with 7 new Ord-backed types and `ord-hash` feature row added.

- **[2026-04-26] R.16 — OrdInsertionOrderSet: sorted insertion-ordered set.**
  `OrdInsertionOrderSet<A>` / `GenericOrdInsertionOrderSet<A, P>` in `src/ord_insertion_order_set.rs`.
  Backed by `GenericOrdInsertionOrderMap<A, ()>`. `A: Ord + Clone` only. Full trait coverage.
  Set ops: union, difference, intersection, symmetric_difference. 28 tests.

- **[2026-04-26] R.16 — OrdInsertionOrderMap: sorted insertion-ordered map.**
  `OrdInsertionOrderMap<K, V>` / `GenericOrdInsertionOrderMap<K, V, P>` in
  `src/ord_insertion_order_map.rs`. Backed by two OrdMaps:
  `OrdMap<K, usize>` (key→counter) + `OrdMap<usize, (K, V)>` (counter→entry). No hasher,
  `K: Ord + Clone`. O(log n) delete, no tombstones (vs HashMap+Vector approach). Full trait
  coverage + IndexMut. 29 tests.

- **[2026-04-26] R.16 — OrdTrie: sorted persistent prefix tree.**
  `OrdTrie<K, V>` / `GenericOrdTrie<K, V, P>` in `src/ord_trie.rs`. Children stored in
  `GenericOrdMap<K, GenericOrdTrie<K, V, P>, P>` — no hasher param. `K: Ord + Clone`.
  Iteration visits paths in sorted lexicographic order. `subtrie`/`get`/`contains_path` use
  `Comparable<K>`. Full trait coverage + IndexMut. `prune()`, `iter_prefix()`, set ops. 37 tests.

- **[2026-04-26] R.16 — OrdBiMap: sorted bidirectional map.**
  `OrdBiMap<K, V>` / `GenericOrdBiMap<K, V, P>` in `src/ord_bimap.rs`. Backed by two OrdMaps
  (`OrdMap<K, V>` forward + `OrdMap<V, K>` backward). Bijection invariant maintained. No hasher,
  `K: Ord + Clone`, `V: Ord + Clone`. Full trait coverage. 31 tests.

- **[2026-04-26] R.16 — OrdSymMap: sorted symmetric bidirectional map.**
  `OrdSymMap<A>` / `GenericOrdSymMap<A, P>` in `src/ord_symmap.rs`. Backed by two OrdMaps.
  O(1) `swap()`. No hasher, `A: Ord + Clone`. `PartialOrd`/`Ord` via forward iter. Full trait
  coverage. Reuses `Direction` from `symmap.rs`. 34 tests.

- **[2026-04-26] R.16 (partial) — OrdMultiMap: sorted persistent multimap.**
  `OrdMultiMap<K, V>` / `GenericOrdMultiMap<K, V, P>` added in `src/ord_multimap.rs`.
  Backed by `GenericOrdMap<K, GenericOrdSet<V, P>, P>`. No hasher parameter; requires
  only `K: Ord + Clone`, `V: Ord + Clone`. Full trait coverage: Clone, Debug, Default,
  PartialEq, Eq, PartialOrd, Ord, Hash, FromIterator, Extend, IntoIterator (owned + &),
  Index, From<Vec/&Vec/[T;N]>. Set ops (all &self): union, intersection, difference,
  symmetric_difference. `key_count()`, `contains_key()`, `contains()`, `get()`,
  `iter_sets()`, `keys()`. Hash uses sequential sorted-order (no XOR combiner). 34 tests.
  Exported as `pds::OrdMultiMap` and `pds::GenericOrdMultiMap`.

- **[2026-04-26] R.16 (partial) — OrdBag: sorted persistent multiset.**
  `OrdBag<A>` / `GenericOrdBag<A, P>` added in `src/ord_bag.rs`. Backed by
  `GenericOrdMap<A, usize, P>`. No hasher parameter; requires only `A: Ord + Clone`.
  Full trait coverage: Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
  FromIterator, Extend, IntoIterator (owned + &), From<Vec/[T;N]/&[T]/&Vec>. Set ops:
  union, intersection, difference, symmetric_difference (all O(n)). Range queries via
  `range()`. Hash uses sequential order (canonical sorted iteration — no XOR combiner
  needed). 32 tests. Exported as `pds::OrdBag` and `pds::GenericOrdBag`.

- **[2026-04-26] R.14 — `ord-hash` content hash for OrdMap and OrdSet.**
  `AtomicU64` content hash cache added to `GenericOrdMap`. Invalidated on every
  mutation; clone preserves the cached value. `PartialEq` gains a full O(1) fast-path:
  when both caches are populated, `eq` returns `h1 == h2` without scanning (positive
  equality, 2^-64 collision risk — same threshold as HashMap's kv_merkle_hash, DEC-023).
  `content_hash()` public method added (K: Hash, V: Hash). `Hash` impl added for OrdMap
  and OrdSet. `ord-hash` added to default features. DEC-036 recorded. `AtomicU64` chosen
  over the planned `Cell<u64>` to preserve `Sync` for rayon `par_iter()`.

- **[2026-04-26] R.15 — Node size re-evaluation for join-heavy workloads.**
  Re-benchmarked `ORD_CHUNK_SIZE` 16/24/32/48 with the join algorithm operations
  (`par_union`, `par_intersection`, `par_difference`) at 10K and 100K entries.
  Added `ordmap_parallel` benchmark group to `benches/ordmap.rs`. Result: size 32
  confirmed optimal for both single-tree and parallel join workloads. Size 48 is
  20–69% slower on parallel join at 100K; sizes 16/24 are 10–44% slower on single-tree
  ops with no parallel advantage. `ORD_CHUNK_SIZE = 32` unchanged. Addendum added to
  DEC-017. R.14 `AtomicU64` is in `GenericOrdMap` root (not in nodes) — DEC-017
  node-size choice is unaffected; no second addendum needed.

- **[2026-04-26] R.11 — Join-based parallel bulk operations for OrdMap and OrdSet.**
  `par_union`, `par_intersection`, `par_difference`, `par_symmetric_difference` added to
  `OrdMap` and `OrdSet` using the Blelloch et al. join algorithm (ACM TOPC 2022; PaC-trees
  PLDI 2022). Primitives: `split_node(node, key)` O(log n), `concat_node(left, right)`
  height-aware O(log n), `concat_ordered` with empty-root normalisation, `root_pivot_key()`
  O(1) median from root. Work: O(m log(n/m+2)); Span: O(log² n). Believed to be the first
  implementation of the Blelloch join algorithm on a blocked-leaf persistent B+ tree in any
  language. All tests green; join algorithm documented in README, architecture.md, and
  references.md.

- **[2026-04-26] R.9, R.10 — Parallel transform operations + tree-native optimisation.**
  `par_filter`, `par_map_values`, `par_map_values_with_key` added to `HashMap`, `OrdMap`,
  `HashSet`, `OrdSet` (R.9). `par_map_values`/`par_map_values_with_key` subsequently
  upgraded to tree-native O(n/p) implementations for both HAMT and B+ tree (R.10 / DEC-035):
  HAMT walks entries via `SparseChunk::entries()` preserving node positions; B+ tree forks
  at the top-level branch children via rayon. `par_filter` remains collect-based.
  `src/lib.rs` `## Parallel operations` section updated to distinguish implementation-
  optimised vs convenience methods. All 905 tests green; zero warnings.

- **[2026-04-26] R.1, R.3, R.5, R.6, R.7 — Residual consistency fixes.**
  R.1: Added all missing set operations — `symmetric_difference` to Bag, HashMultiMap,
  InsertionOrderMap, Trie; `difference`, `intersection`, `symmetric_difference` to BiMap and SymMap.
  Each method has ≥2 unit tests. Pre-existing no_std bug fixed: added `use alloc::vec::Vec` to
  bag.rs, hash_multimap.rs, insertion_order_map.rs, insertion_order_set.rs, bimap.rs, symmap.rs.
  R.3: Replaced two cfg-gated `Default` impls for `GenericTrie` with one generic `where S: Default`
  impl, plus a `default_is_empty` test. R.5: Added `debug` feature row to lib.rs and README feature
  tables. R.6: Rewrote test.sh — added `cargo fmt --check`, `cargo check --no-default-features`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`, `cargo audit`; fixed pre-existing
  broken intra-doc links in persist.rs (`InternPool` → `crate::intern::InternPool`) and pre-existing
  formatting drift across benches/ and vector/mod.rs. R.7: Confirmed rpds 1.2.0 in Cargo.lock —
  README already correct, no change needed.

- **[2026-04-25] Directive conformance pass 3 — From<&Vec>, #[allow] comments, missing tests.**
  `From<&Vec<A>>` added to `Bag`; `From<&Vec<(Vec<K>,V)>>` added to `Trie`. Tests added:
  `Bag` (from_vec_ref, sum_via_iterator), `BiMap` (sum, from_vec/slice/vec_ref), `SymMap`
  (same), `Trie` (debug_format, from_vec_ref), `Vector` (partial_ord_and_ord). Bare
  `#[allow]` comments fixed in util.rs, vector/mod.rs, vector/focus.rs (×2), and
  tests/{ordset,vector,hashset}.rs.

- **[2026-04-25] Directive conformance pass 2 — Index, serde tests, Hash tests, allow comments, From<&Vec>.**
  `Index` added for HashMultiMap (returns `&HashSet`), InsertionOrderMap (returns `&V`),
  BiMap (forward direction), SymMap (forward direction), Trie (`&[K]` path). `IndexMut`
  added for InsertionOrderMap and Trie (where mutation does not break invariants); omitted
  for BiMap/SymMap/HashMultiMap (see DEC-030). `get_mut` added to InsertionOrderMap and
  Trie. `From<&Vec<(K,V)>>` added to HashMultiMap, InsertionOrderMap, BiMap, SymMap.
  Serde round-trip tests added for Bag, HashMultiMap, InsertionOrderMap, BiMap, SymMap,
  Trie in `src/ser.rs`. Hash order-independence tests added for HashMap, HashSet, BiMap,
  SymMap (the four that were missing them; Bag and HashMultiMap already covered above).
  Trait tests added to Vector (From conversions, Hash, Sum, Add ref, Extend), Bag (Debug,
  Hash, Default, Add, Extend, From conversions), HashMultiMap (same), InsertionOrderMap
  (same plus Index/IndexMut/get_mut). Bare `#[allow]` comments fixed in vector/mod.rs,
  nodes/btree.rs, ord/set.rs, ord/map.rs. Decisions DEC-030, DEC-031 recorded.

- **[2026-04-25] Directive conformance fixes (trie traits, serde, Send/Sync assertions).**
  All missing standard traits added to `GenericTrie`: `Hash` (XOR-combine children),
  `Extend<(Vec<K>, V)>`, `FromIterator`, `From<Vec/[T;N]/&[T]>`, `Add` (union, owned+ref),
  `Sum`, `IntoIterator for &GenericTrie` (`TrieIter`), `IntoIterator for GenericTrie`
  (`TrieConsumingIter`). `Serialize`/`Deserialize` for `GenericTrie` added to `src/ser.rs`
  (sequence of (path, value) pairs). `assert_impl_all!(Type: Send, Sync)` added to test
  modules of `Bag`, `BiMap`, `SymMap`, `HashMultiMap`, `InsertionOrderMap` and `Trie`.
  Bare `#[allow(clippy::...)]` suppressions in `hash/map.rs` and `vector/focus.rs` now
  have inline explanatory comments.

- **[2026-04-25] 6.10 Merkle-keyed node deduplication in SSP serialisation.**
  `DedupPoolCollector<A, H>` added in `src/persist.rs`: extends the pointer-keyed
  `PoolCollector` with a secondary `merkle_index: HashMap<u64, Vec<u32>>`. On a
  pointer miss, reads `node.merkle_hash` from the live node, scans candidates
  for structural equality (O(node_size) due to post-order traversal normalising
  child refs before parents), and reuses the existing pool entry if found.
  `SetDedupPoolCollector` mirrors the same design for set nodes (unwraps `Value<A>`).
  New API: `HashMapPool::from_maps_dedup`, `from_map_dedup`; `HashSetPool::from_sets_dedup`,
  `from_set_dedup`; `BagPool::from_bags_dedup`, `from_bag_dedup`;
  `BiMapPool::from_bimaps_dedup`, `from_bimap_dedup`; `SymMapPool::from_symmaps_dedup`,
  `from_symmap_dedup`. All dedup variants require `A: PartialEq`; non-dedup variants
  unchanged. 9 new tests: size reduction (same-lineage clones with identical
  independent mutations), correctness (round-trip), and inflation guard (dedup ≤ plain).
  Note: dedup requires same hasher lineage — independently constructed maps with
  different `RandomState` seeds have incompatible HAMT structures and gain no benefit;
  see `docs/decisions.md` for rationale.

- **[2026-04-25] Cross-feature improvements: Merkle × diff, intern_and_seal.**
  `HashMap::diff()` gains a kv_merkle fast-path (O(1) empty diff for equal maps).
  `HashSet::diff()` gains a root-Merkle fast-path (same semantics, sets only)
  plus per-node Merkle subtree pruning in `set_diff_hamt_nodes` (skips content-
  equal subtrees that aren't pointer-equal). `GenericHashMap::intern_and_seal()`
  combines `intern()` + `recompute_kv_merkle()`, sealing all three fast-paths
  (ptr_eq, kv_merkle, node-level ptr_eq after interning) in one call. Doc notes
  added to `intern()` and `diff()` on both types. 7 new tests across
  `hash/map.rs`, `hash/set.rs`, and `intern.rs`.

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

- **[2026-04-26] R.11: Join-based parallel bulk ops for OrdMap and OrdSet.** `par_union`,
  `par_intersection`, `par_difference`, `par_symmetric_difference` for `OrdMap` and `OrdSet`
  using O(log n) structural split + height-aware concat. Join algorithm: O(m log(n/m + 2))
  work, O(log² n) span (Blelloch et al., TOPC 2022). Unique among Rust persistent DS
  libraries. Root cause of correctness bugs: `difference()`/`symmetric_difference()` retain
  an empty Leaf allocation after all entries are removed (`{root: Some(Leaf([])), size: 0}`);
  fixed by normalising on `size == 0` in `concat_ordered`. 25+ tests.

- **[2026-04-25] 3.4: Parallel bulk ops.** `par_union`,
  `par_intersection`, `par_difference`,
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
  `difference` no longer need `S: Clone`. OrdMap: moved
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
  `difference_with` (asymmetric diff with per-entry resolver),
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

All residual items including R.17 and the range-view API refactor are now complete. No active work item.

---

## Future {#future}

---

## Phase T — Tiered write-behind collections {#phase-t}

A composable, configurable pipeline where hot writes land on a fast mutable tier
and propagate asynchronously to progressively richer (but slower) persistent tiers.
Callers accept a bounded data-loss window in exchange for near-transient write
throughput and "for free" access to structural sharing, disk durability, and Merkle
identity at whatever lag they can tolerate.

**Design:** each tier implements `CollectionBackend<K, V>`. A
`TieredCollection<K, V, Hot, Cold>` is itself a `CollectionBackend`, so stages
compose recursively — three tiers are
`TieredCollection<K, V, S0, TieredCollection<K, V, S1, S2>>`.

**Propagation policy** is configurable per tier boundary, independently of the
backends chosen:

| Policy | Trigger |
|--------|---------|
| `Immediate` | Synchronous — cold tier updated on every write |
| `Batched(n)` | Propagate after `n` writes accumulate |
| `Timed(d)` | Background thread propagates every `d` |
| `Manual` | Only on explicit `flush()` call |

**Available backends (by phase):**

| Backend | Provided by | Status |
|---------|-------------|--------|
| `StdHashMapBackend` | pds (`tiered` feature) | T.0 |
| `PdsHashMapBackend` | pds (`tiered` feature) | T.0 |
| `FolioHamtMapBackend` | pds-folio | T.1 (after G.12) |
| `VersionedHamtBackend` | pds-merkle-spine | T.2 (after H.8) |
| `MerkleWrapperBackend` | pds (`tiered` feature) | T.0 |

**Common compositions:**

| Use case | Composition |
|----------|-------------|
| Fast writes + structural sharing | `Std → Pds` |
| Fast writes + content identity (no disk) | `Std → MerkleWrapper<Pds>` |
| Fast writes + disk durability | `Std → Pds → Folio(Disk)` |
| Fast writes + full stack | `Std → Pds → VersionedHamt` |
| Single tier, no mirroring | any backend directly |

**Blocking `pds-durable::TieredMap`:** once T.1 + T.2 are complete, `TieredMap`
is superseded. Mark `pds-durable` maintenance-only and deprecate `TieredMap` in
favour of `TieredCollection<K, V, StdHashMapBackend, FolioHamtMapBackend>` at
that point (see DEC-DURABLE-1).

---

### T.0 — Core infrastructure {#t0} ✓ Done [2026-07-01]

**Scope:** the `tiered` feature in the pds root crate. No folio or merkle-spine
dependency — backends for those tiers are added in T.1 / T.2.

**Deliverables:**

1. **`CollectionBackend<K, V>` trait** (`src/tiered/backend.rs`)

   ```rust
   pub trait CollectionBackend<K, V>: Send + 'static {
       fn get(&self, key: &K) -> Option<V>;
       fn insert(&mut self, key: K, value: V) -> Option<V>;
       fn remove(&mut self, key: &K) -> Option<V>;
       fn len(&self) -> usize;
       fn is_empty(&self) -> bool;
       /// Bulk-load from an iterator — used during propagation.
       fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>);
       /// Drain all entries — called on the hot tier before propagating.
       fn drain(&mut self) -> Vec<(K, V)>;
       /// Snapshot this backend as an owned value — used to read the cold tier.
       fn snapshot(&self) -> Self where Self: Sized + Clone { self.clone() }
   }
   ```

2. **Concrete backends** (`src/tiered/backends.rs`)
   - `StdHashMapBackend<K, V>` — wraps `std::collections::HashMap<K, V>`
   - `PdsHashMapBackend<K, V>` — wraps `pds::HashMap<K, V>`
   - `MerkleWrapperBackend<K, V>` — wraps `MerkleWrapper<pds::HashMap<K, V>>`

3. **`PropagationPolicy`** (`src/tiered/policy.rs`)

   ```rust
   pub enum PropagationPolicy {
       Immediate,
       Batched(usize),       // propagate after n pending writes
       Timed(Duration),      // background thread wakes every d
       Manual,               // only on explicit flush()
   }
   ```

4. **`TieredCollection<K, V, Hot, Cold>`** (`src/tiered/mod.rs`)

   Internal state in `Arc<Mutex<TieredState<K, V, Hot, Cold>>>` so the collection
   is cheaply cloneable and shareable across threads.

   Public API:
   - `new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self`
   - `insert(&self, key: K, value: V) -> Option<V>` — writes to hot only
   - `get(&self, key: &K) -> Option<V>` — reads hot; falls back to cold
   - `remove(&self, key: &K) -> Option<V>` — removes from hot; marks deletion
   - `flush(&self)` — drains hot into cold synchronously
   - `cold_snapshot(&self) -> Cold` — clone of the current cold tier
   - `start_background_propagation(&self) -> PropagationHandle` — spawns a thread
     (only meaningful with `Timed` or `Batched` policy)
   - `len(&self) -> usize` — hot + cold (deduped)

   `PropagationHandle` wraps a `JoinHandle` + stop channel; dropping it stops the
   background thread.

   Deletion handling: a `pending_deletes: HashSet<K>` in hot-tier state tracks keys
   removed from hot but not yet propagated to cold. `get` checks this set before
   falling back to cold. `flush` applies deletions to cold then clears the set.

5. **Tests** (`src/tiered/tests.rs`)
   - Insert into hot, read back
   - Insert into hot, `flush`, read from `cold_snapshot`
   - Delete from hot, ensure not visible via cold fallback before flush
   - Delete from hot, `flush`, ensure gone from cold snapshot
   - `Batched(n)` policy auto-propagates after n inserts
   - `Immediate` policy: cold snapshot always matches hot
   - Concurrent inserts from two threads (both via cloned `TieredCollection`)
   - `MerkleWrapperBackend` as cold: `cold_snapshot().merkle_root()` changes after flush

**Feature gate:** `tiered = ["std"]` in `Cargo.toml`. No new external crate deps.

**Acceptance:** `test.sh` passes across all three variants (default, all-features,
small-chunks). `cargo clippy --all-features -- -D warnings` clean.

---

### T.1 — Folio backend wrapper {#t1}

Add `FolioHamtMapBackend<K, V, B: Backend>` in `pds-folio`.

**Blocked by:** Phase G.12 (pds-folio Vector + OrdMap/OrdSet complete and stable).

---

### T.2 — Merkle-spine backend wrapper {#t2}

Add `VersionedHamtBackend<K, V, C, B>` in `pds-merkle-spine`.

**Blocked by:** Phase H.8 (VersionedHamt full implementation stable).

---

### T.3 — Migrate and deprecate pds-durable::TieredMap {#t3}

Replace `pds-durable::TieredMap` with `TieredCollection<K, V, StdHashMapBackend,
FolioHamtMapBackend<DiskBackend>>` (or `VersionedHamtBackend` if full history is
needed). Mark `pds-durable` maintenance-only. See DEC-DURABLE-1.

**Blocked by:** T.1 + T.2 complete; folio disk Backend implemented.

---

## Cross-project execution sequence {#cross-project-sequence}

This work spans three projects. The table below shows the complete serial order
with explicit handoffs. Work within a project follows the per-phase plans below;
this table is the entry point for orienting yourself at the start of any session.

| Step | Project | Work | Gate / handoff |
|------|---------|------|----------------|
| 1 | **merkle-spine** | Stages 1–3 (foundations, encoding tactics, DeltaLogIndex + `PageIndexBackend` trait) | MS-10 defines `PageIndexBackend` — **handoff signal to pds** |
| 2 | **pds** | Phase F (cross-variant traits) — no external blockers | F.0 + F.1 done |
| 3 | **pds** | Phase W (workspace consolidation) — add `[workspace]` to Cargo.toml | W.0 + W.1 done — **handoff signal to pds-folio** |
| 4 | **merkle-spine** | Stages 4–10 (version DAG, commits, provenance, lifecycle, compaction, graft, diff) | Stages 1–10 complete → DeltaLogIndex v1 ships; PageIndexBackend API is stable |
| 5 | **pds-folio** | Phase G, G.0–G.4 (create crate, HAMT node types, CRUD, refcount, HashSet) | Can start as soon as Step 3 is done — does not need Step 4 |
| 6 | **pds-folio** | Phase G, G.5 (HamtIndex: PageIndexBackend) | Needs Step 4 (Stages 1–10 complete) for API stability before implementing |
| 7 | **merkle-spine** | MS-F0 — wire HamtIndex into VersionStore | Needs pds-folio G.5 — **handoff signal back to merkle-spine** |
| 8 | **pds-folio** | Phase G, G.6–G.12 (HashMap/HashSet traits, Vector, OrdMap/OrdSet) | Can run in parallel with Step 7 — no MS-F0 dependency |
| 9 | **pds-merkle-spine** | Phase H (VersionedHamt, H.0–H.8) | Needs merkle-spine MS-F0 (Step 7) + pds-folio G.5 (Step 6) |
| 10 | **pds** | Phase P (cross-collection benchmarks) | Needs pds-folio G.12 + pds-merkle-spine H.8 |

**Parallel opportunities:** Steps 7 and 8 can run in parallel (different projects, no dependency between them). Step 5 can start while Step 4 (merkle-spine Stages 4–10) is in progress.

**Project home for each phase:**
- Phases F, W, P → this plan (pds repo)
- Phase G → `pds-folio/docs/impl-plan.md` (once that crate is created at G.0)
- Phase H → `pds-merkle-spine/docs/impl-plan.md` (once that crate is created at H.0)

---

## Phase F — Cross-variant trait layer {#phase-f}

**Status:** Not started. No external blockers — can begin any time.

**Goal:** Define the full cross-variant trait set in `pds` itself (behind a `traits` feature
flag) and implement for existing in-memory collection types.

Traits: `PersistentMap<K, V>`, `PersistentSet<A>`, `PersistentVector<A>`,
`PersistentOrdMap<K, V>`, `PersistentOrdSet<A>`, `VersionedPersistentMap<K, V>`,
`MerklePersistentMap<K, V>`.

**Full spec:** `docs/cross-variant-traits.md`

**Dependency direction:**

```
pds (defines traits, impls for in-memory types)
  └── pds-folio (impls all five base traits)
        └── pds-merkle-spine (impls VersionedPersistentMap + MerklePersistentMap)
```

### F.0 — Define base traits in `src/traits.rs`

- New `src/traits.rs` with `PersistentCollection`, `PersistentMap<K, V>`, `PersistentSet<A>`,
  `PersistentVector<A>`, `PersistentOrdMap<K, V>`, `PersistentOrdSet<A>`
- Behind `features = ["traits"]`
- Implement all five base traits for existing in-memory types:
  `HashMap`, `HashSet`, `Vector`, `OrdMap`, `OrdSet`
- Re-export at crate root under `traits` feature gate
- Tests: generic function over each trait runs with the corresponding in-memory type

**Acceptance:** `cargo test --features traits` green; all five trait impls compile.

### F.1 — Define versioning + Merkle traits

- `VersionedPersistentMap<K, V>` and `MerklePersistentMap<K, V>` in `src/traits.rs`
- No implementations yet (those come with pds-merkle-spine)
- Define associated types: `VersionId`, `Proof`, `DiffEntry<K, V>`
- Export `DiffEntry` as a concrete type (not just as an associated type bound)

**Acceptance:** `cargo build --features traits` with no implementations still compiles;
the trait objects are well-formed; `DiffEntry<K, V>` is public and usable.

---

## Phase W — Workspace consolidation {#phase-w}

**Status:** Not started. No external blockers — can begin any time after F.0.

**Goal:** Convert the `pds` repo root into a Cargo workspace so that `pds-folio`
(Phase G) and `pds-merkle-spine` (Phase H) are created as member crates within the
same repo rather than as separate repositories. The existing `pds` crate stays at the
root — no file moves required; the workspace manifest coexists with `[package]` in the
root `Cargo.toml`.

**Outcome:** one GitHub repo, three member crates with clear roles, a single CI
workflow, and shared `docs/` covering all three backends.

### W.0 — Add `[workspace]` to root `Cargo.toml`

- Add `[workspace]` table to root `Cargo.toml` with `members = []` (empty until G.0 / H.0)
- Add `[workspace.dependencies]` for shared deps (`serde`, `postcard`, `bytemuck`, `folio-core`, etc.) so member crates inherit consistent versions
- `resolver = "2"` if not already set
- Run `test.sh` to confirm root crate is unaffected

**Acceptance:** `cargo test` and `cargo test --all-features` at repo root still pass; `cargo metadata` reports the workspace.

### W.1 — Update CI and scripts for workspace layout

- `build.sh`: add `--workspace` flag so all member crates build together
- `test.sh`: add workspace-aware steps (member crates use their own `test.sh` but CI runs `cargo test --workspace` as a smoke check)
- GitHub Actions: update to run `cargo clippy --workspace` and `cargo test --workspace` in addition to per-crate steps
- `docs/architecture.md`: add workspace layout diagram (pds → pds-folio → pds-merkle-spine dependency direction)

**Acceptance:** CI green; `cargo build --workspace` compiles cleanly once at least one member crate exists.

---

## Phase G — pds-folio {#phase-g}

**Status:** Not started.

**Blocked by (all items):**
- folio **S37, S64, S66** ✓ DONE — zero-copy write, slab allocator, batch free all complete
- pds **Phase F.0** (cross-variant traits)
- pds **Phase W.0** (workspace manifest — G.0 creates pds-folio as a member crate)

**Additionally blocked for G.5 only:**
- merkle-spine **Stages 1–10** — wait for DeltaLogIndex v1 to ship before implementing
  `HamtIndex: PageIndexBackend`. Strict code dependency is only on **Stage 3 (MS-10)**
  which defines the `PageIndexBackend` trait, but waiting for Stages 1–10 ensures the
  trait surface is stable and validated by a real implementation before pds-folio commits to it.

**G.0–G.4 can start as soon as F.0 + W.0 are done** — no merkle-spine dependency.

**Unblocks:** merkle-spine **MS-F0** (HamtIndex integration) once G.5 is done.

**Full spec:** `docs/pds-folio-spec.md`

### G.0 — Create `pds-folio` workspace member

- Create `pds-folio/` subdirectory in the pds repo root
- Scaffold with `cargo new --lib pds-folio` and adapt from rust-template (copy `build.sh`,
  `test.sh`, `flake.nix` devShell entry, `directives.md` shim, `docs/` skeleton)
- Add `pds-folio` to `[workspace.members]` in root `Cargo.toml`
- Add deps in `pds-folio/Cargo.toml` inheriting from `[workspace.dependencies]`:
  `folio-core`, `pds` (for traits), `serde`, `postcard`, `bytemuck` (for PodCodec)
- Define `Codec` trait + `PodCodec`/`PostcardCodec` impls in `src/codec.rs`
- Blank `src/lib.rs` with `#![deny(unsafe_code)]`
- Own impl-plan at `pds-folio/docs/impl-plan.md` (copy G.1–G.12 items there)

### G.1 — Core node types and slab layout

- `LeafNode` — variable-length layout: `count: u8 | key_hashes: [u64; count] | entry_offsets: [u16; count] | data: [u8; …]`
- `InternalNode` — bitmap + array of `SlabPageId` (u64); unchanged regardless of codec
- `LEAF_CAP` constant = max entries before a leaf splits (target: 512-byte slab slot)
- `HamtNodePage` — union of leaf and internal byte representations; slab slot type
- `FolioSlab<HamtNodePage>` wrapper type
- Unit tests: header size checks; leaf insert/read round-trip for `PostcardCodec`; `PodCodec` u64 round-trip

### G.2 — `HamtMap` CRUD

- `HamtMap<K, V, C = PostcardCodec, B = DefaultBackend>` with `K: Serialize + Hash + Eq + Clone, V: Serialize + DeserializeOwned + Clone, C: Codec`
- `new(store)`, `get(key) -> Option<V>`, `insert(key, value) -> Self`, `remove(key) -> (Self, Option<V>)`
- `len()`, `is_empty()`, `contains_key(key)`
- Path-copy on insert/remove: O(log N) new slab slots; leaf split when data overflows slot
- No reference counting yet (G.3)
- Tests: empty map, single insert, multiple inserts, overwrite, remove present/absent; test with both `PodCodec` (u64 keys) and `PostcardCodec` (String keys)

### G.3 — Reference counting and `Drop`

- `FolioBTree<SlabPageId, u32>` refcount table (stored in same folio store)
- `Clone` impl: increment root refcount
- `Drop` impl: decrement refcount, recursively free nodes at zero, batch via S66
- Optimisation: absent from table = refcount 1 (store only refcounts > 1)
- Tests: clone + drop frees nothing while shared; all copies dropped → store empty

### G.4 — `HamtSet` wrapper

- Newtype `HamtSet<A, B>(HamtMap<A, (), B>)`
- Full API: `contains`, `insert`, `remove`, `union`, `intersection`, `difference`, `symmetric_difference`
- Tests: all set operations

### G.5 — `HamtIndex`: PageIndexBackend

- Depends on: merkle-spine Stage 1 (for the `PageIndexBackend` trait definition)
- `HamtIndex<B>(HamtMap<u64, [u8; 32], B>)`
- Node-level BLAKE3 Merkle hashing: each node hash covers its child hashes recursively
- `root_hash()`: hash of root node (O(1) cached)
- `prove_inclusion(page_id) -> Option<MerkleProof>`
- `impl merkle_spine::PageIndexBackend for HamtIndex<B>`
- Tests: root hash changes when any entry changes; proof verifies; empty index has known hash

### G.6 — Implement pds cross-variant traits (HashMap / HashSet)

- `impl<K, V, C, B> PersistentMap<K, V> for HashMap<K, V, C, B>`
- `impl<A, C, B> PersistentSet<A> for HashSet<A, C, B>`
- Tests: generic functions from Phase F tests work with `HashMap`/`HashSet` using both codecs

### G.7 — Integration tests and proptest suite (HashMap / HashSet)

- proptest: insert N random (K, V) pairs; all lookups correct; remove N/2; remaining correct
- Integration: create `HashMap` in folio store; process restart; reopen store; lookups correct

### G.8 — Vector: RRB-tree node types and slab layout

- `VectorLeaf` and `VectorInternal` page layouts (BRANCHING_FACTOR = 32)
- `FolioSlab<VectorNodePage>` wrapper
- Unit tests: node size checks; leaf insert/read round-trip

### G.9 — `Vector` CRUD and `PersistentVector` trait impl

- `Vector<A, C = PostcardCodec, B = DefaultBackend>` — `A: Serialize + DeserializeOwned + Clone, C: Codec`
- `new`, `get`, `push_back`, `push_front`, `update`, `pop_back`, `pop_front`, `concat`, `split_at`, `len`, `iter`
- Path-copy on all mutations; shared refcount table from G.3
- `impl<A, C, B> PersistentVector<A> for Vector<A, C, B>`
- Tests: empty, single push, multiple pushes, update, pop, concat, split; proptest round-trip

### G.10 — OrdMap / OrdSet: B+ tree node types and slab layout

- `BTreeLeaf` (chained via `next_leaf`) and `BTreeInternal` page layouts
- `FolioSlab<BTreeNodePage>` wrapper
- Unit tests: node size checks; leaf insert/read round-trip in sorted order

### G.11 — `OrdMap` + `OrdSet` CRUD and trait impls

- `OrdMap<K, V, C = PostcardCodec, B = DefaultBackend>` — `K: Serialize + DeserializeOwned + Ord + Clone`
- `new`, `get`, `insert`, `remove`, `first`, `last`, `range`, `len`, `contains_key`, `iter`
- B+ tree split/merge on insert/remove; path-copy; shared refcount table from G.3
- `OrdSet<A, C, B>` wrapper over `OrdMap<A, (), C, B>`
- `impl PersistentOrdMap<K, V> for OrdMap<K, V, C, B>`
- `impl PersistentOrdSet<A> for OrdSet<A, C, B>`
- Tests: empty, insert, remove, range queries, ordering invariants; proptest sorted order

### G.12 — Integration tests (Vector + OrdMap / OrdSet)

- proptest: Vector concat/split round-trips; OrdMap range query correctness
- Integration: create OrdMap in folio store; restart; range query still correct

### G.13 — Consensus backend note and feature flag

pds-folio requires no new code to support consensus — the `B: FolioBackend` type
parameter already accommodates a consensus-enabled folio backend. When folio's
consensus backend is available, the following ergonomic aliases are exposed:

```rust
#[cfg(feature = "consensus")]
pub type ConsensusHashMap<K, V, C = PostcardCodec> =
    HashMap<K, V, C, folio::ConsensusBackend>;
#[cfg(feature = "consensus")]
pub type ConsensusOrdMap<K, V, C = PostcardCodec> =
    OrdMap<K, V, C, folio::ConsensusBackend>;
// ... and so on for HashSet, OrdSet, Vector
```

The `consensus` feature flag in `pds-folio/Cargo.toml` enables these aliases and pulls
in folio's consensus backend. No structural sharing, codec, or path-copy logic changes.

**What folio consensus gives at the collection level:** write coordination across peers
(who can commit pages). **What it does not give:** agreement on a canonical collection
version — that is the role of the Merkle root hash in pds-merkle-spine.

- Add `consensus` feature flag to `pds-folio/Cargo.toml`
- Re-export type aliases under `pds_folio::consensus::*`
- Acceptance: `cargo test --features consensus` compiles; `ConsensusHashMap<String, u64>`
  type-checks with folio's consensus backend

### G.14 — `ShardStrategy` trait and built-in strategies

Horizontal sharding: a `ShardedMap<K,V,C,B,S>` routes operations to one of N independent
`HashMap<K,V,C,B>` instances via a user-supplied strategy. Each shard is a separate
folio store — shard boundaries are a folio/filesystem concern, not a pds-folio concern.

**`ShardStrategy` trait:**

```rust
pub trait ShardStrategy<K> {
    /// Returns the shard index for a given key. Must be stable across calls.
    fn shard_for(&self, key: &K) -> usize;
    /// Returns the set of shard indices that may contain keys in [lo, hi].
    fn shards_for_range(&self, lo: Bound<&K>, hi: Bound<&K>) -> Vec<usize>;
    fn shard_count(&self) -> usize;
}
```

**Built-in strategies:**

| Type | Logic | Best for |
|------|-------|----------|
| `HashShard { n: usize }` | `hash(key) % n` | HashMap / HashSet; even distribution |
| `RangeShard<K: Ord>` | sorted split points, binary search | OrdMap / OrdSet; range queries hit one shard |
| `DirectoryShard<K>` | user-supplied `HashMap<KeyPrefix, usize>` | non-uniform or custom partitioning |

All three implement `ShardStrategy<K>`. Users may implement their own.

- `src/shard.rs`: `ShardStrategy` trait + `HashShard`, `RangeShard`, `DirectoryShard`
- Behind `features = ["sharding"]`
- Tests: each strategy routes consistently; `shards_for_range` covers all relevant shards

### G.15 — `ShardedMap` and `ShardedSet`

```rust
pub struct ShardedMap<K, V, C, B, S: ShardStrategy<K>> {
    shards: Vec<HashMap<K, V, C, B>>,
    strategy: S,
}
```

- `get`, `insert`, `remove`, `contains_key`, `len`, `iter` — route via strategy
- `range` (OrdMap variant only): iterate across `shards_for_range()` result in merge order
- `impl<…> PersistentMap<K, V> for ShardedMap<…>` where strategy is `HashShard`
- `impl<…> PersistentOrdMap<K, V> for ShardedOrdMap<…>` where strategy is `RangeShard<K>`
- `ShardedSet` and `ShardedOrdSet` as analogous wrappers
- **Resharding note:** because insert returns a new version (path-copy), old and new shard
  configurations can coexist during migration. Move keys shard-by-shard; old versions
  remain readable via their original shard roots throughout the transition.
- Tests: insert N keys; verify each routes to expected shard; full iter covers all keys;
  shard-by-shard iteration; resharding migration sequence produces correct merged state
- **Merkle integration note:** when backed by pds-merkle-spine, each shard has an
  independent Merkle root. A super-root (`hash_of_shard_roots([r0, r1, …, rN])`) covers
  all shards in a single hash — enables per-shard proof and partial sync (exchange only
  shards whose root hashes differ from peer). This is not implemented here; noted for
  Phase H extension.

---

## Phase H — pds-merkle-spine {#phase-h}

**Status:** Not started.

**Blocked by:**
- pds-folio **G.5** (`HamtIndex: PageIndexBackend`)
- merkle-spine **MS-F0** (HamtIndex integration wired into merkle-spine's `VersionStore`)
- pds **Phase F.1** (VersionedPersistentMap + MerklePersistentMap traits)
- pds **Phase W.0** (workspace manifest — H.0 creates pds-merkle-spine as a member crate)

**Full spec:** `docs/pds-merkle-spine-spec.md`

### H.0 — Create `pds-merkle-spine` workspace member

- Create `pds-merkle-spine/` subdirectory in the pds repo root
- Scaffold as a workspace member (same pattern as G.0 for pds-folio)
- Add `pds-merkle-spine` to `[workspace.members]` in root `Cargo.toml`
- Deps in `pds-merkle-spine/Cargo.toml`: `pds-folio`, `merkle-spine`, `pds` (traits)
- Own impl-plan at `pds-merkle-spine/docs/impl-plan.md` (copy H.1–H.8 items there)

### H.1 — `VersionedHamt` core struct

- `VersionedHamt<K, V, B>` wrapping `HamtMap<K, V, B>` + `VersionStore<HamtIndex<B>>`
- `new(store)` creates v0 (empty map, records initial root hash)
- `version() -> VersionId`, `root_hash() -> [u8; 32]`

### H.2 — Current-version CRUD

- `get`, `insert`, `remove`, `len`, `is_empty`, `contains_key`, `iter`
- `insert`/`remove` create a new version: path-copy HamtMap + record new root in VersionStore
- Tests: basic CRUD; version counter increments; root hash changes on mutation; unchanged on no-op

### H.3 — Historical access

- `get_at(version, key)` — look up historical HAMT root from VersionStore, traverse
- `checkout(version)` — O(1), returns VersionedHamt frozen at historical root
- `root_hash_at(version)` — O(1), from VersionStore record
- Tests: historical values preserved after mutations; checkout + insert branches independently

### H.4 — Structural diff

- `diff(from, to) -> impl Iterator<Item = DiffEntry<K, V>>`
- Walk two HAMT roots in parallel; skip subtrees where root hashes match
- Tests: identical versions → empty diff; single mutation → one entry; full diff correctness

### H.5 — Merkle proofs

- `prove_inclusion(key)`, `prove_inclusion_at(version, key)` → `Option<MerkleProof>`
- `verify_proof(root_hash, key, value, proof) -> bool` — pure function
- Tests: valid proof verifies; tampered proof fails; absent key returns None

### H.6 — Consensus token and sparse sync (deferred)

**Consensus token:** the Merkle root hash returned by `root_hash()` is the lightweight
consensus value for distributed agreement — two nodes that hold the same root hash
provably hold identical state. No additional consensus protocol is required at the
collection level; folio's storage-level consensus (see G.13) handles write coordination,
and the Merkle root handles state verification.

**Sharded super-root:** when `VersionedHamt` is used with `ShardedMap` (G.14–G.15),
each shard has an independent root hash. A super-root covers all shards:
```rust
fn super_root(shard_roots: &[[u8; 32]]) -> [u8; 32] {
    blake3::hash(bytemuck::cast_slice(shard_roots)).into()
}
```
This enables per-shard verification and partial sync: peers compare super-roots, identify
which shard roots differ, and exchange only those subtrees.

**Sparse sync protocol** (wire-format design deferred — requires network layer decision):
- Compare super-roots (or individual root hashes)
- Walk differing HAMT subtrees top-down; exchange only nodes whose hashes differ
- Terminate subtree walk when hashes match — structural sharing means unchanged subtrees
  need zero bytes transferred
- For sharded maps: sparse sync is per-shard and independent

Add wire-format design and implementation as a Future item when H.0–H.5 are complete.

### H.7 — Implement pds common traits

- `impl PersistentMap<K, V> for VersionedHamt<K, V, B>`
- `impl VersionedPersistentMap<K, V> for VersionedHamt<K, V, B>`
- `impl MerklePersistentMap<K, V> for VersionedHamt<K, V, B>`
- Tests: generic functions parameterised over all three trait levels work with `VersionedHamt`

### H.8 — Integration tests and proptest

- proptest: mutation sequences; historical values correct at every version
- Cross-crate integration: `VersionedHamt` used via `PersistentMap` and `VersionedPersistentMap` traits

---

## Phase P — Cross-collection performance {#phase-p}

**Status:** Not started.

**Blocked by:**
- pds-folio **G.12** (all five pds-folio collection types complete)
- pds-merkle-spine **H.8** (VersionedHamt complete)

**Goal:** Establish a rigorous, comparable benchmark suite across all four collection
families, identify performance bottlenecks through profiling, and close the gaps. The
comparison is inherently asymmetric — persistence, MVCC, and Merkle overhead exist for
good reasons — but quantifying those overheads is what makes the trade-offs legible.

**Collection families in scope:**

| Family | Types | Backend |
|--------|-------|---------|
| `pds` | HashMap, HashSet, OrdMap, OrdSet, Vector | in-memory (Arc nodes) |
| `pds-folio` | HashMap, HashSet, OrdMap, OrdSet, Vector | folio mmap slab |
| `folio` | FolioVec, FolioBTree | folio mmap (mutable, for reference) |
| `pds-merkle-spine` | VersionedHamt | folio + Merkle versioning |

### P.0 — Benchmark suite design

- Add `pds-bench/` as a workspace member crate (criterion, no library code)
- Define usage patterns to benchmark across all families:
  - **Bulk construction:** build from N unsorted elements (N = 1K / 10K / 100K / 1M)
  - **Point lookup:** random key access in a pre-built collection
  - **Point update:** single insert/remove on an existing collection
  - **Iteration:** full scan in insertion vs sorted order
  - **Range scan:** OrdMap/OrdSet only — bounded range over sorted keys
  - **Persist and reload:** pds-folio only — commit + reopen + first lookup
  - **Version navigation:** pds-merkle-spine only — checkout historical version + lookup
- Benchmark mutable folio-collections (`FolioVec`, `FolioBTree`) as reference points for the same bulk-build and lookup patterns; these are not persistent but they show the raw I/O floor
- **Trait abstraction cost:** because all families implement the same `PersistentMap<K,V>` / `PersistentVector<A>` etc. traits, write the benchmark harness generically and run it both as a concrete call and through the trait bound. Criterion should produce identical numbers if monomorphisation is working — any regression here is a sign of missed inlining or vtable dispatch leaking through

**Acceptance:** `bench.sh` runs P.0 suite end-to-end; results saved to `docs/baselines.md`.

### P.1 — Establish baselines

- Run P.0 suite for all families; record results in `docs/baselines.md`
- Identify the top-3 regressions vs in-memory pds (by factor overhead)
- Profile each regression with `samply` to locate the hot path

**Acceptance:** baselines committed; profile results in `docs/decisions.md` for each bottleneck.

### P.2 — pds-folio tuning

- Address the top regressions identified in P.1 for pds-folio types
- Expected candidates: slab slot serialisation overhead (hot path for PostcardCodec),
  I/O amplification on path-copy (writing O(depth) pages per mutation),
  refcount table contention on bulk operations
- Each optimisation: baseline → profile → implement → measure → record in `docs/decisions.md`

**Acceptance:** re-run P.0 suite; top regressions each improved by ≥20% or documented as unavoidable.

### P.3 — pds-merkle-spine tuning

- Address top regressions specific to VersionedHamt (Merkle hash computation overhead,
  VersionStore write amplification, proof generation cost)
- Hash caching: ensure BLAKE3 node hashes are cached per-mutation not recomputed on read
- `prove_inclusion` profiling: ensure proof path is O(depth) not O(N)

**Acceptance:** re-run H-phase benchmarks; Merkle overhead ≤ 2× pds-folio baseline for same workload.

### P.4 — Cross-family comparison report

- Write `docs/perf-comparison.md`: tables + prose comparing all four families across all P.0 patterns
- Include absolute numbers and normalised overhead factors (vs in-memory pds)
- Note which overheads are fundamental (persistence I/O, MVCC, Merkle) vs incidental (fixable)
- Recommendations: when to use each family

**Acceptance:** `docs/perf-comparison.md` committed; all four families represented for every applicable benchmark pattern.

---

## Phase 0 — Foundations {#phase-0}

Everything in this phase must land before any structural work begins. The
goal is to make the project safe to change: CI catches regressions,
benchmarks quantify impact, fuzz targets catch edge cases, miri catches UB,
and architecture documentation ensures changes are made with understanding.

### 0.1 CI pipeline, test.sh, build.sh — DONE [2026-04-24]

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

### 0.2 Complete fuzz coverage — DONE [2026-04-24]

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

### 0.3 Complete benchmark coverage — DONE [2026-04-24]

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

### 0.4 Dependency audit — DONE [2026-04-24]

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

### 0.5 Architecture documentation — DONE [2026-04-24]

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

### 1.1 Merge or close stale dependabot PRs — DONE [2026-04-24]

**What:** Five dependabot PRs (#142, #132, #126, #125, #124) bumping rayon,
rand, rpds, criterion, and half have sat unmerged for 6-12 months.

**Why:** Stale PRs signal an unmaintained project. Dependency updates often
contain security fixes.

**Complexity:** Trivial.

---

### 1.2 Remove dead pool code — DONE [2026-04-24]

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

### 1.4 Edition 2021 migration — DONE [2026-04-24]

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

### 2.1 Fix RRB tree concatenation (issue [#35](https://github.com/jneem/imbl/issues/35)) — DONE [2026-04-24]

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

### 2.2 `get_next_exclusive` / `get_prev_exclusive` (issue [#157](https://github.com/jneem/imbl/issues/157)) — DONE [2026-04-24]

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

### 2.3 OrdMap `iter_mut` (issue [#156](https://github.com/jneem/imbl/issues/156)) — DONE [2026-04-24]

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

### 2.4 HashMap/HashSet diff — DONE [2026-04-24]

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

### 2.5 Vector diff — DONE [2026-04-24]

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

### 2.6 Patch/apply from diff — DONE [2026-04-24]

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

### 2.7 General merge — DONE [2026-04-24]

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

### 2.8 Map value and key transformations — DONE [2026-04-24]

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

### 2.9 Map/set partitioning and bulk filtering — DONE [2026-04-24]

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
- `difference_with<F>(&self, other: &Self, f: F) -> Self where F: FnMut(&K, &V, &V) -> Option<V>`
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

### 2.10 Vector convenience operations — DONE [2026-04-24]

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

### 2.11 Companion collection types — DONE [2026-04-24]

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

### 3.1 `Arc::get_mut` in-place mutation — DONE [2026-04-24]

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

### 3.2 Unsafe code audit (issue [#27](https://github.com/jneem/imbl/issues/27)) — DONE [2026-04-24]

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

### 3.4 Parallel iterators and bulk operations (rayon) — DONE [2026-04-25]

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

### 3.5 PartialEq ptr_eq fast paths — DONE [2026-04-24]

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

### 3.6 Pointer-aware subtree skipping in diff — DONE [2026-04-24]

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

### 4.1 Vector prefix buffer — DONE [2026-04-24]

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

### 4.6 Vector Merkle hash caching — DONE [2026-04-25]

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

### 4.7 Pluggable hash width and fast-path hashing — DONE [2026-04-25]

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

**Status:** Deferred — tracked as [R.13](#r13-configurable-branching-factor-via-const-generics-large--deferred). See DEC-011.

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

### R.1 Missing set operations on newer collection types (HIGH) — DONE [2026-04-26]

**What:** Seven methods are missing across five types — all required by the set operation
naming rules in `directives.md`.

| Type | Missing |
|------|---------|
| `Bag` | `symmetric_difference()` |
| `HashMultiMap` | `symmetric_difference()` |
| `InsertionOrderMap` | `symmetric_difference()` |
| `Trie` | `symmetric_difference()` |
| `BiMap` | `difference()`, `intersection()`, `symmetric_difference()` |
| `SymMap` | `difference()`, `intersection()`, `symmetric_difference()` |

**Semantics:**
- `Bag::symmetric_difference`: multiset symmetric diff — result count = `|self_count - other_count|`,
  elements with equal counts excluded. Requires `S: Default` (matches `difference`/`intersection`).
- `HashMultiMap::symmetric_difference`: keys in exactly one map (their full value sets). Consuming.
- `InsertionOrderMap::symmetric_difference`: keys in exactly one map. Consuming.
- `Trie::symmetric_difference`: paths in exactly one trie. Consuming. Can be expressed as
  `self.difference(other_clone).union(other.difference(self_clone))`.
- `BiMap`/`SymMap difference/intersection/symmetric_difference`: match by key (forward direction).
  All consuming. `S: BuildHasher + Clone + Default` block.

**Why:** The directives mandate all four canonical set ops on every type that logically supports
them. The gap was identified in the consistency audit [2026-04-25].

**Complexity:** Low. Each method is ≤15 lines following the existing `difference()`/`intersection()` patterns.

**Acceptance:** `test.sh` passes; each new method has ≥2 unit tests; no new methods violate
the `Add`/`Mul`/`Sum` prohibition.

---

### R.2 Rayon parallel iterators for newer collection types (MEDIUM) — DONE [2026-04-26]

**What:** Add rayon support (`IntoParallelIterator`, `IntoParallelRefIterator`,
`FromParallelIterator`, `ParallelExtend`) to types added after the original 3.4 parallel work.

Candidates (highest priority first):
- `Bag` — backed by a single `GenericHashMap`; par_iter reuses HashMap's `UnindexedProducer`
- `HashMultiMap` — similar; flat pair iteration
- `BiMap` — backed by two `GenericHashMap`s; par_iter over the forward map
- `SymMap` — same as BiMap

InsertionOrderMap/Set: ordering concerns — `par_iter` is safe (read-only), but `FromParallelIterator`
would lose insertion order. Implement read-only parallel iteration only, document the limitation.

Update `lib.rs` and `README.md` feature claims if any type is excluded.

**Why:** `lib.rs` and `README.md` currently claim rayon support for "all collection types" — this
is inaccurate. Either implement for all, or update the claim. The consistency audit flagged this
as a medium-priority documentation/implementation gap.

**Complexity:** Low-medium. Each type ≈ 30–50 lines following `src/hash/map.rs` patterns.

**Prerequisites:** R.1 (so set ops and rayon are consistent).

**Acceptance:** `cargo test --features rayon` passes; claim in lib.rs/README updated to match
actual coverage.

---

### R.3 Fix `Trie::default()` generic impl (MEDIUM) — DONE [2026-04-26]

**What:** Replace the two concrete `Default` impls for `GenericTrie` with a single generic impl:

```rust
// Replace both cfg-gated impls (lines ~347-365 in src/trie.rs) with:
impl<K, V, S, P> Default for GenericTrie<K, V, S, P>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericTrie { value: None, children: GenericHashMap::default() }
    }
}
```

The current two concrete impls (`RandomState` / `foldhash::fast::RandomState`) do not compile
under `no_std + foldhash` if `std` is absent and `foldhash` is absent simultaneously.

**Why:** Directive violation — `Default` must be generic. All other collections use the generic
pattern. This was identified in the consistency audit.

**Complexity:** Trivial (5-line change).

**Acceptance:** `cargo test --no-default-features --features foldhash` and
`cargo test` both pass. No cfg-gated `Default` impls remain for this type.

---

### R.4 Add code examples to legacy module docs (LOW) — DONE [2026-04-26]

**What:** Five legacy module-level `//!` doc blocks lack usage examples. Add at least one
`# Example` block to each:

- `src/hash/map.rs`
- `src/hash/set.rs`
- `src/ord/map.rs`
- `src/ord/set.rs`
- `src/vector/mod.rs`

**Why:** New types (Bag, HashMultiMap, etc.) all have examples; legacy types do not. Inconsistent
first impression for API consumers. Consistency audit flagged as low-priority.

**Complexity:** Low. Examples can be short `create / insert / lookup` sequences.

**Acceptance:** `cargo doc --no-deps` passes with `RUSTDOCFLAGS="-D warnings"`; each module has
≥1 doctest that `cargo test --doc` executes successfully.

---

### R.5 Document `debug` feature in lib.rs and README (LOW) — DONE [2026-04-26]

**What:** The `debug` feature flag exists in `Cargo.toml` but is absent from both the `lib.rs`
feature table and the `README.md` feature table. Add a row to both tables describing what the
feature does.

**Why:** Users cannot discover the feature unless they read `Cargo.toml` directly. Consistency
audit flagged as low-priority.

**Complexity:** Trivial.

**Acceptance:** Both tables list `debug`; `cargo doc --no-deps` passes.

---

### R.6 Expand test.sh quality gate (LOW) — DONE [2026-04-26]

**What:** Add the following checks to `test.sh` in the correct sequence:

1. `cargo fmt --check` — before all other steps (fast, no compilation)
2. `cargo check --no-default-features` — after `cargo test` steps (directive compliance: no_std surface)
3. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` — replaces the current `cargo doc` step
4. `cargo audit` — at the end (requires network; slowest step)

Note: `cargo miri test` is nightly-only and belongs in the `nightly` devShell / CI matrix, not
in `test.sh`. Verify in CI yml that miri is already invoked there; if not, add it.

**Why:** Directive lists these steps as part of the quality gate. `cargo fmt --check`, `--no-default-features`,
`RUSTDOCFLAGS="-D warnings"`, and `cargo audit` are all specified but absent from the current script.
The miri situation needs clarification (it may already be in CI).

**Complexity:** Low (test.sh edits only).

**Acceptance:** `test.sh` exits 0 on a clean tree; each new step is visible in the script with a
comment explaining what it checks.

---

### R.8 Extend proptest / quickcheck / arbitrary to all collection types (LOW) — DONE [2026-04-27]

**What:** Extended `proptest`, `quickcheck`, and `arbitrary` to cover all 20 collection types.

Phase 1 [2026-04-26]: Added the 7 Hash-backed derived types (Bag, HashMultiMap, BiMap, SymMap,
InsertionOrderMap, InsertionOrderSet, Trie).

Phase 2 [2026-04-27]: Added the 7 Ord-backed derived types (OrdBag, OrdMultiMap, OrdBiMap,
OrdSymMap, OrdTrie, OrdInsertionOrderMap, OrdInsertionOrderSet) and UniqueVector.

README updated to "all 20 collection types" for all three feature flags.

---

### R.9 Parallel transform operations — par_filter, par_map_values (LOW) — DONE [2026-04-26]

**What:** Add parallel higher-order transform methods to map and set types:

- `par_filter(f: Fn(&K, &V) -> bool)` on `HashMap`, `OrdMap`
- `par_filter(f: Fn(&A) -> bool)` on `HashSet`, `OrdSet`
- `par_map_values(f: Fn(&V) -> V2)` on `HashMap`, `OrdMap`
- `par_map_values_with_key(f: Fn(&K, &V) -> V2)` on `HashMap`, `OrdMap`

Initially all implemented via `par_iter().filter/map().collect()` (see DEC-034).
`par_map_values` and `par_map_values_with_key` were subsequently upgraded to
tree-native O(n/p) implementations (see R.10 / DEC-035).

Module-level `## Parallel operations` section added to `src/lib.rs` with full
coverage tables for iteration, set ops, and transform ops. Section distinguishes
"implementation-optimised" (tree-native) from "convenience" (collect-based) methods.

**Acceptance:** `test.sh` passes; `cargo test --features rayon` passes (48+ tests
in `hash::rayon`, 35+ in `ord::rayon`); zero compiler warnings.

---

### R.10 Tree-native par_map_values for HashMap and OrdMap (MEDIUM) — DONE [2026-04-26]

**What:** Replaced the `par_iter().map().collect()` implementation of `par_map_values`
and `par_map_values_with_key` on both `HashMap` (HAMT) and `OrdMap` (B+ tree) with
tree-native parallel implementations (see DEC-035).

**HashMap / HAMT:** Added `map_values_hamt_node_par` and helpers in `src/hash/rayon.rs`.
Root HAMT entries are processed in parallel via rayon, preserving node positions from
`SparseChunk::entries()`. Key-hash Merkle values copied verbatim; KV Merkle invalidated.
`GenericSimdNode::map_values()` added (cfg-gated) to handle the private `control` field.

**OrdMap / B+ tree:** Added `par_map_values_ord_node` in `src/ord/rayon.rs`. Branch
separator keys cloned unchanged; leaf children processed in parallel at the top level.
`Branch::map_values`, `Leaf::map_values`, `Node::map_values` added (cfg-gated) to
`src/nodes/btree.rs`.

**Result:** Both `par_map_values` and `par_map_values_with_key` are now O(n/p) on
`HashMap` and `OrdMap`. `par_filter` remains collect-based — tree topology changes
require re-insertion.

**Acceptance:** `test.sh` passes; all 905 tests green with `--all-features`; zero warnings.

---

### R.7 Fix rpds version in README comparison table (LOW) — DONE [2026-04-26]

**What:** README states rpds 1.2.0 in the comparison table, but `Cargo.toml` dev-dep pins 1.1.0.
Check `Cargo.lock` for the actual resolved version and update the table to match. If the lock file
shows 1.1.x, use that; if 1.2.0 has been released and is available, update the dev-dep too.

**Why:** Inaccurate version in a comparison table misleads users.

**Complexity:** Trivial (1-2 line change).

**Acceptance:** README version matches `Cargo.lock` resolved version.

---

### R.12 Cross-session interning: verbatim-hash pool reconstruction (MEDIUM-HIGH) — DEFERRED

**Context:** `to_maps()` rebuilds maps via `FromIterator`, which re-hashes each
key with a fresh `RandomState`. This means loaded maps have a different hasher
seed from the original session. Consequently, their HAMT Merkle hashes differ
from the original, and `InternPool` cannot merge loaded nodes with in-memory
ones — they appear content-different even when semantically equal.

**Option A — Deterministic hashing (recommended simple path):**
If the caller uses a fixed-seed hasher (`IdentityHasher` for integer keys, or a
seeded `FxHasher`/`AHash` with a hard-coded seed), the same key always produces
the same hash across sessions. In that case `to_maps()` already works — no new
API needed. The maps round-trip with identical HAMT structure, identical Merkle
hashes, and `InternPool` merges them correctly.

Additional benefits of deterministic hashing:
- **Reproducible test failures:** property tests that expose a bug can be replayed
  exactly; non-deterministic hasher seeds cause spurious failures to not reproduce.
- **Deterministic debugging:** inspecting a map's internal HAMT layout is stable
  across runs — the same key always lands in the same slot.
- **Comparable snapshots:** Merkle hashes for the same logical content are equal
  across processes and restarts, enabling efficient diff and sync operations.

Trade-off: Deterministic hashers (especially identity hashers) are vulnerable to
Hash DoS if keys come from untrusted sources (web inputs, adversarial data). For
controlled environments (internal data, pre-validated keys), this is not a concern.
Recommended hasher for integer keys: `IdentityHasher` (already in this crate).
For string/byte keys in non-adversarial contexts: `FxHasher` with a fixed seed.

**Option B — Verbatim reconstruction (original design, higher complexity):**
Add `to_maps_verbatim()` (and set/bag/bimap/symmap variants) that reconstruct
maps by inserting (key, value, pre-computed-hash) triples directly into the HAMT,
bypassing re-hashing. The stored H values in `PoolEntry::Value(A, H)` are verbatim
from the original session. After verbatim reconstruction, the loaded map has the
same H values, same HAMT structure, and same Merkle hashes as the original.

**API sketch (Option B):**
```rust
impl<K: Clone, V: Clone, H: HashWidth> HashMapPool<K, V, H> {
    /// Reconstruct maps using stored hash values verbatim (no re-hashing).
    /// Callers must ensure the hasher configuration matches the serialising session.
    pub fn to_maps_verbatim<P: SharedPointerKind>(
        &self,
        hasher: &impl BuildHasher,
    ) -> Vec<GenericHashMap<K, V, ..., P, H>>
}
```

**Precondition (Option B):** The pool must have been serialised from a map using the same
hasher configuration that will be active when loading. Violating this produces a
structurally valid but semantically incorrect map (wrong slot assignments). This
should be documented as a user invariant and, where possible, enforced by storing
a hasher fingerprint in the pool format.

**Recommendation:** Start with Option A (document deterministic hashing as the
cross-session interning pattern). Implement Option B only if a concrete use case
requires hash-randomisation to remain enabled (e.g. public-facing APIs accepting
untrusted keys). Option A is zero additional implementation cost.

**Complexity:** Option A — zero (documentation + IdentityHasher example). Option B —
medium-high (new HAMT insertion path + fuzz coverage).

**Prerequisites:** 6.6 ✓ (SSP serialisation), 6.5 ✓ (InternPool).

**Acceptance:** Either: (A) documentation + example demonstrating cross-session
round-trip with deterministic hasher, confirming Merkle hash stability; or (B)
`to_maps_verbatim()` and variants with fuzz target and round-trip test. `test.sh` passes.

---

### R.13 Configurable branching factor via const generics (LARGE) — DEFERRED

**Blocker:** `generic_const_exprs` is unstable on stable Rust (rust-lang/rust#76560).
The HAMT's SIMD node hierarchy requires `SparseChunk<..., 2^HASH_LEVEL_SIZE>` — a
computed const generic argument. Full historical context in [Phase 5.3](#phase-5).

**What:** Replace the hard-coded size constants in `config.rs` (`HASH_LEVEL_SIZE`,
`VECTOR_CHUNK_SIZE`, `ORD_CHUNK_SIZE`) with const generic parameters on the
collection types, letting callers specialise pds for their workload at compile time.

**Nightly-gate approach:** Add a `nightly-branching` feature flag that enables
`#![feature(generic_const_exprs)]`. Gated behind the feature, all collection types
accept const generic size parameters.

**Complexity:** Large (~140 type sites, ~80 impl blocks).

**Dependencies:** `generic_const_exprs` stabilisation, or decision to accept a
nightly-only `nightly-branching` feature flag for specific consumers.

**Acceptance:** All collection types accept const generic size parameters under
the `nightly-branching` flag. `test.sh` passes including the `small-chunks` variant.
A stable-Rust path exists once `generic_const_exprs` stabilises.

### R.14 Content hash for OrdMap and OrdSet ✓ DONE 2026-04-26

**What:** Add a lazily-computed, cached content hash to `OrdMap` and `OrdSet` behind
an `ord-hash` feature flag.  When the feature is enabled, a `content_hash()` method
returns a `u64` fingerprint of the map's key-value content.  The hash is computed on
first call and cached; it is invalidated (reset to "dirty") on any mutation
(insert, remove, clone-and-mutate via CoW).

**Why:**

This item directly closes one of the last remaining reasons to prefer `HashMap` over
`OrdMap`.  See the "Choosing the right map" section in `README.md` for the full
comparison; the remaining HashMap-exclusive advantages after this item lands are:
(a) keys that implement `Hash + Eq` but not `Ord`, and (b) high-frequency
clone-and-compare workflows on same-origin maps where the Merkle hash fires before
any entry is compared.  For general use `OrdMap` has equal or better allocation
count, cache behaviour, parallel bulk ops, iteration, and equality speed.

Specific capabilities gained:

- **Use as a `HashMap` key or in `HashSet`**: OrdMap/OrdSet currently lack a `Hash`
  impl because hashing requires `K: Hash` (not in the base constraint `K: Ord`).
  With `ord-hash`, `Hash` is implemented for `GenericOrdMap<K, V, P>` when
  `K: Hash, V: Hash`, giving O(1) amortised hashing after the first call.
- **O(1) inequality fast-path in `PartialEq`**: if both maps have a valid cached hash
  and the hashes differ, they cannot be equal — skip the O(n) sorted scan.  Equal
  hashes still fall through to the scan (collision safety).  Mirrors the
  `kv_merkle_hash` fast-path on `HashMap`.
- **Change detection**: call `content_hash()` before and after a pipeline stage to
  detect whether the map changed in O(1).
- **Content-addressed caching / memoisation**: use OrdMap as a cache key in a
  `HashMap<OrdMap<K,V>, Result>` without first converting to a sorted `Vec`.

**Design:**

*Feature gate:* New `ord-hash` feature in `Cargo.toml` (`default = false`).
No code change and no overhead without the feature.

*Storage:*
```rust
// GenericOrdMap fields added under #[cfg(feature = "ord-hash")]:
content_hash_cache: Cell<u64>,   // cached hash value (0 = uninitialised)
hash_valid: Cell<bool>,          // true = cache is current
```
`Cell` (not `AtomicU64`) is sufficient — `GenericOrdMap` is not `Sync` through
the cache field; `Sync` is derived from the element type as before.  The `Hash`
impl takes `&self` which is valid because `Cell` is not `Sync` and we never
alias across threads.

*Hash scheme:* Sequential XOR over `(hash(k) ^ hash(v))` for each entry in
sorted order.  XOR is order-independent (same result whether computed forward or
backward), but OrdMap's deterministic order means a stronger polynomial rolling
hash is also feasible.  Start with XOR to match the pattern established by
`kv_merkle_hash` on HashMap; note the decision in `docs/decisions.md`.

*Invalidation:* every mutation path (`insert`, `remove`, CoW via
`SharedPointer::make_mut`) calls `self.hash_valid.set(false)`.

*`Hash` impl:*
```rust
#[cfg(feature = "ord-hash")]
impl<K: Hash + Ord + Clone, V: Hash + Clone, P: SharedPointerKind> Hash
    for GenericOrdMap<K, V, P>
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.content_hash().hash(state);
    }
}
```

*`PartialEq` short-circuit:*
```rust
// Early exit: different hashes → definitely unequal (no false negatives,
// possible false positives which the scan below handles).
#[cfg(feature = "ord-hash")]
if self.hash_valid.get() && other.hash_valid.get()
    && self.content_hash_cache.get() != other.content_hash_cache.get()
{
    return false;
}
// Fall through to existing O(n) sorted scan.
```

**Scope:**
- `OrdMap` and `OrdSet` (OrdSet reuses OrdMap's impl via its inner map).
- `Hash` impl gated on `K: Hash, V: Hash` (for OrdSet: `A: Hash`).
- `PartialEq` short-circuit gated on `ord-hash` feature.
- `content_hash()` public method for explicit use (change detection, etc.).
- Add `ord-hash` to the feature table in `README.md` and `lib.rs`.

**Not in scope:** Per-node hash caching (would enable O(log n) hash on partial
updates but requires structural changes to `Branch`/`Leaf`; can follow as a future
item if the map-level cache proves insufficient).

**Acceptance criteria:**
- `cargo test --features ord-hash` passes.
- `cargo test` (without feature) passes — no regression.
- `OrdMap<K, V>` and `OrdSet<A>` are usable as `HashMap` keys when
  `K: Hash + Ord, V: Hash` and `A: Hash + Ord`.
- `PartialEq` short-circuit fires and is tested (equal-hash maps, different-hash maps,
  hash-collision maps where scan must confirm).
- `content_hash()` returns consistent values across calls on an unmodified map.
- `content_hash()` is invalidated and recomputed correctly after insert/remove.
- `test.sh` passes.

**Complexity:** Low–medium.  The field addition and invalidation sites are
mechanical; the main design decision is XOR vs polynomial hashing (record in
`decisions.md`).  No structural change to the B+ tree nodes.

---

### R.17 Head-to-head OrdMap vs HashMap criterion benchmarks (LOW)

**What:** Add a criterion benchmark suite that measures `OrdMap` and `HashMap` side-by-side
on the same operations, key types, and collection sizes.  Write the results to
`docs/baselines.md`.

**Why:** The "Choosing the right map" section in `README.md` and the B+ tree vs HAMT
analysis in `docs/architecture.md` make comparative claims without hard numbers.  The
existing `docs/baselines.md` has:
- `HashMap` vs `std::HashMap` comparisons (from the original criterion suite)
- `OrdMap` vs `std::BTreeMap` comparisons
- dhat allocation counts for both types separately

Missing: a direct `OrdMap` vs `HashMap` wall-clock comparison on equivalent workloads.

**Operations to benchmark (i64 keys, sizes 100 / 1K / 10K / 100K):**

| Operation | Notes |
|-----------|-------|
| lookup | sequential random lookups |
| insert\_mut | sequential inserts into a pre-built map |
| remove\_mut | sequential removes |
| iter | full iteration |
| from\_iter | bulk construction |
| par\_union (10K / 100K) | parallel set union — join vs filter+reduce |
| par\_intersection | same |

**Implementation:** Add `benches/compare.rs` (or extend `benches/ordmap.rs` /
`benches/hashmap.rs` with cross-type groups).  Use `criterion::BenchmarkGroup` with
a shared ID so criterion plots them together.

**Output:** Summary table in `docs/baselines.md` § "OrdMap vs HashMap — head-to-head".

**Complexity:** Low — criterion boilerplate; no new library code.

**Prerequisites:** R.15 ✓ (so node size is confirmed before recording baseline numbers)

**Acceptance:** Table in `docs/baselines.md` with OrdMap and HashMap rows side-by-side.
`bench.sh compare` runs without error.

---

### R.15 Re-evaluate OrdMap node size for join-heavy workloads (LOW)

**What:** Re-benchmark `ORD_CHUNK_SIZE` at 16, 24, 32, and 48 with the join algorithm
operations added in R.11 (`par_union`, `par_intersection`, `par_difference`,
`par_symmetric_difference`) and add the results to `docs/decisions.md` as an addendum
to DEC-017.  If a different size wins, update `src/config.rs` and record the change.

**Why:** DEC-017 selected `ORD_CHUNK_SIZE = 32` based exclusively on single-tree
operations (lookup, insert\_mut, remove\_mut, iter, range\_iter).  The join algorithm
introduces a qualitatively different access pattern not present in that benchmark suite:

- `split_node` copies up to `NODE_SIZE/2` entries at each level and produces new
  Arc-wrapped node allocations.
- `concat_node` redistributes entries between nodes when reassembling the result tree.
- Both input trees are traversed simultaneously (dual-tree cache pressure):
  at NODE\_SIZE=32 each node is 512 bytes (4 Apple Silicon cache lines); with two
  trees in flight that is 8 cache lines per recursion level.
- The recursion depth determines how many `rayon::join` tasks are spawned before
  bottoming out.  Smaller nodes → deeper trees → more parallelism exposed earlier.

These factors pull in opposite directions: larger nodes reduce tree height and total
allocations in the result but increase per-split copy cost and dual-tree cache
pressure; smaller nodes expose more parallelism and reduce per-split cost but
increase total allocations.  Whether 32 remains optimal is an open empirical question.

**Benchmark plan:**

```bash
# Save single-tree baseline (already done in DEC-017; use saved criterion baseline
# or re-run if not available)
bench.sh -- --save-baseline node-size-32

# For each candidate size (edit src/config.rs):
#   ORD_CHUNK_SIZE = 16 / 24 / 32 / 48
# Run:
bench.sh ordmap -- --baseline node-size-32   # single-tree ops: lookup, insert_mut, iter
bench.sh ordmap_parallel                     # join ops: par_union, par_intersection, par_difference
# Record results in /private/tmp/bench_nodesize_<N>_$(date +%s).txt
```

Measure at N = 10K and 100K entries.  Compare:

| Operation | size 16 | size 24 | **size 32** | size 48 |
|-----------|---------|---------|-------------|---------|
| lookup (10K/100K) | | | baseline | |
| insert\_mut (10K/100K) | | | baseline | |
| par\_union (10K/100K) | | | baseline | |
| par\_intersection (10K/100K) | | | baseline | |
| split\_node (synthetic) | | | baseline | |

**Decision rule:** If join ops at any candidate size are ≥5% faster than size 32
without a commensurate regression on lookup/insert\_mut (or vice versa), update
`ORD_CHUNK_SIZE` and record the full table in an addendum to DEC-017.  If size 32
remains optimal across all workloads, record that finding — closing the question
explicitly.

**Acceptance:** Benchmark table added to DEC-017 addendum.  If `ORD_CHUNK_SIZE`
changes: `src/config.rs` updated, `test.sh` passes, `docs/baselines.md` updated.

**Complexity:** Low — benchmark-only unless node size changes.

**Prerequisites:** R.11 ✓

---

### R.16 Ord-backed compound collection types (LARGE)

**What:** Add `Ord`-backed variants of the compound collection types that are currently
backed exclusively by `HashMap`/`HashSet`.  Each new type uses `OrdMap` or `OrdSet` as
its underlying store, inheriting the B+ tree's allocation density, sorted iteration,
parallel join operations, and `Ord`/`PartialOrd` trait coverage.

**Current compound types and their backing store:**

| Type | Current backing | Ord variant | Backing |
|------|----------------|-------------|---------|
| `Bag<A>` | `HashMap<A, usize>` | `OrdBag<A>` | `OrdMap<A, usize>` |
| `BiMap<K, V>` | two `HashMap`s | `OrdBiMap<K, V>` | two `OrdMap`s |
| `SymMap<K>` | `HashMap<K, K>` | `OrdSymMap<K>` | `OrdMap<K, K>` |
| `HashMultiMap<K, V>` | `HashMap<K, Vector<V>>` | `OrdMultiMap<K, V>` | `OrdMap<K, Vector<V>>` |
| `InsertionOrderMap<K,V>` | `HashMap` + `Vector` | keep as-is | insertion order ≠ sorted order |
| `InsertionOrderSet<A>` | `HashSet` + `Vector` | keep as-is | insertion order ≠ sorted order |

`InsertionOrderMap`/`InsertionOrderSet` preserve insertion order, which is semantically
distinct from sorted order — they are complementary types, not candidates for Ord variants.

**Why:**

All of the B+ tree advantages that motivated R.11 and R.14 currently stop at
`OrdMap`/`OrdSet`.  The compound types inherit none of them:
- ~4.5× more allocations than an OrdMap-backed equivalent at 100K entries
- No sorted iteration, no `Ord`/`PartialOrd`, no range queries
- Parallel set operations use filter+reduce rather than the join algorithm
- No path to `ord-hash` (R.14) content hashing

`OrdBag`, `OrdBiMap`, `OrdSymMap`, `OrdMultiMap` would give callers that have
`K: Ord` all the same advantages they get from choosing `OrdMap` over `HashMap`.

**Scope per type:**

- Full standard trait coverage (see directives: `Clone`, `Debug`, `PartialEq`/`Eq`,
  `PartialOrd`/`Ord`, `Hash`, `Default`, `FromIterator`, `IntoIterator`, `Extend`,
  `Serialize`/`Deserialize` behind `serde`)
- Set operations (where applicable): `union`, `intersection`, `difference`,
  `symmetric_difference` with canonical names per directives
- Parallel variants (`par_union`, etc.) delegating to the join algorithm via the
  underlying `OrdMap`
- Proptest strategies and property tests

**Complexity:** Large — four new types, full trait coverage, tests.  Largely mechanical
once the first (`OrdBag`) is done; the rest follow the same pattern.

**Sequencing:** Can be done one type at a time.  Suggested order: `OrdBag` (simplest,
only wraps `OrdMap<A, usize>`) → `OrdMultiMap` → `OrdSymMap` → `OrdBiMap` (bijection
invariant with two `OrdMap`s is the most complex).

**Prerequisites:** R.11 ✓ (for parallel join to be available on `OrdMap`)

**Acceptance:** Each type passes `test.sh`; trait coverage table in `directives.md` has
no gaps; benchmark entry added to `docs/baselines.md`.

---

### 3.4: Parallel bulk operations — DONE

**What:** Parallel `union`, `intersection`, `difference`,
`symmetric_difference` for HashMap/HashSet via rayon.

**Status:** DONE. All parallel operations implemented:
- `par_union`, `par_intersection`, `par_difference`,
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
                                   ▼
Phase F (cross-variant traits)
  F.0 base traits ◄── (none — self-contained)
  F.1 versioning + Merkle traits ◄── F.0
                                   │
Phase W (workspace consolidation)  │
  W.0 [workspace] in Cargo.toml ◄── F.0 (trait crate stable before adding members)
  W.1 CI + scripts update ◄── W.0
                                   │
Phase G (pds-folio)                │
  G.0 create crate ◄── W.0, F.0, folio S64/S37/S66
  G.1–G.5 HAMT + HamtIndex ◄── G.0, merkle-spine Stage 1
  G.6–G.7 trait impls (HashMap/HashSet) ◄── G.2, F.0
  G.8–G.12 Vector, OrdMap/OrdSet ◄── G.3 (refcount), F.0
                                   │
Phase H (pds-merkle-spine)         │
  H.0 create crate ◄── W.0, G.5, merkle-spine MS-F0, F.1
  H.1–H.5 VersionedHamt ◄── H.0
  H.6 sparse sync ◄── H.5 (deferred)
  H.7–H.8 trait impls + tests ◄── H.2, F.1
                                   │
Phase P (cross-collection perf)    │
  P.0 benchmark suite design ◄── G.12, H.8
  P.1 baselines ◄── P.0
  P.2 pds-folio tuning ◄── P.1
  P.3 pds-merkle-spine tuning ◄── P.1
  P.4 comparison report ◄── P.2, P.3
```

### Parallel tracks — status

All major tracks complete. Remaining open items listed in [Residual](#residual).

1. **Vector track:** ✓ COMPLETE (2.1, 4.1, 4.6 all done)
2. **Hash track:** ✓ COMPLETE (4.2→4.3✗→6.7✗→6.8✗; 4.7 stage 1+2 done;
   stage 3 identity hasher is residual)
3. **Mutation track:** ✓ COMPLETE (3.1→3.2→3.3→5.2→4.5 all done)
4. **Parallel track:** ✓ COMPLETE (3.4 par_iter/par_iter_mut/par_sort ✓;
   parallel bulk ops — par_union/par_intersection/par_difference/
   par_symmetric_difference for HashMap+HashSet ✓)
5. **Diff track:** ✓ COMPLETE (2.4→2.5→2.6→3.6, 3.5 all done)
6. **Map API track:** ✓ COMPLETE (2.7, 2.8, 2.9, 2.10, 2.11 all done)
7. **Hash integrity track:** ✓ COMPLETE (4.4→6.5→6.6 all done)
8. **Serialisation track:** ✓ COMPLETE (6.6 done: HashMap, HashSet via
   HashMapPool; OrdMap, OrdSet via OrdMapPool; Vector via VectorPool)
9. **Trie track:** ✓ COMPLETE (6.9 done)

---

## Architectural open items (post-H.8 review) {#arch-open}

These items were identified during the 2026-07-01 architectural review. None are
blocking; all are deferred until the relevant infrastructure lands.

- **DEC-DURABLE-1 follow-up:** Evaluate whether `pds-durable`'s `TieredMap` is
  superseded once `pds-folio` gains a disk `Backend` implementation. If yes, mark
  `pds-durable` maintenance-only and deprecate `TieredMap`. Gate: `pds-folio` disk
  backend must be complete and benchmarked against `pds-durable` for the relevant
  workload patterns.

- **pds-folio disk Backend:** Add a CoW+WAL disk-backed `Backend` implementation to
  `pds-folio`. This is the prerequisite for the DEC-DURABLE-1 re-evaluation and
  enables the folio tier in the `MerkleWrapper` → `VersionedHamt` upgrade path.
  (Unblocks DEC-DURABLE-1 re-evaluation.)

---

## References {#references}

See `docs/references.md` for the full bibliography — papers, implementations,
and Rust crates referenced by plan items above.
