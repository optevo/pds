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

## Choosing the right map

`HashMap` and `OrdMap` have the same asymptotic complexity for individual operations
(O(log n) insert and lookup) but differ substantially in memory layout, iteration
semantics, and bulk-operation performance. The right choice is usually obvious from
the constraints, but the non-obvious parts are noted below.

### OrdMap / OrdSet — B+ tree

**When to choose:**

- You need **sorted iteration order** or **range queries** (`get_range`, `iter_from`).
  This is the primary reason to prefer OrdMap. If you do not need ordering, prefer
  HashMap.
- You need **parallel bulk set operations** (`par_union`, `par_intersection`,
  `par_difference`, `par_symmetric_difference`). The B+ tree's structural split is
  O(log n); `concat_ordered` rebuilds the spine in O(log n). Combined, these give
  the join algorithm of Blelloch et al. (TOPC 2022): O(m log(n/m + 2)) work and
  O(log² n) span. This is the most efficient parallel join available for any Rust
  persistent map. HashMap's parallel set ops are also fast, but do not exploit the
  structural split the same way.
- Keys implement `Ord` but not `Hash`, or the hash function is significantly more
  expensive than comparison.

**Memory layout and allocation count:**

B+ tree leaves pack up to **16 key-value pairs per allocation** (`NODE_SIZE = 16`,
`THIRD = 5` minimum). For n entries the tree needs roughly n/16 leaf allocations,
regardless of key distribution. Sequential iteration and range scans are
cache-friendly: a single cache line covers several adjacent entries in a leaf.

HAMT nodes allocate per trie level rather than per entry batch. The exact count
depends on hash distribution and trie depth — see [Allocation profiling] below.

### HashMap / HashSet — SIMD HAMT

**When to choose:**

- Keys implement `Hash + Eq` but **not `Ord`**. This is the clearest reason.
- You make heavy use of **structural sharing from the same origin.** `ptr_eq` fast-paths
  in `par_union` and `par_intersection` detect when both operands share the same root
  pointer and short-circuit to O(1). Maps that are frequently cloned and then re-unioned
  with minimal changes benefit from this.
- You need **O(1) equality on frequently cloned maps.** Both `HashMap` and `OrdMap`
  have content-hash equality fast-paths, but the HAMT's per-node Merkle hashes update
  incrementally on each mutation so the root hash is always valid without a rescan.
  `OrdMap`'s `ord-hash` (default-on) gives the same O(1) positive and negative
  fast-paths but recomputes lazily after each mutation. For patterns where a map is
  cloned many times, mutated once, and compared immediately, the HAMT avoids the
  deferred rescan cost.

The `ord-hash` feature (default-on) adds a cached content hash to `OrdMap`/`OrdSet`,
giving them an O(1) `PartialEq` negative fast-path (different hash → definitely unequal)
and a `Hash` impl (when `K: Hash, V: Hash`) so they can be used as `HashMap` keys. The
meaningful reasons to prefer `HashMap` now narrow to: keys without `Ord`, and
high-frequency same-origin clone patterns where the HAMT Merkle hash fires before any
entry is compared. See DEC-036.

**Small-map behaviour:**

For ≤ 16 entries, HashMap uses a `SmallSimdNode` — a single flat allocation with SIMD
lookup. For ≤ 32 entries it promotes to `LargeSimdNode`. At these sizes, HashMap's
allocation count is comparable to OrdMap (one or two allocations for the entire map).
The HAMT trie levels only accumulate for larger maps.

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
- Fourteen additional collection types: Bag, OrdBag, HashMultiMap, OrdMultiMap, InsertionOrderMap, InsertionOrderSet, OrdInsertionOrderMap, OrdInsertionOrderSet, BiMap, OrdBiMap, SymMap, OrdSymMap, Trie, OrdTrie
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

All collection types gain parallel capabilities under the `rayon` feature flag.

### Parallel iteration

Every collection type that supports sequential iteration also supports parallel iteration via `par_iter()` and — where ordering semantics allow — `FromParallelIterator` and `ParallelExtend`.

| Type | `par_iter` | `FromParallelIterator` | `ParallelExtend` | Notes |
|------|:----------:|:---------------------:|:----------------:|-------|
| `HashMap<K, V>` | ✓ | ✓ | ✓ | |
| `HashSet<A>` | ✓ | ✓ | ✓ | |
| `OrdMap<K, V>` | ✓ | ✓ | ✓ | |
| `OrdSet<A>` | ✓ | ✓ | ✓ | |
| `Vector<A>` | ✓ | ✓ | ✓ | |
| `Bag<A>` | ✓ | ✓ | ✓ | Also `par_elements()` for flat expansion |
| `HashMultiMap<K, V>` | ✓ | ✓ | ✓ | Default hasher only |
| `BiMap<K, V>` | ✓ | ✓ | ✓ | Default hasher only |
| `SymMap<A>` | ✓ | ✓ | ✓ | Default hasher only |
| `InsertionOrderMap<K, V>` | ✓ | — | — | Parallel collection loses insertion order |
| `InsertionOrderSet<A>` | ✓ | — | — | Parallel collection loses insertion order |
| `Trie<K, V>` | — | — | — | Not supported |

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