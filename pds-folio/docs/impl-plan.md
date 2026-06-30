# pds-folio — Implementation Plan

Phased implementation plan for `pds-folio`: folio-backed persistent data
structures with structural sharing.

See `../docs/pds-folio-spec.md` for the full design specification.

---

## Contents

- [Done](#done)
- [Current](#current)
- [Future](#future)

---

## Done {#done}

*Newest first.*

- **[2026-07-01] G.13–G.15 — Documentation polish and feature flag notes.**
  - `src/lib.rs` crate-level doc updated: collection type table, storage model,
    codec, structural sharing invariants, consensus backend note, phase status.
  - `Cargo.toml` features section: commented-out `consensus` and `serde` placeholders
    with explanation.
  - `src/lib.rs` status updated to "Phase G complete — G.1–G.12 done".
  - No new feature flags added (no folio-consensus crate yet; serde deferred).

- **[2026-07-01] G.12 — Integration tests (Vector + OrdMap / OrdSet).**
  `tests/ordmap_vector_integration.rs`: 15 integration/proptest tests.

  Deterministic tests:
  - `ordmap_insert_many_and_verify_all` (2*BTREE_ORDER+5 keys)
  - `ordmap_insert_reverse_order` (ascending output verified)
  - `ordmap_range_query_across_leaf_boundary` (range spanning the split point)
  - `ordmap_remove_half_and_verify_remaining` (even keys removed)
  - `ordmap_snapshot_isolation_insert_does_not_affect_original`
  - `ordmap_snapshot_isolation_remove_does_not_affect_original`
  - `ordset_insert_many_and_verify_sorted`
  - `ordset_range_query`
  - `vector_concat_split_round_trip_small`
  - `vector_concat_split_round_trip_cross_boundary`
  - `vector_snapshot_isolation`
  - `vector_large_n_push_and_get` (BRANCHING_FACTOR²/2 elements)

  Proptest (20 cases each):
  - `prop_vector_concat_split_inverse` (concat/split are inverses)
  - `prop_ordmap_sorted_order_invariant` (iter is always sorted, unique)
  - `prop_ordmap_range_matches_full_iter_filtered` (range == iter filtered)

  All 15 integration tests green.  Full workspace `test.sh` passes.

- **[2026-07-01] G.11 — `FolioOrdMap` + `FolioOrdSet` CRUD and trait impls.**
  `src/folio_ordmap.rs`: `FolioOrdMap<K, V, C, B>` backed by `OrdMapNodeStore<B>`.
  `src/folio_ordset.rs`: `FolioOrdSet<A, C, B>` — thin newtype over
  `FolioOrdMap<A, (), C, B>`.

  Key operations — all path-copy, O(log N):
  - `get(key)` / `contains_key(key)` — descend via `find_child` separator routing.
  - `insert(key, value)` — decode/merge entries in leaf; split leaf at midpoint when
    full (`BTREE_ORDER` entries); propagate split separator up through internal nodes;
    grow tree height when root splits.  Internal nodes split likewise when they absorb
    a child split and exceed `BTREE_ORDER` separators.
  - `remove(key)` — remove from leaf (no rebalancing); propagate empty-child removal
    upward; collapse single-child root.
  - `first()` / `last()` — descend to leftmost / rightmost leaf entry.
  - `range(bounds)` — recursive in-order tree walk with subtree pruning against bounds.
  - `iter()` — delegates to `range(..)`.

  Refcount invariant: same path-copy pattern as HAMT G.3 and FolioVector G.9.
  Unchanged siblings have refcounts incremented on every internal-node rebuild.
  Newly allocated pages (from recursive splits) are never double-incremented.

  `Clone` — O(1) root refcount increment.
  `Drop` — iterative DFS over reachable pages; decrement refcounts; free pages
  that reach zero via batch `free_nodes`.

  `PersistentCollection` / `PersistentOrdMap<K, V>` / `PersistentOrdSet<A>` impls
  delegate to inherent methods with `.expect()`.

  Two clippy fixes in pre-existing test code:
  - `folio_vector.rs`: removed unused `use pds::traits::PersistentVector` import.
  - `node.rs`, `vector.rs`: changed `assert!(const_expr)` → `const { assert!(...) }`.

  FolioOrdMap: 18 unit tests.  FolioOrdSet: 9 unit tests.
  All 151 lib + 12 integration + 7 doc tests green.
  Full workspace `test.sh` passes (fmt + tests + clippy + doc).

- **[2026-07-01] G.10 — OrdMap / OrdSet: B+ tree node types and slab layout.**
  `src/btree.rs`: `BTreeNodePage([u8; 512])` — `#[repr(transparent)]` Pod + Zeroable
  newtype (manual `Default` via `bytemuck::Zeroable::zeroed()`).  Two discriminant
  values: `DISCRIMINANT_LEAF = 0x01`, `DISCRIMINANT_INTERNAL = 0x02`.
  `BTREE_ORDER = 32`.

  Leaf layout (512 bytes):
  - Bytes 0..2: discriminant + count
  - Bytes 2..10: `next_leaf: u64` (0 = None)
  - Bytes 10..76: `entry_offsets [u16; 33]` — offsets[count] = total data bytes
  - Bytes 76..512: data (436 bytes) — codec-encoded K||V pairs in sorted order

  Internal layout (512 bytes):
  - Bytes 0..2: discriminant + count (separator key count; children = count+1)
  - Bytes 2..4: pad
  - Bytes 4..260: `children [u64; 32]` (256 bytes)
  - Bytes 260..324: `key_offsets [u16; 32]` (64 bytes)
  - Bytes 324..512: `key_data` (188 bytes)

  `LeafBuilder` / `LeafReader` — write/read leaf pages with `push_encoded<K,V,C>` /
  `decode_kv<K,V,C>` / `decode_key<K,C>`.  `LeafReader::next_leaf()` for range scans.
  `build_internal_node<K,C>(children, separator_keys)` / `InternalReader` — construct
  and read internal pages.  `InternalReader::find_child<K,C>` routes by key via linear
  scan of decoded separator keys (O(count), count ≤ 32).

  Note: `decode_separator_key` for the last key uses `postcard::take_from_bytes` since
  there is no sentinel offset for the last key's end.

  14 unit tests: size assertions, Pod check, discriminant uniqueness, data-section
  arithmetic, leaf empty/single/multiple/full/overflow/string/next_leaf/decode_key
  round-trips, internal single/three/routing/max-order round-trips and `find_child`
  correctness.  All 125 lib + 12 integration + 7 doc tests green.  Full workspace
  `test.sh` passes.

- **[2026-07-01] G.9 — `FolioVector` CRUD and `PersistentVector` trait impl.**
  `src/folio_vector.rs`: `FolioVector<A, C, B>` backed by a `VectorNodeStore<B>` (thin
  wrapper over `FolioStore<B>` with `refcounts: HashMap<u64, u32>`).

  Key operations — all path-copy, O(log_32 N):
  - `get(pos)` — navigate via discriminant byte and cumulative sizes; decode with `C`.
  - `update(pos, value)` — path-copy; unchanged siblings have refcounts incremented.
  - `push_back(value)` — recursive descent to rightmost leaf; on overflow, grows tree
    depth by wrapping in a new internal.  Refcounts incremented for all reused children.
  - `pop_back()` — recursive descent; collapses root if single child remains.
  - `concat(other)` — iterate other, push_back each element.
  - `split_at(mid)` — rebuild both halves by sequential push_back.
  - `push_front` / `pop_front` — delegate to `split_at(1)` / `split_at(len-1)` (not
    O(1) but correct; deferred optimisation).

  Critical refcount invariant: whenever a new internal node is created by path-copy
  and reuses existing child page IDs from the old internal, those shared children
  have their refcounts incremented immediately.  Mirrors the HAMT G.3 pattern exactly.
  Applied in three sites: push non-overflow, push overflow-add-child, pop shrink.

  `PersistentCollection` impl: empty (just the `Clone` bound).
  `PersistentVector<A>` impl: all methods delegate to inherent with `.expect()`.

  23 unit tests: empty, single push, push across leaf boundary, multi-level tree,
  update, pop_back to empty, pop_back multi-level, concat, split_at, structural
  sharing verification, push_front/pop_front, trait surface (generic helpers).
  All tests green.  Full workspace `test.sh` passes.

- **[2026-07-01] G.8 — Vector: RRB-tree node types and slab layout.**
  `src/vector.rs`: `VectorNodePage([u8; 512])` — `#[repr(transparent)]` Pod + Zeroable
  newtype (manual `Default` via `bytemuck::Zeroable::zeroed()` since `[u8; 512]` lacks
  a `derive(Default)`).  Discriminant byte 0: `0x01` = leaf, `0x02` = internal.

  Leaf layout (`BRANCHING_FACTOR = 32`):
  - Bytes 0..2: discriminant + count
  - Bytes 2..68: `entry_offsets [u16; 33]` — `offsets[i]` is start of entry `i` in the
    data section; `offsets[count]` = total data bytes.
  - Bytes 68..512: data section (444 bytes); entries are variable-length, no framing.

  Internal layout:
  - Bytes 0..2: discriminant + count
  - Bytes 2..130: `sizes [u32; 32]` — cumulative subtree element counts.
  - Bytes 130..386: `children [u64; 32]` — folio page IDs.
  - Bytes 386..512: reserved.

  `LeafBuilder` / `LeafReader` — write/read leaf pages with `push_encoded<T,C>` /
  `get_entry<T,C>` and `entry_bytes`.  `build_internal(children, cumulative_sizes)`
  / `InternalReader` — construct and read internal pages.  `InternalReader::find_child`
  locates the child containing a given position via O(count) linear scan on
  cumulative sizes (count ≤ 32).

  `CodecError::EncodeTooLarge` variant added to `codec.rs`.

  14 unit tests: size assertions, Pod check, discriminant uniqueness, leaf
  empty/single/multiple/full/overflow round-trips with PostcardCodec and PodCodec,
  `is_full` flag, internal empty/single/three-children/max-children round-trips and
  `find_child` correctness.  All 106 tests (87 lib + 12 integration + 7 doc) green.
  Full workspace `test.sh` passes.

- **[2026-06-30] G.5 — `HamtIndex`: `PageIndexBackend`.**
  `src/hamt_index.rs`: `HamtIndex<B>` implements `merkle_spine::index::PageIndexBackend`
  using a `HamtMap<IndexKey, IndexValue, PodCodec, B>` as the underlying store.

  `IndexKey` — `#[repr(C)]` Pod pair `(region_id: u64, page_id: u64)`.
  `IndexValue` — 48-byte `#[repr(C)]` Pod encoding of `PageEntry` (content_hash,
  folio_page_id, encoding_tag, chain_depth, 6-byte pad).

  `HamtIndex` maintains an in-memory `HashMap<Hash, HamtMap<…>>` keyed by the 32-byte
  Merkle root hash.  `ZERO_HASH` is always the genesis root (empty HAMT).  All snapshots
  share the same `Arc<Mutex<NodeStore>>` created from the `FolioStore` passed to `new()`.

  `compute_root_hash`: sorts all `(IndexKey, IndexValue)` pairs by `(region_id, page_id)`,
  serialises them into a flat byte buffer, and hashes with `hash_hamt_node` (BLAKE3
  keyed with `ms:hamt-node-v1`).  Empty HAMT maps to `hash_hamt_node(b"")`.

  `PageIndexBackend` methods:
  - `lookup` — O(log N) HAMT get at the requested root.
  - `commit_delta` — clone parent snapshot (O(1)), apply all inserts, compute new
    Merkle root, store snapshot.
  - `delete_index_page` — removes snapshot from map; HAMT `Drop` frees folio pages.
  - `snapshot` — HAMT is already a complete snapshot; returns `index_root` unchanged.

  `merkle-spine` path dep added to `pds-folio/Cargo.toml`.

  9 unit tests: empty genesis, single entry, multiple entries, chained commits,
  overwrite, same-content hash identity, snapshot, delete, unknown-root error,
  multi-region.  All 61 lib + 7 doc tests green.  Full workspace `test.sh` passes.

- **[2026-06-30] G.4 — `HamtSet` wrapper.**
  `src/set.rs`: `HamtSet<A, C, B>` as a thin newtype over `HamtMap<A, (), C, B>`.

  Public API: `new`, `len`, `is_empty`, `contains`, `insert`, `remove`, `iter`,
  `union`, `intersection`, `difference`, `symmetric_difference`.

  `HamtMapIter<'a, K, V, C, B>` added to `src/hamt.rs`: iterative DFS tree traversal
  with explicit work-stack and per-leaf entry buffer; acquires the store lock once per
  leaf page.  `HamtMap::iter()` returns a `HamtMapIter`.

  `Clone`/`Drop` semantics for `HamtSet` inherited from the inner `HamtMap`.

  14 unit tests + 1 doc-test, all green.  Full workspace `test.sh` passes (51 lib +
  7 doc tests).

- **[2026-06-30] G.3 — Reference counting and `Drop`.**
  `NodeStore<B>` gains a `refcounts: HashMap<u64, u32>` field tracking structural
  sharing across `HamtMap` snapshots.  Absent from the table = implicit refcount 1
  (page has exactly one owner).

  `Clone` impl: increments the root page's refcount — O(1).

  `Drop` impl: calls `collect_pages_to_free` (iterative, explicit stack — no
  recursion risk) which decrements refcounts for all reachable pages; pages that
  reach 0 are batch-freed via `NodeStore::free_nodes` (single WAL commit).

  `insert_into_internal` and `remove_from_internal`: when a new internal node is
  allocated that reuses existing child page IDs (path-copy leaves unchanged
  subtrees shared), those children have their refcounts incremented immediately —
  they are now owned by both the old and new internal nodes.  On Drop, the old
  node's `collect_pages_to_free` decrements them back to 1, leaving the children
  live under the new node.

  `remove_from_leaf` and `remove_from_internal` (absent-key path): now return the
  original `page_id` unchanged instead of re-allocating — eliminates the wasteful
  copy noted in G.2.

  5 new unit tests (drop empty, clone shares + original drop leaves clone intact,
  all clones dropped refcounts empty, multiple snapshots independent,
  remove-absent-key no extra alloc).  All 37 lib + 6 doc tests green.

- **[2026-07-01] G.2 — `HamtMap` CRUD.**
  `src/hamt.rs` implements `HamtMap<K, V, C, B>` with full path-copy CRUD.

  `NodeStore<B>` — thin wrapper over `FolioStore<B>` providing `alloc_node`,
  `read_node`, `free_node` (typed `HamtNodePage` read/write as folio page payloads).
  Multiple `HamtMap` snapshots share one `Arc<Mutex<NodeStore<B>>>`.

  `HamtMap` public API: `new(store)`, `len()`, `is_empty()`, `contains_key()`,
  `get()`, `insert()`, `remove()`. All mutations return a new snapshot; original
  is unchanged (path-copy semantics). O(log N) page writes per insert/remove.

  Leaf split strategy: when a leaf is full (`LEAF_CAP = 16` entries or data
  section full), `split_leaf_and_insert` collects all entries + the new entry
  and calls `build_trie_from_entries`, which recursively partitions by 5-bit
  hash slices and builds a subtrie of leaves and internal nodes.

  10 unit tests: empty map, single insert, multiple inserts (32 keys), overwrite,
  remove present/absent, remove all, contains_key; plus PodCodec (u64 keys)
  insert/get and overwrite/remove. 6 doc-tests. All 38 green. `test.sh` passes.

  Design note: absent-key removes allocate a new copy of unchanged pages (no
  path sharing). This is correct but wasteful; G.3 ref-counting will eliminate
  the unnecessary copies by tracking which subtrees are shared.

- **[2026-07-01] G.1 — Core node types and slab layout.**
  `src/node.rs` implements the 512-byte slab slot type and both HAMT node variants.

  `HamtNodePage([u8; 512])` — `#[repr(transparent)]` newtype with `Pod` + `Zeroable`
  derived via `bytemuck`'s proc-macro (no `unsafe` in this crate). Discriminant at
  byte 0 identifies leaf (`0x01`) vs internal (`0x02`) vs unallocated (`0x00`).

  Fixed-header leaf layout (avoids data shifting on append):
  - Bytes 0..2: discriminant + count
  - Bytes 2..130: `key_hashes [u64; LEAF_CAP]`
  - Bytes 130..164: `entry_offsets [u16; LEAF_CAP+1]` — offsets[count] = total data written
  - Bytes 164..512: data section (348 bytes); entries framed as `[key_len: u16][key][value]`

  `LEAF_CAP = 16` (max entries before split).

  Internal node layout (5-bit HAMT, 32-bit bitmap, compressed child array):
  - Bytes 0..8: discriminant + 3-byte pad + bitmap u32
  - Bytes 8..: `children [u64; popcount(bitmap)]` — max 264 bytes total

  `LeafBuilder` / `LeafReader` — write/read leaf pages with `push_framed` / `get_entry`.
  `build_internal` / `InternalReader` — construct and read internal pages.

  13 unit tests: size assertions, Pod check, leaf empty/single/multiple/overflow round-trips
  with PostcardCodec and PodCodec, internal node single/three/all-32 children round-trips,
  discriminant uniqueness. All green. Full workspace `test.sh` passes.

  Design note: `LEAF_CAP = 16` uses a fixed-width header rather than variable offsets to
  avoid shifting existing data on each `push_framed`. This wastes at most 10*(16−count)
  header bytes but simplifies addressing to O(1) with no memmove.

- **[2026-06-30] G.0 — Scaffold.**
  Created `pds-folio` as a Cargo workspace member of the `pds` repo.
  `Cargo.toml` with deps: `folio-core` (path), `folio-collections` (path),
  `pds` (workspace, traits feature), `serde`, `postcard`, `bytemuck`, `thiserror`.
  `src/lib.rs` with `#![deny(unsafe_code)]` and module declarations.
  `src/codec.rs`: `Codec` trait, `PodCodec` (raw bytes + postcard fallback),
  `PostcardCodec` — 10 unit tests, all green.
  `docs/impl-plan.md` (this file) with G.1–G.12 items in Future.

---

- **[2026-07-01] G.7 — Integration tests and proptest suite (HashMap / HashSet).**
  `tests/hamt_integration.rs`: 12 integration/proptest tests.

  Deterministic tests: `insert_many_and_verify_all_via_trait` (128 keys),
  `insert_remove_half_and_verify_remaining` (64 keys, even-half removed),
  `snapshot_isolation_insert_does_not_affect_sibling`,
  `snapshot_isolation_remove_does_not_affect_original`,
  `overwrite_updates_without_growing_map`, `pod_codec_u64_keys_large_insertion`,
  `set_insert_many_and_verify`, `set_snapshot_isolation`.

  Proptest (20 cases each, limited for folio I/O overhead):
  `prop_hamt_map_matches_std_hashmap` (model-based: HAMT vs `HashMap`),
  `prop_snapshot_isolation` (two concurrent snapshots diverge independently),
  `prop_round_trip_key_lookup` (insert N, lookup all),
  `prop_hamt_set_matches_std_hashset` (model-based: HamtSet vs `HashSet`).

  Added `proptest = "1"` to dev-dependencies.  All 91 (72 lib + 12 integration
  + 7 doc) tests green.  Clippy and `cargo doc` warnings clean.

- **[2026-07-01] G.6 — Implement pds cross-variant traits (HashMap / HashSet).**
  `src/traits.rs`: `PersistentCollection`, `PersistentMap<K, V>`, and
  `PersistentSet<A>` implemented for `HamtMap<K, V, C, B>` and `HamtSet<A, C, B>`.

  Trait methods are infallible by contract; `HamtMap`/`HamtSet` methods return
  `Result`. Trait impls use `expect()` to propagate folio I/O/codec panics — correct
  for `MemBackend` (never fails) and documented in the module doc.

  11 unit tests: `pm_get_insert_contains`, `pm_remove`, `pm_is_empty`,
  `pm_remove_absent`, `hamt_map_snapshot_isolation`,
  `ps_insert_contains`, `ps_remove`, `ps_is_empty`,
  `hamt_set_snapshot_isolation`, `hamt_map_round_trip_key_lookup` (64 keys),
  `two_hamt_maps_same_type_different_stores`.

  Fixed 8 `cargo doc` warnings (unresolved links in lib.rs, private `NodeStore`
  link in hamt.rs, unclosed HTML tag in codec.rs).  All 79 lib + 7 doc tests green.

---

## Current {#current}

*Nothing in progress.*

---

## Future {#future}

### G.1 — Core node types and slab layout (DONE — see above)

- `LeafNode` — variable-length layout: `discriminant: u8 | count: u8 | key_hashes: [u64; count] | entry_offsets: [u16; count] | data: [u8; …]`
- `InternalNode` — `discriminant: u8 | bitmap: u64 | children: [SlabPageId; popcount(bitmap)]`
- `LEAF_CAP` constant = max entries before a leaf splits (target: 512-byte slab slot)
- `HamtNodePage` — union type for leaf and internal byte representations; slab slot type
- `FolioSlab<HamtNodePage>` wrapper type
- Unit tests: header size checks; leaf insert/read round-trip for `PostcardCodec`; `PodCodec` u64 round-trip

**Acceptance:** `cargo test` green; size assertions pass.

### G.2 — `HamtMap` CRUD (DONE — see above)

### G.3 — Reference counting and `Drop` (DONE — see above)

### G.4 — `HamtSet` wrapper (DONE — see above)

### G.5 — `HamtIndex`: PageIndexBackend (DONE — see above)

### G.6 — Implement pds cross-variant traits (HashMap / HashSet) — DONE — see above

### G.7 — Integration tests and proptest suite (HashMap / HashSet) — DONE — see above

### G.8 — Vector: RRB-tree node types and slab layout (DONE — see above)

### G.9 — `FolioVector` CRUD and `PersistentVector` trait impl (DONE — see above)

### G.10 — OrdMap / OrdSet: B+ tree node types and slab layout (DONE — see above)

### G.11 — `FolioOrdMap` + `FolioOrdSet` CRUD and trait impls (DONE — see above)

### G.12 — Integration tests (Vector + OrdMap / OrdSet) (DONE — see above)

### G.13 — Consensus backend note and feature flag (DONE — see above)

### G.14 — Serde feature flag (DONE — deferred as planned; placeholder in Cargo.toml)

### G.15 — Documentation and public API polish (DONE — see above)

---

## Performance work {#perf}

Benchmarked baselines (2026-07-01, MemBackend, u64→u64, PostcardCodec):

| Collection | insert 10K | get n=10K | clone |
|------------|--------:|-------:|------:|
| HamtMap | 85.7 ms | 722 ns | 40 ns |
| FolioOrdMap | 123.6 ms | 1.04 µs | 40 ns |
| FolioVector | 66.9 ms | 745 ns | 40 ns |

Each item below is a PoC-gated exploration: benchmark the specific hypothesis,
record the result, and implement only if the measured gain exceeds 5%.

---

### PERF-1 — Page read cache in NodeStore (High impact)

**[DONE 2026-07-01] KEPT — 78–88% improvement on hamt_get across all sizes.**

**Hypothesis:** `get` traverses O(log N) pages, each requiring folio decode (checksum
verify + postcard deserialise). Adding a small LRU/fixed-size cache of decoded
internal nodes in `NodeStore` would eliminate repeated decodes on shared tree paths
(the upper levels of the HAMT trie / B+ tree are read on every operation).

**Expected gain:** `get` at n=10 000: 722–1 040 ns → ~250–300 ns (3–4×). Insert
indirectly benefits when the descent path is cached.

**PoC:** Implemented `page_cache: HashMap<u64, HamtNodePage>` + `cache_order: VecDeque<u64>`
(FIFO, 128-entry cap) in `NodeStore`.  Cache populated on `alloc_node` and `read_node`;
invalidated on `free_node`/`free_nodes`.  `read_node` signature changed from `&self`
to `&mut self`.

**Results:** n=10: 381→58 ns (−85%), n=100: 752→89 ns (−88%), n=1000: 863→167 ns
(−81%), n=10000: 873→191 ns (−78%).  All ≥15%.  `test.sh` green.

See `docs/baselines.md` for full numbers.

---

### PERF-2 — PodCodec for numeric key/value types (High impact)

**[DONE 2026-07-01] DEFERRED — Codec trait redesign required; no measurable gain possible with current trait design.**

**Hypothesis:** `PostcardCodec` calls `postcard::to_allocvec` (heap allocation) on
every key/value encode and `postcard::from_bytes` on every decode. For `Pod` types
(`u64`, `f32`, `i32`, etc.) the correct encoding is a direct cast of the raw bytes —
no heap allocation, no field framing.

**Finding:** The `Codec` trait's generic `encode<T: Serialize>` / `decode<T: Deserialize>`
signature prevents zero-copy dispatch without stable Rust specialization.  The existing
`PodCodec` trait impl falls back to postcard for all T.  Benchmarks confirmed that
`PodCodec` and `PostcardCodec` produce identical timings at all sizes (within noise).

After PERF-1, the `get` path at n=10 000 is ~108 ns.  The postcard decode of a `u64`
is a small fraction of the Mutex lock (~4.5 ns) and HashMap cache lookup.  A true
zero-copy codec would require a breaking `Codec` trait redesign (v2.0.0, Phase 5).

See `docs/baselines.md` for benchmark numbers.  Added `pod_codec/get` and
`pod_codec/insert` benchmarks to `benches/bench.rs` for future regression tracking.

---

### PERF-3 — Single-threaded NodeStore path (Medium impact)

**[DONE 2026-07-01] NOT IMPLEMENTED — Mutex overhead measured at 2.4%; below 10% threshold.**

**Hypothesis:** `Arc<Mutex<NodeStore<B>>>` acquires a kernel-level mutex on every
node read and write, even for single-threaded callers (the overwhelming common case).
At ~30–50 ns per lock/unlock, this costs 30–50 ns × O(log N) = 400–650 ns per
`HamtMap::get` at n=10 000, which represents ~60% of the current 720 ns get cost.

**Finding:** `get` acquires the Mutex once per call (held for the entire descent, not
once per page).  Uncontended `std::sync::Mutex` lock/unlock on M5 Max: **4.5 ns**.
After PERF-1, `hamt_get` at n=10 000 is ~191 ns.  Mutex overhead: 4.5 / 191 = 2.4%.

The original hypothesis was based on pre-PERF-1 costs where the folio store reads
dominated.  With the page cache, the mutex overhead is negligible.

See `docs/baselines.md` for measurement methodology.

---

### PERF-4 — Write batching (Medium impact)

**[DONE 2026-07-01] NOT APPLICABLE to MemBackend — deferred to disk-backed benchmarking.**

**Hypothesis:** Each HAMT insert currently writes O(log N) pages to the WAL
independently (one WAL flush per page). Grouping all pages modified in a single
insert into one WAL commit (atomic write) would both reduce WAL flush overhead and
improve write amplification.

**Finding:** `MemBackend` has no WAL.  The folio WAL is gated by `feature = "wal"`
(not enabled in pds-folio).  `alloc_node` and `write_page_data` are pure HashMap
insertions on `MemBackend`.  Batching has zero measurable effect here.

The hypothesis holds for disk-backed stores.  When a disk-backed benchmark is added,
write batching is the primary candidate for insert-heavy workload improvement.

See `docs/baselines.md` for analysis.

---

## Dependency map

```
G.0 (scaffold) → G.1 (nodes) → G.2 (HamtMap) → G.3 (refcount) → G.4 (HamtSet)
                                                                 ↓
                           merkle-spine Stage 1 ───────────────→ G.5 (HamtIndex)
                                                                 ↓
G.6 (traits HashMap/HashSet) ←─────────────────────────────── G.4 + F.0
G.7 (proptest HashMap/HashSet) ←──────────────────────────── G.6

G.8 (vector nodes) → G.9 (Vector + PersistentVector)
G.10 (btree nodes) → G.11 (OrdMap/OrdSet + traits) → G.12 (integration)

G.5 + G.6 + G.9 + G.11 → G.13/G.14/G.15 (polish)
```
