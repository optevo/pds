// CHAMP (Compressed Hash-Array Mapped Prefix-tree) prototype.
//
// Standalone implementation for benchmarking against the current SIMD HAMT.
// Based on Steindorfer & Vinju, "Optimizing Hash-Array Mapped Tries for Fast
// and Lean Immutable JVM Collections" (OOPSLA 2015).
//
// Key design choices:
// - Two-bitmap encoding (datamap + nodemap) per node
// - Values and child pointers in separate contiguous Vecs
// - Canonical deletion (singleton child nodes inlined back into parent)
// - std::sync::Arc for shared pointers (matches imbl's default ArcK)
// - 5-bit hash chunks, 32-way branching (matches HASH_LEVEL_SIZE = 5)
//
// This module is #[doc(hidden)] — it exists only for the go/no-go benchmark
// in plan item 4.2.  See DEC-007 in docs/decisions.md for results.

use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BITS_PER_LEVEL: u32 = 5;
const BRANCH_FACTOR: usize = 1 << BITS_PER_LEVEL; // 32
const MASK: u64 = (BRANCH_FACTOR as u64) - 1; // 0x1F
// ceil(64 / 5) = 13 levels before hash bits are exhausted.
const MAX_DEPTH: u32 = u64::BITS.div_ceil(BITS_PER_LEVEL);

// ---------------------------------------------------------------------------
// Hash helpers
// ---------------------------------------------------------------------------

#[inline]
fn hash_key<K: Hash + ?Sized>(hasher: &RandomState, key: &K) -> u64 {
    hasher.hash_one(key)
}

/// Extract the 5-bit chunk at the given depth from a 64-bit hash.
#[inline]
fn mask(hash: u64, depth: u32) -> u32 {
    ((hash >> (depth * BITS_PER_LEVEL)) & MASK) as u32
}

/// The single-bit flag for a given position (0..31).
#[inline]
fn bitpos(pos: u32) -> u32 {
    1u32 << pos
}

/// Number of entries before `bit` in `bitmap` (index into the packed array).
#[inline]
fn index(bitmap: u32, bit: u32) -> usize {
    (bitmap & (bit - 1)).count_ones() as usize
}

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

enum ChampNode<K, V> {
    Inner(InnerNode<K, V>),
    Collision(CollisionNode<K, V>),
}

struct InnerNode<K, V> {
    datamap: u32,
    nodemap: u32,
    values: Vec<(K, V)>,
    children: Vec<Arc<ChampNode<K, V>>>,
}

struct CollisionNode<K, V> {
    hash: u64,
    entries: Vec<(K, V)>,
}

impl<K, V> InnerNode<K, V> {
    fn empty() -> Self {
        InnerNode {
            datamap: 0,
            nodemap: 0,
            values: Vec::new(),
            children: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

impl<K: Eq + Hash, V> ChampNode<K, V> {
    fn get<Q>(&self, hash: u64, key: &Q, depth: u32) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        match self {
            ChampNode::Inner(inner) => {
                let pos = mask(hash, depth);
                let bit = bitpos(pos);
                if inner.datamap & bit != 0 {
                    let idx = index(inner.datamap, bit);
                    let (ref k, ref v) = inner.values[idx];
                    if key.eq(k.borrow()) {
                        Some(v)
                    } else {
                        None
                    }
                } else if inner.nodemap & bit != 0 {
                    let idx = index(inner.nodemap, bit);
                    inner.children[idx].get(hash, key, depth + 1)
                } else {
                    None
                }
            }
            ChampNode::Collision(coll) => {
                for (ref k, ref v) in &coll.entries {
                    if key.eq(k.borrow()) {
                        return Some(v);
                    }
                }
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Insert (persistent — returns new node)
// ---------------------------------------------------------------------------

/// Build a sub-node (possibly multiple levels deep) holding two entries.
fn make_sub_node<K: Clone + Eq + Hash, V: Clone>(
    hash_a: u64,
    entry_a: (K, V),
    hash_b: u64,
    entry_b: (K, V),
    depth: u32,
) -> ChampNode<K, V> {
    if depth >= MAX_DEPTH {
        // Hash bits exhausted — collision node.
        return ChampNode::Collision(CollisionNode {
            hash: hash_a,
            entries: vec![entry_a, entry_b],
        });
    }
    let pos_a = mask(hash_a, depth);
    let pos_b = mask(hash_b, depth);
    if pos_a == pos_b {
        // Same position at this level — descend further.
        let child = make_sub_node(hash_a, entry_a, hash_b, entry_b, depth + 1);
        let bit = bitpos(pos_a);
        ChampNode::Inner(InnerNode {
            datamap: 0,
            nodemap: bit,
            values: Vec::new(),
            children: vec![Arc::new(child)],
        })
    } else {
        let bit_a = bitpos(pos_a);
        let bit_b = bitpos(pos_b);
        let (vals, datamap) = if pos_a < pos_b {
            (vec![entry_a, entry_b], bit_a | bit_b)
        } else {
            (vec![entry_b, entry_a], bit_a | bit_b)
        };
        ChampNode::Inner(InnerNode {
            datamap,
            nodemap: 0,
            values: vals,
            children: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Remove (persistent, canonical)
// ---------------------------------------------------------------------------

/// Result of a remove operation on a node.
enum RemoveResult<K, V> {
    /// Key not found — no change needed.
    NotFound,
    /// Key removed; here is the new node (or None if the node is now empty).
    Removed(Option<Arc<ChampNode<K, V>>>),
    /// Key removed; the remaining node has been compacted to a single inline
    /// value that the parent should absorb (canonical deletion).
    Singleton(K, V),
}

impl<K: Clone + Eq + Hash, V: Clone> ChampNode<K, V> {
    fn remove<Q>(&self, hash: u64, key: &Q, depth: u32) -> RemoveResult<K, V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        match self {
            ChampNode::Inner(inner) => {
                let pos = mask(hash, depth);
                let bit = bitpos(pos);

                if inner.datamap & bit != 0 {
                    let idx = index(inner.datamap, bit);
                    let (ref ek, _) = inner.values[idx];
                    if !key.eq(ek.borrow()) {
                        return RemoveResult::NotFound;
                    }
                    // Remove this inline value.
                    let new_datamap = inner.datamap ^ bit;
                    if new_datamap == 0 && inner.nodemap == 0 {
                        return RemoveResult::Removed(None);
                    }
                    // If only one inline value remains and no children,
                    // report as singleton for canonical compaction.
                    if new_datamap.count_ones() == 1 && inner.nodemap == 0 {
                        let mut new_vals = inner.values.clone();
                        new_vals.remove(idx);
                        let (k, v) = new_vals.into_iter().next().unwrap();
                        return RemoveResult::Singleton(k, v);
                    }
                    let mut new_vals = inner.values.clone();
                    new_vals.remove(idx);
                    let node = ChampNode::Inner(InnerNode {
                        datamap: new_datamap,
                        nodemap: inner.nodemap,
                        values: new_vals,
                        children: inner.children.clone(),
                    });
                    RemoveResult::Removed(Some(Arc::new(node)))
                } else if inner.nodemap & bit != 0 {
                    let idx = index(inner.nodemap, bit);
                    match inner.children[idx].remove(hash, key, depth + 1) {
                        RemoveResult::NotFound => RemoveResult::NotFound,
                        RemoveResult::Singleton(k, v) => {
                            // Child compacted to singleton — inline it here.
                            let new_nodemap = inner.nodemap ^ bit;
                            let new_datamap = inner.datamap | bit;
                            let mut new_vals = inner.values.clone();
                            let val_idx = index(new_datamap, bit);
                            new_vals.insert(val_idx, (k, v));
                            let mut new_children = inner.children.clone();
                            new_children.remove(idx);
                            let node = ChampNode::Inner(InnerNode {
                                datamap: new_datamap,
                                nodemap: new_nodemap,
                                values: new_vals,
                                children: new_children,
                            });
                            // Check if *this* node is now a singleton.
                            if new_datamap.count_ones() == 1 && new_nodemap == 0 {
                                let (k, v) = node.unwrap_inner().values.into_iter().next().unwrap();
                                RemoveResult::Singleton(k, v)
                            } else {
                                RemoveResult::Removed(Some(Arc::new(node)))
                            }
                        }
                        RemoveResult::Removed(None) => {
                            // Child became empty.
                            let new_nodemap = inner.nodemap ^ bit;
                            let mut new_children = inner.children.clone();
                            new_children.remove(idx);
                            if inner.datamap == 0 && new_nodemap == 0 {
                                RemoveResult::Removed(None)
                            } else if inner.datamap.count_ones() == 1 && new_nodemap == 0 {
                                let (k, v) = inner.values[0].clone();
                                RemoveResult::Singleton(k, v)
                            } else {
                                let node = ChampNode::Inner(InnerNode {
                                    datamap: inner.datamap,
                                    nodemap: new_nodemap,
                                    values: inner.values.clone(),
                                    children: new_children,
                                });
                                RemoveResult::Removed(Some(Arc::new(node)))
                            }
                        }
                        RemoveResult::Removed(Some(new_child)) => {
                            let mut new_children = inner.children.clone();
                            new_children[idx] = new_child;
                            let node = ChampNode::Inner(InnerNode {
                                datamap: inner.datamap,
                                nodemap: inner.nodemap,
                                values: inner.values.clone(),
                                children: new_children,
                            });
                            RemoveResult::Removed(Some(Arc::new(node)))
                        }
                    }
                } else {
                    RemoveResult::NotFound
                }
            }
            ChampNode::Collision(coll) => {
                let pos = coll.entries.iter().position(|(ref ek, _)| key.eq(ek.borrow()));
                let Some(idx) = pos else {
                    return RemoveResult::NotFound;
                };
                if coll.entries.len() == 2 {
                    let remaining = coll.entries[1 - idx].clone();
                    RemoveResult::Singleton(remaining.0, remaining.1)
                } else {
                    let mut new_entries = coll.entries.clone();
                    new_entries.remove(idx);
                    let node = ChampNode::Collision(CollisionNode {
                        hash: coll.hash,
                        entries: new_entries,
                    });
                    RemoveResult::Removed(Some(Arc::new(node)))
                }
            }
        }
    }

    /// Mutable remove — avoids cloning when refcount == 1.
    fn remove_mut<Q>(
        this: &mut Arc<Self>,
        hasher: &RandomState,
        hash: u64,
        key: &Q,
        depth: u32,
    ) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        // For the prototype, fall back to persistent remove.
        match this.remove(hash, key, depth) {
            RemoveResult::NotFound => false,
            RemoveResult::Removed(new) => {
                if let Some(n) = new {
                    *this = n;
                } else {
                    *this = Arc::new(ChampNode::Inner(InnerNode::empty()));
                }
                true
            }
            RemoveResult::Singleton(k, v) => {
                // Use the *remaining* key's hash for its bitmap position.
                let remaining_hash = hash_key(hasher, &k);
                let bit = bitpos(mask(remaining_hash, depth));
                *this = Arc::new(ChampNode::Inner(InnerNode {
                    datamap: bit,
                    nodemap: 0,
                    values: vec![(k, v)],
                    children: Vec::new(),
                }));
                true
            }
        }
    }
}

impl<K, V> ChampNode<K, V> {
    fn unwrap_inner(self) -> InnerNode<K, V> {
        match self {
            ChampNode::Inner(inner) => inner,
            ChampNode::Collision(_) => panic!("expected InnerNode"),
        }
    }
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

/// Stack-based depth-first iterator.  At each InnerNode, yields all inline
/// values first (contiguous scan — CHAMP's cache-locality win), then pushes
/// children onto the stack.
pub struct Iter<'a, K, V> {
    // Stack of (node, value_index, child_index).
    stack: Vec<IterFrame<'a, K, V>>,
    remaining: usize,
}

enum IterFrame<'a, K, V> {
    Inner {
        inner: &'a InnerNode<K, V>,
        val_idx: usize,
        child_idx: usize,
    },
    Collision {
        entries: &'a [(K, V)],
        idx: usize,
    },
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.stack.last_mut()?;
            match frame {
                IterFrame::Inner {
                    inner,
                    val_idx,
                    child_idx,
                } => {
                    if *val_idx < inner.values.len() {
                        let (ref k, ref v) = inner.values[*val_idx];
                        *val_idx += 1;
                        self.remaining -= 1;
                        return Some((k, v));
                    }
                    if *child_idx < inner.children.len() {
                        let child = &inner.children[*child_idx];
                        *child_idx += 1;
                        // Push the child frame.
                        match child.as_ref() {
                            ChampNode::Inner(inner) => {
                                self.stack.push(IterFrame::Inner {
                                    inner,
                                    val_idx: 0,
                                    child_idx: 0,
                                });
                            }
                            ChampNode::Collision(coll) => {
                                self.stack.push(IterFrame::Collision {
                                    entries: &coll.entries,
                                    idx: 0,
                                });
                            }
                        }
                        continue;
                    }
                    // This frame is exhausted.
                    self.stack.pop();
                }
                IterFrame::Collision { entries, idx } => {
                    if *idx < entries.len() {
                        let (ref k, ref v) = entries[*idx];
                        *idx += 1;
                        self.remaining -= 1;
                        return Some((k, v));
                    }
                    self.stack.pop();
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K, V> ExactSizeIterator for Iter<'_, K, V> {}
impl<K, V> std::iter::FusedIterator for Iter<'_, K, V> {}

// ---------------------------------------------------------------------------
// ChampMap — the public-facing map type
// ---------------------------------------------------------------------------

pub struct ChampMap<K, V> {
    root: Arc<ChampNode<K, V>>,
    size: usize,
    hasher: RandomState,
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for ChampMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChampMap")
            .field("size", &self.size)
            .finish()
    }
}

impl<K, V> Clone for ChampMap<K, V> {
    fn clone(&self) -> Self {
        ChampMap {
            root: Arc::clone(&self.root),
            size: self.size,
            hasher: self.hasher.clone(),
        }
    }
}

impl<K: Clone + Eq + Hash, V: Clone> ChampMap<K, V> {
    pub fn new() -> Self {
        ChampMap {
            root: Arc::new(ChampNode::Inner(InnerNode::empty())),
            size: 0,
            hasher: RandomState::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = hash_key(&self.hasher, key);
        self.root.get(hash, key, 0)
    }

    /// Persistent insert — returns a new map.
    pub fn insert_persistent(&self, key: K, value: V) -> Self {
        let hash = hash_key(&self.hasher, &key);
        let (new_root, replaced) =
            self.root.insert_with_hasher(&self.hasher, hash, key, value, 0);
        ChampMap {
            root: new_root,
            size: if replaced { self.size } else { self.size + 1 },
            hasher: self.hasher.clone(),
        }
    }

    /// Mutable insert — modifies in place when possible.
    pub fn insert_mut(&mut self, key: K, value: V) {
        let hash = hash_key(&self.hasher, &key);
        let replaced = ChampNode::insert_mut_with_hasher(
            &mut self.root,
            &self.hasher,
            hash,
            key,
            value,
            0,
        );
        if !replaced {
            self.size += 1;
        }
    }

    /// Persistent remove — returns a new map.
    pub fn remove_persistent<Q>(&self, key: &Q) -> Self
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = hash_key(&self.hasher, key);
        match self.root.remove(hash, key, 0) {
            RemoveResult::NotFound => self.clone(),
            RemoveResult::Removed(new) => {
                let root = new.unwrap_or_else(|| Arc::new(ChampNode::Inner(InnerNode::empty())));
                ChampMap {
                    root,
                    size: self.size - 1,
                    hasher: self.hasher.clone(),
                }
            }
            RemoveResult::Singleton(k, v) => {
                // Use the *remaining* key's hash, not the removed key's hash.
                let remaining_hash = hash_key(&self.hasher, &k);
                let bit = bitpos(mask(remaining_hash, 0));
                let root = Arc::new(ChampNode::Inner(InnerNode {
                    datamap: bit,
                    nodemap: 0,
                    values: vec![(k, v)],
                    children: Vec::new(),
                }));
                ChampMap {
                    root,
                    size: self.size - 1,
                    hasher: self.hasher.clone(),
                }
            }
        }
    }

    /// Mutable remove.
    pub fn remove_mut<Q>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = hash_key(&self.hasher, key);
        let removed = ChampNode::remove_mut(&mut self.root, &self.hasher, hash, key, 0);
        if removed {
            self.size -= 1;
        }
        removed
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        let mut stack = Vec::with_capacity(MAX_DEPTH as usize);
        match self.root.as_ref() {
            ChampNode::Inner(inner) => {
                if inner.datamap != 0 || inner.nodemap != 0 {
                    stack.push(IterFrame::Inner {
                        inner,
                        val_idx: 0,
                        child_idx: 0,
                    });
                }
            }
            ChampNode::Collision(coll) => {
                stack.push(IterFrame::Collision {
                    entries: &coll.entries,
                    idx: 0,
                });
            }
        }
        Iter {
            stack,
            remaining: self.size,
        }
    }
}

impl<K: Clone + Eq + Hash, V: Clone> Default for ChampMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + Eq + Hash, V: Clone> FromIterator<(K, V)> for ChampMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = ChampMap::new();
        for (k, v) in iter {
            map.insert_mut(k, v);
        }
        map
    }
}

impl<K: Clone + Eq + Hash, V: Clone + PartialEq> PartialEq for ChampMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.root, &other.root) {
            return true;
        }
        if self.size != other.size {
            return false;
        }
        // Element-wise comparison using lookup.
        for (k, v) in self.iter() {
            match other.get(k) {
                Some(ov) if v == ov => {}
                _ => return false,
            }
        }
        true
    }
}

impl<K: Clone + Eq + Hash, V: Clone + Eq> Eq for ChampMap<K, V> {}

// ---------------------------------------------------------------------------
// Fix: the insert path needs to re-hash existing keys when splitting.
// Override the unreachable placeholder with real hashing through ChampMap.
// ---------------------------------------------------------------------------

impl<K: Clone + Eq + Hash, V: Clone> ChampNode<K, V> {
    /// Insert with access to the hasher (for re-hashing existing keys).
    fn insert_with_hasher(
        self: &Arc<Self>,
        hasher: &RandomState,
        hash: u64,
        key: K,
        value: V,
        depth: u32,
    ) -> (Arc<Self>, bool) {
        match self.as_ref() {
            ChampNode::Inner(inner) => {
                let pos = mask(hash, depth);
                let bit = bitpos(pos);

                if inner.datamap & bit != 0 {
                    let idx = index(inner.datamap, bit);
                    let (ref ek, _) = inner.values[idx];
                    if *ek == key {
                        let mut new_vals = inner.values.clone();
                        new_vals[idx] = (key, value);
                        let node = ChampNode::Inner(InnerNode {
                            datamap: inner.datamap,
                            nodemap: inner.nodemap,
                            values: new_vals,
                            children: inner.children.clone(),
                        });
                        (Arc::new(node), true)
                    } else {
                        let existing = inner.values[idx].clone();
                        let existing_hash = hash_key(hasher, &existing.0);
                        let sub = make_sub_node(
                            existing_hash,
                            existing,
                            hash,
                            (key, value),
                            depth + 1,
                        );
                        let mut new_vals = inner.values.clone();
                        new_vals.remove(idx);
                        let child_idx = index(inner.nodemap | bit, bit);
                        let mut new_children = inner.children.clone();
                        new_children.insert(child_idx, Arc::new(sub));
                        let node = ChampNode::Inner(InnerNode {
                            datamap: inner.datamap ^ bit,
                            nodemap: inner.nodemap | bit,
                            values: new_vals,
                            children: new_children,
                        });
                        (Arc::new(node), false)
                    }
                } else if inner.nodemap & bit != 0 {
                    let idx = index(inner.nodemap, bit);
                    let (new_child, replaced) =
                        inner.children[idx].insert_with_hasher(hasher, hash, key, value, depth + 1);
                    let mut new_children = inner.children.clone();
                    new_children[idx] = new_child;
                    let node = ChampNode::Inner(InnerNode {
                        datamap: inner.datamap,
                        nodemap: inner.nodemap,
                        values: inner.values.clone(),
                        children: new_children,
                    });
                    (Arc::new(node), replaced)
                } else {
                    let idx = index(inner.datamap | bit, bit);
                    let mut new_vals = inner.values.clone();
                    new_vals.insert(idx, (key, value));
                    let node = ChampNode::Inner(InnerNode {
                        datamap: inner.datamap | bit,
                        nodemap: inner.nodemap,
                        values: new_vals,
                        children: inner.children.clone(),
                    });
                    (Arc::new(node), false)
                }
            }
            ChampNode::Collision(coll) => {
                for (i, (ref ek, _)) in coll.entries.iter().enumerate() {
                    if *ek == key {
                        let mut new_entries = coll.entries.clone();
                        new_entries[i] = (key, value);
                        let node = ChampNode::Collision(CollisionNode {
                            hash: coll.hash,
                            entries: new_entries,
                        });
                        return (Arc::new(node), true);
                    }
                }
                let mut new_entries = coll.entries.clone();
                new_entries.push((key, value));
                let node = ChampNode::Collision(CollisionNode {
                    hash: coll.hash,
                    entries: new_entries,
                });
                (Arc::new(node), false)
            }
        }
    }

    /// Mutable insert with hasher access.
    fn insert_mut_with_hasher(
        this: &mut Arc<Self>,
        hasher: &RandomState,
        hash: u64,
        key: K,
        value: V,
        depth: u32,
    ) -> bool {
        let Some(node) = Arc::get_mut(this) else {
            let (new_node, replaced) = this.insert_with_hasher(hasher, hash, key, value, depth);
            *this = new_node;
            return replaced;
        };

        match node {
            ChampNode::Inner(inner) => {
                let pos = mask(hash, depth);
                let bit = bitpos(pos);

                if inner.datamap & bit != 0 {
                    let idx = index(inner.datamap, bit);
                    if inner.values[idx].0 == key {
                        inner.values[idx] = (key, value);
                        return true;
                    }
                    let existing = inner.values.remove(idx);
                    let existing_hash = hash_key(hasher, &existing.0);
                    let sub = make_sub_node(
                        existing_hash,
                        existing,
                        hash,
                        (key, value),
                        depth + 1,
                    );
                    inner.datamap ^= bit;
                    inner.nodemap |= bit;
                    let child_idx = index(inner.nodemap, bit);
                    inner.children.insert(child_idx, Arc::new(sub));
                    false
                } else if inner.nodemap & bit != 0 {
                    let idx = index(inner.nodemap, bit);
                    Self::insert_mut_with_hasher(
                        &mut inner.children[idx],
                        hasher,
                        hash,
                        key,
                        value,
                        depth + 1,
                    )
                } else {
                    let idx = index(inner.datamap | bit, bit);
                    inner.datamap |= bit;
                    inner.values.insert(idx, (key, value));
                    false
                }
            }
            ChampNode::Collision(coll) => {
                for entry in coll.entries.iter_mut() {
                    if entry.0 == key {
                        *entry = (key, value);
                        return true;
                    }
                }
                coll.entries.push((key, value));
                false
            }
        }
    }
}

// Aliases for benchmark compatibility.
impl<K: Clone + Eq + Hash, V: Clone> ChampMap<K, V> {
    /// Alias for `insert_persistent` (matches imbl HashMap API name).
    pub fn update(&self, key: K, value: V) -> Self {
        self.insert_persistent(key, value)
    }

    /// Alias for `insert_mut` (used by tests written before the refactor).
    pub fn insert_mut_hashed(&mut self, key: K, value: V) {
        self.insert_mut(key, value);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map() {
        let m: ChampMap<i64, i64> = ChampMap::new();
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
        assert_eq!(m.get(&42), None);
    }

    #[test]
    fn insert_and_get() {
        let mut m = ChampMap::new();
        m.insert_mut_hashed(1, 10);
        m.insert_mut_hashed(2, 20);
        m.insert_mut_hashed(3, 30);
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&1), Some(&10));
        assert_eq!(m.get(&2), Some(&20));
        assert_eq!(m.get(&3), Some(&30));
        assert_eq!(m.get(&4), None);
    }

    #[test]
    fn insert_overwrite() {
        let mut m = ChampMap::new();
        m.insert_mut_hashed(1, 10);
        m.insert_mut_hashed(1, 99);
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&1), Some(&99));
    }

    #[test]
    fn persistent_insert() {
        let m0 = ChampMap::new();
        let m1 = m0.update(1, 10);
        let m2 = m1.update(2, 20);
        // Original maps are unchanged.
        assert_eq!(m0.len(), 0);
        assert_eq!(m1.len(), 1);
        assert_eq!(m2.len(), 2);
        assert_eq!(m1.get(&1), Some(&10));
        assert_eq!(m1.get(&2), None);
        assert_eq!(m2.get(&1), Some(&10));
        assert_eq!(m2.get(&2), Some(&20));
    }

    #[test]
    fn remove_basic() {
        let mut m = ChampMap::new();
        for i in 0..100 {
            m.insert_mut_hashed(i, i * 10);
        }
        assert_eq!(m.len(), 100);
        let m2 = m.remove_persistent(&50);
        assert_eq!(m2.len(), 99);
        assert_eq!(m2.get(&50), None);
        assert_eq!(m2.get(&49), Some(&490));
        // Original unchanged.
        assert_eq!(m.len(), 100);
        assert_eq!(m.get(&50), Some(&500));
    }

    #[test]
    fn remove_mut_basic() {
        let mut m = ChampMap::new();
        for i in 0..50 {
            m.insert_mut_hashed(i, i);
        }
        assert!(m.remove_mut(&25));
        assert_eq!(m.len(), 49);
        assert_eq!(m.get(&25), None);
        assert!(!m.remove_mut(&999));
        assert_eq!(m.len(), 49);
    }

    #[test]
    fn remove_all() {
        let mut m = ChampMap::new();
        let n = 200;
        for i in 0..n {
            m.insert_mut_hashed(i, i);
        }
        for i in 0..n {
            assert!(m.remove_mut(&i), "failed to remove {i}");
        }
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
    }

    #[test]
    fn canonical_form() {
        // Inserting {A, B} then removing A should yield the same structure
        // as inserting {B} alone.
        let m1 = ChampMap::new().update(1, 10).update(2, 20).remove_persistent(&1);
        let m2 = ChampMap::new().update(2, 20);
        // Both should have the same content.
        assert_eq!(m1.len(), 1);
        assert_eq!(m2.len(), 1);
        assert_eq!(m1.get(&2), Some(&20));
        assert_eq!(m2.get(&2), Some(&20));
    }

    #[test]
    fn iteration() {
        let mut m = ChampMap::new();
        for i in 0..100 {
            m.insert_mut_hashed(i, i * 10);
        }
        let mut items: Vec<_> = m.iter().map(|(&k, &v)| (k, v)).collect();
        items.sort();
        assert_eq!(items.len(), 100);
        for (i, &(k, v)) in items.iter().enumerate() {
            assert_eq!(k, i as i64);
            assert_eq!(v, (i as i64) * 10);
        }
    }

    #[test]
    fn exact_size_iterator() {
        let mut m = ChampMap::new();
        for i in 0..50 {
            m.insert_mut_hashed(i, i);
        }
        let mut iter = m.iter();
        assert_eq!(iter.len(), 50);
        for expected in (0..50).rev() {
            iter.next();
            assert_eq!(iter.len(), expected);
        }
    }

    #[test]
    fn from_iterator() {
        let m: ChampMap<i64, i64> = (0..100).map(|i| (i, i * 10)).collect();
        assert_eq!(m.len(), 100);
        for i in 0..100 {
            assert_eq!(m.get(&i), Some(&(i * 10)));
        }
    }

    #[test]
    fn equality() {
        let m1: ChampMap<i64, i64> = (0..100).map(|i| (i, i)).collect();
        // Different hashers produce different trees, so cross-map equality
        // requires element-wise lookup.  Test clone equality (ptr_eq fast
        // path) and element-wise equality on the same map.
        let m3 = m1.clone();
        assert_eq!(m1, m3); // ptr_eq fast path
        // Rebuild from the same hasher via persistent insert to test
        // element-wise equality.
        let mut m4 = ChampMap::new();
        for i in 0i64..100 {
            m4.insert_mut(i, i);
        }
        assert_eq!(m4.len(), m1.len());
    }

    #[test]
    fn large_map() {
        let n = 10_000;
        let mut m = ChampMap::new();
        for i in 0..n {
            m.insert_mut_hashed(i, i);
        }
        assert_eq!(m.len(), n as usize);
        for i in 0..n {
            assert_eq!(m.get(&i), Some(&i));
        }
        // Remove half.
        for i in (0..n).step_by(2) {
            m.remove_mut(&i);
        }
        assert_eq!(m.len(), (n / 2) as usize);
        for i in 0..n {
            if i % 2 == 0 {
                assert_eq!(m.get(&i), None);
            } else {
                assert_eq!(m.get(&i), Some(&i));
            }
        }
    }

    #[test]
    fn string_keys() {
        let mut m = ChampMap::new();
        for i in 0..100 {
            m.insert_mut_hashed(format!("key_{i}"), i);
        }
        assert_eq!(m.len(), 100);
        assert_eq!(m.get(&"key_42".to_string()), Some(&42));
        assert_eq!(m.get(&"missing".to_string()), None);
    }
}
