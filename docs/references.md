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

---

## External Documentation {#sec:external-docs}

| Resource | URL / Location | Notes |
|----------|---------------|-------|
| imbl upstream | github.com/jneem/imbl | Upstream repository |
| archery crate | docs.rs/archery | SharedPointer abstraction (`get_mut`, `make_mut`) |
| imbl docs.rs | docs.rs/imbl | Published API documentation |
