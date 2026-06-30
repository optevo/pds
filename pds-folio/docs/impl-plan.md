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

### G.2 — `HamtMap` CRUD

- `HamtMap<K, V, C = PostcardCodec, B = DefaultBackend>` with `K: Serialize + Hash + Eq + Clone, V: Serialize + DeserializeOwned + Clone, C: Codec`
- `new(store)`, `get(key) -> Option<V>`, `insert(key, value) -> Self`, `remove(key) -> (Self, Option<V>)`
- `len()`, `is_empty()`, `contains_key(key)`
- Path-copy on insert/remove: O(log N) new slab slots; leaf split when data overflows slot
- No reference counting yet (G.3)
- Tests: empty map, single insert, multiple inserts, overwrite, remove present/absent; test with both `PodCodec` (u64 keys) and `PostcardCodec` (String keys)

**Acceptance:** `cargo test` green; all operations correct on both codecs.

### G.3 — Reference counting and `Drop`

- `FolioBTree<SlabPageId, u32>` refcount table (stored in same folio store)
- `Clone` impl: increment root refcount
- `Drop` impl: decrement refcount, recursively free nodes at zero, batch via folio S66 (`free_pages`)
- Optimisation: absent from table = refcount 1 (store only refcounts > 1)
- Tests: clone + drop frees nothing while shared; all copies dropped → store empty

**Acceptance:** `cargo test` green; refcount semantics verified.

### G.4 — `HamtSet` wrapper

- Newtype `HamtSet<A, B>(HamtMap<A, (), B>)`
- Full API: `contains`, `insert`, `remove`, `union`, `intersection`, `difference`, `symmetric_difference`
- Tests: all set operations

**Acceptance:** `cargo test` green; all set operations correct.

### G.5 — `HamtIndex`: PageIndexBackend

**Blocked by:** merkle-spine Stage 1 (for the `PageIndexBackend` trait definition).

- `HamtIndex<B>(HamtMap<u64, [u8; 32], B>)`
- Node-level BLAKE3 Merkle hashing: each node hash covers its child hashes recursively
- `root_hash()`: hash of root node (O(1) cached)
- `prove_inclusion(page_id) -> Option<MerkleProof>`
- `impl merkle_spine::PageIndexBackend for HamtIndex<B>`
- Tests: root hash changes when any entry changes; proof verifies; empty index has known hash

**Acceptance:** `cargo test` green; `HamtIndex` passes all `PageIndexBackend` contract tests.

### G.6 — Implement pds cross-variant traits (HashMap / HashSet)

- `impl<K, V, C, B> PersistentMap<K, V> for HamtMap<K, V, C, B>`
- `impl<A, C, B> PersistentSet<A> for HamtSet<A, C, B>`
- Tests: generic functions from pds Phase F tests work with `HamtMap`/`HamtSet` using both codecs

**Acceptance:** `cargo test` green; trait impls correct.

### G.7 — Integration tests and proptest suite (HashMap / HashSet)

- proptest: insert N random (K, V) pairs; all lookups correct; remove N/2; remaining correct
- Integration: create `HamtMap` in folio store; process restart simulation; reopen store; lookups correct

**Acceptance:** proptest passes (256 cases default); integration round-trip green.

### G.8 — Vector: RRB-tree node types and slab layout

- `VectorLeaf` and `VectorInternal` page layouts (BRANCHING_FACTOR = 32)
- `FolioSlab<VectorNodePage>` wrapper
- Unit tests: node size checks; leaf insert/read round-trip

**Acceptance:** `cargo test` green; size assertions pass.

### G.9 — `Vector` CRUD and `PersistentVector` trait impl

- `Vector<A, C = PostcardCodec, B = DefaultBackend>` — `A: Serialize + DeserializeOwned + Clone, C: Codec`
- `new`, `get`, `push_back`, `push_front`, `update`, `pop_back`, `pop_front`, `concat`, `split_at`, `len`, `iter`
- Path-copy on all mutations; shared refcount table from G.3
- `impl<A, C, B> PersistentVector<A> for Vector<A, C, B>`
- Tests: empty, single push, multiple pushes, update, pop, concat, split; proptest round-trip

**Acceptance:** `cargo test` green; all operations correct; `PersistentVector` trait impl passes.

### G.10 — OrdMap / OrdSet: B+ tree node types and slab layout

- `BTreeLeaf` (chained via `next_leaf`) and `BTreeInternal` page layouts
- `FolioSlab<BTreeNodePage>` wrapper
- Unit tests: node size checks; leaf insert/read round-trip in sorted order

**Acceptance:** `cargo test` green; size assertions pass.

### G.11 — `OrdMap` + `OrdSet` CRUD and trait impls

- `OrdMap<K, V, C = PostcardCodec, B = DefaultBackend>` — `K: Serialize + DeserializeOwned + Ord + Clone`
- `new`, `get`, `insert`, `remove`, `first`, `last`, `range`, `len`, `contains_key`, `iter`
- B+ tree split/merge on insert/remove; path-copy; shared refcount table from G.3
- `OrdSet<A, C, B>` wrapper over `OrdMap<A, (), C, B>`
- `impl PersistentOrdMap<K, V> for OrdMap<K, V, C, B>`
- `impl PersistentOrdSet<A> for OrdSet<A, C, B>`
- Tests: empty, insert, remove, range queries, ordering invariants; proptest sorted order

**Acceptance:** `cargo test` green; sorted order invariant verified; range queries correct.

### G.12 — Integration tests (Vector + OrdMap / OrdSet)

- proptest: Vector concat/split round-trips; OrdMap range query correctness
- Integration: create OrdMap in folio store; restart simulation; range query still correct

**Acceptance:** proptest green; integration round-trip green.

### G.13 — Consensus backend note and feature flag

`pds-folio` does not implement consensus itself — the `B: FolioBackend` type
parameter allows callers to pass a consensus-aware backend. This is a note,
not a code item. Add `consensus = ["folio-consensus"]` feature flag if/when
needed.

### G.14 — Serde feature flag

Add `serde = ["dep:serde_core"]` feature gate so that pds-folio can be used
in `no_std + alloc` environments without pulling in serde when unneeded.
Defer to Phase H if not required before pds-merkle-spine.

### G.15 — Documentation and public API polish

- Module-level docs for each collection type
- `# Examples` blocks for all public methods
- `docs/decisions.md` entries for codec choice and node layout
- `docs/glossary.md` for pds-folio-specific terms
- `docs/references.md` for folio and HAMT papers

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
