# Architecture {#sec:architecture}

Internal architecture of imbl's core data structure modules. This document
covers the current implementation as of v7.0.0 (~7.5K lines across 6 core
files). It is a prerequisite for making safe structural changes — read this
before modifying any module described here.

## Contents

- [Overview](#overview)
- [SharedPointer abstraction](#shared-pointer)
- [HAMT — Hash Array Mapped Trie](#hamt)
- [RRB tree — Relaxed Radix Balanced tree](#rrb)
- [B+ tree](#btree)
- [Focus and FocusMut](#focus)
- [Unsafe inventory](#unsafe-inventory)

---

## Overview {#overview}

imbl provides five persistent collection types, each backed by a different
tree structure:

| Type | Backing structure | Module | Lines |
|------|-------------------|--------|-------|
| `Vector<A>` | RRB tree | `vector/mod.rs`, `nodes/rrb.rs` | ~4K |
| `HashMap<K, V>` | SIMD HAMT | `nodes/hamt.rs` | ~1.1K |
| `HashSet<A>` | SIMD HAMT (via HashMap) | `nodes/hamt.rs` | (shared) |
| `OrdMap<K, V>` | B+ tree | `nodes/btree.rs` | ~1.3K |
| `OrdSet<A>` | B+ tree (via OrdMap) | `nodes/btree.rs` | (shared) |

All collections are generic over `SharedPointerKind` (see @sec:shared-pointer),
enabling copy-on-write via `SharedPointer::make_mut`. The concrete pointer
type is selected by the `DefaultSharedPtr` alias in `shared_ptr.rs`.

---

## SharedPointer abstraction {#shared-pointer}

**File:** `src/shared_ptr.rs` (24 lines — re-export shim)

All internal node pointers use `archery::SharedPointer<T, P>` where `P`
implements `SharedPointerKind`. This abstraction provides:

- `SharedPointer::new(T)` — allocate
- `SharedPointer::make_mut(&mut self) -> &mut T` — clone-on-write (clones
  if refcount > 1)
- `SharedPointer::get_mut(&mut self) -> Option<&mut T>` — in-place mutation
  if sole owner (refcount == 1).
- `SharedPointer::ptr_eq(&self, &other) -> bool` — identity comparison
- `SharedPointer::strong_count(&self) -> usize` — refcount

### DefaultSharedPtr

The `DefaultSharedPtr` type alias selects the concrete pointer:

- **With `triomphe` feature (default):** `ArcTK` → `triomphe::Arc` (no weak
  count, 8 bytes smaller per allocation)
- **Without `triomphe` feature:** `ArcK` → `std::sync::Arc`

---

## HAMT — Hash Array Mapped Trie {#hamt}

**File:** `src/nodes/hamt.rs` (1084 lines)
**Backs:** `HashMap<K, V>`, `HashSet<A>`

### Architecture — 3-tier SIMD hybrid

This is NOT a textbook bitmap HAMT. The current implementation (introduced
in v6.1/v7.0) uses a 3-tier node hierarchy with SIMD-accelerated lookup:

```
Tier 1: SmallSimdNode  — 16 slots, 1×u8x16 SIMD group   (leaf-only)
Tier 2: LargeSimdNode  — 32 slots, 2×u8x16 SIMD groups  (leaf-only)
Tier 3: HamtNode       — 32 slots, classic bitmap-indexed (can hold children)
```

Both SIMD node types are instantiations of `GenericSimdNode<A, WIDTH, GROUPS>`:

```rust
type SmallSimdNode<A> = GenericSimdNode<A, SMALL_NODE_WIDTH, 1>;  // 16 slots
type LargeSimdNode<A> = GenericSimdNode<A, HASH_WIDTH, 2>;        // 32 slots
```

**Promotion:** Nodes promote as they fill: Small → Large → Hamt. Only
`HamtNode` can hold child pointers (other `Entry` variants). SIMD nodes
store `(value, hash)` pairs directly.

### Entry enum

The `Entry` enum (line 617) has 5 variants:

```rust
enum Entry<A, P: SharedPointerKind> {
    HamtNode(SharedPointer<HamtNode<A, P>, P>),
    SmallSimdNode(SharedPointer<SmallSimdNode<A>, P>),
    LargeSimdNode(SharedPointer<LargeSimdNode<A>, P>),
    Value(A, HashBits),
    Collision(SharedPointer<CollisionNode<A>, P>),
}
```

`HamtNode.data` is a `SparseChunk<Entry<A, P>, HASH_WIDTH>` — a
bitmap-indexed array that can hold any `Entry` variant, including child
nodes. SIMD nodes store only `SparseChunk<(A, HashBits), WIDTH>` — flat
value arrays with no children.

### SIMD lookup

Lookup uses `wide::u8x16` for parallel byte comparison:

1. Compute `ctrl_hash` — the top 8 bits of the hash, clamped to `≥ 1`
   (0 means empty slot). (`ctrl_hash`, line 191)
2. Determine which SIMD group to search (for `LargeSimdNode` with 2
   groups, the group is selected by hash bits). (`ctrl_hash_and_group`,
   line 181)
3. SIMD `cmp_eq` + `move_mask` produces a bitmap of matching slots.
   (`group_find`, line 54)
4. Iterate matches, verify with full key equality.

### hash_may_eq optimisation

`hash_may_eq` (line 660) skips the hash equality check for small,
non-Drop keys (≤ 16 bytes). For these types, the key comparison itself
is cheap enough that the extra hash comparison is wasted work:

```rust
fn hash_may_eq<A: HashValue>(hash: HashBits, other_hash: HashBits) -> bool {
    (!mem::needs_drop::<A::Key>() && mem::size_of::<A::Key>() <= 16) || hash == other_hash
}
```

### node_with — zero-copy construction

`node_with` (line 62) constructs a `SharedPointer<T>` without extra
copies. It allocates an uninitialised `UnsafeCell<MaybeUninit<T>>`,
writes the default, then transmutes the pointer type. This avoids the
memcpy that `SharedPointer::new(T::default())` would incur for large
node types.

### Configuration

Constants from `config.rs`:

- `HASH_LEVEL_SIZE` (aliased as `HASH_SHIFT`): 5 (or 3 with `small-chunks`)
- `HASH_WIDTH`: 2^HASH_SHIFT = 32 (or 8)
- `SMALL_NODE_WIDTH`: HASH_WIDTH / 2 = 16 (or 4)
- `GROUP_WIDTH`: HASH_WIDTH / 2 = 16 (or 4)

---

## RRB tree — Relaxed Radix Balanced tree {#rrb}

**Files:** `src/nodes/rrb.rs` (1117 lines), `src/vector/mod.rs` (2916 lines)
**Backs:** `Vector<A>`

### VectorInner — 3-tier representation

`GenericVector<A, P>` wraps `VectorInner<A, P>` (line 156):

```rust
enum VectorInner<A, P: SharedPointerKind> {
    Inline(InlineArray<A, RRB<A, P>>),  // stack-allocated, ≤ CHUNK_SIZE elements
    Single(SharedPointer<Chunk<A>, P>), // one heap-allocated chunk
    Full(RRB<A, P>),                    // full RRB tree
}
```

`Inline` stores elements directly in the space that an `RRB` struct would
occupy (union-like layout via `InlineArray`). Promotion to `Single` happens
when the inline array fills or reaches `CHUNK_SIZE`. Promotion to `Full`
happens when the single chunk fills.

### RRB struct — 4-buffer + middle tree

The `RRB<A, P>` struct (line 163) uses a finger-tree-like structure with
4 buffers flanking a central tree:

```
┌─────────┬─────────┬─────────────────┬─────────┬─────────┐
│ outer_f │ inner_f │     middle      │ inner_b │ outer_b │
└─────────┴─────────┴─────────────────┴─────────┴─────────┘
```

```rust
pub struct RRB<A, P: SharedPointerKind> {
    length: usize,
    middle_level: usize,
    outer_f: SharedPointer<Chunk<A>, P>,
    inner_f: SharedPointer<Chunk<A>, P>,
    middle: SharedPointer<Node<A, P>, P>,
    inner_b: SharedPointer<Chunk<A>, P>,
    outer_b: SharedPointer<Chunk<A>, P>,
}
```

- `outer_f` / `outer_b` are the outermost buffers (amortise push_front/push_back)
- `inner_f` / `inner_b` are inner buffers (overflow from outer)
- `middle` is the central RRB tree of `Node`s
- `middle_level` tracks the tree height

### Node and Entry (rrb.rs)

```rust
pub(crate) struct Node<A, P: SharedPointerKind> {
    children: Entry<A, P>,
}

enum Entry<A, P: SharedPointerKind> {
    Nodes(Size<P>, SharedPointer<Chunk<Node<A, P>>, P>),
    Values(SharedPointer<Chunk<A>, P>),
    Empty,
}
```

- `Values` — leaf node containing a chunk of elements
- `Nodes` — internal node containing child nodes + size tracking
- `Empty` — sentinel for uninitialised middle trees

### Size tracking

```rust
enum Size<P: SharedPointerKind> {
    Size(usize),                          // dense: all children are full
    Table(SharedPointer<Chunk<usize>, P>), // relaxed: cumulative size table
}
```

A dense node (`Size::Size`) has all children of equal (maximum) size, so
child lookup is O(1) division. A relaxed node (`Size::Table`) stores a
cumulative size array and requires O(log n) binary search for indexing.
Relaxed nodes arise from concatenation.

### Concatenation — Stucki's algorithm

The current concatenation implements Stucki et al.'s algorithm (ICFP 2015).
Known issue: repeated concatenation produces excessively deep trees — height
7 for ~40K elements where height 3 should suffice. Item 2.1 replaces this
with L'orange's algorithm.

### Configuration

- `CHUNK_SIZE` (from `config.rs`): 64 (or 4 with `small-chunks`)
- `NODE_SIZE` in rrb.rs: equals `CHUNK_SIZE`

---

## B+ tree {#btree}

**File:** `src/nodes/btree.rs` (1327 lines)
**Backs:** `OrdMap<K, V>`, `OrdSet<A>`

### Node types

```rust
enum Node<K, V, P: SharedPointerKind> {
    Branch(SharedPointer<Branch<K, V, P>, P>),
    Leaf(SharedPointer<Leaf<K, V>, P>),
}
```

**Branch** (line 108):
```rust
struct Branch<K, V, P: SharedPointerKind> {
    keys: Chunk<K, NODE_SIZE>,
    children: Children<K, V, P>,
}

enum Children<K, V, P: SharedPointerKind> {
    Leaves { leaves: Chunk<SharedPointer<Leaf<K, V>, P>, NUM_CHILDREN> },
    Branches { branches: Chunk<SharedPointer<Branch<K, V, P>, P>, NUM_CHILDREN>,
               level: NonZeroUsize },
}
```

**Leaf** (line 259):
```rust
struct Leaf<K, V> {
    keys: Chunk<(K, V), NODE_SIZE>,
}
```

Keys and values are stored together in leaves as `(K, V)` tuples (not
separated as in some B+ tree variants). Branch nodes store only keys for
navigation, with children in a separate `Children` enum that tracks whether
children are leaves or branches.

### Constants

- `NODE_SIZE`: 16 (or 6 with `small-chunks`) — from `ORD_CHUNK_SIZE`
- `MEDIAN`: `NODE_SIZE / 2` = 8 — minimum keys for non-root branches
- `THIRD`: `NODE_SIZE / 3` = 5 — minimum keys for non-root leaves
- `NUM_CHILDREN`: `NODE_SIZE + 1` = 17

### Invariants

Branch (documented in source, line 100):
- Keys are ordered and unique
- `keys.len() + 1 == children.len()`
- All children have level = branch level - 1
- Root branch: ≥ 1 key; non-root branch: ≥ `MEDIAN - 1` keys

Leaf (line 255):
- Keys are ordered and unique
- Root leaf: ≥ 0 keys; non-root leaf: ≥ `THIRD` keys

### Cursor

`Cursor<'a, K, V, P>` (line 1073) provides stack-based tree navigation:

```rust
struct Cursor<'a, K, V, P: SharedPointerKind> {
    stack: Vec<(usize, &'a Branch<K, V, P>)>,
    leaf: Option<(usize, &'a Leaf<K, V>)>,
}
```

The stack stores `(child_index, branch_ref)` pairs from root to current
position. Navigation methods push/pop the stack to move between nodes.
Used by `Iter`, `get_next`, `get_prev`, and range queries.

### Binary search

A custom `binary_search_by` (line 1279) optimises for non-trivial
comparison functions. For small non-Drop types (≤ 16 bytes), it defers to
the stdlib's branchless implementation. For larger types (e.g. string keys),
it uses an early-return loop that minimises comparisons.

---

## Focus and FocusMut {#focus}

**File:** `src/vector/focus.rs` (1007 lines)
**Used by:** `Vector<A>` iteration and indexed access

Focus and FocusMut are zipper-like cursors that cache the last-accessed
tree leaf for efficient sequential access. They are the performance-critical
path for `Vector` iteration and contain the densest unsafe code in the
crate.

### Focus (immutable)

```rust
pub enum Focus<'a, A, P: SharedPointerKind> {
    Single(&'a Chunk<A>),
    Full(TreeFocus<A, P>),
}
```

`TreeFocus` (line 276):
```rust
struct TreeFocus<A, P: SharedPointerKind> {
    tree: RRB<A, P>,              // cloned from the Vector
    view: Range<usize>,           // visible range (for narrow/split_at)
    middle_range: Range<usize>,   // range covered by the middle tree
    target_range: Range<usize>,   // range of the cached chunk
    target_ptr: *const Chunk<A>,  // raw pointer to cached chunk
}
```

`target_ptr` is a raw pointer to the most recently accessed chunk. It
avoids re-traversing the tree for adjacent index lookups. The pointer is
valid because `tree` (a clone of the Vector's RRB) keeps all nodes alive.

Manual `Send`/`Sync` impls (lines 306-307) are required because raw
pointers are not automatically `Send`/`Sync`:
```rust
unsafe impl<A: Send, P: SharedPointerKind + Send> Send for TreeFocus<A, P> {}
unsafe impl<A: Sync, P: SharedPointerKind + Sync> Sync for TreeFocus<A, P> {}
```

### FocusMut (mutable)

```rust
pub enum FocusMut<'a, A, P: SharedPointerKind> {
    Single(&'a mut [A]),
    Full(TreeFocusMut<'a, A, P>),
}
```

`TreeFocusMut` (line 808):
```rust
struct TreeFocusMut<'a, A, P: SharedPointerKind> {
    tree: Lock<&'a mut RRB<A, P>>,  // mutex-wrapped mutable reference
    view: Range<usize>,
    middle_range: Range<usize>,
    target_range: Range<usize>,
    target_ptr: AtomicPtr<Chunk<A>>, // atomic pointer to cached chunk
}
```

The mutable variant uses a `Lock` (mutex) around the tree reference because
`split_at` can create multiple `TreeFocusMut`s over disjoint ranges of the
same tree. Each sub-focus locks the tree when it needs to traverse to a
new chunk, making the pointer swap atomic.

`target_ptr` is an `AtomicPtr` rather than a raw pointer. The source
comments note uncertainty about whether atomicity is actually needed
(line 823: "Not actually sure why this needs to be an atomic").

### Chunk access pattern

Both Focus variants follow the same pattern:

1. Check if `index` falls within `target_range`
2. If yes, index directly into the cached chunk via the raw pointer
3. If no, traverse the tree to find the new chunk, update `target_ptr`
   and `target_range`

This gives O(1) access for sequential/local patterns and O(log n) for
random access (amortised over a scan).

---

## Unsafe inventory {#unsafe-inventory}

Summary of all `unsafe` code in the core modules. The crate root has
`#![deny(unsafe_code)]`; only `vector/mod.rs` has a module-level
`#![allow(unsafe_code)]`. Other modules use inline `#[allow(unsafe_code)]`
on specific functions.

### nodes/hamt.rs — 4 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| 62-77 | `node_with` | `UnsafeCell` + `transmute_copy` for zero-copy `SharedPointer` construction. Avoids memcpy for large node types. |
| 237-238 | `get_mut` | Reborrow `&mut self` through raw pointer to work around borrow checker limitation in SIMD probe loop. |
| 495-560 | `insert` | `ptr::read` + `ptr::write` for in-place entry replacement without intermediate drop/clone. |

### nodes/btree.rs — 3 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| 1307 | `get_unchecked(mid)` | Skip bounds check in binary search hot path. Guarded by loop invariant `mid < slice.len()`. |
| 1315-1317 | `assert_unchecked(mid < slice.len())` | Hint to optimiser after early return on `Equal`. |
| 1322-1324 | `assert_unchecked(low <= slice.len())` | Hint to optimiser for return value. |

All three are in the custom `binary_search_by` function. For small
non-Drop keys (≤ 16 bytes), the function delegates to stdlib instead,
so the unsafe path only runs for expensive-to-compare types.

### nodes/rrb.rs — 0 unsafe sites

The RRB node module is entirely safe Rust.

### vector/mod.rs — 9 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| 46 | `#![allow(unsafe_code)]` | Module-level allow (only module with this). |
| 868 | `swap` | Pointer-based swap in `GenericVector::swap`. |
| 2036, 2057 | `Iter` methods | Self-referential focus cast: `&mut Focus` → raw pointer → `&'a mut Focus`. Extends the borrow lifetime to match the iterator's lifetime. |
| 2112, 2136 | `IterMut` methods | Same pattern for `FocusMut`. |
| 2216, 2232 | `Chunks` methods | Same pattern for chunk iteration. |
| 2273, 2289 | `ChunksMut` methods | Same pattern for mutable chunk iteration. |

The 8 iterator casts all follow the same pattern: they cast a `&mut self`
field to a raw pointer and back with a longer lifetime, because Rust's
borrow checker cannot express that the iterator borrows from itself.

### vector/focus.rs — 6 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| 306 | `unsafe impl Send for TreeFocus` | Raw `target_ptr` is not auto-Send. Safe because the pointer is derived from `tree` which is Send. |
| 307 | `unsafe impl Sync for TreeFocus` | Same reasoning for Sync. |
| 400 | `&*self.target_ptr` | Dereference cached raw pointer to chunk. Safe because `tree` keeps the chunk alive. |
| 660 | `&mut *chunk.add(index)` | Index into chunk via raw pointer in `get_many_mut`. Bounds checked by `check_indices`. |
| 898 | `&mut *self.target_ptr.load(Relaxed)` | Dereference `AtomicPtr` in `TreeFocusMut`. The `Lock` on `tree` ensures exclusive access. |
| 979-983 | `get_many` | Raw pointer arithmetic for multi-index access. Bounds checked by `check_indices`. |

### shared_ptr.rs — 0 unsafe sites

Pure re-export module.

---

## Cross-references

- Decision log: `docs/decisions.md`
- Glossary: `docs/glossary.md`
- Implementation plan: `docs/impl-plan.md`
- References (papers, implementations): `docs/references.md`
