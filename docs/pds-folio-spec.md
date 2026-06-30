# pds-folio — Spec

<!-- Folio-backed persistent HAMT with structural sharing -->
<!-- Depends on: folio (S37, S64, S66), pds (cross-variant traits) -->
<!-- Unlocks: merkle-spine MS-F0 (HamtIndex: PageIndexBackend) -->

---

## Contents

- [Overview](#overview)
- [Position in the ecosystem](#position-in-the-ecosystem)
- [What pds-folio is NOT](#what-pds-folio-is-not)
- [Dependencies and unlocks](#dependencies-and-unlocks)
- [Core design](#core-design)
  - [Node types](#node-types)
  - [Slab storage](#slab-storage)
  - [Structural sharing via path-copy](#structural-sharing-via-path-copy)
  - [Reference counting](#reference-counting)
  - [K and V constraints](#k-and-v-constraints)
- [Public API](#public-api)
  - [HamtMap](#hamtmapk-v-b)
  - [HamtSet](#hamtseta-b)
  - [HamtIndex](#hamtindexb)
- [Standard trait coverage](#standard-trait-coverage)
- [Integration with pds common traits](#integration-with-pds-common-traits)
- [Integration with merkle-spine](#integration-with-merkle-spine)
- [Implementation plan cross-reference](#implementation-plan-cross-reference)

---

## Overview

`pds-folio` is a new Rust library crate that provides **folio-backed persistent
hash maps and sets with structural sharing**.

It offers the same logical contract as `pds::HashMap` — functional updates (insert,
remove) return a new version of the map; the original is unchanged; both versions
share all unmodified nodes — but the nodes live in folio's mmap'd pages rather
than heap-allocated `Arc`-backed objects.

This gives pds-folio the following properties that in-memory pds lacks:

| Property | pds (in-memory) | pds-folio |
|----------|----------------|-----------|
| Durability across process restarts | No | Yes (folio WAL) |
| Crash safety | No | Yes (folio CoW + WAL) |
| Memory-mapped access | No | Yes |
| Node count visible to OS (swappable) | No | Yes |
| Structural sharing | Yes | Yes (path-copy + refcount) |
| O(log N) point operations | Yes | Yes |
| O(1) clone | Yes | Yes (refcount increment) |

---

## Position in the ecosystem

```
folio                   (physical pages, crash safety, MVCC reader slots)
    │
    └── pds-folio       (this crate: persistent HAMT, structural sharing,
                         path-copy, refcount deallocation, HamtIndex)
              │
              └── merkle-spine (MS-F0: uses HamtIndex as PageIndexBackend)
                        │
                        └── pds-merkle-spine  (VersionedHamt facade)
```

`pds-folio` sits at the **intersection of durability and persistence**:
- Durability (crash-safe pages) comes from folio
- Persistence (structural sharing, functional updates) comes from the HAMT design
- Versioning and cryptographic identity come from merkle-spine (a later layer)

---

## What pds-folio is NOT

**Not a replacement for folio-collections.** folio-collections provides transient
mutable structures (FolioVec, FolioHashMap, FolioBTree, etc.) that store data in
folio pages and support bulk mutation in place. These are the right choice when you
need a mutable, in-place-updatable structure backed by folio.

pds-folio provides **persistent/immutable** structures: no in-place mutation, path-copy
on every update, structural sharing between versions. Use pds-folio when you need to
keep multiple concurrent versions of a map alive and modify one without affecting the
others — or when you need to pass the map to HamtIndex for merkle-spine versioning.

---

## Dependencies and unlocks

**Requires from folio:**

| Folio stage | What it provides | Why pds-folio needs it |
|-------------|-----------------|----------------------|
| S37 (FG-01) | `write_page_in_place` / zero-copy write path | Efficient in-place node initialisation during path-copy |
| S64 (FG-02) | `FolioSlab<T>` sub-page slab allocator | Fixed-size HAMT node packing — O(1) alloc/free per node |
| S66 (FG-04) | `free_pages(&[u64])` batch free | Efficient bulk deallocation when refcount drops to 0 |

**Requires from pds:**

| pds item | What it provides | Why pds-folio needs it |
|---------|-----------------|----------------------|
| Phase F.0 cross-variant traits | `PersistentMap<K, V>`, `PersistentSet<A>` | pds-folio implements these traits |
| HAMT internals (reference only) | 3-tier node design, bitmap operations | Basis for pds-folio HAMT design |

**Unlocks:**

| Unlocked stage | Location | What becomes possible |
|---------------|---------|----------------------|
| MS-F0 | merkle-spine | HamtIndex: PageIndexBackend; Merkle proofs, structural diff, sparse sync |
| Phase H | pds-merkle-spine | VersionedHamt facade over pds-folio + MS-F0 |

**Build order:**

```
folio S64 ships
     │
     └── pds-folio G.0–G.4 (HAMT CRUD + HamtIndex)
               │
               └── merkle-spine MS-F0 (HamtIndex integration)
                         │
                         └── pds-merkle-spine H.0–H.6 (VersionedHamt)
```

**Note:** merkle-spine v1 (DeltaLogIndex) should ship before pds-folio is built,
because it validates the merkle-spine API surface that HamtIndex must satisfy.
See merkle-spine `docs/impl-plan.md` § MS-F0.

---

## Core design

### Node types

The pds-folio HAMT uses two node types, each sized to fit within a single slab slot:

**`LeafNode`** — stores up to `LEAF_CAP` key-value pairs at the same hash prefix.
Variable-length entries with a fixed-size header for O(1) slot lookup.

```
┌────────────────────────────────────────────────────┐
│ discriminant: u8 = LEAF                             │
│ count: u8                                           │
│ key_hashes: [u64; count]   (pre-computed at insert) │
│ entry_offsets: [u16; count] (byte offset into data) │
│ data: [u8; ...]             (encoded K||V pairs)    │
└────────────────────────────────────────────────────┘
```

`key_hashes` are full 64-bit hashes of each key, stored upfront for O(1) HAMT
routing and fast collision detection without deserialising keys. On hash match,
the key at `entry_offsets[i]` is deserialised to confirm equality (rare: only
on collision).

`entry_offsets[i]` points to the start of the i-th entry's encoded bytes within
`data`. Each entry is codec-encoded as `encode(key) || encode(value)`. The length
of each entry is `entry_offsets[i+1] - entry_offsets[i]` (last entry ends at
the total data length, stored implicitly as `slot_size - header_size`).

`LEAF_CAP` (max entries per leaf) is chosen so the worst-case leaf fits in a slab
slot (512 bytes target). For `PodCodec` with `K = u64, V = u64`: each entry is 16
bytes, giving `LEAF_CAP ≈ 20`. For variable-length types, the leaf splits when
adding an entry would overflow the slot.

**`InternalNode`** — bitmap-indexed array of child `SlabPageId` values.

```
┌──────────────────────────────────────────────────┐
│ discriminant: u8 = INTERNAL                       │
│ bitmap: u64     (one bit per hash-trie slot)      │
│ children: [SlabPageId; popcount(bitmap)]          │
│   (SlabPageId = u64 — slab slot index)           │
└──────────────────────────────────────────────────┘
```

Unlike in-memory pds's 3-tier HAMT (SmallSimdNode / LargeSimdNode / HamtNode),
pds-folio uses only these two types. The SmallSimdNode SIMD optimisation requires
in-register data; mmap'd page access has different access patterns and the
optimisation does not transfer.

### Slab storage

All nodes are stored in a `FolioSlab<HamtNode>` (folio S64). The slab:

- Uses a single backing `FolioStore` (or a dedicated `FolioRegion` for the HAMT)
- Provides O(1) slot allocation and deallocation
- Packs multiple node slots per page to reduce page fragmentation
- The free-slot bitmap is stored in the slab's metadata page

A `HamtMap` root is identified by a `SlabPageId` (u64 slab slot index). The empty
map uses `root: None`.

### Structural sharing via path-copy

Insert and remove work by path-copy, identical in concept to in-memory pds:

**Insert(K, V) into map with root R:**

1. Traverse the HAMT from R, following hash bits, to find the insertion point.
2. Allocate a new leaf or internal node for the modification.
3. Copy the path from the insertion point back to the root, creating new internal
   nodes that reference the new child. Only O(log N) nodes are allocated.
4. The new root `R'` references the new path; unchanged subtrees are shared.
5. Return a new `HamtMap` with root `R'`. The old map with root `R` is unchanged.

Shared subtrees have their refcounts incremented (step 4 implicitly increases
the refcount when a new parent references them).

### Reference counting

pds-folio tracks structural sharing via a refcount table:

```rust
// Stored in folio alongside the HAMT slab
type RefCountTable = FolioBTree<SlabPageId, u32>;
```

**Rules:**
- Every node starts with refcount 1 when first allocated.
- When a path-copy creates a new parent that references an existing child, the
  child's refcount is incremented.
- When a `HamtMap` is cloned (O(1)): increment the root's refcount.
- When a `HamtMap` is dropped: decrement the root's refcount. If it reaches 0:
  recursively decrement children, then free the root via `FolioSlab::free`.
  If a child's refcount also reaches 0, recurse; stop at any node still referenced.
- Batch free (folio S66): collect all page IDs whose refcount just hit 0, then call
  `free_pages` once to minimise WAL writes.

The refcount table is a `FolioBTree<u64, u32>` (page_id → refcount). Absent entries
have refcount 1 implicitly; only refcounts > 1 need explicit storage (saves space
for unshared nodes, which is the common case after mutations settle).

**Optimisation:** for frequently-mutated maps, most nodes are unshared (refcount 1).
Store only refcounts > 1 explicitly; treat "absent from table" as refcount 1. On free:
check the table — if absent, free immediately; if present (refcount > 1), decrement.

### Codec and key/value types

pds-folio uses a **`Codec` type parameter** to abstract over how keys and values are
encoded into leaf page bytes. This makes `HamtMap` general: it accepts any
`K: Serialize + Hash + Eq` and `V: Serialize + DeserializeOwned` — not just Pod types.

```rust
/// Encodes keys and values into leaf page byte regions and decodes them back.
pub trait Codec: 'static {
    fn encode<T: Serialize>(value: &T, buf: &mut Vec<u8>) -> Result<(), CodecError>;
    fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, CodecError>;
}

/// Zero-copy codec for types that are `bytemuck::Pod` (fixed-size, no padding).
/// Encodes as raw bytes; no serialisation overhead. Use for `u64`, `[u8; 32]`, etc.
pub struct PodCodec;

/// Compact variable-length codec using `postcard`. Supports `no_std` and all
/// `#[derive(Serialize, Deserialize)]` types: strings, enums, structs, vecs.
pub struct PostcardCodec;

/// Zero-copy deserialisation codec using `rkyv`. Encoded form is the archived
/// representation; `decode` returns a value constructed from the mmap'd bytes
/// without heap allocation (when `V: Archive + Deserialize<Archived = V>`).
pub struct RkyvCodec;
```

**Key type support:**

| K type | Works with | Notes |
|--------|-----------|-------|
| `u32`, `u64`, `i64`, `[u8; 32]` | `PodCodec`, `PostcardCodec`, `RkyvCodec` | Preferred: `PodCodec` for zero overhead |
| `String` | `PostcardCodec`, `RkyvCodec` | Hash pre-computed at insert; stored as length-prefixed UTF-8 |
| `#[derive(Serialize, Deserialize, Hash, Eq)]` enums | `PostcardCodec`, `RkyvCodec` | Covers virtually all enums regardless of discriminant layout |
| Serialisable structs | `PostcardCodec`, `RkyvCodec` | Any `Serialize + Hash + Eq` struct works |

**`HamtIndex`** (G.5) uses `HamtMap<u64, [u8; 32], PodCodec>` — both key and value
are Pod, so `PodCodec` is always correct for the Merkle index regardless of what
codec the outer `HamtMap` uses.

**Leaf layout:** because keys and values are variable-length with `PostcardCodec`/
`RkyvCodec`, the leaf node uses an offset-table layout (see Node types below) rather
than a fixed-size array. `PodCodec` types could use a fixed-size layout, but the
offset-table layout is used uniformly to keep node-handling code simple.

---

## Public API

### `HamtMap<K, V, B>`

```rust
/// A folio-backed persistent hash map with structural sharing.
///
/// `K` and `V` are encoded into leaf pages via the `C: Codec` type parameter.
/// Use `PodCodec` for zero-copy fixed-size types (`u64`, `[u8; 32]`, etc.),
/// `PostcardCodec` for general serialisable types (strings, enums, structs),
/// or `RkyvCodec` for zero-copy deserialisation from mmap'd pages.
///
/// `B: FolioBackend` defaults to `folio::DefaultBackend` (CoW + WAL).
///
/// Clone is O(1): increments the root node's reference count.
/// Drop is O(path-to-leaf × changed_nodes): decrements refcounts recursively.
pub struct HamtMap<K, V, C = PostcardCodec, B = DefaultBackend>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    slab: FolioSlab<HamtNodePage, B>,
    refcounts: FolioBTree<SlabPageId, u32, B>,
    root: Option<SlabPageId>,
    len: usize,
    _phantom: PhantomData<(K, V, C)>,
}

impl<K, V, C, B> HamtMap<K, V, C, B>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    /// Creates a new empty map backed by `store`.
    pub fn new(store: FolioStore<B>) -> Self;

    /// Returns the decoded value for `key`, or `None` if absent.
    ///
    /// Deserialises from the mmap'd leaf page. O(log N).
    pub fn get(&self, key: &K) -> Option<V>;

    /// Returns a new map with `key` → `value` inserted.
    ///
    /// Time: O(log N). Allocates O(log N) new slab slots.
    pub fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new map with `key` removed, plus the evicted value.
    ///
    /// Time: O(log N). Allocates O(log N) new slab slots.
    pub fn remove(&self, key: &K) -> (Self, Option<V>);

    /// Returns the number of key-value pairs.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize;

    /// Tests whether the map is empty.
    pub fn is_empty(&self) -> bool;

    /// Tests whether `key` is present.
    ///
    /// Time: O(log N).
    pub fn contains_key(&self, key: &K) -> bool;

    /// Returns an iterator over all `(K, V)` pairs in arbitrary order.
    ///
    /// Time: O(N) total. Each element access reads one or more slab pages.
    pub fn iter(&self) -> HamtMapIter<'_, K, V, C, B>;
}
```

**`Clone` implementation:**

```rust
impl<K, V, C, B> Clone for HamtMap<K, V, C, B>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    fn clone(&self) -> Self {
        if let Some(root) = self.root {
            increment_refcount(&self.refcounts, root);
        }
        Self {
            slab: self.slab.clone(),       // Arc-like handle clone — O(1)
            refcounts: self.refcounts.clone(),
            root: self.root,
            len: self.len,
            _phantom: PhantomData,
        }
    }
}
```

**`Drop` implementation:**

```rust
impl<K, V, C, B> Drop for HamtMap<K, V, C, B>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    fn drop(&mut self) {
        if let Some(root) = self.root {
            let freed = collect_freed_nodes(&self.slab, &mut self.refcounts, root);
            if !freed.is_empty() {
                self.slab.free_pages(&freed); // S66 batch free
            }
        }
    }
}
```

### `HamtSet<A, B>`

Thin wrapper over `HamtMap<A, (), B>`.

```rust
/// A folio-backed persistent hash set with structural sharing.
pub struct HamtSet<A, C = PostcardCodec, B = DefaultBackend>(HamtMap<A, (), C, B>)
where
    A: Serialize + Hash + Eq + Clone,
    C: Codec,
    B: FolioBackend;

impl<A, C, B> HamtSet<A, C, B>
where
    A: Serialize + Hash + Eq + Clone,
    C: Codec,
    B: FolioBackend,
{
    pub fn new(store: FolioStore<B>) -> Self;
    pub fn contains(&self, value: &A) -> bool;
    pub fn insert(&self, value: A) -> Self;
    pub fn remove(&self, value: &A) -> (Self, bool);  // (new_set, was_present)
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = A> + '_;
    pub fn union(&self, other: &Self) -> Self;
    pub fn intersection(&self, other: &Self) -> Self;
    pub fn difference(&self, other: &Self) -> Self;
    pub fn symmetric_difference(&self, other: &Self) -> Self;
}
```

### `HamtIndex<B>`

The merkle-spine integration type. Implements `PageIndexBackend` as defined by
merkle-spine (MS-F0), enabling O(log N) historical lookup, Merkle proofs,
and structural diff.

```rust
/// A folio-backed HAMT that serves as the page index backend for merkle-spine.
///
/// Maps `page_id: u64` → `content_hash: [u8; 32]`.
///
/// Satisfies `merkle_spine::PageIndexBackend`. Provides structural sharing
/// across versions: when merkle-spine creates a new version by recording
/// changed pages, only the changed paths in the HAMT are duplicated.
///
/// The Merkle root hash of the HAMT *is* the version's root hash: the hash
/// of the root HAMT node (over its children's hashes, recursively) commits to
/// the entire set of `(page_id, content_hash)` pairs. Any external party
/// holding the root hash can verify individual entries via `MerkleProof`.
pub struct HamtIndex<B = DefaultBackend>(HamtMap<u64, [u8; 32], B>)
where
    B: FolioBackend;

impl<B: FolioBackend> HamtIndex<B> {
    pub fn new(store: FolioStore<B>) -> Self;

    /// Returns the BLAKE3 content hash recorded for `page_id`, or `None`.
    ///
    /// Time: O(log N).
    pub fn content_hash(&self, page_id: u64) -> Option<[u8; 32]>;

    /// Records `hash` as the content hash for `page_id`.
    ///
    /// Returns a new `HamtIndex` (path-copy). The original is unchanged.
    ///
    /// Time: O(log N). Allocates O(log N) new slab slots.
    pub fn set_content_hash(&self, page_id: u64, hash: [u8; 32]) -> Self;

    /// Removes the content hash for `page_id`.
    ///
    /// Returns a new `HamtIndex`. Time: O(log N).
    pub fn remove_content_hash(&self, page_id: u64) -> Self;

    /// Returns the Merkle root hash of the entire index.
    ///
    /// This is the hash of the root HAMT node, computed as:
    ///   BLAKE3(node_type || child_hash_0 || … || child_hash_k)
    ///
    /// A change to any leaf propagates up through parent hashes to the root.
    ///
    /// Time: O(1) — cached at the root node; recomputed only when the root changes.
    pub fn root_hash(&self) -> [u8; 32];

    /// Generates an inclusion proof for `page_id`.
    ///
    /// Proof size: O(log N × node_children × 32 bytes).
    /// For 1M pages: depth ≈ 3 → proof ≈ 3 × 256 × 32 = 24 KiB (worst case).
    ///
    /// Time: O(log N).
    pub fn prove_inclusion(&self, page_id: u64) -> Option<MerkleProof>;
}

/// A Merkle inclusion proof for a single page_id → content_hash entry.
///
/// Verifiable without access to any folio pages — only the root hash is needed.
pub struct MerkleProof {
    /// The `page_id` this proof covers.
    pub page_id: u64,
    /// The `content_hash` value asserted for `page_id`.
    pub content_hash: [u8; 32],
    /// Sibling hashes on the path from root to leaf, root-to-leaf order.
    pub path: Vec<SiblingHashes>,
}

pub struct SiblingHashes {
    pub level: u8,
    pub slot: u8,
    pub hashes: Vec<[u8; 32]>,   // one per occupied sibling slot
}

impl MerkleProof {
    /// Verifies this proof against `root_hash`.
    ///
    /// Returns `true` if the proof demonstrates that `page_id` maps to
    /// `content_hash` in the index whose root hash is `root_hash`.
    ///
    /// Pure function — no folio access required.
    pub fn verify(&self, root_hash: &[u8; 32]) -> bool;
}
```

**merkle-spine integration:** `HamtIndex<B>` implements `merkle_spine::PageIndexBackend`:

```rust
impl<B: FolioBackend> merkle_spine::PageIndexBackend for HamtIndex<B> {
    fn content_hash(&self, page_id: u64) -> Option<[u8; 32]> { /* ... */ }
    fn set_content_hash(&mut self, page_id: u64, hash: [u8; 32]) { /* path-copy */ }
    fn remove_content_hash(&mut self, page_id: u64) { /* path-copy */ }
    fn root_hash(&self) -> [u8; 32] { /* cached HAMT root hash */ }
}
```

Note: `merkle_spine::PageIndexBackend` takes `&mut self` for mutations (it is
designed for in-place update backends). `HamtIndex` adapts by storing the
current root internally and updating it on each call. External references
to historical roots must be retained explicitly.

---

## Standard trait coverage

| Trait | `HamtMap` | `HamtSet` | `HamtIndex` |
|-------|-----------|-----------|-------------|
| `Clone` | Y (O(1) refcount) | Y | Y |
| `Debug` | Y | Y | Y |
| `PartialEq` / `Eq` | Y (full traversal or hash-compare) | Y | Y |
| `Default` | Y (empty map) | Y | Y |
| `Send` / `Sync` | auto (if B: Send+Sync) | auto | auto |
| `FromIterator` | Y | Y | — |
| `IntoIterator` (`&`) | Y | Y | — |
| `Extend` | Y | Y | — |
| `Hash` | Y (XOR-combine over (K,V) hashes) | Y | — |
| `Serialize` / `Deserialize` | Y (behind `serde` feature) | Y | — |
| `From<pds::HashMap>` | Y (one-time migration) | Y | — |
| `PersistentMap<K, V>` | Y | — | — |
| `PersistentSet<A>` | — | Y | — |
| `PageIndexBackend` (merkle-spine) | — | — | Y |

`PartialOrd` / `Ord`: not applicable — hash maps have no deterministic order.

---

## Integration with pds common traits

```rust
impl<K, V, C, B> PersistentMap<K, V> for HamtMap<K, V, C, B>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    fn get_cloned(&self, key: &K) -> Option<V> { self.get(key) }
    fn insert(&self, key: K, value: V) -> Self { HamtMap::insert(self, key, value) }
    fn remove(&self, key: &K) -> (Self, Option<V>) { HamtMap::remove(self, key) }
    fn len(&self) -> usize { self.len }
    fn contains_key(&self, key: &K) -> bool { self.contains_key(key) }
}
```

The `V: Clone` bound required by `PersistentMap` is satisfied by the `V: Clone`
bound on `HamtMap` itself. `get_cloned` returns the value produced by
deserialisation — an owned allocation — so no copy of a stored value is needed.
`Clone` on `V` is needed only for the trait's own API surface (callers that
clone a returned value).

---

## Integration with merkle-spine

The integration is the primary reason pds-folio exists (beyond its value as a
standalone persistent store). Full details in `merkle-spine/docs/impl-plan.md § MS-F0`.

**What the integration enables:**

| Capability | How it works |
|-----------|-------------|
| Merkle proofs | `HamtIndex::prove_inclusion(page_id)` returns a path of sibling hashes |
| Structural diff | Walk two `HamtIndex` roots in parallel; skip subtrees where root hashes match |
| O(log N) historical lookup | Old HAMT roots stay alive (refcounted); traverse from historical root |
| Sparse sync | Send only the HAMT nodes that differ between two versions |

**Dense structures (tensors, blobs):**

For content like ML tensors or binary blobs where the entire value changes on
every mutation (not structurally shared), pds-folio is **not** the right layer —
path-copy over a blob-sized structure is O(N) per mutation.

For these, use folio directly (or folio-collections) and register page hashes
with merkle-spine's DeltaLogIndex (v1) or a non-HAMT PageIndexBackend.
The HAMT's structural sharing is only beneficial when the data is structured
(dictionary/map) and typically changes a small fraction of its entries per mutation.

---

## Implementation plan cross-reference

Phases in `pds/docs/impl-plan.md`:

| Phase | Description |
|-------|-------------|
| F.0 | Define cross-variant traits in `pds/src/traits.rs`; impl for in-memory types |
| G.0 | Create `pds-folio` crate via `mkrust pds-folio`; add deps |
| G.1 | Core node types: `LeafNode<K, V>`, `InternalNode`, `FolioSlab<HamtNode>` |
| G.2 | `HamtMap` CRUD: get, insert (path-copy), remove (path-copy), iter |
| G.3 | Reference counting: `FolioBTree<SlabPageId, u32>` + Drop impl |
| G.4 | `HamtSet` wrapper |
| G.5 | `HamtIndex`: HAMT node hashing + `PageIndexBackend` impl |
| G.6 | Implement `PersistentMap` / `PersistentSet` traits |
| G.7 | Integration tests; proptest suite; miri run on unsafe paths |

Phases G.0–G.5 are blocked until `folio S64` ships.
Phase G.5 is blocked until `merkle-spine MS-F0 interface` is defined (but not implemented).
Phases G.0–G.4 can proceed without merkle-spine; G.5 needs the `PageIndexBackend` trait.
