# pds

Persistent data structures with structural sharing for Rust.

Forked from [imbl](https://github.com/jneem/imbl) (itself a fork of
[`im`](https://github.com/bodil/im-rs)) with different design priorities:
performance over compatibility, Merkle hashing for O(1) equality checks,
SIMD-accelerated HAMT nodes, and no_std support.

## Collections

All collections use structural sharing: cloning is O(1) and modified copies
share unchanged subtrees with the original.

### Lists

| Type | Algorithm | Constraints | Order | Insert | Lookup |
|------|-----------|-------------|-------|--------|--------|
| `Vector<A>` | RRB tree | `Clone` | insertion | O(1)* | O(log n) |

*Amortised O(1) push; O(log n) split/concat.

### Maps

| Type | Algorithm | Constraints | Order | Insert | Lookup |
|------|-----------|-------------|-------|--------|--------|
| `HashMap<K, V>` | SIMD HAMT | `Clone + Hash + Eq` | undefined | O(log n) | O(log n) |
| `OrdMap<K, V>` | B+ tree | `Clone + Ord` | sorted | O(log n) | O(log n) |

### Sets

| Type | Algorithm | Constraints | Order | Insert | Lookup |
|------|-----------|-------------|-------|--------|--------|
| `HashSet<A>` | SIMD HAMT | `Clone + Hash + Eq` | undefined | O(log n) | O(log n) |
| `OrdSet<A>` | B+ tree | `Clone + Ord` | sorted | O(log n) | O(log n) |

### Other collections

| Type | Algorithm | Constraints | Description |
|------|-----------|-------------|-------------|
| `Bag<A>` | SIMD HAMT | `Clone + Hash + Eq` | Persistent multiset — tracks element counts |
| `OrdBag<A>` | B+ tree | `Clone + Ord` | Sorted multiset — `Ord`, `Hash`, range queries |
| `OrdMultiMap<K, V>` | B+ tree | `Clone + Ord` | Sorted key → sorted value-set multimap — `Ord`, `Hash`, range queries |
| `OrdSymMap<A>` | 2× B+ tree | `Clone + Ord` | Sorted symmetric bidirectional map — `Ord`, `Hash` |
| `OrdBiMap<K, V>` | 2× B+ tree | `Clone + Ord` | Sorted bidirectional map — bijection, `Ord`, `Hash` |
| `OrdTrie<K, V>` | B+ tree of B+ trees | `Clone + Ord` | Sorted prefix tree — lexicographic path iteration |
| `OrdInsertionOrderMap<K, V>` | 2× B+ tree | `Clone + Ord` | Insertion-ordered map — `Ord`-only, O(log n) delete |
| `OrdInsertionOrderSet<A>` | 2× B+ tree | `Clone + Ord` | Insertion-ordered set — `Ord`-only, O(log n) delete |
| `HashMultiMap<K, V>` | SIMD HAMT | `Clone + Hash + Eq` | Key → set of values multimap |
| `InsertionOrderMap<K, V>` | SIMD HAMT + B+ tree | `Clone + Hash + Eq` | Map that iterates in insertion order |
| `InsertionOrderSet<A>` | SIMD HAMT + B+ tree | `Clone + Hash + Eq` | Set that iterates in insertion order |
| `BiMap<K, V>` | 2× SIMD HAMT | `Clone + Hash + Eq` | Bidirectional map — bijection between two types |
| `SymMap<A>` | 2× SIMD HAMT | `Clone + Hash + Eq` | Symmetric bidirectional map with O(1) swap |
| `Trie<K, V>` | HAMT of HAMTs | `Clone + Hash + Eq` | Persistent prefix tree — paths to values |
| `UniqueVector<A>` | RRB tree + SIMD HAMT | `Clone + Hash + Eq` | Persistent sequence with uniqueness — dedup queue/stack with O(log n) index access |

## Choosing the right map

The `Hash`-backed and `Ord`-backed variants of every collection type share the same
persistent semantics, the same structural-sharing clone model, and the same API shape.
The choice between them is driven first by key constraints, then by which operations
dominate your workload.

This section covers the primary `HashMap` / `OrdMap` pair. The same reasoning applies
to every derived type: `Bag` / `OrdBag`, `HashMultiMap` / `OrdMultiMap`,
`BiMap` / `OrdBiMap`, `SymMap` / `OrdSymMap`, `Trie` / `OrdTrie`,
`InsertionOrderMap` / `OrdInsertionOrderMap`, and
`InsertionOrderSet` / `OrdInsertionOrderSet`.

---

### Functional differences

These are hard constraints, not preferences:

| | `HashMap` / Hash variants | `OrdMap` / Ord variants |
|--|--------------------------|------------------------|
| Key constraint | `Clone + Hash + Eq` | `Clone + Ord` |
| Iteration order | arbitrary (HAMT layout) | sorted by key |
| Range queries | — | `get_range`, `iter_from`, `split_at_key` |
| `get_min` / `get_max` | — | O(log n) |
| `without_min` / `without_max` | — | O(log n), structural sharing |
| `split_at_key` | — | O(log n) |
| Parallel join algorithm | filter+reduce (O(n)) | Blelloch join (O(m log(n/m))) |
| Used as a `HashMap` key | yes (`Hash` via `ord-hash`) | yes (`Hash` via `ord-hash`) |
| `no_std` without `foldhash` | — | yes |

If you need sorted iteration, range queries, or access to the minimum/maximum key,
`OrdMap` is the only option. If your keys lack `Ord`, `HashMap` is the only option.
When both constraints are satisfied the choice is a performance question.

---

### Measured performance — `i64` keys, M5 Max, Rust 1.95

All numbers from `cargo bench --bench compare --features rayon` (release profile).
Full results in `docs/baselines.md`.

#### Random point lookup

| Size    | HashMap  | OrdMap   | Faster |
|--------:|---------:|---------:|--------|
| 100     | 549 ns   | 630 ns   | HashMap ×1.15 |
| 1,000   | 5.76 µs  | 10.6 µs  | HashMap ×1.84 |
| 10,000  | 74.6 µs  | 157 µs   | HashMap ×2.11 |
| 100,000 | 1.17 ms  | 2.27 ms  | HashMap ×1.94 |

HAMT gives O(1) amortised lookup (fixed trie depth). OrdMap is O(log n) with a
small constant from B+ node binary search. HashMap is consistently ~2× faster for
random point queries, and this is the **only operation where HashMap wins**.

#### Write-heavy / build-from-scratch (`insert_mut`, `from_iter`)

| Size    | HashMap insert | OrdMap insert | OrdMap faster |
|--------:|---------------:|--------------:|:-------------:|
| 100     | 2.20 µs        | 1.26 µs       | ×1.74 |
| 1,000   | 30.4 µs        | 16.2 µs       | ×1.88 |
| 10,000  | 236 µs         | 230 µs        | ≈ equal |
| 100,000 | 3.97 ms        | 2.01 ms       | ×1.98 |

OrdMap wins under sole-owner writes: copy-on-write detects the sole reference and
mutates in-place without allocating. HashMap rewrites HAMT nodes on every insert
regardless of ownership. The same pattern holds for `from_iter` (OrdMap ×1.4–2.0×
faster) and `remove_mut` at small sizes (OrdMap ×1.7–2.2× faster at ≤1K).

#### Iteration

| Size    | HashMap iter | OrdMap iter | OrdMap faster |
|--------:|-------------:|------------:|:-------------:|
| 100     | 199 ns       | 145 ns      | ×1.37 |
| 1,000   | 1.89 µs      | 1.35 µs     | ×1.40 |
| 10,000  | 33.3 µs      | 14.3 µs     | ×2.33 |
| 100,000 | 553 µs       | 155 µs      | ×3.57 |

B+ tree leaves are contiguous arrays; iterating a leaf scans cache-linearly.
HAMT traversal follows pointer chains through bitmapped nodes — poor spatial
locality. The gap widens with size as the HAMT grows deeper.

#### Parallel set operations

| Operation | Size    | HashMap | OrdMap  | OrdMap faster |
|-----------|--------:|--------:|--------:|:-------------:|
| `par_union` | 10,000 | 1.08 ms | 267 µs | ×4.0 |
| `par_union` | 100,000 | 13.0 ms | 840 µs | ×15.5 |
| `par_intersection` | 10,000 | 929 µs | 437 µs | ×2.1 |
| `par_intersection` | 100,000 | 9.55 ms | 1.49 ms | ×6.4 |

OrdMap uses the O(m log(n/m)) parallel join algorithm (split → recurse → concat);
HashMap uses filter+reduce, which has a sequential O(n) bottleneck. The gap grows
with size and is the dominant reason to choose OrdMap for any set-merge workload.

#### Allocation efficiency

From `cargo bench --bench memory` (dhat, 100,000 `i64` entries):

| | HashMap | OrdMap | OrdMap fewer |
|--|:-------:|:------:|:------------:|
| Allocations | 29,633 | 6,641 | ×4.5 |
| Bytes | 13.9 MB | 3.6 MB | ×3.9 |

OrdMap packs up to 16 key-value pairs per leaf allocation. HAMT nodes are
per-trie-level and multiply with tree depth.

---

### Usage patterns

**Use `HashMap` / Hash variants when:**
- Keys are not `Ord` (e.g. unordered tuples, custom types without a natural order).
- Your workload is dominated by **random point lookups** and writes are infrequent.
  The ~2× lookup advantage compounds when lookups are the overwhelming majority of
  operations.
- You rely on `ptr_eq` structural-sharing fast-paths: `HashMap::par_union` short-circuits
  to O(1) when both operands share the same root (common after `clone()` with no mutation).
  This benefits patterns like "start from a common snapshot, make one change, union back".
- You accumulate incremental Merkle hashes without rescan. HAMT root hashes update
  atomically on each insert; `OrdMap` (`ord-hash`) recomputes lazily. If you clone a map,
  mutate it once, and compare it immediately, HAMT avoids the deferred rescan.

**Use `OrdMap` / Ord variants when:**
- You need **sorted order** at any point — sorted output, priority processing, stable
  serialisation, or deterministic comparison. Iteration is sorted by definition and
  ×2–4× faster than HashMap at large sizes.
- You need **range queries**: "all keys between X and Y", "everything from key K onwards".
  This is only available on `OrdMap`.
- You need **minimum / maximum access** without a full scan (`get_min`, `get_max`,
  `without_min`, `without_max`). These are O(log n).
- You perform **parallel set operations** (`par_union`, `par_intersection`, etc.). At
  100K entries OrdMap's join algorithm is ~15× faster than HashMap's filter+reduce.
- Your workload is **write-heavy or bulk-construction-heavy**. Sole-owner in-place
  mutation gives OrdMap a consistent ×1.5–2× advantage over HashMap for inserts,
  removes, and `from_iter`.
- You are in a **`no_std` environment** without the `foldhash` feature. OrdMap requires
  no hasher.
- You want **lower memory pressure**. At 100K entries OrdMap uses 4.5× fewer allocations
  and 3.9× less memory than HashMap.
- Keys are expensive to hash. `Ord` comparison is often cheaper than hashing for
  numeric or short-string keys, and OrdMap's B+ tree stops as soon as a comparison
  resolves the branch.

**Use either when:**
- Immutable snapshots / structural sharing: both types share subtrees on clone and write
  only the changed path.
- Serde round-trips: both implement `Serialize`/`Deserialize` (behind `serde` feature).
- Rayon parallel iteration: both support `par_iter()` and `FromParallelIterator`.
- As a key in another map: both implement `Hash` (via `ord-hash` for `OrdMap`; built-in
  for `HashMap`) and can be used as keys in `HashMap<OrdMap<_,_>, _>` etc.

---

### The same choice for derived types

Every Hash-variant / Ord-variant pair follows the same pattern:

| Hash variant | Ord variant | Primary addition in Ord variant |
|---|---|---|
| `Bag<A>` | `OrdBag<A>` | sorted element order, range count queries |
| `HashMultiMap<K,V>` | `OrdMultiMap<K,V>` | sorted keys and values, range scans |
| `BiMap<K,V>` | `OrdBiMap<K,V>` | sorted forward and reverse iteration |
| `SymMap<A>` | `OrdSymMap<A>` | sorted pair iteration |
| `Trie<K,V>` | `OrdTrie<K,V>` | lexicographic prefix iteration in sorted order |
| `InsertionOrderMap<K,V>` | `OrdInsertionOrderMap<K,V>` | no `Hash` needed on keys |
| `InsertionOrderSet<A>` | `OrdInsertionOrderSet<A>` | no `Hash` needed on elements |

For each pair: the Hash variant requires `Hash + Eq`, the Ord variant requires only `Ord`.
Performance characteristics mirror the `HashMap` / `OrdMap` comparison above: the Ord
variant is faster for writes, iteration, and parallel ops; the Hash variant is faster for
random point lookups on large collections. The `OrdInsertionOrder*` types have no Hash
variant analogue for the iteration-order guarantee when keys lack `Hash`.

---

### Allocation counts — measured with dhat

`dhat` measures exact heap allocation counts per operation. The table below is from
`cargo bench --bench memory` on an M5 Max (Rust 1.95.0, release profile).

**`from_iter` allocations and bytes — `HashMap<i64,i64>` vs `OrdMap<i64,i64>`:**

| Entries | HashMap allocs | OrdMap allocs | Ratio | HashMap bytes | OrdMap bytes | Ratio |
|--------:|:--------------:|:-------------:|:-----:|:-------------:|:------------:|:-----:|
| 1,000   | 226            | 68            | 3.3×  | 120,288       | 36,576       | 3.3×  |
| 10,000  | 1,134          | 666           | 1.7×  | 528,216       | 358,224      | 1.5×  |
| 100,000 | 29,633         | 6,641         | **4.5×**| 13,874,968  | 3,572,024    | **3.9×** |

**`from_iter` allocations — `HashSet<i64>` vs `OrdSet<i64>`:**

| Entries | HashSet allocs | OrdSet allocs | Ratio |
|--------:|:--------------:|:-------------:|:-----:|
| 1,000   | 248            | 68            | 3.6×  |
| 10,000  | 1,147          | 666           | 1.7×  |
| 100,000 | 29,709         | 6,641         | **4.5×** |

The gap opens with scale because HAMT promotes nodes through three tiers (SmallSimdNode →
LargeSimdNode → HamtNode) and each trie level adds allocations. OrdMap's B+ tree is
bounded by the tree height: at 100,000 entries, roughly one allocation per 15 entries
(100,000 / 16 ≈ 6,250 leaves; 6,641 includes internal branch nodes).

**Small-map behaviour:** for ≤ 16 entries, `HashMap` uses a `SmallSimdNode` — a single
flat allocation with SIMD lookup. For ≤ 32 entries it promotes to `LargeSimdNode`. At
these sizes HashMap's allocation count is comparable to OrdMap.

For your own workload, run `cargo bench --bench memory` or instrument with:

```rust
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

let _profiler = dhat::Profiler::new_heap();
// ... operations under test ...
```

Full results are in `docs/baselines.md`.

[Allocation profiling]: #allocation-profiling-with-dhat

---

## Comparison with similar crates

pds is forked from [imbl](https://github.com/jneem/imbl), which is itself a
fork of the unmaintained [im](https://github.com/bodil/im-rs). The API is
largely compatible with imbl 7.x, but pds prioritises performance and
capability over strict backward compatibility.

[rpds](https://github.com/orium/rpds) is an independent implementation with
a different collection set and design philosophy.

| | **pds** | **imbl** | **im** | **rpds** |
|---|---|---|---|---|
| **Version** | 1.0.0 | 7.0.0 | 15.1.0 | 1.2.0 |
| **Last release** | 2026 | Jan 2026 | Apr 2022 | Nov 2025 |
| **Vector** | RRB tree | RRB tree | RRB tree | — (indexable sequence) |
| **HashMap / Set** | SIMD HAMT | SIMD HAMT | HAMT | HAMT |
| **OrdMap / Set** | B+ tree | B+ tree | B-tree | Red-black tree |
| **Bag** | yes | — | — | — |
| **OrdBag** | yes | — | — | — |
| **HashMultiMap** | yes | — | — | — |
| **OrdMultiMap** | yes | — | — | — |
| **InsertionOrderMap** | yes | — | — | — |
| **InsertionOrderSet** | yes | — | — | — |
| **OrdInsertionOrderMap** | yes | — | — | — |
| **OrdInsertionOrderSet** | yes | — | — | — |
| **BiMap** | yes | — | — | — |
| **OrdBiMap** | yes | — | — | — |
| **SymMap** | yes | — | — | — |
| **OrdSymMap** | yes | — | — | — |
| **Trie** | yes | — | — | — |
| **OrdTrie** | yes | — | — | — |
| **UniqueVector** | yes | — | — | — |
| **List / Stack / Queue** | — | — | — | yes |
| **Merkle hashing** | O(1) equality | — | — | — |
| **SIMD node ops** | yes | yes | — | — |
| **`no_std`** | yes (via `foldhash`) | — | — | yes |
| **`triomphe::Arc`** | yes | — | — | — |
| **Hash consing** | yes (`InternPool`) | — | — | — |
| **SSP serialisation** | yes (`HashMapPool`) | — | — | — |
| **serde** | yes | yes | yes | yes |
| **rayon** | yes | yes | yes | yes (hash maps only) |
| **Par set ops** | yes (all types) | — | — | — |
| **proptest / quickcheck** | yes | yes | yes | — |

**Key differences from imbl:**
- Merkle hashing on all collections for O(1) structural equality checks
- Fifteen additional collection types: Bag, OrdBag, HashMultiMap, OrdMultiMap, InsertionOrderMap, InsertionOrderSet, OrdInsertionOrderMap, OrdInsertionOrderSet, BiMap, OrdBiMap, SymMap, OrdSymMap, Trie, OrdTrie, UniqueVector
- Hash consing via `InternPool` — deduplicates identical HAMT subtrees across collections
- Structural-sharing-preserving serialisation via `HashMapPool` — serialises/deserialises trees with node deduplication and cross-session interning
- `no_std` support via the `foldhash` feature flag
- `triomphe::Arc` support (no weak count, 8 bytes smaller per node)
- Deprecated API aliases removed; breaking changes for correctness accepted

## Documentation

- API docs — build locally with `rm -rf rustdocs && cargo doc --no-deps --all-features --target-dir rustdocs --open`
- [Architecture](docs/architecture.md) — internal data structure design
- [Decision log](docs/decisions.md) — architectural choices and rationale
- [Glossary](docs/glossary.md) — project terminology
- [Implementation plan](docs/impl-plan.md) — phased improvement roadmap
- [References](docs/references.md) — papers and external resources

## Feature flags

| Feature | Default | Description |
|---------|:-------:|-------------|
| `std` | Yes | Enables `std`-dependent type aliases (`HashMap`, `HashSet`, etc.), `From<std::collections::*>` conversions, and `Mutex`-based locking. Disable for `no_std + alloc` environments. |
| `triomphe` | Yes | Use `triomphe::Arc` as the default shared pointer — no weak count, 8 bytes smaller per node, one fewer atomic op per clone/drop. |
| `proptest` | No | Proptest strategies for `Vector`, `OrdMap`, `OrdSet`, `HashMap`, `HashSet`. Newer types (Bag, HashMultiMap, etc.) not yet covered. |
| `quickcheck` | No | `Arbitrary` implementations for `Vector`, `OrdMap`, `OrdSet`, `HashMap`, `HashSet`. Newer types not yet covered. |
| `rayon` | No | Parallel iterators and parallel set operations for all collection types. See "Parallel support" below for full coverage. |
| `serde` | No | `Serialize` / `Deserialize` for all collection types |
| `arbitrary` | No | `Arbitrary` implementations for fuzzing (`Vector`, `OrdMap`, `OrdSet`, `HashMap`, `HashSet`). Newer types not yet covered. |
| `foldhash` | No | Enables `HashMap`/`HashSet`/etc. type aliases in `no_std` via `foldhash::fast::RandomState` |
| `atom` | No | Thread-safe atomic state holder via `arc-swap` (requires `std`) |
| `hash-intern` | No | Hash consing / node interning for HAMT collections via `InternPool` — deduplicates identical subtrees for memory savings and O(1) pointer equality |
| `persist` | No | Structural-sharing-preserving serialisation via `HashMapPool` — serialises HAMT trees with node deduplication, reconstructs with hash consing. Requires `hash-intern` |
| `ord-hash` | Yes | Cached content hash on `OrdMap` and `OrdSet` — O(1) `PartialEq` fast-path, `content_hash()` method, and `Hash` impl when `K: Hash, V: Hash`. One atomic store per mutation; overhead is unmeasurable for typical workloads. |
| `small-chunks` | No | Reduces internal chunk sizes so tree structures can be exercised with small collections. For testing only — not intended for production use. |
| `debug` | No | Enables internal invariant-checking methods on `Vector` (RRB tree validation). For testing and debugging only. |

## Parallel support

The core types and their Hash-backed derived types gain parallel capabilities under
the `rayon` feature flag. **Ord-backed derived types** (`OrdBag`, `OrdMultiMap`,
`OrdBiMap`, `OrdSymMap`, `OrdTrie`, `OrdInsertionOrderMap`, `OrdInsertionOrderSet`)
do not currently have rayon support — only sequential operations.

### Parallel iteration

| Type | `par_iter` | `FromParallelIterator` | `ParallelExtend` | Notes |
|------|:----------:|:---------------------:|:----------------:|-------|
| `HashMap<K, V>` | ✓ | ✓ | ✓ | |
| `HashSet<A>` | ✓ | ✓ | ✓ | |
| `OrdMap<K, V>` | ✓ | ✓ | ✓ | |
| `OrdSet<A>` | ✓ | ✓ | ✓ | |
| `Vector<A>` | ✓ | ✓ | ✓ | |
| `Bag<A>` | ✓ | ✓ | ✓ | Also `par_elements()` for flat expansion |
| `OrdBag<A>` | — | — | — | No rayon support |
| `HashMultiMap<K, V>` | ✓ | ✓ | ✓ | Default hasher only |
| `OrdMultiMap<K, V>` | — | — | — | No rayon support |
| `BiMap<K, V>` | ✓ | ✓ | ✓ | Default hasher only |
| `OrdBiMap<K, V>` | — | — | — | No rayon support |
| `SymMap<A>` | ✓ | ✓ | ✓ | Default hasher only |
| `OrdSymMap<A>` | — | — | — | No rayon support |
| `InsertionOrderMap<K, V>` | ✓ | — | — | Parallel collection loses insertion order |
| `OrdInsertionOrderMap<K, V>` | — | — | — | No rayon support |
| `InsertionOrderSet<A>` | ✓ | — | — | Parallel collection loses insertion order |
| `OrdInsertionOrderSet<A>` | — | — | — | No rayon support |
| `Trie<K, V>` | — | — | — | No rayon support |
| `OrdTrie<K, V>` | — | — | — | No rayon support |

### Parallel set operations

Every collection type that exposes `union`, `intersection`, `difference`, and
`symmetric_difference` also has parallel counterparts named with the `par_`
prefix. These work identically to the sequential versions but use rayon to
parallelise the computation.

| Type | `par_union` | `par_intersection` | `par_difference` | `par_symmetric_difference` |
|------|:-----------:|:-----------------:|:----------------:|:---------------------------:|
| `HashMap<K, V>` | ✓ | ✓ | ✓ | ✓ |
| `HashSet<A>` | ✓ | ✓ | ✓ | ✓ |
| `OrdMap<K, V>` | ✓ | ✓ | ✓ | ✓ |
| `OrdSet<A>` | ✓ | ✓ | ✓ | ✓ |
| `Bag<A>` | ✓ | ✓ | ✓ | ✓ |
| `HashMultiMap<K, V>` | ✓† | ✓ | ✓ | ✓ |
| `BiMap<K, V>` | ✓† | ✓ | ✓ | ✓ |
| `SymMap<A>` | ✓† | ✓ | ✓ | ✓ |

All Ord-backed derived types (`OrdBag`, `OrdMultiMap`, `OrdBiMap`, `OrdSymMap`,
`OrdTrie`, `OrdInsertionOrderMap`, `OrdInsertionOrderSet`): no parallel set operations.

† `par_union` delegates to the sequential implementation for these types.
`BiMap` and `SymMap` maintain bijection/symmetry invariants that require
sequential conflict resolution on each insert; `HashMultiMap` value-set merging
has the same constraint. The other three par ops are fully parallelised via
parallel filter + collect.

**Fast paths for `HashMap` / `HashSet`:** The HAMT-backed types also exploit
structural sharing for O(1) short-circuits:
- `ptr_eq` — if both collections share the same root pointer they are identical;
  union returns one copy, difference returns empty, intersection returns one copy.
- Merkle hash — same-lineage maps with equal length and equal Merkle hash are
  definitively equal; the fast-path fires without comparing individual entries.

**Join algorithm for `OrdMap` / `OrdSet` (B+ tree):** These types use a
fundamentally different parallel strategy based on Blelloch et al., "Joinable
Parallel Balanced Binary Trees" (ACM TOPC 2022) and "PaC-trees" (PLDI 2022).
A single structural `split` at the root's median key divides both inputs into
independent halves, which are merged recursively in parallel via `rayon::join`,
then concatenated with a height-aware `concat`. This gives:

- **Work:** O(m log(n/m + 2)) — optimal for set operations on inputs of size m ≤ n
- **Span:** O(log² n) — polylogarithmic, scales with thread count

Both the split and concat are O(log n) structural operations on the B+ tree spine —
no per-entry hashing or re-insertion required. This is believed to be the first
implementation of the Blelloch join algorithm on a blocked-leaf persistent B+ tree
in any language. No other Rust persistent map library implements join-based parallel
set operations.

**Rayon-join parallelism for `symmetric_difference`:** `par_symmetric_difference`
on all types uses `rayon::join` to compute the two halves (`self \ other` and
`other \ self`) simultaneously on separate threads.

### Example

```rust
use pds::{Bag, HashMap, HashSet};
use rayon::iter::ParallelIterator;

// Parallel iteration
let map: HashMap<i32, &str> = (0..10_000).map(|i| (i, "x")).collect();
let sum: i32 = map.par_iter().map(|(&k, _)| k).sum();

// Parallel set operations
let mut a = Bag::new();
let mut b = Bag::new();
a.insert_many("apple", 5);
a.insert_many("banana", 3);
b.insert_many("banana", 7);
b.insert_many("cherry", 2);

let union = a.par_union(&b);          // apple:5, banana:10, cherry:2
let intersection = a.par_intersection(&b);  // banana:3
let difference = a.par_difference(&b);     // apple:5
```

## Building

```bash
# Development (requires Nix)
nix develop              # enter devShell with stable Rust + sccache
bash test.sh             # run full quality gate (tests + clippy + doc)
bash bench.sh            # run criterion benchmarks
bash bench.sh vector     # run a single benchmark suite

# Nightly tools (miri, fuzzing)
nix develop .#nightly    # enter nightly devShell
cargo miri test          # run tests under miri
cd fuzz && cargo fuzz list  # list fuzz targets
```

## Minimum supported Rust version

This crate supports Rust 1.85 and later.

## Licence

Copyright 2017–2021 Bodil Stokke
Copyright 2021 Joe Neeman

This software is subject to the terms of the Mozilla Public
License, v. 2.0. If a copy of the MPL was not distributed with this
file, You can obtain one at <http://mozilla.org/MPL/2.0/>.