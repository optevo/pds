# imbl

[![crates.io](https://img.shields.io/crates/v/imbl)](https://crates.io/crates/imbl)
![tests](https://github.com/optevo/imbl/actions/workflows/ci.yml/badge.svg)
[![docs.rs](https://docs.rs/imbl/badge.svg)](https://docs.rs/imbl/)

Blazing fast immutable collection datatypes for Rust.

This is a fork of [jneem/imbl](https://github.com/jneem/imbl), itself a fork
of the [`im`](https://github.com/bodil/im-rs) crate. Changes are structured
as independent, upstreamable PRs (see [DEC-001](docs/decisions.md)).

## Collections

| Type | Backing structure | Key operations |
|------|-------------------|----------------|
| `Vector<A>` | RRB tree | O(log n) index, push, split, concat |
| `HashMap<K, V>` | SIMD HAMT | O(log n) insert, lookup, set operations |
| `HashSet<A>` | SIMD HAMT | O(log n) insert, lookup, set operations |
| `OrdMap<K, V>` | B+ tree | O(log n) insert, lookup, range queries |
| `OrdSet<A>` | B+ tree | O(log n) insert, lookup, range queries |
| `PBag<A>` | SIMD HAMT | Persistent multiset (bag) with element counts |
| `HashMultiMap<K, V>` | SIMD HAMT | Key → set of values multimap |
| `InsertionOrderMap<K, V>` | SIMD HAMT + B+ tree | Map iterating in insertion order |

All collections use structural sharing for efficient cloning — cloning a
collection is O(1), and modified versions share unchanged subtrees with the
original.

## Documentation

- [API docs (docs.rs)](https://docs.rs/imbl/)
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
| `bincode` | **Deprecated** — will be removed in v8.0.0. Use serde instead. |

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

[coc]: https://github.com/jneem/imbl/blob/master/CODE_OF_CONDUCT.md
