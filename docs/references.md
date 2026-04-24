# References {#sec:references}

## Contents

- [Local Projects](#local-projects)
- [Papers and Theses](#papers-and-theses)
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
