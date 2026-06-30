# Cross-Variant Trait Layer — pds

<!-- Spec for PersistentMap, PersistentSet, VersionedPersistentMap, MerklePersistentMap -->

---

## Contents

- [Motivation](#motivation)
- [Trait hierarchy](#trait-hierarchy)
- [Trait definitions](#trait-definitions)
  - [PersistentCollection](#persistentcollection)
  - [PersistentMap](#persistentmapk-v)
  - [PersistentSet](#persistentseta)
  - [VersionedPersistentMap](#versionedpersistentmapk-v)
  - [MerklePersistentMap](#merklepersistentmapk-v)
- [Implementing types](#implementing-types)
- [Design notes](#design-notes)

---

## Motivation

pds currently provides heap-backed, in-memory persistent data structures.
Two planned extension crates (`pds-folio`, `pds-merkle-spine`) extend the
ecosystem with folio page-backed persistence and versioning respectively.

All three variants share the same logical contract — a functional,
persistent collection that supports O(log N) point operations and O(1) clone
via structural sharing — but differ in:

| Aspect | pds (in-memory) | pds-folio | pds-merkle-spine |
|--------|----------------|-----------|-----------------|
| Node storage | Heap (`Arc`) | Folio slab pages | Folio slab pages |
| Durability | None | Crash-safe via folio WAL | Crash-safe + versioned |
| Version history | No | No | Yes |
| Merkle proofs | No | No | Yes |

This file specifies a layered trait hierarchy that lets code work uniformly
across all three backends without sacrificing backend-specific capabilities.

---

## Trait hierarchy

```
PersistentCollection (marker; all variants)
    │
    ├── PersistentMap<K, V>           (K: Hash + Eq; all hash map variants)
    │       │
    │       └── VersionedPersistentMap<K, V>    (pds-merkle-spine only)
    │               │
    │               └── MerklePersistentMap<K, V>  (pds-merkle-spine only)
    │
    ├── PersistentSet<A>              (A: Hash + Eq; all hash set variants)
    │
    ├── PersistentVector<A>           (all vector variants)
    │
    ├── PersistentOrdMap<K, V>        (K: Ord; all ordered map variants)
    │
    └── PersistentOrdSet<A>           (A: Ord; all ordered set variants)
```

`PersistentMap` and `PersistentOrdMap` are parallel, not related by inheritance:
they have different key bounds (`Hash + Eq` vs `Ord`). Both extend
`PersistentCollection`. Code generic over any persistent map must pick one or
the other, not unify them.

---

## Trait definitions

### `PersistentCollection`

Marker trait. Blanket-implemented for any type that implements `PersistentMap`
or `PersistentSet`. Signals that cloning is O(1) structural-sharing, not O(N)
deep copy.

```rust
/// Marker trait for persistent (immutable with structural sharing) collections.
///
/// # Clone semantics
///
/// Types implementing this trait must provide O(1) `Clone` via structural
/// sharing — incrementing a reference count, not copying all elements.
///
/// Implementations: [`pds::HashMap`], [`pds::HashSet`],
/// [`pds_folio::HamtMap`], [`pds_folio::HamtSet`],
/// [`pds_merkle_spine::VersionedHamt`].
pub trait PersistentCollection: Clone {}
```

### `PersistentMap<K, V>`

The core map trait. Implemented by all three variants. Uses owned value returns
so the trait works regardless of whether values live in heap allocations (pds),
mmap'd pages (pds-folio), or versioned pages (pds-merkle-spine).

```rust
/// A persistent (functional) map with O(log N) point operations and O(1) clone.
///
/// # Value return convention
///
/// `get_cloned` returns an owned `V`, not a reference. This is necessary for
/// portability: folio-backed variants store values in mmap'd pages whose
/// lifetime is not directly tied to `&self`. In-memory pds implements this
/// via `HashMap::get(...).cloned()`.
///
/// For in-memory-only code that needs a reference (`Option<&V>`), use the
/// concrete `HashMap` type directly rather than this trait.
pub trait PersistentMap<K, V>: PersistentCollection
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// Returns a clone of the value associated with `key`, or `None` if absent.
    ///
    /// Time: O(log N).
    fn get_cloned(&self, key: &K) -> Option<V>;

    /// Returns a new collection with `key` mapped to `value`.
    ///
    /// If `key` is already present, the old value is replaced. The original
    /// collection is unchanged.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new collection with `key` removed, plus the evicted value.
    ///
    /// If `key` is absent, returns `(self.clone(), None)`.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn remove(&self, key: &K) -> (Self, Option<V>)
    where
        Self: Sized;

    /// Returns the number of key-value pairs.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the collection is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tests whether `key` is present.
    ///
    /// Time: O(log N).
    fn contains_key(&self, key: &K) -> bool;
}
```

### `PersistentVector<A>`

```rust
/// A persistent (functional) vector with O(log N) point operations and O(1) clone.
///
/// Based on an RRB-tree. Index operations are O(log_{B} N) where B is the
/// branching factor (typically 32 — effectively O(1) for practical sizes).
pub trait PersistentVector<A>: PersistentCollection
where
    A: Clone,
{
    /// Returns the element at `index`, or `None` if out of bounds. Time: O(log N).
    fn get(&self, index: usize) -> Option<A>;

    /// Returns a new vector with `value` appended. Time: O(log N) amortised.
    fn push_back(&self, value: A) -> Self;

    /// Returns a new vector with `value` prepended. Time: O(log N) amortised.
    fn push_front(&self, value: A) -> Self;

    /// Returns a new vector with the element at `index` replaced. Time: O(log N).
    fn update(&self, index: usize, value: A) -> Self;

    /// Returns a new vector with the last element removed, plus the element.
    fn pop_back(&self) -> (Self, Option<A>) where Self: Sized;

    /// Returns a new vector with the first element removed, plus the element.
    fn pop_front(&self) -> (Self, Option<A>) where Self: Sized;

    /// Concatenates `self` and `other`. Time: O(log N).
    fn concat(&self, other: &Self) -> Self;

    /// Splits at `index`, returning `(left, right)`. Time: O(log N).
    fn split_at(&self, index: usize) -> (Self, Self) where Self: Sized;

    /// Returns the number of elements. Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the vector is empty. Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
```

### `PersistentOrdMap<K, V>`

```rust
/// A persistent ordered map with O(log N) point operations, O(log N + k) range
/// queries, and O(1) clone.
///
/// `K: Ord` — keys are maintained in sorted order. Does not require `Hash + Eq`.
/// Parallel to `PersistentMap` (hash-based); they are not related by inheritance.
pub trait PersistentOrdMap<K, V>: PersistentCollection
where
    K: Clone + Ord,
    V: Clone,
{
    /// Returns a clone of the value for `key`, or `None`. Time: O(log N).
    fn get_cloned(&self, key: &K) -> Option<V>;

    /// Returns a new map with `key` → `value` inserted. Time: O(log N).
    fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new map with `key` removed, plus the evicted value. Time: O(log N).
    fn remove(&self, key: &K) -> (Self, Option<V>) where Self: Sized;

    /// Returns the number of key-value pairs. Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the map is empty. Time: O(1).
    fn is_empty(&self) -> bool { self.len() == 0 }

    /// Tests whether `key` is present. Time: O(log N).
    fn contains_key(&self, key: &K) -> bool;

    /// Returns the smallest key-value pair, or `None`. Time: O(log N).
    fn first(&self) -> Option<(K, V)>;

    /// Returns the largest key-value pair, or `None`. Time: O(log N).
    fn last(&self) -> Option<(K, V)>;

    /// Returns an iterator over pairs with keys in `bounds`, in ascending order.
    /// Time: O(log N) to seek; O(k) to iterate k results.
    fn range<R: RangeBounds<K>>(&self, bounds: R) -> impl Iterator<Item = (K, V)> + '_;
}
```

### `PersistentOrdSet<A>`

```rust
/// A persistent ordered set. Parallel to `PersistentSet` (hash-based).
pub trait PersistentOrdSet<A>: PersistentCollection
where
    A: Clone + Ord,
{
    fn contains(&self, value: &A) -> bool;
    fn insert(&self, value: A) -> Self;
    fn remove(&self, value: &A) -> Self;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
    fn first(&self) -> Option<A>;
    fn last(&self) -> Option<A>;
    fn range<R: RangeBounds<A>>(&self, bounds: R) -> impl Iterator<Item = A> + '_;
}
```

### `PersistentSet<A>`

Mirrors `PersistentMap<A, ()>` but expresses the set contract directly.

```rust
/// A persistent (functional) set with O(log N) point operations and O(1) clone.
pub trait PersistentSet<A>: PersistentCollection
where
    A: Clone + Eq + Hash,
{
    /// Tests whether `value` is a member of the set.
    ///
    /// Time: O(log N).
    fn contains(&self, value: &A) -> bool;

    /// Returns a new set with `value` inserted.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn insert(&self, value: A) -> Self;

    /// Returns a new set with `value` removed.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn remove(&self, value: &A) -> Self;

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
```

### `VersionedPersistentMap<K, V>`

Extends `PersistentMap` with access to version history. Only implemented by
`pds_merkle_spine::VersionedHamt`.

```rust
/// A persistent map that retains its full mutation history as navigable versions.
///
/// Every mutation (insert, remove) creates a new version. Past versions remain
/// readable indefinitely at O(log N) cost, with structural sharing between
/// adjacent versions: only the O(log N) mutated nodes are distinct.
///
/// # Version identity
///
/// `VersionId` is a stable, O(1)-comparable handle to a specific point in the
/// collection's history. In pds-merkle-spine, `VersionId` is a `u64` counter
/// paired with a Merkle root hash for self-certification.
pub trait VersionedPersistentMap<K, V>: PersistentMap<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// The stable identifier for a snapshot of this collection.
    type VersionId: Copy + Eq + Hash + Debug;

    /// Returns the identifier of the current version.
    ///
    /// Time: O(1).
    fn version(&self) -> Self::VersionId;

    /// Returns a clone of the value of `key` at a specific historical version,
    /// or `None` if absent at that version.
    ///
    /// Time: O(log N). Does not require materialising the full historical map.
    fn get_at(&self, version: Self::VersionId, key: &K) -> Option<V>;

    /// Returns a read-only view frozen at `version`.
    ///
    /// The returned map implements `PersistentMap<K, V>` but not `VersionedPersistentMap`
    /// (it is read-only; mutations would need to branch from that version explicitly).
    ///
    /// Returns `None` if `version` is not in this collection's history.
    ///
    /// Time: O(1) — just changes the root pointer to the historical root.
    fn checkout(&self, version: Self::VersionId) -> Option<Self>;

    /// Returns an iterator over `(VersionId, key, DiffOp)` triples describing
    /// mutations between `from` and `to`.
    ///
    /// `DiffOp` is either `Inserted(V)`, `Removed(V)`, or `Updated { old: V, new: V }`.
    ///
    /// Exploits Merkle-hash subtree equality to skip unchanged subtrees.
    ///
    /// Time: O(changed_entries × log N). O(1) if from == to.
    fn diff(
        &self,
        from: Self::VersionId,
        to: Self::VersionId,
    ) -> impl Iterator<Item = DiffEntry<K, V>> + '_;
}

/// A single entry from a structural diff between two versions.
pub enum DiffEntry<K, V> {
    /// Key was added between `from` and `to`.
    Inserted { key: K, value: V },
    /// Key was removed between `from` and `to`.
    Removed { key: K, old_value: V },
    /// Key's value changed between `from` and `to`.
    Updated { key: K, old_value: V, new_value: V },
}
```

### `MerklePersistentMap<K, V>`

Extends `VersionedPersistentMap` with cryptographic identity via BLAKE3 Merkle
hashing. Only implemented by `pds_merkle_spine::VersionedHamt`.

```rust
/// A versioned persistent map with cryptographic Merkle identity.
///
/// The root hash of each version fully determines its contents: any two maps
/// with identical root hashes are identical. Inclusion proofs let external
/// parties verify that a key-value pair exists in a specific version without
/// access to the full collection.
pub trait MerklePersistentMap<K, V>: VersionedPersistentMap<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// An inclusion proof that key K has value V in a specific version.
    ///
    /// Size: O(log N × branching_factor) bytes. For a 256-way HAMT at 1M
    /// entries, depth ≈ 3 → proof ≈ 3 × 256 × 32 bytes ≈ 24 KiB.
    type Proof;

    /// Returns the BLAKE3 Merkle root hash of the current version.
    ///
    /// Two `VersionedHamt` values with equal root hashes have identical contents
    /// at that version (up to BLAKE3's 2^-256 collision probability).
    ///
    /// Time: O(1) — cached in the version record.
    fn root_hash(&self) -> [u8; 32];

    /// Returns the BLAKE3 Merkle root hash of the given historical version.
    ///
    /// Time: O(1) — retrieved from the version DAG.
    fn root_hash_at(&self, version: Self::VersionId) -> Option<[u8; 32]>;

    /// Generates a Merkle inclusion proof for `key` at the current version.
    ///
    /// Returns `None` if `key` is absent.
    ///
    /// The proof can be verified by any party holding only `root_hash()` —
    /// no access to folio pages or network required.
    ///
    /// Time: O(log N).
    fn prove_inclusion(&self, key: &K) -> Option<Self::Proof>;

    /// Generates a Merkle inclusion proof for `key` at a historical version.
    ///
    /// Returns `None` if `key` is absent at that version or the version is unknown.
    ///
    /// Time: O(log N).
    fn prove_inclusion_at(&self, version: Self::VersionId, key: &K) -> Option<Self::Proof>;

    /// Verifies a Merkle inclusion proof against a trusted root hash.
    ///
    /// Returns `true` if `proof` demonstrates that `key` maps to `value` in
    /// the collection whose root hash is `root_hash`.
    ///
    /// This is a pure function — no collection access required. Can be called
    /// by a remote party that only holds the root hash from a trusted source.
    ///
    /// Time: O(log N).
    fn verify_proof(root_hash: &[u8; 32], key: &K, value: &V, proof: &Self::Proof) -> bool
    where
        Self: Sized;
}
```

---

## Implementing types

| Type | `PMap` | `PSet` | `PVec` | `POrdMap` | `POrdSet` | `VPMap` | `MPMap` |
|------|--------|--------|--------|-----------|-----------|---------|---------|
| `pds::HashMap<K, V>` | Y | — | — | — | — | — | — |
| `pds::HashSet<A>` | — | Y | — | — | — | — | — |
| `pds::Vector<A>` | — | — | Y | — | — | — | — |
| `pds::OrdMap<K, V>` | — | — | — | Y | — | — | — |
| `pds::OrdSet<A>` | — | — | — | — | Y | — | — |
| `pds_folio::HashMap<K, V, C>` | Y | — | — | — | — | — | — |
| `pds_folio::HashSet<A, C>` | — | Y | — | — | — | — | — |
| `pds_folio::Vector<A, C>` | — | — | Y | — | — | — | — |
| `pds_folio::OrdMap<K, V, C>` | — | — | — | Y | — | — | — |
| `pds_folio::OrdSet<A, C>` | — | — | — | — | Y | — | — |
| `pds_merkle_spine::VersionedHamt<K, V, C>` | Y | — | — | — | — | Y | Y |

Column headers: `PMap` = `PersistentMap`, `PSet` = `PersistentSet`,
`PVec` = `PersistentVector`, `POrdMap` = `PersistentOrdMap`,
`POrdSet` = `PersistentOrdSet`, `VPMap` = `VersionedPersistentMap`,
`MPMap` = `MerklePersistentMap`.

`C: Codec` defaults to `PostcardCodec`. Use `PodCodec` for fixed-size numeric
key/value types. `VersionedVector`, `VersionedOrdMap` etc. are future additions
to pds-merkle-spine.

`OrdMap`, `OrdSet`, `Vector` and the 15 derived types may gain `PersistentMap`
/ `PersistentSet` impls in a later pass.

---

## Design notes

### Why `get_cloned` instead of `get(&self, key: &K) -> Option<&V>`

Reference-returning `get` is impossible to unify across backends:

- **In-memory pds**: values live in `Arc<HamtNode>`, behind `&self`. Lifetime tied to `self`.
- **pds-folio**: values live in mmap'd pages, behind a `PageGuard`. The guard's lifetime
  is not `&self` — it requires holding a page pin while the reference is live.
- **pds-merkle-spine**: same as pds-folio.

A generic associated type (GAT) solution:

```rust
type ValueRef<'a>: Deref<Target = V> where Self: 'a;
fn get<'a>(&'a self, key: &K) -> Option<Self::ValueRef<'a>>;
```

is technically possible with stable GATs but forces every caller to name
`<M as PersistentMap<K, V>>::ValueRef<'_>` — worse ergonomics than `get_cloned`.

The clone cost is acceptable for the key use case of this trait: code that is
generic over the storage backend but not in a tight inner loop. Hot-path code
should use the concrete type (`HashMap` or `HamtMap`) directly.

### Why `insert` takes `self` (by reference) and returns `Self`

This is the standard functional update pattern: `new_map = old_map.insert(k, v)`.
The original map is unchanged. Both old and new map share all unmodified nodes.

A `mut self`-based API (where `insert` modifies in place and returns `Self`) would
work but loses the "keep both versions" property, which is the distinguishing
feature of persistent data structures. The trait reflects the intended usage.

### pds-folio impl bounds vs trait bounds

`PersistentMap<K, V>` requires `K: Clone + Eq + Hash, V: Clone`. These are the
minimum bounds for the trait contract itself. `pds_folio::HamtMap` additionally
requires `K: Serialize` and `V: Serialize + DeserializeOwned` (for page encoding).
These extra bounds appear on the `impl` block, not on the trait — callers using
the concrete type need them; callers using the trait abstraction only see
`Clone + Eq + Hash`.

`pds::HashMap` uses `K: Clone + Eq + Hash, V: Clone` internally (heap nodes). No
Serialize bound is needed for the in-memory backend.

### Trait location

These traits are defined in `pds` (`src/traits.rs`) and re-exported at the crate
root. This avoids a separate `pds-traits` crate and keeps the dependency graph clean:

```
pds          (defines + implements)
pds-folio    (depends on pds → implements traits)
pds-merkle-spine (depends on pds-folio + merkle-spine → implements traits)
```

No circular dependencies.
