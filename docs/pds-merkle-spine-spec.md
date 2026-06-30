# pds-merkle-spine — Spec

<!-- Thin VersionedHamt facade over pds-folio + merkle-spine HamtIndex -->
<!-- Depends on: pds-folio G.5 (HamtIndex), merkle-spine MS-F0 (PageIndexBackend + VersionStore) -->
<!-- Implements: pds VersionedPersistentMap + MerklePersistentMap traits -->

---

## Contents

- [Overview](#overview)
- [Position in the ecosystem](#position-in-the-ecosystem)
- [Dependencies and unlocks](#dependencies-and-unlocks)
- [Core design](#core-design)
  - [What a VersionedHamt contains](#what-a-versionedhamt-contains)
  - [Version semantics](#version-semantics)
  - [Historical lookup](#historical-lookup)
  - [Structural diff](#structural-diff)
  - [Merkle proofs](#merkle-proofs)
  - [Sparse sync](#sparse-sync)
- [Public API](#public-api)
  - [VersionedHamt](#versionedhamtk-v-b)
  - [VersionId](#versionid)
  - [MerkleProof](#merkleproof)
  - [DiffEntry](#diffentry)
- [Standard trait coverage](#standard-trait-coverage)
- [Integration with pds common traits](#integration-with-pds-common-traits)
- [Dense structures — out of scope](#dense-structures--out-of-scope)
- [Implementation plan cross-reference](#implementation-plan-cross-reference)

---

## Overview

`pds-merkle-spine` is a thin facade crate that combines two lower-level libraries:

- **`pds-folio`** — provides `HamtMap<K, V, B>`: folio-backed persistent HAMT with
  structural sharing, path-copy updates, and `HamtIndex<B>: PageIndexBackend`
- **`merkle-spine`** — provides the version DAG (`VersionStore`), BLAKE3 Merkle tree
  over pages, and the `PageIndexBackend` trait

Together they produce `VersionedHamt<K, V, B>`: a persistent, versioned,
cryptographically-identified hash map with:
- O(log N) point reads and writes (HAMT structural sharing)
- O(1) historical version checkout
- O(log N) historical point lookup without materialising the full historical map
- O(changed × log N) structural diff between any two versions
- O(log N) Merkle inclusion proofs, verifiable without folio access
- Sparse sync: transmit only differing nodes, not full snapshots

This is the "nirvana" described in `merkle-spine/docs/impl-plan.md § MS-F0`:

```
folio (crash-safe, MVCC)
  + pds-folio HAMT (structural sharing, O(log N) everything)
  + merkle-spine HamtIndex (cryptographic version identity, Merkle proofs, structural diffs)
```

---

## Position in the ecosystem

```
folio                       (physical pages, crash safety, MVCC, WAL)
    │
    └── pds-folio           (structural sharing, path-copy, refcount, HamtIndex)
              │
              └── merkle-spine  (version DAG, BLAKE3 Merkle, PageIndexBackend contract)
                        │
                        └── pds-merkle-spine  (VersionedHamt — this crate)
```

`pds-merkle-spine` is intentionally thin: it adds no new data structures of its own.
It is the integration layer that makes the two lower-level crates work together as a
coherent, ergonomic API.

---

## Dependencies and unlocks

**Requires:**

| Dependency | Stage | What it provides |
|-----------|-------|-----------------|
| `pds-folio` | G.5 | `HamtMap<K,V,B>`, `HamtIndex<B>: PageIndexBackend` |
| `merkle-spine` | MS-F0 | `PageIndexBackend` trait, `VersionStore<HamtIndex>`, `VersionId` |
| `pds` | F.0 + F.1 | Cross-variant trait definitions |

**Unlocks:** nothing further in the current roadmap. `pds-merkle-spine` is the
terminal layer for map-type versioning in the pds ecosystem.

---

## Core design

### What a VersionedHamt contains

```rust
pub struct VersionedHamt<K, V, B = DefaultBackend>
where
    K: Pod + Eq + Hash,
    V: Pod,
    B: FolioBackend,
{
    // The current version's map data.
    // Structural sharing: most nodes are shared with prior versions.
    data: HamtMap<K, V, B>,

    // merkle-spine version store backed by HamtIndex.
    // Records: VersionId → (HamtRootSlabId, MerkleRootHash)
    // The HamtIndex maps page_id → content_hash for all pages in all versions.
    versions: VersionStore<HamtIndex<B>>,
}
```

The `VersionStore<HamtIndex<B>>` is merkle-spine's version DAG. Each version record
stores the root of the `HamtMap` (as a folio page ID) and the BLAKE3 Merkle root
hash of that HAMT (computed by `HamtIndex::root_hash()`).

Because the `HamtIndex` is itself a `HamtMap<u64, [u8; 32]>`, the Merkle root of
the *index* commits to every `(page_id, content_hash)` pair. This root hash is
what merkle-spine records per version.

### Version semantics

Every mutation creates a new version:

```
v0: root=A  (empty map)
v1: root=B  (inserted "foo"→1)  [B shares most nodes with A]
v2: root=C  (inserted "bar"→2)  [C shares most nodes with B]
v3: root=B  (removed "bar")     [B is the v1 root — shared, refcount incremented]
```

Each version is a self-contained, immutable snapshot. Structural sharing means
only the mutated path (O(log N) nodes) differs between adjacent versions.

A `VersionId` is a `u64` monotonic counter (cheap to store and compare), paired
with a `[u8; 32]` Merkle root hash (self-certifying — the hash commits to all
`(page_id, content_hash)` pairs, which commit to all key-value data).

### Historical lookup

`get_at(version, key)`:

1. Look up `version` in the `VersionStore` → retrieve the historical HAMT root
   `SlabPageId` and its Merkle root hash.
2. Construct a temporary `HamtMap` with that historical root (O(1) — no copy,
   just a different root pointer). The slab and refcount table are shared.
3. Call `HamtMap::get(key)` on the temporary map (O(log N)).

The historical HAMT nodes remain in the folio slab as long as any `VersionedHamt`
(or `VersionStore`) holds a reference to the version. The version record holds a
refcount on the HAMT root via the same `FolioSlab` refcount mechanism.

### Structural diff

`diff(from_version, to_version)`:

Walk two HAMT roots in parallel (DFS):
1. At each node, compare the content hashes of corresponding subtrees.
2. If the hashes match: **skip the entire subtree** — it is identical.
3. If the hashes differ: recurse into the subtree.
4. At leaves: emit `DiffEntry::Inserted`, `Removed`, or `Updated`.

Time: O(changed_entries × log N). For identical versions: O(1) (root hashes match).
For completely disjoint versions: O(N) (full traversal of both, no shared subtrees).

This is strictly better than DeltaLogIndex (merkle-spine v1), which must scan
all version records between `from` and `to` — O(delta_chain_length × entries_changed).
With HamtIndex, diff cost depends only on the actual content difference, not
on how many intermediate mutations occurred.

### Merkle proofs

A Merkle inclusion proof for key K in version V:

**Contents of the proof:**
```
root_hash: [u8; 32]          — the version's Merkle root hash (from VersionStore)
page_id: u64                  — the folio page containing the leaf with K
content_hash: [u8; 32]       — the content hash of that page
path: Vec<SiblingHashes>     — sibling hashes at each level, root-to-leaf
```

The proof demonstrates:
1. The content hash of the leaf page is `content_hash` (from the leaf's data).
2. The path of sibling hashes from the leaf up to the root produces `root_hash`.
3. The leaf page at `page_id` contains key K with value V.

**Proof size:** For a 256-way branching HAMT at 1M entries (depth ≈ 3):
≈ 3 × (256 × 32 bytes) = 24 KiB worst case. Typically much smaller as only
occupied siblings' hashes are included.

**Verification:** pure function, no folio access:
1. Hash the leaf page content → must equal `content_hash`
2. Walk the path, combining sibling hashes at each level → must produce `root_hash`
3. Confirm K→V is in the leaf's entries

The `root_hash` must come from a trusted source (e.g. a version record in a
local `VersionStore`, or a signed beacon from a trusted publisher). If the root
hash is trusted, the proof is cryptographically sound.

### Sparse sync

To synchronise two `VersionedHamt` instances that share a common ancestor:

1. Compute `diff(common_ancestor, local_head)` → list of changed `(page_id, content_hash)` pairs.
2. Transmit only the folio pages in that diff (not the entire map).
3. Receiver reconstructs the new version by inserting the transmitted pages into
   its local folio and updating its HamtIndex accordingly.

This works because:
- `page_id` is stable across instances (folio page IDs are deterministic given the
  same insert sequence, or can be mapped via an exchange protocol)
- Unchanged pages are already present on both sides (they share the same content hash)
- The Merkle root hash validates the received snapshot end-to-end

Full sparse sync protocol design is deferred to the implementation phase (H.5).

---

## Public API

### `VersionedHamt<K, V, B>`

```rust
/// A folio-backed, persistently versioned, Merkle-verified hash map.
///
/// Every mutation (insert, remove) creates a new immutable version.
/// All historical versions are accessible in O(log N) per lookup.
/// Structural diff between any two versions runs in O(changed × log N).
/// Merkle inclusion proofs are O(log N) to generate and O(log N) to verify.
pub struct VersionedHamt<K, V, B = DefaultBackend>
where
    K: Pod + Eq + Hash,
    V: Pod,
    B: FolioBackend;

impl<K, V, B> VersionedHamt<K, V, B>
where
    K: Pod + Eq + Hash,
    V: Pod,
    B: FolioBackend,
{
    // --- Construction ---

    /// Creates a new empty `VersionedHamt` backed by `store`.
    ///
    /// Creates an initial version (v0 = empty map) in the version store.
    pub fn new(store: FolioStore<B>) -> Self;

    // --- Current-version operations ---

    /// Returns a clone of the value for `key` in the current version, or `None`.
    ///
    /// Time: O(log N).
    pub fn get(&self, key: &K) -> Option<V>;

    /// Returns a new `VersionedHamt` with `key` → `value` inserted.
    ///
    /// Creates a new version. The original is unchanged.
    ///
    /// Time: O(log N). Allocates O(log N) new folio slab slots.
    pub fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new `VersionedHamt` with `key` removed, plus the evicted value.
    ///
    /// Creates a new version. The original is unchanged.
    ///
    /// Time: O(log N). Allocates O(log N) new folio slab slots.
    pub fn remove(&self, key: &K) -> (Self, Option<V>);

    /// Returns the number of entries in the current version.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize;

    /// Tests whether the current version is empty.
    pub fn is_empty(&self) -> bool;

    /// Tests whether `key` is present in the current version.
    ///
    /// Time: O(log N).
    pub fn contains_key(&self, key: &K) -> bool;

    /// Returns an iterator over all `(K, V)` pairs in the current version.
    ///
    /// Time: O(N) total.
    pub fn iter(&self) -> impl Iterator<Item = (K, V)> + '_;

    // --- Version identity ---

    /// Returns the current version's identifier.
    ///
    /// Time: O(1).
    pub fn version(&self) -> VersionId;

    /// Returns the BLAKE3 Merkle root hash of the current version.
    ///
    /// Two `VersionedHamt` values with equal root hashes have identical contents.
    ///
    /// Time: O(1) — cached in the version record.
    pub fn root_hash(&self) -> [u8; 32];

    // --- Historical access ---

    /// Returns a clone of the value for `key` at the given historical version.
    ///
    /// Returns `None` if `key` was absent at that version, or if `version`
    /// is not in this collection's history.
    ///
    /// Time: O(log N). Does not materialise the full historical map.
    pub fn get_at(&self, version: VersionId, key: &K) -> Option<V>;

    /// Returns a `VersionedHamt` frozen at the given historical version.
    ///
    /// The returned value has the same root HAMT and same folio backend;
    /// only its current-version pointer differs. Mutations on the returned
    /// value create new versions branching from `version`.
    ///
    /// Returns `None` if `version` is not in this collection's history.
    ///
    /// Time: O(1).
    pub fn checkout(&self, version: VersionId) -> Option<Self>;

    /// Returns the Merkle root hash of the given historical version.
    ///
    /// Returns `None` if `version` is unknown.
    ///
    /// Time: O(1).
    pub fn root_hash_at(&self, version: VersionId) -> Option<[u8; 32]>;

    // --- Diff ---

    /// Returns an iterator over all entries that differ between `from` and `to`.
    ///
    /// Exploits HAMT structure: subtrees with matching content hashes are skipped.
    ///
    /// Time: O(changed_entries × log N).
    /// Special cases: O(1) if from == to; O(N) if completely disjoint.
    pub fn diff(
        &self,
        from: VersionId,
        to: VersionId,
    ) -> impl Iterator<Item = DiffEntry<K, V>> + '_;

    // --- Merkle proofs ---

    /// Generates an inclusion proof for `key` in the current version.
    ///
    /// Returns `None` if `key` is absent.
    ///
    /// Time: O(log N).
    pub fn prove_inclusion(&self, key: &K) -> Option<MerkleProof>;

    /// Generates an inclusion proof for `key` at a historical version.
    ///
    /// Returns `None` if `key` is absent at that version or the version is unknown.
    ///
    /// Time: O(log N).
    pub fn prove_inclusion_at(&self, version: VersionId, key: &K) -> Option<MerkleProof>;

    /// Verifies a Merkle inclusion proof against a trusted root hash.
    ///
    /// Pure function — no folio access required. Can be called by a remote party
    /// that holds only the root hash from a trusted source.
    ///
    /// Returns `true` if `proof` demonstrates that `key` maps to `value` in the
    /// collection whose root hash is `root_hash`.
    ///
    /// Time: O(log N).
    pub fn verify_proof(root_hash: &[u8; 32], key: &K, value: &V, proof: &MerkleProof) -> bool;
}
```

### `VersionId`

```rust
/// A stable identifier for a specific version of a `VersionedHamt`.
///
/// The `seq` counter is monotonically increasing and cheap to compare.
/// The `root_hash` is self-certifying: it commits to all data in the version.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct VersionId {
    /// Monotonic sequence number. Version 0 is the empty initial map.
    pub seq: u64,
    /// BLAKE3 Merkle root hash of the HamtIndex at this version.
    pub root_hash: [u8; 32],
}
```

### `MerkleProof`

Re-exported from `pds_folio::MerkleProof`. See `pds-folio-spec.md`.

### `DiffEntry`

```rust
/// A single change between two `VersionedHamt` versions.
pub enum DiffEntry<K, V> {
    /// Key was added (present in `to`, absent in `from`).
    Inserted { key: K, value: V },
    /// Key was removed (absent in `to`, present in `from`).
    Removed { key: K, old_value: V },
    /// Key's value changed between `from` and `to`.
    Updated { key: K, old_value: V, new_value: V },
}
```

---

## Standard trait coverage

| Trait | `VersionedHamt` |
|-------|----------------|
| `Clone` | Y (O(1) — increments HAMT root refcount + bumps VersionStore ref) |
| `Debug` | Y |
| `PartialEq` / `Eq` | Y (compare `VersionId.root_hash` for O(1) equality) |
| `Default` | Y (empty map, v0) |
| `Send` / `Sync` | auto (if B: Send+Sync) |
| `PersistentCollection` | Y |
| `PersistentMap<K, V>` | Y (delegates to current version) |
| `VersionedPersistentMap<K, V>` | Y |
| `MerklePersistentMap<K, V>` | Y |

Note: `FromIterator`, `IntoIterator`, `Extend` are omitted from the initial design.
These can be added if there is a demand; `FromIterator` is straightforward (fold
`insert` calls). They are not in the common trait hierarchy so their absence does
not affect generic code.

---

## Integration with pds common traits

```rust
impl<K: Pod + Eq + Hash, V: Pod + Clone, B: FolioBackend>
    PersistentMap<K, V> for VersionedHamt<K, V, B>
{
    fn get_cloned(&self, key: &K) -> Option<V> { self.get(key) }
    fn insert(&self, key: K, value: V) -> Self  { VersionedHamt::insert(self, key, value) }
    fn remove(&self, key: &K) -> (Self, Option<V>) { VersionedHamt::remove(self, key) }
    fn len(&self) -> usize { self.len() }
    fn contains_key(&self, key: &K) -> bool { self.contains_key(key) }
}

impl<K: Pod + Eq + Hash, V: Pod + Clone, B: FolioBackend>
    VersionedPersistentMap<K, V> for VersionedHamt<K, V, B>
{
    type VersionId = VersionId;
    fn version(&self) -> VersionId { self.version() }
    fn get_at(&self, version: VersionId, key: &K) -> Option<V> { self.get_at(version, key) }
    fn checkout(&self, version: VersionId) -> Option<Self> { self.checkout(version) }
    fn diff(&self, from: VersionId, to: VersionId) -> impl Iterator<Item = DiffEntry<K, V>> + '_ {
        self.diff(from, to)
    }
}

impl<K: Pod + Eq + Hash, V: Pod + Clone, B: FolioBackend>
    MerklePersistentMap<K, V> for VersionedHamt<K, V, B>
{
    type Proof = MerkleProof;
    fn root_hash(&self) -> [u8; 32] { self.root_hash() }
    fn root_hash_at(&self, version: VersionId) -> Option<[u8; 32]> { self.root_hash_at(version) }
    fn prove_inclusion(&self, key: &K) -> Option<MerkleProof> { self.prove_inclusion(key) }
    fn prove_inclusion_at(&self, v: VersionId, key: &K) -> Option<MerkleProof> {
        self.prove_inclusion_at(v, key)
    }
    fn verify_proof(root: &[u8; 32], key: &K, value: &V, proof: &MerkleProof) -> bool {
        MerkleProof::verify(proof, root)  // key/value encoded in proof
    }
}
```

---

## Dense structures — out of scope

`VersionedHamt` is designed for **sparse, structured data** where individual
key-value pairs can be independently versioned. Path-copy costs O(log N) per
mutation, which is acceptable when N is the number of distinct keys.

For **dense structures** (tensors, image frames, binary blobs) where the
"value" is a large byte region and the entire region changes on every mutation:

- Path-copy of the entire region would be O(region_size) — unacceptable.
- Use folio directly to allocate the region as a folio page.
- Register the page with merkle-spine's `DeltaLogIndex` (v1) or a future
  non-HAMT `PageIndexBackend` (e.g. a flat array page index).
- Merkle proofs still work at the page level: merkle-spine can prove that
  page P has content hash H in version V.

This is the same conclusion as in `pds-folio-spec.md § Dense structures`.

---

## Implementation plan cross-reference

Phases in `pds/docs/impl-plan.md`:

| Phase | Description | Blocked by |
|-------|-------------|-----------|
| H.0 | Create `pds-merkle-spine` crate; deps on pds-folio + merkle-spine | pds-folio G.5, merkle-spine MS-F0 |
| H.1 | `VersionedHamt<K,V,B>` struct + `new()` + `version()` | H.0 |
| H.2 | Current-version CRUD (delegates to `HamtMap`) | H.1 |
| H.3 | Historical lookup: `get_at`, `checkout`, `root_hash_at` | H.2 |
| H.4 | Structural diff: `diff()` with hash-guided subtree skipping | H.3 |
| H.5 | Merkle proofs: `prove_inclusion`, `prove_inclusion_at`, `verify_proof` | H.3 |
| H.6 | Sparse sync protocol | H.4 + H.5 |
| H.7 | Implement pds common traits (`PersistentMap`, `VersionedPersistentMap`, `MerklePersistentMap`) | H.2–H.5 |
| H.8 | Integration tests; proptest suite | H.7 |
