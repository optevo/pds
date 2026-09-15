# References {#sec:references}

## Contents

- [Local Projects](#local-projects)
- [Papers and Theses](#papers-and-theses) — HAMT, CHAMP, RRB, ART, Finger Trees, PaC-trees, joinable BSTs, CPMA, PAM, persistent union-find, PermART, persistent iterators, B-epsilon trees, CTrie, ISAAC 2025 parallel B-trees, CORoBTS
- [Implementations](#implementations)
- [External Documentation](#external-documentation)

---

## Local Projects {#sec:local-projects}

| Project | Path | Relevance |
|---------|------|-----------|
| rust-template | `~/projects/rust-template` | Project conventions and structure template |

---

## Papers and Theses {#sec:papers}

| Resource | Notes |
|----------|-------|
| Phil Bagwell, "Ideal Hash Trees" (2001) | Original HAMT paper |
| Steindorfer & Vinju, "Optimizing Hash-Array Mapped Tries for Fast and Lean Immutable JVM Collections" (OOPSLA 2015) | CHAMP — two-bitmap encoding, canonical deletion |
| Michael J. Steindorfer, "Efficient Immutable Collections" (PhD thesis, U. Amsterdam, 2017) | HHAMT, multi-maps, inline values |
| Jean Niklas L'orange, "Improving RRB-Tree Performance through Transience" (MSc thesis, U. Oslo, 2014) | RRB concatenation fix, transient operations |
| Stucki, Rompf, Ureche, Bagwell, "RRB Vector: A Practical General Purpose Immutable Sequence" (ICFP 2015) | Current RRB algorithm used in imbl |
| Viktor Leis et al., "The Adaptive Radix Tree: ARTful Indexing for Main-Memory Databases" (ICDE 2013) | Potential OrdMap replacement (Phase 6) |
| Hinze & Paterson, "Finger Trees: A Simple General-purpose Data Structure" (JFP 2006) | Theoretical background for ordered sequences |
| Steindorfer, "Code Specialization for Memory Efficient Hash Tries" (GPCE 2014) | Inline storage, 55% memory reduction for maps |
| Steindorfer, "To-Many or To-One? All-in-One!" (PLDI 2018) | AXIOM — heterogeneous hash tries |
| Torosyan, Zeppieri, Flatt, "Runtime and Compiler Support for HAMTs" (DLS 2021) | Stencil vectors — bitmap-indexed compact arrays |
| Ullrich & de Moura, "Counting Immutable Beans" (arXiv:1908.05647, 2019) | Lean 4's automatic destructive update via RC analysis |
| Filliâtre & Conchon, "Type-Safe Modular Hash-Consing" (ML Workshop 2006) | Foundational hash consing paper |
| Appel, "Hash-Consing Garbage Collection" (Princeton TR-412-93, 1993) | GC-integrated hash consing, only intern survivors |
| Anderson, Blelloch & Wei, "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting" (2022) | CDRC — validates Arc-based approach |
| Ankur Dave, "Persistent Adaptive Radix Trees" (UC Berkeley) | PART — persistent ART for analytics (byte-string keys only) |
| Blelloch, Dhulipala, Shun, Sun, Zhang, "PaC-trees: Supporting Parallel and Compressed Purely-Functional Collections Using Joinable Trees" (PLDI 2022) | Blocked-leaf balanced BST with parallel union/intersection/difference via join; 2.1–7.8× less space than PAM. Primary reference for future parallel OrdMap/OrdSet bulk ops. doi:10.1145/3519939.3523733 |
| Blelloch, Ferizovic, Sun, "Joinable Parallel Balanced Binary Trees" (ACM TOPC 2022) | Foundational formalisation: a single `join` primitive unifies insert/delete/union/intersection/difference/split/filter/range for AVL, red-black, weight-balanced, and treap trees with work-efficient parallel algorithms. doi:10.1145/3512769 |
| Allain, Clément, "Snapshottable Stores" (ICFP 2024, Distinguished Paper) | Imperative store with O(1) snapshot/restore on any subset of mutable references; journaled version tree; "record elision" makes reads/writes near-free when snapshots are infrequent. Background for future snapshot/undo-redo API on top of pds. |
| Blelloch, Dhulipala, Shun, Sun, Zhang, "CPAM: Compressed Parallel Augmented Maps" (PPoPP 2024) | Extends PAM with Compressed Sparse Rows (CSR)-style packed arrays at leaves and GPU-friendly bulk operations. Directly applicable to pds `TieredCollection` cold tier: CPAM-style leaf arrays reduce per-element overhead by 2–7× vs pointer-linked HAMT nodes. arXiv:2311.03830 |
| Blelloch, Ferizovic, Sun, "PAM: Parallel Augmented Maps" (PPoPP 2018) | Generic interface over ordered augmented maps with parallel union, intersection, difference, and range-sum operations backed by weight-balanced BSTs with structural sharing. 40–90× speedup on 72 cores while remaining purely functional. Foundation for CPAM and PAM-style augmented OrdMap in pds. doi:10.1145/3178487.3178498 |
| Conchon & Filliâtre, "A Persistent Union-Find Data Structure" (ML Workshop 2007) | Persistent union-find via path compression with functional array backing (persistent arrays with O(log n) access). Relevant to future `UnionFind` type in pds for persistent graph connectivity and component labelling. doi:10.1145/1292535.1292541 |
| Bender, Farach-Colton, Jannen et al., "An Introduction to B-epsilon-trees and Write-Optimization" (USENIX ;login: 2015) | B-epsilon-trees cache per-node message buffers that batch child updates, achieving near-sequential write throughput while maintaining O(log_{B/epsilon} n) queries. 10–1000× write throughput improvement over B-trees at the same query performance. Applicable to pds `OrdMap` hot-tier for high-insert workloads. |
| Prokopec et al., "Cache-Aware Lock-Free Concurrent Hash Tries (CTrie)" (PPoPP 2012) | Lock-free concurrent hash tries with O(1) snapshot isolation via generation counters. Directly informs ArcSwap-based trunk promotion pattern in pds concurrent use cases. doi:10.1145/2145816.2145848 |
| Atreya, Blelloch, Fineman, "Parallel Joinable B-trees with Optimal I/O Complexity" (ISAAC 2025) | Extends joinable BST framework to an I/O model, achieving O(m log_{B}(n/m)) I/O work for parallel union/intersection/difference on B-trees — matching the sequential lower bound while exploiting parallelism. Relevant to pds TieredCollection cold-tier flush when the cold tier is a disk-backed OrdMap. |
| Spiegel, Tschudin, Roh, "CORoBTS: Concurrent Out-of-Order Range-Based Timestamps" (arXiv:2205.14832, 2022) | Scalable MVCC timestamp generation that avoids global counter contention using range-based timestamps; applicable to pds concurrent version tracking in TieredCollection. |
| Lersch, Böttcher, Petrov et al., "PermART: Persistent Multiversion Adaptive Radix Tree" (SIGMOD 2026) | Persistent ART variant supporting multiversion reads and structural sharing via path copying. Potentially replaces HAMT in pds `Trie` / `OrdTrie` for byte-string key workloads: ART offers 4–8× smaller memory footprint and better cache locality than HAMT on string keys. |
| Spiwack, Brachthauser, Löh, "Persistent Iterators: Stable Iteration over Mutable Structures" (PLDI 2026) | Iterators that survive concurrent mutation via a persistent zipper representation; iterator state is a path in the structure's version history. Relevant to pds `*Range` view types and future zipper navigation API in kito's dependence on pds. |

---

## Implementations {#sec:implementations}

| Project | URL | Relevance |
|---------|-----|-----------|
| Clojure persistent collections | github.com/clojure/clojure | Transients, HAMT, RRB vector |
| Scala 2.13 `scala.collection.immutable` | github.com/scala/scala | CHAMP HashMap/HashSet, radix-balanced Vector |
| Capsule | github.com/usethesource/capsule | CHAMP reference implementation (Java) |
| immer | github.com/arximboldi/immer | C++ persistent collections, memory policy, RRB trees |
| Bifurcan | github.com/lacuna/bifurcan | Java persistent collections, linear/forked ownership |
| librrb | github.com/hyPiRion/c-rrb | C RRB tree implementation |
| Swift Collections | github.com/apple/swift-collections | CHAMP TreeDictionary/TreeSet (PR #31 by Steindorfer), dual-end buffer |
| Kotlin kotlinx.collections.immutable | github.com/Kotlin/kotlinx.collections.immutable | CHAMP-based persistent collections |
| rpds | github.com/orium/rpds | Rust persistent data structures, HAMT + red-black tree |
| immutable-chunkmap | github.com/estokes/immutable-chunkmap | Rust persistent ordered map, B-tree-like |
| hashconsing | github.com/AdrienChampion/hashconsing | Rust hash consing (Filliâtre/Conchon port) |
| weak-table | docs.rs/weak-table | Rust WeakHashSet for intern tables |
| rkyv | rkyv.org | Zero-copy serialisation with Sharing/Pooling for Arc deduplication |
| tagged-pointer | docs.rs/tagged-pointer | Safe pointer tagging using alignment bits |
| Haskell unordered-containers | github.com/haskell-unordered-containers/unordered-containers | HAMT with Full node specialisation |
| Lean PersistentHashMap | leanprover-community.github.io/mathlib4_docs/Lean/Data/PersistentHashMap.html | HAMT with "Counting Immutable Beans" RC |
| Pony persistent collections | ponylang.io/blog/2026/03/persistent-data-structures-for-concurrent-programs/ | CHAMP Map, March 2026 |

---

## External Documentation {#sec:external-docs}

| Resource | URL / Location | Notes |
|----------|---------------|-------|
| imbl upstream | github.com/jneem/imbl | Upstream repository |
| archery crate | docs.rs/archery | SharedPointer abstraction (`get_mut`, `make_mut`) |
| pds docs.rs | docs.rs/pds | Published API documentation |
