# pds-folio — Spec

<!-- Folio-backed persistent data structures with structural sharing -->
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
  - [Codec and key/value types](#codec-and-keyvalue-types)
- [Public API](#public-api)
  - [HashMap](#hashmapk-v-c-b)
  - [HashSet](#hashseta-c-b)
  - [Vector](#vectora-c-b)
  - [OrdMap](#ordmapk-v-c-b)
  - [OrdSet](#ordseta-c-b)
  - [HamtIndex](#hamtindexb)
- [Standard trait coverage](#standard-trait-coverage)
- [Integration with pds common traits](#integration-with-pds-common-traits)
- [Integration with merkle-spine](#integration-with-merkle-spine)
- [Implementation plan cross-reference](#implementation-plan-cross-reference)

---

## Overview

`pds-folio` is a new Rust library crate that provides **folio-backed persistent
data structures with structural sharing**: `HashMap`, `HashSet`, `Vector`,
`OrdMap`, and `OrdSet`.

All five types share the same logical contract as their in-memory `pds`
counterparts — functional updates (insert, remove, push) return a new version;
the original is unchanged; both versions share all unmodified nodes — but the
nodes live in folio's mmap'd pages rather than heap-allocated `Arc`-backed objects.

| Property | pds (in-memory) | pds-folio |
|----------|----------------|-----------|
| Durability across process restarts | No | Yes (folio WAL) |
| Crash safety | No | Yes (folio CoW + WAL) |
| Memory-mapped access | No | Yes |
| Datasets larger than RAM | No | Yes |
| Structural sharing | Yes | Yes (path-copy + refcount) |
| O(log N) point operations | Yes | Yes |
| O(1) clone | Yes | Yes (refcount increment) |

---

## Position in the ecosystem

```
folio                   (physical pages, crash safety, MVCC reader slots)
    │
    └── pds-folio       (this crate: HashMap, HashSet, Vector, OrdMap, OrdSet,
                         HamtIndex — all with structural sharing)
              │
              └── merkle-spine (MS-F0: uses HamtIndex as PageIndexBackend)
                        │
                        └── pds-merkle-spine  (VersionedHamt facade)
```

---

## What pds-folio is NOT

**Not a replacement for folio-collections.** folio-collections provides transient
mutable structures (FolioVec, FolioHashMap, FolioBTree, etc.) that support bulk
mutation in place. These are the right choice when you need a mutable, in-place-
updatable structure backed by folio.

pds-folio provides **persistent/immutable** structures: no in-place mutation,
path-copy on every update, structural sharing between versions. Use pds-folio when
you need to keep multiple concurrent versions of a collection alive, or when you
need to hand the map to `HamtIndex` for merkle-spine versioning.

---

## Dependencies and unlocks

**Requires from folio:**

| Folio stage | What it provides | Why pds-folio needs it |
|-------------|-----------------|----------------------|
| S37 (FG-01) | `write_page_in_place` | Efficient in-place node initialisation during path-copy |
| S64 (FG-02) | `FolioSlab<T>` sub-page slab allocator | Fixed-size node packing — O(1) alloc/free per node |
| S66 (FG-04) | `free_pages(&[u64])` batch free | Efficient bulk deallocation when refcount drops to 0 |

**Requires from pds:**

| pds item | What it provides |
|---------|-----------------|
| Phase F.0 | `PersistentMap<K, V>`, `PersistentSet<A>`, `PersistentVector<A>`, `PersistentOrdMap<K, V>`, `PersistentOrdSet<A>` |

**Unlocks:**

| Unlocked stage | What becomes possible |
|---------------|----------------------|
| merkle-spine MS-F0 | `HamtIndex: PageIndexBackend`; Merkle proofs, structural diff, sparse sync |
| pds-merkle-spine Phase H | `VersionedHamt` facade over pds-folio + MS-F0 |

---

## Core design

### Node types

Each collection uses two node types stored in folio slab pages.

**`HashMap` / `HashSet` — HAMT nodes:**

`LeafNode` — variable-length; stores key-value pairs at the same hash prefix:

```
┌────────────────────────────────────────────────────┐
│ discriminant: u8 = LEAF                             │
│ count: u8                                           │
│ key_hashes: [u64; count]   (pre-computed at insert) │
│ entry_offsets: [u16; count] (byte offset into data) │
│ data: [u8; ...]             (encoded K||V pairs)    │
└────────────────────────────────────────────────────┘
```

`key_hashes` enables O(1) HAMT routing and fast collision detection without
deserialising keys. On hash match, the key is deserialised only to confirm
equality (rare: only on hash collision).

`InternalNode` — bitmap-indexed array of child `SlabPageId` values:

```
┌──────────────────────────────────────────────────┐
│ discriminant: u8 = INTERNAL                       │
│ bitmap: u64     (one bit per hash-trie slot)      │
│ children: [SlabPageId; popcount(bitmap)]          │
└──────────────────────────────────────────────────┘
```

**`Vector` — RRB-tree nodes:**

`VectorLeaf` — up to `BRANCHING_FACTOR` encoded values:

```
┌──────────────────────────────────────────────────┐
│ discriminant: u8 = LEAF                           │
│ count: u8                                         │
│ entry_offsets: [u16; count]                       │
│ data: [u8; ...]              (encoded A values)   │
└──────────────────────────────────────────────────┘
```

`VectorInternal` — up to `BRANCHING_FACTOR` children with sizes for relaxed
radix balancing:

```
┌──────────────────────────────────────────────────┐
│ discriminant: u8 = INTERNAL                       │
│ count: u8                                         │
│ sizes: [u32; count]  (cumulative subtree sizes)   │
│ children: [SlabPageId; count]                     │
└──────────────────────────────────────────────────┘
```

`BRANCHING_FACTOR = 32` (target: 512-byte slab slot). Gives depth ≈ 3 for 32,000
elements; ≈ 4 for 1M elements. Index operations are effectively O(1) in practice.

**`OrdMap` / `OrdSet` — persistent B-tree nodes:**

`BTreeLeaf` — ordered array of key-value pairs, chained via `next_leaf` for
efficient range scans:

```
┌──────────────────────────────────────────────────┐
│ discriminant: u8 = LEAF                           │
│ count: u8                                         │
│ next_leaf: Option<SlabPageId>                     │
│ entry_offsets: [u16; count]                       │
│ data: [u8; ...]   (encoded K||V pairs, sorted)   │
└──────────────────────────────────────────────────┘
```

`BTreeInternal` — separator keys with child pointers:

```
┌──────────────────────────────────────────────────┐
│ discriminant: u8 = INTERNAL                       │
│ count: u8         (number of separator keys)      │
│ children: [SlabPageId; count + 1]                 │
│ key_offsets: [u16; count]                         │
│ key_data: [u8; ...]    (encoded separator keys)   │
└──────────────────────────────────────────────────┘
```

`BTREE_ORDER` chosen so `BTreeInternal` fits in a slab slot.

### Slab storage

All nodes for all collection types are stored in a shared `FolioSlab<NodePage>`
(folio S64). The slab:

- Uses a single backing `FolioStore` (or a dedicated `FolioRegion`)
- Provides O(1) slot allocation and deallocation
- Packs multiple node slots per folio page to reduce fragmentation

A collection root is identified by a `SlabPageId` (u64). The empty collection
uses `root: None`.

### Structural sharing via path-copy

All five collection types use path-copy for mutations: only the O(log N) nodes
on the path from root to the modified leaf are duplicated. All unchanged subtrees
are shared between the old and new versions.

### Reference counting

A shared `FolioBTree<SlabPageId, u32>` refcount table tracks structural sharing
across all collection types in the same store:

- Refcount 1 is implicit (absent from table = unshared)
- `Clone` increments the root's refcount — O(1)
- `Drop` decrements refcounts recursively, frees nodes at zero via S66 batch free
- Batch free minimises WAL writes

### Codec and key/value types

pds-folio uses a **`Codec` type parameter** to abstract over how keys and values
are encoded into node page bytes. This makes all five collection types general:
they accept any serialisable K/V, not just Pod types.

```rust
/// Encodes values into node page bytes and decodes them back.
pub trait Codec: 'static {
    fn encode<T: Serialize>(value: &T, buf: &mut Vec<u8>) -> Result<(), CodecError>;
    fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, CodecError>;
}

/// Zero-copy codec for `bytemuck::Pod` types (fixed-size, no padding).
/// Encodes as raw bytes; no serialisation overhead. Use for `u64`, `[u8; 32]`, etc.
pub struct PodCodec;

/// Compact variable-length codec using `postcard`. Supports `no_std` and all
/// `#[derive(Serialize, Deserialize)]` types: strings, enums, structs, Vecs.
pub struct PostcardCodec;
```

**Key type support:**

| K type | Codec | Notes |
|--------|-------|-------|
| `u32`, `u64`, `i64`, `[u8; 32]` | `PodCodec` | Zero overhead; preferred for numeric keys |
| `String` | `PostcardCodec` | Hash pre-computed at insert; stored as length-prefixed UTF-8 |
| Enums (`#[derive(Serialize, Deserialize, Hash, Eq)]`) | `PostcardCodec` | Any enum regardless of discriminant layout |
| Serialisable structs | `PostcardCodec` | Any `Serialize + Hash + Eq` struct |

**Zero-copy large values:** if `V` is a large variable-length blob (an ML tensor,
a document, raw bytes), store a `FolioRef` (a `u64` page+offset into a folio blob
region) as the map value and access the actual content directly through folio.
This gives true zero-copy access without any codec dependency — folio already
mmap's the pages into the process address space. pds-folio does not add an rkyv
dependency; `PodCodec` and `PostcardCodec` cover all cases; the `FolioRef`
pattern handles large values.

**`HamtIndex`** (G.5) uses `HashMap<u64, [u8; 32], PodCodec>` internally — both
key and value are Pod, so zero-copy storage applies regardless of the outer map's
codec choice.

---

## Public API

### `HashMap<K, V, C, B>`

```rust
/// A folio-backed persistent hash map with structural sharing.
///
/// `K` and `V` are encoded into HAMT leaf pages via `C: Codec`.
/// Use `PodCodec` for fixed-size types, `PostcardCodec` for general types.
/// Default codec is `PostcardCodec`.
///
/// `B: FolioBackend` defaults to `folio::DefaultBackend` (CoW + WAL).
///
/// Clone is O(1). Drop is O(changed_path_length).
pub struct HashMap<K, V, C = PostcardCodec, B = DefaultBackend>
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

impl<K, V, C, B> HashMap<K, V, C, B>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    pub fn new(store: FolioStore<B>) -> Self;

    /// Returns the decoded value for `key`, or `None`. Time: O(log N).
    pub fn get(&self, key: &K) -> Option<V>;

    /// Returns a new map with `key` → `value` inserted. Time: O(log N).
    pub fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new map with `key` removed, plus the evicted value. Time: O(log N).
    pub fn remove(&self, key: &K) -> (Self, Option<V>);

    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn contains_key(&self, key: &K) -> bool;

    /// Returns an iterator over all `(K, V)` pairs in arbitrary order. Time: O(N).
    pub fn iter(&self) -> HashMapIter<'_, K, V, C, B>;
}
```

**`Clone`:** increments root refcount — O(1).
**`Drop`:** decrements refcounts recursively, batch-frees via S66.

### `HashSet<A, C, B>`

Thin wrapper over `HashMap<A, (), C, B>`.

```rust
pub struct HashSet<A, C = PostcardCodec, B = DefaultBackend>(HashMap<A, (), C, B>)
where
    A: Serialize + Hash + Eq + Clone,
    C: Codec,
    B: FolioBackend;

impl<A, C, B> HashSet<A, C, B>
where
    A: Serialize + Hash + Eq + Clone,
    C: Codec,
    B: FolioBackend,
{
    pub fn new(store: FolioStore<B>) -> Self;
    pub fn contains(&self, value: &A) -> bool;
    pub fn insert(&self, value: A) -> Self;
    pub fn remove(&self, value: &A) -> (Self, bool);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = A> + '_;
    pub fn union(&self, other: &Self) -> Self;
    pub fn intersection(&self, other: &Self) -> Self;
    pub fn difference(&self, other: &Self) -> Self;
    pub fn symmetric_difference(&self, other: &Self) -> Self;
}
```

### `Vector<A, C, B>`

Folio-backed persistent vector using a Relaxed Radix Balanced (RRB) tree.
Structural sharing on `push`, `update`, `split`, and `concat` — only the O(log N)
path nodes are duplicated.

```rust
/// A folio-backed persistent vector with structural sharing.
///
/// Based on an RRB-tree (Relaxed Radix Balanced B-tree) in folio slab pages.
/// `BRANCHING_FACTOR = 32`: depth ≈ 3 for 32K elements; ≈ 4 for 1M elements.
/// Index operations are effectively O(1) in practice.
///
/// Clone is O(1). Concat and split are O(log N).
pub struct Vector<A, C = PostcardCodec, B = DefaultBackend>
where
    A: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    slab: FolioSlab<VectorNodePage, B>,
    refcounts: FolioBTree<SlabPageId, u32, B>,
    root: Option<SlabPageId>,
    len: usize,
    depth: u8,
    _phantom: PhantomData<(A, C)>,
}

impl<A, C, B> Vector<A, C, B>
where
    A: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    pub fn new(store: FolioStore<B>) -> Self;

    /// Returns the element at `index`, or `None` if out of bounds. Time: O(log N).
    pub fn get(&self, index: usize) -> Option<A>;

    /// Returns a new vector with `value` appended. Time: O(log N) amortised.
    pub fn push_back(&self, value: A) -> Self;

    /// Returns a new vector with `value` prepended. Time: O(log N) amortised.
    pub fn push_front(&self, value: A) -> Self;

    /// Returns a new vector with the element at `index` replaced. Time: O(log N).
    pub fn update(&self, index: usize, value: A) -> Self;

    /// Returns a new vector with the last element removed, plus the element.
    /// Time: O(log N).
    pub fn pop_back(&self) -> (Self, Option<A>);

    /// Returns a new vector with the first element removed, plus the element.
    /// Time: O(log N).
    pub fn pop_front(&self) -> (Self, Option<A>);

    /// Concatenates two vectors. Time: O(log N).
    pub fn concat(&self, other: &Self) -> Self;

    /// Splits at `index`, returning `(left, right)`. Time: O(log N).
    pub fn split_at(&self, index: usize) -> (Self, Self);

    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Returns an iterator in index order. Time: O(N) total.
    pub fn iter(&self) -> VectorIter<'_, A, C, B>;
}
```

### `OrdMap<K, V, C, B>`

Folio-backed persistent ordered map using a persistent B+ tree. Maintains keys
in sorted order; supports O(log N + k) range queries.

```rust
/// A folio-backed persistent ordered map with structural sharing.
///
/// Based on a persistent B+ tree in folio slab pages.
/// Leaf nodes are chained for efficient range iteration.
/// `K: Ord` — keys are compared by deserialising; `PodCodec` eliminates this
/// cost for numeric types where byte order == value order.
///
/// Clone is O(1). Range queries are O(log N + k) where k is result count.
pub struct OrdMap<K, V, C = PostcardCodec, B = DefaultBackend>
where
    K: Serialize + DeserializeOwned + Ord + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    slab: FolioSlab<BTreeNodePage, B>,
    refcounts: FolioBTree<SlabPageId, u32, B>,
    root: Option<SlabPageId>,
    len: usize,
    _phantom: PhantomData<(K, V, C)>,
}

impl<K, V, C, B> OrdMap<K, V, C, B>
where
    K: Serialize + DeserializeOwned + Ord + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{
    pub fn new(store: FolioStore<B>) -> Self;

    /// Returns the decoded value for `key`, or `None`. Time: O(log N).
    pub fn get(&self, key: &K) -> Option<V>;

    /// Returns a new map with `key` → `value` inserted. Time: O(log N).
    pub fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new map with `key` removed, plus the evicted value. Time: O(log N).
    pub fn remove(&self, key: &K) -> (Self, Option<V>);

    /// Returns the smallest key-value pair, or `None`. Time: O(log N).
    pub fn first(&self) -> Option<(K, V)>;

    /// Returns the largest key-value pair, or `None`. Time: O(log N).
    pub fn last(&self) -> Option<(K, V)>;

    /// Returns an iterator over all pairs with keys in `bounds`, in ascending order.
    /// Time: O(log N) to seek + O(k) to iterate k results.
    pub fn range<R: RangeBounds<K>>(&self, bounds: R) -> OrdMapRange<'_, K, V, C, B>;

    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn contains_key(&self, key: &K) -> bool;

    /// Returns an iterator over all `(K, V)` pairs in ascending key order.
    /// Time: O(N) total.
    pub fn iter(&self) -> OrdMapIter<'_, K, V, C, B>;
}
```

**Note on `PodCodec` with `OrdMap`:** for numeric keys where the Pod byte
representation is naturally ordered (e.g. big-endian `u64`), key comparison during
B-tree traversal reduces to a `memcmp`. This requires the codec to store keys in
a comparable byte layout — `PodCodec` on little-endian platforms needs the caller
to use a big-endian wrapper or a byte-reversal newtype for correct ordering.
`PostcardCodec` always deserialises to compare.

### `OrdSet<A, C, B>`

Thin wrapper over `OrdMap<A, (), C, B>`.

```rust
pub struct OrdSet<A, C = PostcardCodec, B = DefaultBackend>(OrdMap<A, (), C, B>)
where
    A: Serialize + DeserializeOwned + Ord + Clone,
    C: Codec,
    B: FolioBackend;

impl<A, C, B> OrdSet<A, C, B>
where
    A: Serialize + DeserializeOwned + Ord + Clone,
    C: Codec,
    B: FolioBackend,
{
    pub fn new(store: FolioStore<B>) -> Self;
    pub fn contains(&self, value: &A) -> bool;
    pub fn insert(&self, value: A) -> Self;
    pub fn remove(&self, value: &A) -> (Self, bool);
    pub fn first(&self) -> Option<A>;
    pub fn last(&self) -> Option<A>;
    pub fn range<R: RangeBounds<A>>(&self, bounds: R) -> impl Iterator<Item = A> + '_;
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
merkle-spine (MS-F0), enabling Merkle proofs, structural diff, and sparse sync.

```rust
/// A folio-backed HAMT that serves as the page index backend for merkle-spine.
///
/// Maps `page_id: u64` → `content_hash: [u8; 32]` using `PodCodec` (both are Pod).
///
/// The Merkle root hash of the HAMT *is* the version's root hash: the hash of
/// the root HAMT node (over its children's hashes, recursively) commits to the
/// entire set of `(page_id, content_hash)` pairs.
pub struct HamtIndex<B = DefaultBackend>(HashMap<u64, [u8; 32], PodCodec, B>)
where
    B: FolioBackend;

impl<B: FolioBackend> HamtIndex<B> {
    pub fn new(store: FolioStore<B>) -> Self;

    /// Returns the BLAKE3 content hash for `page_id`, or `None`. Time: O(log N).
    pub fn content_hash(&self, page_id: u64) -> Option<[u8; 32]>;

    /// Returns a new index with `hash` recorded for `page_id`. Time: O(log N).
    pub fn set_content_hash(&self, page_id: u64, hash: [u8; 32]) -> Self;

    /// Returns a new index with `page_id` removed. Time: O(log N).
    pub fn remove_content_hash(&self, page_id: u64) -> Self;

    /// Returns the BLAKE3 Merkle root hash of the entire index.
    ///
    /// Computed as: BLAKE3(node_type || child_hash_0 || … || child_hash_k)
    /// A change to any leaf propagates up to the root.
    ///
    /// Time: O(1) — cached at the root node.
    pub fn root_hash(&self) -> [u8; 32];

    /// Generates an inclusion proof for `page_id`. Proof size: O(log N × 32).
    /// Time: O(log N).
    pub fn prove_inclusion(&self, page_id: u64) -> Option<MerkleProof>;
}

pub struct MerkleProof {
    pub page_id: u64,
    pub content_hash: [u8; 32],
    pub path: Vec<SiblingHashes>,
}

pub struct SiblingHashes {
    pub level: u8,
    pub slot: u8,
    pub hashes: Vec<[u8; 32]>,
}

impl MerkleProof {
    /// Verifies this proof against `root_hash`. Pure function — no folio access.
    pub fn verify(&self, root_hash: &[u8; 32]) -> bool;
}
```

**merkle-spine integration:**

```rust
impl<B: FolioBackend> merkle_spine::PageIndexBackend for HamtIndex<B> {
    fn content_hash(&self, page_id: u64) -> Option<[u8; 32]> { /* ... */ }
    fn set_content_hash(&mut self, page_id: u64, hash: [u8; 32]) { /* path-copy */ }
    fn remove_content_hash(&mut self, page_id: u64) { /* path-copy */ }
    fn root_hash(&self) -> [u8; 32] { /* cached HAMT root hash */ }
}
```

Note: `PageIndexBackend` takes `&mut self` for mutations. `HamtIndex` adapts by
storing the current root internally and updating it on each call. External
references to historical roots must be retained explicitly.

---

## Standard trait coverage

| Trait | `HashMap` | `HashSet` | `Vector` | `OrdMap` | `OrdSet` | `HamtIndex` |
|-------|-----------|-----------|----------|----------|----------|-------------|
| `Clone` | Y (O(1)) | Y | Y | Y | Y | Y |
| `Debug` | Y | Y | Y | Y | Y | Y |
| `PartialEq` / `Eq` | Y | Y | Y | Y | Y | Y |
| `Default` | Y | Y | Y | Y | Y | Y |
| `Send` / `Sync` | auto | auto | auto | auto | auto | auto |
| `FromIterator` | Y | Y | Y | Y | Y | — |
| `IntoIterator` (`&`) | Y | Y | Y | Y | Y | — |
| `Extend` | Y | Y | Y | Y | Y | — |
| `Hash` | Y (XOR) | Y | — | — | — | — |
| `Serialize` / `Deserialize` | Y (feature) | Y | Y | Y | Y | — |
| `PartialOrd` / `Ord` | — | — | Y | Y | Y | — |
| `Index` | — | — | Y | — | — | — |
| `From<pds::HashMap>` | Y | Y | — | — | — | — |
| `From<pds::Vector>` | — | — | Y | — | — | — |
| `From<pds::OrdMap>` | — | — | — | Y | Y | — |
| `PersistentMap<K, V>` | Y | — | — | — | — | — |
| `PersistentSet<A>` | — | Y | — | — | — | — |
| `PersistentVector<A>` | — | — | Y | — | — | — |
| `PersistentOrdMap<K, V>` | — | — | — | Y | — | — |
| `PersistentOrdSet<A>` | — | — | — | — | Y | — |
| `PageIndexBackend` | — | — | — | — | — | Y |

`PartialOrd`/`Ord` on `OrdMap`/`OrdSet`: ordered by key sequence.
`Index` on `Vector`: panics on out-of-bounds (standard Rust contract).

---

## Integration with pds common traits

```rust
// HashMap → PersistentMap
impl<K, V, C, B> PersistentMap<K, V> for HashMap<K, V, C, B>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{ /* get_cloned, insert, remove, len, contains_key */ }

// Vector → PersistentVector
impl<A, C, B> PersistentVector<A> for Vector<A, C, B>
where
    A: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{ /* get, push_back, push_front, update, pop_back, pop_front, concat, split_at, len */ }

// OrdMap → PersistentOrdMap (which extends PersistentCollection)
impl<K, V, C, B> PersistentOrdMap<K, V> for OrdMap<K, V, C, B>
where
    K: Serialize + DeserializeOwned + Ord + Clone,
    V: Serialize + DeserializeOwned + Clone,
    C: Codec,
    B: FolioBackend,
{ /* get_cloned, insert, remove, range, first, last, len, contains_key */ }
```

The trait bounds (`K: Clone + Eq + Hash + Serialize`, `V: Clone + Serialize +
DeserializeOwned`) appear on the `impl` blocks, not on the traits. Callers using
the trait abstraction only see the trait's minimal bounds.

---

## Integration with merkle-spine

Full details in `merkle-spine/docs/impl-plan.md § MS-F0`.

| Capability | How it works |
|-----------|-------------|
| Merkle proofs | `HamtIndex::prove_inclusion(page_id)` returns a path of sibling hashes |
| Structural diff | Walk two `HamtIndex` roots in parallel; skip subtrees where root hashes match |
| O(log N) historical lookup | Old HAMT roots stay alive (refcounted); traverse from historical root |
| Sparse sync | Send only the HAMT nodes that differ between two versions |

**Dense structures (tensors, blobs):** path-copy over a blob-sized structure is
O(N) per mutation — pds-folio's structures are not appropriate here. Use folio
directly and register page hashes with merkle-spine's DeltaLogIndex (v1).

---

## Implementation plan cross-reference

Phases in `pds/docs/impl-plan.md`:

| Phase | Description |
|-------|-------------|
| F.0 | Cross-variant traits in `pds/src/traits.rs` (PersistentMap, PersistentVector, PersistentOrdMap, PersistentOrdSet) |
| G.0 | Create `pds-folio` crate; Codec trait + PodCodec + PostcardCodec |
| G.1 | HAMT node types + slab layout (HashMap/HashSet) |
| G.2 | `HashMap` CRUD + iter |
| G.3 | Reference counting + Drop |
| G.4 | `HashSet` wrapper |
| G.5 | `HamtIndex`: HAMT node hashing + `PageIndexBackend` |
| G.6 | `PersistentMap` / `PersistentSet` trait impls |
| G.7 | HashMap/HashSet integration tests + proptest |
| G.8 | RRB-tree node types + slab layout (Vector) |
| G.9 | `Vector` CRUD + `PersistentVector` trait impl |
| G.10 | B+ tree node types + slab layout (OrdMap/OrdSet) |
| G.11 | `OrdMap` + `OrdSet` CRUD + `PersistentOrdMap`/`PersistentOrdSet` trait impls |
| G.12 | Vector + OrdMap integration tests + proptest |

G.0–G.5 blocked on folio S64. G.5 blocked on merkle-spine Stage 1 interface.
G.8–G.12 blocked on G.3 (shared refcount infrastructure).
