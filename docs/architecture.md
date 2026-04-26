# Architecture {#sec:architecture}

Internal architecture of pds's core data structure modules. This document
covers the current implementation as of v1.0.0 (~10K lines across 6 core
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

pds provides twenty persistent collection types. The five core types each
have their own backing structure; the remaining fifteen are derived types
built on top of the core five:

| Type | Backing structure | Module | Lines |
|------|-------------------|--------|-------|
| `Vector<A>` | RRB tree | `vector/mod.rs`, `nodes/rrb.rs` | ~6K |
| `HashMap<K, V>` | SIMD HAMT | `nodes/hamt.rs` | ~1.3K |
| `HashSet<A>` | SIMD HAMT (via HashMap) | `nodes/hamt.rs` | (shared) |
| `OrdMap<K, V>` | B+ tree | `nodes/btree.rs` | ~1.9K |
| `OrdSet<A>` | B+ tree (via OrdMap) | `nodes/btree.rs` | (shared) |

Derived types (`Bag`, `OrdBag`, `HashMultiMap`, `OrdMultiMap`, `BiMap`,
`OrdBiMap`, `SymMap`, `OrdSymMap`, `Trie`, `OrdTrie`, `InsertionOrderMap`,
`InsertionOrderSet`, `OrdInsertionOrderMap`, `OrdInsertionOrderSet`,
`UniqueVector`) delegate to the core five internally. See `src/*.rs` for
their implementations.

All collections are generic over `SharedPointerKind` (see @sec:shared-pointer),
enabling copy-on-write via `SharedPointer::make_mut`. The concrete pointer
type is selected by the `DefaultSharedPtr` alias in `shared_ptr.rs`.

---

## SharedPointer abstraction {#shared-pointer}

**File:** `src/shared_ptr.rs` (~32 lines — re-export shim)

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

**File:** `src/nodes/hamt.rs` (~1330 lines)
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

The `Entry` enum (line ~757) has 5 variants:

```rust
enum Entry<A, P: SharedPointerKind, H: HashWidth = u64> {
    HamtNode(SharedPointer<HamtNode<A, P, H>, P>),
    SmallSimdNode(SharedPointer<SmallSimdNode<A, H>, P>),
    LargeSimdNode(SharedPointer<LargeSimdNode<A, H>, P>),
    Value(A, H),
    Collision(SharedPointer<CollisionNode<A, H>, P>),
}
```

`HamtNode.data` is a `SparseChunk<Entry<A, P>, HASH_WIDTH>` — a
bitmap-indexed array that can hold any `Entry` variant, including child
nodes. SIMD nodes store only `SparseChunk<(A, HashBits), WIDTH>` — flat
value arrays with no children.

### SIMD lookup

Lookup uses `wide::u8x16` for parallel byte comparison:

1. Compute the control byte — the top 8 bits of the hash, clamped to `≥ 1`
   (0 means empty slot). (`HashWidth::ctrl_byte()` in `hash_width.rs`)
2. Determine which SIMD group to search (for `LargeSimdNode` with 2
   groups, the group is selected by hash bits). (`HashWidth::ctrl_group()`
   in `hash_width.rs`)
3. SIMD `cmp_eq` + `move_mask` produces a bitmap of matching slots.
   (`group_find`, line ~69 in `nodes/hamt.rs`)
4. Iterate matches, verify with full key equality.

### hash_may_eq optimisation

`hash_may_eq` (line ~894) skips the hash equality check for small,
non-Drop keys (≤ 16 bytes). For these types, the key comparison itself
is cheap enough that the extra hash comparison is wasted work:

```rust
fn hash_may_eq<A: HashValue, H: HashWidth>(hash: H, other_hash: H) -> bool {
    (!mem::needs_drop::<A::Key>() && mem::size_of::<A::Key>() <= 16) || hash == other_hash
}
```

### node_with — zero-copy construction

`node_with` (line ~78) constructs a `SharedPointer<T>` without extra
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

**Files:** `src/nodes/rrb.rs` (~1327 lines), `src/vector/mod.rs` (~4737 lines)
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

**File:** `src/nodes/btree.rs` (~1936 lines)
**Backs:** `OrdMap<K, V>`, `OrdSet<A>`

### Node types

```rust
enum Node<K, V, P: SharedPointerKind> {
    Branch(SharedPointer<Branch<K, V, P>, P>),
    Leaf(SharedPointer<Leaf<K, V>, P>),
}
```

**Branch** (line ~126):
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

**Leaf** (line ~277):
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

- `NODE_SIZE`: 32 (or 6 with `small-chunks`) — from `ORD_CHUNK_SIZE`
- `MEDIAN`: `NODE_SIZE / 2` = 16 — minimum keys for non-root branches
- `THIRD`: `NODE_SIZE / 3` = 10 — minimum keys for non-root leaves
- `NUM_CHILDREN`: `NODE_SIZE + 1` = 33

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

`Cursor<'a, K, V, P>` (line ~1140) provides stack-based tree navigation:

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

A custom `binary_search_by` (line ~1883) optimises for non-trivial
comparison functions. For small non-Drop types (≤ 16 bytes), it defers to
the stdlib's branchless implementation. For larger types (e.g. string keys),
it uses an early-return loop that minimises comparisons.

### Parallel bulk operations — join algorithm

The parallel set operations on `OrdMap` / `OrdSet` (`par_union`,
`par_intersection`, `par_difference`, `par_symmetric_difference` in
`src/ord/rayon.rs`) use the join algorithm of Blelloch et al.:

- **"Joinable Parallel Balanced Binary Trees"** (ACM TOPC 2022,
  doi:10.1145/3512769) — foundational formalisation of split + join
  as a single primitive for work-efficient parallel set operations on
  balanced BSTs.
- **"PaC-trees: Supporting Parallel and Compressed Purely-Functional
  Collections Using Joinable Trees"** (PLDI 2022, doi:10.1145/3519939.3523733)
  — extends the approach to blocked-leaf trees structurally similar to pds's
  B+ tree.

**Algorithm sketch:**

```
par_union(self, other):
  if either is small: fall back to sequential union
  pivot = self.root_pivot_key()          // median key from root — O(1)
  (l1, v, r1) = split_node(self, pivot)  // O(log n) structural split
  (l2, _, r2) = split_node(other, pivot)
  (rl, rr) = rayon::join(
      || par_union(l1, l2),
      || par_union(r1, r2),
  )
  concat_ordered(rl, insert(rr, pivot, v))
```

`split_node` walks the spine from root to leaf in O(log n), collecting
left/right halves at each level. `concat_node` rebuilds the spine in
O(log n) using height-aware insertion at the correct level.

**Complexity:**
- Work: O(m log(n/m + 2)) for inputs of size m ≤ n
- Span: O(log² n)

This is believed to be the first implementation of this algorithm on a
blocked-leaf persistent B+ tree. The HAMT parallel ops use a different
approach (filter + fold/reduce) that does not achieve the same span bound.
See `src/ord/rayon.rs` for the implementation.

---

## Focus and FocusMut {#focus}

**File:** `src/vector/focus.rs` (~1031 lines)
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

`TreeFocusMut` (line ~820):
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

### nodes/hamt.rs — 2 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| ~78–94 | `node_with` | `UnsafeCell` + `transmute_copy` for zero-copy `SharedPointer` construction. Avoids memcpy for large node types. |
| ~271–290 | `SmallSimdNode::get_mut` | Reborrow `&mut self` through raw pointer to work around borrow checker limitation in SIMD probe loop. |

### nodes/btree.rs — 3 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| ~1912 | `get_unchecked(mid)` | Skip bounds check in binary search hot path. Guarded by loop invariant `mid < slice.len()`. |
| ~1922 | `assert_unchecked(mid < slice.len())` | Hint to optimiser after early return on `Equal`. |
| ~1931 | `assert_unchecked(low <= slice.len())` | Hint to optimiser for return value. |

All three are in the custom `binary_search_by` function. For small
non-Drop keys (≤ 16 bytes), the function delegates to stdlib instead,
so the unsafe path only runs for expensive-to-compare types.

### nodes/rrb.rs — 0 unsafe sites

The RRB node module is entirely safe Rust.

### vector/mod.rs — multiple unsafe sites

The module has `#![allow(unsafe_code)]` at line ~64 (only module with this).

All unsafe blocks follow one of two patterns:

1. **Self-referential focus cast** — `Iter`, `IterMut`, `Chunks`, `ChunksMut`,
   and `Drain` iterators cast `&mut self.focus` to a raw pointer and back
   with a longer lifetime, because Rust's borrow checker cannot express that
   the iterator borrows from itself. There are approximately 12 sites of
   this pattern across the iterator implementations.

2. **Pointer-based swap** — `GenericVector::swap` uses raw pointer arithmetic
   for an element swap operation.

Line numbers are omitted here; the module has grown to ~4737 lines and
exact references go stale quickly. Search for `unsafe {` in `vector/mod.rs`
to locate all sites.

### vector/focus.rs — 6 unsafe sites

| Line | Code | Purpose |
|------|------|---------|
| ~310 | `unsafe impl Send for TreeFocus` | Raw `target_ptr` is not auto-Send. Safe because the pointer is derived from `tree` which is Send. |
| ~311 | `unsafe impl Sync for TreeFocus` | Same reasoning for Sync. |
| ~411 | `&*self.target_ptr` | Dereference cached raw pointer to chunk. Safe because `tree` keeps the chunk alive. |
| ~672 | `&mut *chunk.add(index)` | Index into chunk via raw pointer in `get_many_mut`. Bounds checked by `check_indices`. |
| ~919 | `&mut *self.target_ptr.load(Relaxed)` | Dereference `AtomicPtr` in `TreeFocusMut`. The `Lock` on `tree` ensures exclusive access. |
| ~1003 | `get_many` | Raw pointer arithmetic for multi-index access. Bounds checked by `check_indices`. |

### shared_ptr.rs — 0 unsafe sites

Pure re-export module.

---

## Cross-references

- Decision log: `docs/decisions.md`
- Glossary: `docs/glossary.md`
- Implementation plan: `docs/impl-plan.md`
- References (papers, implementations): `docs/references.md`
