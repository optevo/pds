# pds

Persistent data structures with structural sharing for Rust.

Forked from [imbl](https://github.com/jneem/imbl) (itself a fork of
[`im`](https://github.com/bodil/im-rs)) with different design priorities:
performance over compatibility, Merkle hashing for O(1) equality checks,
SIMD-accelerated HAMT nodes, and no_std support.

## Collections

| Type | Backing structure | Key operations |
|------|-------------------|----------------|
| `Vector<A>` | RRB tree | O(log n) index, push, split, concat |
| `HashMap<K, V>` | SIMD HAMT | O(log n) insert, lookup, set operations |
| `HashSet<A>` | SIMD HAMT | O(log n) insert, lookup, set operations |
| `OrdMap<K, V>` | B+ tree | O(log n) insert, lookup, range queries |
| `OrdSet<A>` | B+ tree | O(log n) insert, lookup, range queries |
| `Bag<A>` | SIMD HAMT | Persistent multiset (bag) with element counts |
| `HashMultiMap<K, V>` | SIMD HAMT | Key → set of values multimap |
| `InsertionOrderMap<K, V>` | SIMD HAMT + B+ tree | Map iterating in insertion order |
| `BiMap<K, V>` | 2× SIMD HAMT | Bidirectional map (bijection between two types) |
| `SymMap<A>` | 2× SIMD HAMT | Symmetric bidirectional map with O(1) swap |
| `Trie<K, V>` | HAMT of HAMTs | Hierarchical path-keyed map |

All collections use structural sharing for efficient cloning — cloning a
collection is O(1), and modified versions share unchanged subtrees with the
original.

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
| **HashMap / Set** | SIMD HAMT | HAMT | HAMT | HAMT |
| **OrdMap / Set** | B+ tree | B+ tree | B-tree | Red-black tree |
| **Bag** | yes | — | — | — |
| **HashMultiMap** | yes | — | — | — |
| **InsertionOrderMap** | yes | — | — | — |
| **BiMap** | yes | — | — | — |
| **SymMap** | yes | — | — | — |
| **Trie** | yes | — | — | — |
| **List / Stack / Queue** | — | — | — | yes |
| **Merkle hashing** | O(1) equality | — | — | — |
| **SIMD node ops** | yes | — | — | — |
| **`no_std`** | yes (via `foldhash`) | — | — | yes |
| **`triomphe::Arc`** | yes | — | — | — |
| **Hash consing** | yes (`InternPool`) | — | — | — |
| **SSP serialisation** | yes (`HashMapPool`) | — | — | — |
| **serde** | yes | yes | yes | yes |
| **rayon** | yes | yes | yes | yes (hash maps only) |
| **proptest / quickcheck** | yes | yes | yes | — |

**Key differences from imbl:**
- SIMD-accelerated HAMT nodes for faster hash map/set operations
- Merkle hashing on all collections for O(1) structural equality checks
- Six additional collection types (Bag, HashMultiMap, InsertionOrderMap, BiMap, SymMap, Trie)
- Hash consing via `InternPool` — deduplicates identical HAMT subtrees across collections
- Structural-sharing-preserving serialisation via `HashMapPool` — serialises/deserialises trees with node deduplication and cross-session interning
- `no_std` support via the `foldhash` feature flag
- `triomphe::Arc` support (no weak count, 8 bytes smaller per node)
- Deprecated API aliases removed; breaking changes for correctness accepted

## Documentation

- API docs — build locally with `cargo doc --open --all-features`
- [Architecture](docs/architecture.md) — internal data structure design
- [Decision log](docs/decisions.md) — architectural choices and rationale
- [Glossary](docs/glossary.md) — project terminology
- [Implementation plan](docs/impl-plan.md) — phased improvement roadmap
- [References](docs/references.md) — papers and external resources

## Feature flags

| Feature | Description |
|---------|-------------|
| `proptest` | Proptest strategies for all collection types |
| `quickcheck` | `Arbitrary` implementations for all collection types |
| `rayon` | Parallel iterators for all collection types |
| `serde` | `Serialize` / `Deserialize` for all collection types |
| `triomphe` | Use `triomphe::Arc` (no weak count, 8 bytes smaller per node) |
| `foldhash` | Enables `HashMap`/`HashSet`/etc. type aliases in `no_std` via `foldhash::fast::RandomState` |
| `arbitrary` | `Arbitrary` implementations for fuzzing |
| `atom` | Thread-safe atomic state holder via `arc-swap` (requires `std`) |
| `hash-intern` | Hash consing / node interning for HAMT collections via `InternPool` — deduplicates identical subtrees for memory savings and O(1) pointer equality |
| `persist` | Structural-sharing-preserving serialisation via `HashMapPool` — serialises HAMT trees with node deduplication, reconstructs with hash consing. Requires `hash-intern` |

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

## Code of Conduct

Please note that this project is released with a [Contributor Code of
Conduct][coc]. By participating in this project you agree to abide by its
terms.

[coc]: https://github.com/optevo/pds/blob/main/CODE_OF_CONDUCT.md
