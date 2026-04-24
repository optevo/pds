# Glossary {#sec:glossary}

Project-specific terminology. Use these terms consistently in code, doc
comments, and documentation — do not substitute synonyms.

---

**HAMT** — Hash Array Mapped Trie. The data structure backing `HashMap` and
  `HashSet`. imbl uses a SIMD-accelerated hybrid with three node types
  (SmallSimdNode, LargeSimdNode, HamtNode).

**CHAMP** — Compressed Hash-Array Mapped Prefix-tree. A HAMT optimisation
  from Steindorfer & Vinju (OOPSLA 2015) using two-bitmap encoding for
  canonical deletion and better cache locality. Evaluated as a potential
  replacement for the current HAMT internals but rejected after three
  independent PoC failures (DEC-007, DEC-015, DEC-019, DEC-020).

**RRB tree** — Relaxed Radix Balanced tree. The data structure backing
  `Vector`. Supports efficient concatenation, splitting, and indexed access.

**Structural sharing** — Shared ownership of subtrees across versions of a
  persistent data structure. Mutations create new paths; unchanged subtrees
  are shared via reference counting (`Arc` / `triomphe::Arc`).

**Transient** — A temporarily mutable view of a persistent data structure.
  Allows batch mutations without per-operation cloning, converting back to
  persistent form when done. See: Clojure transients, Phase 3.3.

**Focus / FocusMut** — Zipper-like cursors for efficient sequential access
  into `Vector`. Located in `src/vector/focus.rs`. Contains the densest
  unsafe code in the crate.

**archery** — Crate providing the `SharedPointer` abstraction that wraps
  `Arc` or `triomphe::Arc` via the `SharedPointerKind` trait. Key methods:
  `get_mut()` (check refcount, no clone) and `make_mut()` (clone if shared).

**Chunk** — Fixed-capacity inline array (from `imbl-sized-chunks` crate).
  The building block for all tree nodes. Size controlled by `VECTOR_CHUNK_SIZE`
  and `ORD_CHUNK_SIZE` in `src/config.rs`.

**SparseChunk** — Bitmap-indexed sparse array backed by a `Chunk`. Used in
  HAMT nodes and internally for compact storage.

**Merkle hash** — A u64 hash maintained incrementally on each HAMT node. The
  root Merkle hash is the sum of `mixer(key_hash)` across all entries (wyhash
  wide-multiply mixer). Provides an O(1) negative equality fast path: if two
  maps/sets have different root hashes, they are definitely unequal. Added in
  Phase 4.4 (DEC-009).

**BuildHasher** — The `core::hash::BuildHasher` trait, parameterised as `S`
  on all hash-based collections. In `std` mode, defaults to
  `std::collections::hash_map::RandomState`. In `no_std` mode, users supply
  their own implementation via the `Generic*` type variants.

