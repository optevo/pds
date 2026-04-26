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
| `HashMultiMap<K, V>` | SIMD HAMT | `Clone + Hash + Eq` | Key → set of values multimap |
| `InsertionOrderMap<K, V>` | SIMD HAMT + B+ tree | `Clone + Hash + Eq` | Map that iterates in insertion order |
| `InsertionOrderSet<A>` | SIMD HAMT + B+ tree | `Clone + Hash + Eq` | Set that iterates in insertion order |
| `BiMap<K, V>` | 2× SIMD HAMT | `Clone + Hash + Eq` | Bidirectional map — bijection between two types |
| `SymMap<A>` | 2× SIMD HAMT | `Clone + Hash + Eq` | Symmetric bidirectional map with O(1) swap |
| `Trie<K, V>` | HAMT of HAMTs | `Clone + Hash + Eq` | Persistent prefix tree — paths to values |

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
| **HashMultiMap** | yes | — | — | — |
| **InsertionOrderMap** | yes | — | — | — |
| **InsertionOrderSet** | yes | — | — | — |
| **BiMap** | yes | — | — | — |
| **SymMap** | yes | — | — | — |
| **Trie** | yes | — | — | — |
| **List / Stack / Queue** | — | — | — | yes |
| **Merkle hashing** | O(1) equality | — | — | — |
| **SIMD node ops** | yes | yes | — | — |
| **`no_std`** | yes (via `foldhash`) | — | — | yes |
| **`triomphe::Arc`** | yes | — | — | — |
| **Hash consing** | yes (`InternPool`) | — | — | — |
| **SSP serialisation** | yes (`HashMapPool`) | — | — | — |
| **serde** | yes | yes | yes | yes |
| **rayon** | yes | yes | yes | yes (hash maps only) |
| **proptest / quickcheck** | yes | yes | yes | — |

**Key differences from imbl:**
- Merkle hashing on all collections for O(1) structural equality checks
- Seven additional collection types (Bag, HashMultiMap, InsertionOrderMap, InsertionOrderSet, BiMap, SymMap, Trie)
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
| `rayon` | No | Parallel iterators for all collection types. `InsertionOrderMap` and `InsertionOrderSet` support read-only `par_iter` only (`FromParallelIterator`/`ParallelExtend` omitted — parallel collection loses insertion order). `Trie` is excluded. `Bag` adds `par_elements()` for flat element expansion. |
| `serde` | No | `Serialize` / `Deserialize` for all collection types |
| `arbitrary` | No | `Arbitrary` implementations for fuzzing (`Vector`, `OrdMap`, `OrdSet`, `HashMap`, `HashSet`). Newer types not yet covered. |
| `foldhash` | No | Enables `HashMap`/`HashSet`/etc. type aliases in `no_std` via `foldhash::fast::RandomState` |
| `atom` | No | Thread-safe atomic state holder via `arc-swap` (requires `std`) |
| `hash-intern` | No | Hash consing / node interning for HAMT collections via `InternPool` — deduplicates identical subtrees for memory savings and O(1) pointer equality |
| `persist` | No | Structural-sharing-preserving serialisation via `HashMapPool` — serialises HAMT trees with node deduplication, reconstructs with hash consing. Requires `hash-intern` |
| `small-chunks` | No | Reduces internal chunk sizes so tree structures can be exercised with small collections. For testing only — not intended for production use. |
| `debug` | No | Enables internal invariant-checking methods on `Vector` (RRB tree validation). For testing and debugging only. |

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