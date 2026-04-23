# Glossary {#sec:glossary}

Project-specific terminology. Use these terms consistently in code, doc
comments, and documentation — do not substitute synonyms.

---

**HAMT** — Hash Array Mapped Trie. The data structure backing `HashMap` and
  `HashSet`. imbl uses a SIMD-accelerated hybrid with three node types
  (SmallSimdNode, LargeSimdNode, HamtNode).

**CHAMP** — Compressed Hash-Array Mapped Prefix-tree. A HAMT optimisation
  from Steindorfer & Vinju (OOPSLA 2015) using two-bitmap encoding for
  canonical deletion and better cache locality. Candidate to replace the
  current HAMT internals (Phase 4).

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

**Pool / RRBPool** — Object pool for reusing `Chunk` allocations. Reduces
  allocation pressure during bulk operations. Located in `src/util/pool.rs`
  and `src/vector/pool.rs`.
