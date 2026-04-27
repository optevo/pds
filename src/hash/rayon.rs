// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Parallel iterators for HashMap and HashSet.
//!
//! These are only available when using the `rayon` feature flag.

use core::hash::{BuildHasher, Hash};

use ::rayon::iter::plumbing::{bridge_unindexed, Folder, UnindexedConsumer, UnindexedProducer};
use ::rayon::iter::{
    FromParallelIterator, IntoParallelIterator, IntoParallelRefIterator,
    IntoParallelRefMutIterator, ParallelExtend, ParallelIterator,
};

use archery::{SharedPointer, SharedPointerKind};
use bitmaps::BitsImpl;

use crate::config::HASH_LEVEL_SIZE as HASH_SHIFT;
use crate::hash_width::HashWidth;
use crate::nodes::hamt::{CollisionNode, Entry, GenericSimdNode, HamtNode, Node};

const HASH_WIDTH: usize = 2_usize.pow(HASH_SHIFT as u32);
const SMALL_NODE_WIDTH: usize = HASH_WIDTH / 2;

// ---------------------------------------------------------------------------
// HashMap
// ---------------------------------------------------------------------------

use super::map::{next_hasher_id, GenericHashMap};

impl<'a, K, V, S, P> IntoParallelRefIterator<'a> for GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Send + Sync + 'a,
    V: Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);
    type Iter = ParIterMap<'a, K, V, P>;

    fn par_iter(&'a self) -> Self::Iter {
        ParIterMap {
            entries: root_entries(self.root.as_deref()),
        }
    }
}

/// A parallel iterator over the entries of a [`GenericHashMap`].
pub struct ParIterMap<'a, K, V, P: SharedPointerKind> {
    entries: Vec<&'a Entry<(K, V), P>>,
}

impl<'a, K, V, P> ParallelIterator for ParIterMap<'a, K, V, P>
where
    K: Send + Sync + 'a,
    V: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        bridge_unindexed(
            MapProducer {
                entries: self.entries,
            },
            consumer,
        )
    }
}

struct MapProducer<'a, K, V, P: SharedPointerKind> {
    entries: Vec<&'a Entry<(K, V), P>>,
}

impl<'a, K, V, P> UnindexedProducer for MapProducer<'a, K, V, P>
where
    K: Send + Sync + 'a,
    V: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);

    fn split(self) -> (Self, Option<Self>) {
        let (left, right) = split_entries(self.entries);
        (
            MapProducer { entries: left },
            right.map(|entries| MapProducer { entries }),
        )
    }

    fn fold_with<F>(self, folder: F) -> F
    where
        F: Folder<Self::Item>,
    {
        let iter = self.entries.into_iter().flat_map(map_entry_iter);
        folder.consume_iter(iter)
    }
}

/// Iterates all (K, V) pairs reachable from a single Entry, yielding (&K, &V).
fn map_entry_iter<'a, K, V, P: SharedPointerKind>(
    entry: &'a Entry<(K, V), P>,
) -> MapEntryIter<'a, K, V, P> {
    let mut iter = MapEntryIter { stack: Vec::new() };
    iter.push_entry(entry);
    iter
}

struct MapEntryIter<'a, K, V, P: SharedPointerKind> {
    stack: Vec<MapIterFrame<'a, K, V, P>>,
}

/// A frame in the DFS stack for iterating map entries.
enum MapIterFrame<'a, K, V, P: SharedPointerKind> {
    /// A single leaf value
    Leaf(&'a K, &'a V),
    /// Iterating entries in a HamtNode's SparseChunk
    Hamt(imbl_sized_chunks::sparse_chunk::Iter<'a, Entry<(K, V), P>, HASH_WIDTH>),
    /// Iterating kv-pairs in a SmallSimdNode
    SmallSimd(imbl_sized_chunks::sparse_chunk::Iter<'a, ((K, V), u64), SMALL_NODE_WIDTH>),
    /// Iterating kv-pairs in a LargeSimdNode
    LargeSimd(imbl_sized_chunks::sparse_chunk::Iter<'a, ((K, V), u64), HASH_WIDTH>),
    /// Iterating kv-pairs in a CollisionNode
    Collision(core::slice::Iter<'a, (K, V)>),
}

impl<'a, K, V, P: SharedPointerKind> MapEntryIter<'a, K, V, P> {
    fn push_entry(&mut self, entry: &'a Entry<(K, V), P>) {
        match entry {
            Entry::Value((k, v), _) => {
                self.stack.push(MapIterFrame::Leaf(k, v));
            }
            Entry::SmallSimdNode(node) => {
                self.stack.push(MapIterFrame::SmallSimd(node.data.iter()));
            }
            Entry::LargeSimdNode(node) => {
                self.stack.push(MapIterFrame::LargeSimd(node.data.iter()));
            }
            Entry::HamtNode(node) => {
                self.stack.push(MapIterFrame::Hamt(node.data.iter()));
            }
            Entry::Collision(coll) => {
                self.stack.push(MapIterFrame::Collision(coll.data.iter()));
            }
        }
    }
}

impl<'a, K, V, P: SharedPointerKind> Iterator for MapEntryIter<'a, K, V, P> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(frame) = self.stack.last_mut() {
            match frame {
                MapIterFrame::Leaf(k, v) => {
                    let result = (*k, *v);
                    self.stack.pop();
                    return Some(result);
                }
                MapIterFrame::SmallSimd(iter) => {
                    if let Some(((k, v), _)) = iter.next() {
                        return Some((k, v));
                    }
                }
                MapIterFrame::LargeSimd(iter) => {
                    if let Some(((k, v), _)) = iter.next() {
                        return Some((k, v));
                    }
                }
                MapIterFrame::Hamt(iter) => {
                    if let Some(child) = iter.next() {
                        self.push_entry(child);
                        continue;
                    }
                }
                MapIterFrame::Collision(iter) => {
                    if let Some((k, v)) = iter.next() {
                        return Some((k, v));
                    }
                }
            }
            self.stack.pop();
        }
        None
    }
}

// --- Mutable parallel iteration for HashMap ---

impl<'a, K, V, S, P> IntoParallelRefMutIterator<'a> for GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync + 'a,
    V: Clone + Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a mut V);
    type Iter = ParIterMutMap<'a, K, V, P>;

    fn par_iter_mut(&'a mut self) -> Self::Iter {
        let root = self.root.as_mut().map(SharedPointer::make_mut);
        ParIterMutMap {
            entries: root_entries_mut(root),
        }
    }
}

/// A parallel mutable iterator over the entries of a [`GenericHashMap`].
pub struct ParIterMutMap<'a, K, V, P: SharedPointerKind> {
    entries: Vec<&'a mut Entry<(K, V), P>>,
}

impl<'a, K, V, P> ParallelIterator for ParIterMutMap<'a, K, V, P>
where
    K: Clone + Send + Sync + 'a,
    V: Clone + Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a mut V);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        bridge_unindexed(
            MapMutProducer {
                entries: self.entries,
            },
            consumer,
        )
    }
}

struct MapMutProducer<'a, K, V, P: SharedPointerKind> {
    entries: Vec<&'a mut Entry<(K, V), P>>,
}

impl<'a, K, V, P> UnindexedProducer for MapMutProducer<'a, K, V, P>
where
    K: Clone + Send + Sync + 'a,
    V: Clone + Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a mut V);

    fn split(self) -> (Self, Option<Self>) {
        let (left, right) = split_entries_mut(self.entries);
        (
            MapMutProducer { entries: left },
            right.map(|entries| MapMutProducer { entries }),
        )
    }

    fn fold_with<F>(self, folder: F) -> F
    where
        F: Folder<Self::Item>,
    {
        let iter = self
            .entries
            .into_iter()
            .flat_map(|entry| map_entry_iter_mut(entry));
        folder.consume_iter(iter)
    }
}

/// Creates a mutable DFS iterator from a single Entry, yielding (&K, &mut V).
fn map_entry_iter_mut<'a, K, V, P>(entry: &'a mut Entry<(K, V), P>) -> MapEntryIterMut<'a, K, V, P>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
    let mut iter = MapEntryIterMut {
        first: None,
        stack: Vec::new(),
    };
    match entry {
        Entry::Value(kv, _) => {
            iter.first = Some((&kv.0, &mut kv.1));
        }
        Entry::HamtNode(ptr) => {
            let node = SharedPointer::make_mut(ptr);
            iter.stack.push(MapMutIterFrame::Hamt(node.data.iter_mut()));
        }
        Entry::SmallSimdNode(ptr) => {
            let node = SharedPointer::make_mut(ptr);
            iter.stack
                .push(MapMutIterFrame::SmallSimd(node.data.iter_mut()));
        }
        Entry::LargeSimdNode(ptr) => {
            let node = SharedPointer::make_mut(ptr);
            iter.stack
                .push(MapMutIterFrame::LargeSimd(node.data.iter_mut()));
        }
        Entry::Collision(ptr) => {
            let coll = SharedPointer::make_mut(ptr);
            iter.stack
                .push(MapMutIterFrame::Collision(coll.data.iter_mut()));
        }
    }
    iter
}

struct MapEntryIterMut<'a, K, V, P: SharedPointerKind> {
    first: Option<(&'a K, &'a mut V)>,
    stack: Vec<MapMutIterFrame<'a, K, V, P>>,
}

/// A frame in the DFS stack for mutably iterating map entries.
enum MapMutIterFrame<'a, K, V, P: SharedPointerKind> {
    Hamt(imbl_sized_chunks::sparse_chunk::IterMut<'a, Entry<(K, V), P>, HASH_WIDTH>),
    SmallSimd(imbl_sized_chunks::sparse_chunk::IterMut<'a, ((K, V), u64), SMALL_NODE_WIDTH>),
    LargeSimd(imbl_sized_chunks::sparse_chunk::IterMut<'a, ((K, V), u64), HASH_WIDTH>),
    Collision(core::slice::IterMut<'a, (K, V)>),
}

impl<'a, K, V, P: SharedPointerKind> Iterator for MapEntryIterMut<'a, K, V, P>
where
    K: Clone + 'a,
    V: Clone + 'a,
{
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(result) = self.first.take() {
            return Some(result);
        }
        while let Some(frame) = self.stack.last_mut() {
            match frame {
                MapMutIterFrame::SmallSimd(iter) => {
                    if let Some(item) = iter.next() {
                        return Some((&item.0 .0, &mut item.0 .1));
                    }
                }
                MapMutIterFrame::LargeSimd(iter) => {
                    if let Some(item) = iter.next() {
                        return Some((&item.0 .0, &mut item.0 .1));
                    }
                }
                MapMutIterFrame::Hamt(iter) => {
                    if let Some(entry) = iter.next() {
                        let new_frame = match entry {
                            Entry::Value(kv, _) => {
                                return Some((&kv.0, &mut kv.1));
                            }
                            Entry::HamtNode(ptr) => {
                                let node = SharedPointer::make_mut(ptr);
                                MapMutIterFrame::Hamt(node.data.iter_mut())
                            }
                            Entry::SmallSimdNode(ptr) => {
                                let node = SharedPointer::make_mut(ptr);
                                MapMutIterFrame::SmallSimd(node.data.iter_mut())
                            }
                            Entry::LargeSimdNode(ptr) => {
                                let node = SharedPointer::make_mut(ptr);
                                MapMutIterFrame::LargeSimd(node.data.iter_mut())
                            }
                            Entry::Collision(ptr) => {
                                let coll = SharedPointer::make_mut(ptr);
                                MapMutIterFrame::Collision(coll.data.iter_mut())
                            }
                        };
                        self.stack.push(new_frame);
                        continue;
                    }
                }
                MapMutIterFrame::Collision(iter) => {
                    if let Some(kv) = iter.next() {
                        return Some((&kv.0, &mut kv.1));
                    }
                }
            }
            self.stack.pop();
        }
        None
    }
}

impl<K, V, S, P> FromParallelIterator<(K, V)> for GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Hash + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn from_par_iter<I>(par_iter: I) -> Self
    where
        I: IntoParallelIterator<Item = (K, V)>,
    {
        par_iter
            .into_par_iter()
            .fold(Self::default, |mut map, (k, v)| {
                map.insert(k, v);
                map
            })
            .reduce(Self::default, |a, b| a.union(b))
    }
}

impl<K, V, S, P> ParallelExtend<(K, V)> for GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Hash + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn par_extend<I>(&mut self, par_iter: I)
    where
        I: IntoParallelIterator<Item = (K, V)>,
    {
        let collected: Self = par_iter.into_par_iter().collect();
        *self = core::mem::take(self).union(collected);
    }
}

// ---------------------------------------------------------------------------
// HashMap — parallel bulk operations
// ---------------------------------------------------------------------------

impl<K, V, S, P> GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Constructs the union of two maps in parallel.
    ///
    /// Values from `self` take precedence for keys present in both maps.
    /// Parallel speedup comes from filtering `other`'s elements against
    /// `self` concurrently.  For maps that share structure (one derived
    /// from the other via insert/remove), `ptr_eq` and Merkle-hash checks
    /// short-circuit in O(1) before any iteration begins.
    ///
    /// This is the parallel equivalent of [`union`][GenericHashMap::union].
    ///
    /// Time: O(n log n / p) where p is the thread pool size; O(1) when
    /// the maps are structurally identical.
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        if other.is_empty() || self.ptr_eq(&other) {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        // Same-lineage Merkle fast-path: equal len + equal kv_merkle → maps equal.
        if self.hasher_id == other.hasher_id
            && self.size == other.size
            && self.kv_merkle_valid
            && other.kv_merkle_valid
            && self.kv_merkle_hash == other.kv_merkle_hash
        {
            return self;
        }
        let only_in_other: Self = other
            .par_iter()
            .filter_map(|(k, v)| {
                if self.contains_key(k) {
                    None
                } else {
                    Some((k.clone(), v.clone()))
                }
            })
            .fold(Self::default, |mut acc, (k, v)| {
                acc.insert_invalidate_kv(k, v);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b));
        self.union(only_in_other)
    }

    /// Constructs the intersection of two maps in parallel, keeping
    /// values from `self`.
    ///
    /// `ptr_eq` and Merkle-hash fast-paths short-circuit in O(1) for
    /// structurally identical maps.
    ///
    /// This is the parallel equivalent of
    /// [`intersection`][GenericHashMap::intersection].
    ///
    /// Time: O(min(n, m) log max(n, m) / p); O(1) when maps are identical.
    #[must_use]
    pub fn par_intersection(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
        }
        if self.ptr_eq(&other) {
            return self;
        }
        if self.hasher_id == other.hasher_id
            && self.size == other.size
            && self.kv_merkle_valid
            && other.kv_merkle_valid
            && self.kv_merkle_hash == other.kv_merkle_hash
        {
            return self;
        }
        if self.len() <= other.len() {
            self.par_iter()
                .filter_map(|(k, v)| {
                    if other.contains_key(k) {
                        Some((k.clone(), v.clone()))
                    } else {
                        None
                    }
                })
                .fold(Self::default, |mut acc, (k, v)| {
                    acc.insert_invalidate_kv(k, v);
                    acc
                })
                .reduce(Self::default, |a, b| a.union(b))
        } else {
            other
                .par_iter()
                .filter_map(|(k, _)| self.get(k).map(|v| (k.clone(), v.clone())))
                .fold(Self::default, |mut acc, (k, v)| {
                    acc.insert_invalidate_kv(k, v);
                    acc
                })
                .reduce(Self::default, |a, b| a.union(b))
        }
    }

    /// Constructs the relative complement (self − other) in parallel:
    /// elements in `self` whose keys are not in `other`.
    ///
    /// `ptr_eq` and Merkle-hash fast-paths short-circuit in O(1) for
    /// identical maps (empty result) or an empty `other` (self returned).
    ///
    /// This is the parallel equivalent of
    /// [`difference`][GenericHashMap::difference].
    ///
    /// Time: O(n log m / p); O(1) for identical maps.
    #[must_use]
    pub fn par_difference(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return self;
        }
        if self.ptr_eq(&other) {
            return Self::default();
        }
        if self.hasher_id == other.hasher_id
            && self.size == other.size
            && self.kv_merkle_valid
            && other.kv_merkle_valid
            && self.kv_merkle_hash == other.kv_merkle_hash
        {
            return Self::default();
        }
        self.par_iter()
            .filter_map(|(k, v)| {
                if other.contains_key(k) {
                    None
                } else {
                    Some((k.clone(), v.clone()))
                }
            })
            .fold(Self::default, |mut acc, (k, v)| {
                acc.insert_invalidate_kv(k, v);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b))
    }

    /// Constructs the symmetric difference of two maps in parallel:
    /// elements present in exactly one of the two maps.
    ///
    /// Uses `rayon::join` to compute both halves (self \ other and
    /// other \ self) concurrently.  `ptr_eq` and Merkle-hash fast-paths
    /// short-circuit in O(1) for identical maps (empty result).
    ///
    /// This is the parallel equivalent of
    /// [`symmetric_difference`][GenericHashMap::symmetric_difference].
    ///
    /// Time: O((n + m) log max(n, m) / p); O(1) for identical maps.
    #[must_use]
    pub fn par_symmetric_difference(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        if self.ptr_eq(&other) {
            return Self::default();
        }
        if self.hasher_id == other.hasher_id
            && self.size == other.size
            && self.kv_merkle_valid
            && other.kv_merkle_valid
            && self.kv_merkle_hash == other.kv_merkle_hash
        {
            return Self::default();
        }
        let (left, right) = ::rayon::join(
            || {
                self.par_iter()
                    .filter_map(|(k, v)| {
                        if other.contains_key(k) {
                            None
                        } else {
                            Some((k.clone(), v.clone()))
                        }
                    })
                    .fold(Self::default, |mut acc, (k, v)| {
                        acc.insert_invalidate_kv(k, v);
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(b))
            },
            || {
                other
                    .par_iter()
                    .filter_map(|(k, v)| {
                        if self.contains_key(k) {
                            None
                        } else {
                            Some((k.clone(), v.clone()))
                        }
                    })
                    .fold(Self::default, |mut acc, (k, v)| {
                        acc.insert_invalidate_kv(k, v);
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(b))
            },
        );
        left.union(right)
    }
}

// ---------------------------------------------------------------------------
// HashMap — tree-native par_map_values helpers
//
// `par_map_values` is implemented by walking the HAMT tree structure directly
// rather than going through `par_iter().map().collect()`.
//
// Why this is faster than the iterator-based approach:
// - The `collect()` path calls `HashMap::insert` for each entry, which
//   recomputes hashes and re-walks the tree: O(n log n) total.
// - The tree-native path visits each entry exactly once and reconstructs
//   nodes by copying their structure (same bitmap/positions, only values
//   change): O(n) total, O(n/p) with rayon parallelism at the root level.
// - Merkle hashes on intermediate nodes are preserved unchanged because
//   they depend only on key hashes, not on values.
//
// `par_filter` does NOT benefit from this optimisation — removing keys changes
// tree topology, so reconstruction via insert is required.
// ---------------------------------------------------------------------------

/// Transform the values in a HAMT collision node. Keys and hash are preserved.
/// `f` receives `(&K, &V)` and returns `V2`.
fn map_values_collision<K, V, V2, H, F>(
    coll: &CollisionNode<(K, V), H>,
    f: &F,
) -> CollisionNode<(K, V2), H>
where
    K: Clone,
    V2: Clone,
    H: HashWidth,
    F: Fn(&K, &V) -> V2,
{
    CollisionNode {
        hash: coll.hash,
        data: coll
            .data
            .iter()
            .map(|(k, v)| (k.clone(), f(k, v)))
            .collect(),
    }
}

/// Transform the values in a SIMD leaf node. Control bytes and merkle_hash
/// are copied directly — they depend only on key hashes, not values.
fn map_values_simd<K, V, V2, H, F, const W: usize, const G: usize>(
    node: &GenericSimdNode<(K, V), H, W, G>,
    f: &F,
) -> GenericSimdNode<(K, V2), H, W, G>
where
    BitsImpl<W>: bitmaps::Bits,
    K: Clone,
    V2: Clone,
    H: HashWidth,
    F: Fn(&K, &V) -> V2,
{
    // map_values copies control bytes and merkle_hash from the source node.
    node.map_values(|(k, v)| (k.clone(), f(k, v)))
}

/// Transform the values in a single HAMT `Entry`, returning a new entry of
/// the output type. The entry's position in its parent node is unchanged.
fn map_values_entry<K, V, V2, P, H, F>(entry: &Entry<(K, V), P, H>, f: &F) -> Entry<(K, V2), P, H>
where
    K: Clone + Send + Sync,
    V: Clone + Send + Sync,
    V2: Clone + Send + Sync,
    P: SharedPointerKind + Send + Sync,
    H: HashWidth + Send + Sync,
    F: Fn(&K, &V) -> V2 + Send + Sync,
{
    match entry {
        Entry::Value((k, v), h) => Entry::Value((k.clone(), f(k, v)), *h),
        Entry::SmallSimdNode(node) => Entry::SmallSimdNode(SharedPointer::new(map_values_simd::<
            _,
            _,
            _,
            _,
            _,
            SMALL_NODE_WIDTH,
            1,
        >(node, f))),
        Entry::LargeSimdNode(node) => Entry::LargeSimdNode(SharedPointer::new(map_values_simd::<
            _,
            _,
            _,
            _,
            _,
            HASH_WIDTH,
            2,
        >(node, f))),
        Entry::HamtNode(node) => {
            // Recurse sequentially for interior nodes — the root-level rayon
            // fork already distributes work across threads.
            Entry::HamtNode(SharedPointer::new(map_values_hamt_node_seq(node, f)))
        }
        Entry::Collision(coll) => {
            Entry::Collision(SharedPointer::new(map_values_collision(coll, f)))
        }
    }
}

/// Sequentially transform all values in a `HamtNode`. Used for child nodes
/// below the root (root-level parallelism is sufficient for most maps).
fn map_values_hamt_node_seq<K, V, V2, P, H, F>(
    node: &HamtNode<(K, V), P, H>,
    f: &F,
) -> HamtNode<(K, V2), P, H>
where
    K: Clone + Send + Sync,
    V: Clone + Send + Sync,
    V2: Clone + Send + Sync,
    P: SharedPointerKind + Send + Sync,
    H: HashWidth + Send + Sync,
    F: Fn(&K, &V) -> V2 + Send + Sync,
{
    let mut new_node: HamtNode<(K, V2), P, H> = HamtNode::default();
    for (idx, entry) in node.data.entries() {
        new_node.data.insert(idx, map_values_entry(entry, f));
    }
    // Key-hash Merkle is preserved — only values changed.
    new_node.merkle_hash = node.merkle_hash;
    new_node
}

/// Parallel transform of a root `HamtNode`. Forks a rayon task per top-level
/// entry so that child subtrees are processed concurrently.
fn map_values_hamt_node_par<K, V, V2, P, H, F>(
    node: &HamtNode<(K, V), P, H>,
    f: &F,
) -> HamtNode<(K, V2), P, H>
where
    K: Clone + Send + Sync,
    V: Clone + Send + Sync,
    V2: Clone + Send + Sync,
    P: SharedPointerKind + Send + Sync,
    H: HashWidth + Send + Sync,
    F: Fn(&K, &V) -> V2 + Send + Sync,
{
    // At most HASH_WIDTH (32) entries at the root level.
    let pairs: Vec<(usize, &Entry<(K, V), P, H>)> = node.data.entries().collect();
    // Each entry is an independent subtree — process in parallel.
    let new_entries: Vec<(usize, Entry<(K, V2), P, H>)> = pairs
        .into_par_iter()
        .map(|(idx, entry)| (idx, map_values_entry(entry, f)))
        .collect();
    // Reassemble at the correct sparse positions.
    let mut new_node: HamtNode<(K, V2), P, H> = HamtNode::default();
    for (idx, entry) in new_entries {
        new_node.data.insert(idx, entry);
    }
    new_node.merkle_hash = node.merkle_hash;
    new_node
}

// ---------------------------------------------------------------------------
// HashMap — parallel transform operations
// ---------------------------------------------------------------------------

impl<K, V, S, P> GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Returns a new map keeping only entries that satisfy the predicate.
    ///
    /// Evaluates `f(&key, &value)` for every entry in parallel; returns a new
    /// map containing only those where `f` returns `true`. The original map
    /// is unchanged.
    ///
    /// Because removing keys changes tree topology, this method uses
    /// `par_iter().filter().collect()` rather than direct tree manipulation.
    ///
    /// This is the immutable, parallel equivalent of [`retain`][Self::retain].
    ///
    /// Time: O(n / p) to scan + O(k log k) to rebuild (k = surviving entries).
    #[must_use]
    pub fn par_filter<F>(&self, f: F) -> Self
    where
        F: Fn(&K, &V) -> bool + Sync + Send,
    {
        if self.is_empty() {
            return Self::default();
        }
        self.par_iter()
            .filter(|(k, v)| f(*k, *v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

impl<K, V, S, P> GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Returns a new map with values transformed by `f`, applied in parallel.
    ///
    /// Keys are unchanged; each value is replaced by `f(&value)`. This method
    /// walks the HAMT tree directly rather than using `par_iter().collect()`:
    /// since keys (and therefore tree topology and key-hash Merkle values) are
    /// preserved, nodes are reconstructed at their original positions without
    /// re-hashing or re-walking the tree. This is O(n/p) end-to-end vs the
    /// O(n/p scan + n log n rebuild) of a collect-based approach.
    ///
    /// Equivalent to the sequential [`map_values`][Self::map_values] but
    /// evaluated in parallel.
    ///
    /// Time: O(n / p) — tree walk + value transform, no O(n log n) rebuild.
    #[must_use]
    pub fn par_map_values<V2, F>(&self, f: F) -> GenericHashMap<K, V2, S, P>
    where
        V2: Clone + Send + Sync,
        F: Fn(&V) -> V2 + Sync + Send,
    {
        if self.is_empty() {
            return GenericHashMap::default();
        }
        // Adapt Fn(&V) -> V2 to the Fn(&K, &V) -> V2 signature used by helpers.
        let g = |_k: &K, v: &V| f(v);
        let new_root = self
            .root
            .as_ref()
            .map(|root_ptr| SharedPointer::new(map_values_hamt_node_par(root_ptr, &g)));
        GenericHashMap {
            size: self.size,
            root: new_root,
            hasher: self.hasher.clone(),
            hasher_id: next_hasher_id(),
            kv_merkle_hash: 0,
            // V2 may hash differently from V, so KV Merkle is no longer valid.
            kv_merkle_valid: false,
        }
    }

    /// Returns a new map with values transformed by `f(key, value)`, applied in
    /// parallel.
    ///
    /// Each entry's value is replaced by `f(&key, &value)`. Uses the same
    /// tree-native approach as [`par_map_values`][Self::par_map_values] — no
    /// re-hashing or re-walking.
    ///
    /// Equivalent to the sequential
    /// [`map_values_with_key`][Self::map_values_with_key].
    ///
    /// Time: O(n / p) — tree walk + value transform, no O(n log n) rebuild.
    #[must_use]
    pub fn par_map_values_with_key<V2, F>(&self, f: F) -> GenericHashMap<K, V2, S, P>
    where
        V2: Clone + Send + Sync,
        F: Fn(&K, &V) -> V2 + Sync + Send,
    {
        if self.is_empty() {
            return GenericHashMap::default();
        }
        let new_root = self
            .root
            .as_ref()
            .map(|root_ptr| SharedPointer::new(map_values_hamt_node_par(root_ptr, &f)));
        GenericHashMap {
            size: self.size,
            root: new_root,
            hasher: self.hasher.clone(),
            hasher_id: next_hasher_id(),
            kv_merkle_hash: 0,
            kv_merkle_valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// HashSet — parallel transform operations
// ---------------------------------------------------------------------------

impl<A, S, P> GenericHashSet<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Returns a new set keeping only elements that satisfy the predicate.
    ///
    /// Evaluates `f(&element)` for every element in parallel; returns a new
    /// set containing only those where `f` returns `true`. The original set
    /// is unchanged.
    ///
    /// This is the immutable, parallel equivalent of [`retain`][Self::retain].
    ///
    /// Time: O(n / p) to scan + O(k log k) to rebuild (k = surviving elements).
    #[must_use]
    pub fn par_filter<F>(&self, f: F) -> Self
    where
        F: Fn(&A) -> bool + Sync + Send,
    {
        if self.is_empty() {
            return Self::default();
        }
        self.par_iter()
            .filter(|a| f(*a))
            .map(|a| a.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// HashSet
// ---------------------------------------------------------------------------

use super::set::{GenericHashSet, Value};

impl<'a, A, S, P> IntoParallelRefIterator<'a> for GenericHashSet<A, S, P>
where
    A: Hash + Eq + Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = &'a A;
    type Iter = ParIterSet<'a, A, P>;

    fn par_iter(&'a self) -> Self::Iter {
        ParIterSet {
            entries: root_entries(self.root.as_deref()),
        }
    }
}

/// A parallel iterator over the elements of a [`GenericHashSet`].
pub struct ParIterSet<'a, A, P: SharedPointerKind> {
    entries: Vec<&'a Entry<Value<A>, P>>,
}

impl<'a, A, P> ParallelIterator for ParIterSet<'a, A, P>
where
    A: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = &'a A;

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        bridge_unindexed(
            SetProducer {
                entries: self.entries,
            },
            consumer,
        )
    }
}

struct SetProducer<'a, A, P: SharedPointerKind> {
    entries: Vec<&'a Entry<Value<A>, P>>,
}

impl<'a, A, P> UnindexedProducer for SetProducer<'a, A, P>
where
    A: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = &'a A;

    fn split(self) -> (Self, Option<Self>) {
        let (left, right) = split_entries(self.entries);
        (
            SetProducer { entries: left },
            right.map(|entries| SetProducer { entries }),
        )
    }

    fn fold_with<F>(self, folder: F) -> F
    where
        F: Folder<Self::Item>,
    {
        let iter = self.entries.into_iter().flat_map(set_entry_iter);
        folder.consume_iter(iter)
    }
}

/// Iterates all A values reachable from a single Entry<Value<A>, P>.
fn set_entry_iter<'a, A, P: SharedPointerKind>(
    entry: &'a Entry<Value<A>, P>,
) -> SetEntryIter<'a, A, P> {
    let mut iter = SetEntryIter { stack: Vec::new() };
    iter.push_entry(entry);
    iter
}

struct SetEntryIter<'a, A, P: SharedPointerKind> {
    stack: Vec<SetIterFrame<'a, A, P>>,
}

enum SetIterFrame<'a, A, P: SharedPointerKind> {
    Leaf(&'a A),
    Hamt(imbl_sized_chunks::sparse_chunk::Iter<'a, Entry<Value<A>, P>, HASH_WIDTH>),
    SmallSimd(imbl_sized_chunks::sparse_chunk::Iter<'a, (Value<A>, u64), SMALL_NODE_WIDTH>),
    LargeSimd(imbl_sized_chunks::sparse_chunk::Iter<'a, (Value<A>, u64), HASH_WIDTH>),
    Collision(core::slice::Iter<'a, Value<A>>),
}

impl<'a, A, P: SharedPointerKind> SetEntryIter<'a, A, P> {
    fn push_entry(&mut self, entry: &'a Entry<Value<A>, P>) {
        match entry {
            Entry::Value(v, _) => {
                self.stack.push(SetIterFrame::Leaf(&v.0));
            }
            Entry::SmallSimdNode(node) => {
                self.stack.push(SetIterFrame::SmallSimd(node.data.iter()));
            }
            Entry::LargeSimdNode(node) => {
                self.stack.push(SetIterFrame::LargeSimd(node.data.iter()));
            }
            Entry::HamtNode(node) => {
                self.stack.push(SetIterFrame::Hamt(node.data.iter()));
            }
            Entry::Collision(coll) => {
                self.stack.push(SetIterFrame::Collision(coll.data.iter()));
            }
        }
    }
}

impl<'a, A, P: SharedPointerKind> Iterator for SetEntryIter<'a, A, P> {
    type Item = &'a A;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(frame) = self.stack.last_mut() {
            match frame {
                SetIterFrame::Leaf(a) => {
                    let result = *a;
                    self.stack.pop();
                    return Some(result);
                }
                SetIterFrame::SmallSimd(iter) => {
                    if let Some((v, _)) = iter.next() {
                        return Some(&v.0);
                    }
                }
                SetIterFrame::LargeSimd(iter) => {
                    if let Some((v, _)) = iter.next() {
                        return Some(&v.0);
                    }
                }
                SetIterFrame::Hamt(iter) => {
                    if let Some(child) = iter.next() {
                        self.push_entry(child);
                        continue;
                    }
                }
                SetIterFrame::Collision(iter) => {
                    if let Some(v) = iter.next() {
                        return Some(&v.0);
                    }
                }
            }
            self.stack.pop();
        }
        None
    }
}

impl<A, S, P> FromParallelIterator<A> for GenericHashSet<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn from_par_iter<I>(par_iter: I) -> Self
    where
        I: IntoParallelIterator<Item = A>,
    {
        par_iter
            .into_par_iter()
            .fold(Self::default, |mut set, a| {
                set.insert(a);
                set
            })
            .reduce(Self::default, |a, b| a.union(b))
    }
}

impl<A, S, P> ParallelExtend<A> for GenericHashSet<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn par_extend<I>(&mut self, par_iter: I)
    where
        I: IntoParallelIterator<Item = A>,
    {
        let collected: Self = par_iter.into_par_iter().collect();
        *self = core::mem::take(self).union(collected);
    }
}

// ---------------------------------------------------------------------------
// HashSet — parallel bulk operations
// ---------------------------------------------------------------------------

impl<A, S, P> GenericHashSet<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Constructs the union of two sets in parallel.
    ///
    /// `ptr_eq` fast-path short-circuits in O(1) for structurally
    /// identical sets.
    ///
    /// This is the parallel equivalent of [`union`][GenericHashSet::union].
    ///
    /// Time: O(n log n / p); O(1) for identical sets.
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        if other.is_empty() || self.ptr_eq(&other) {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        let only_in_other: Self = other
            .par_iter()
            .filter_map(|a| {
                if self.contains(a) {
                    None
                } else {
                    Some(a.clone())
                }
            })
            .fold(Self::default, |mut acc, a| {
                acc.insert(a);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b));
        self.union(only_in_other)
    }

    /// Constructs the intersection of two sets in parallel.
    ///
    /// `ptr_eq` fast-path short-circuits in O(1) for structurally
    /// identical sets.
    ///
    /// This is the parallel equivalent of
    /// [`intersection`][GenericHashSet::intersection].
    ///
    /// Time: O(min(n, m) log max(n, m) / p); O(1) for identical sets.
    #[must_use]
    pub fn par_intersection(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
        }
        if self.ptr_eq(&other) {
            return self;
        }
        let (smaller, larger) = if self.len() <= other.len() {
            (&self, &other)
        } else {
            (&other, &self)
        };
        smaller
            .par_iter()
            .filter_map(|a| {
                if larger.contains(a) {
                    Some(a.clone())
                } else {
                    None
                }
            })
            .fold(Self::default, |mut acc, a| {
                acc.insert(a);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b))
    }

    /// Constructs the relative complement (self − other) in parallel:
    /// elements in `self` not in `other`.
    ///
    /// `ptr_eq` fast-path returns empty in O(1) for identical sets.
    ///
    /// This is the parallel equivalent of
    /// [`difference`][GenericHashSet::difference].
    ///
    /// Time: O(n log m / p); O(1) for identical sets.
    #[must_use]
    pub fn par_difference(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return self;
        }
        if self.ptr_eq(&other) {
            return Self::default();
        }
        self.par_iter()
            .filter_map(|a| {
                if other.contains(a) {
                    None
                } else {
                    Some(a.clone())
                }
            })
            .fold(Self::default, |mut acc, a| {
                acc.insert(a);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b))
    }

    /// Constructs the symmetric difference of two sets in parallel:
    /// elements in exactly one of the two sets.
    ///
    /// Uses `rayon::join` to compute both halves concurrently.
    /// `ptr_eq` fast-path returns empty in O(1) for identical sets.
    ///
    /// This is the parallel equivalent of
    /// [`symmetric_difference`][GenericHashSet::symmetric_difference].
    ///
    /// Time: O((n + m) log max(n, m) / p); O(1) for identical sets.
    #[must_use]
    pub fn par_symmetric_difference(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        if self.ptr_eq(&other) {
            return Self::default();
        }
        let (left, right) = ::rayon::join(
            || {
                self.par_iter()
                    .filter_map(|a| {
                        if other.contains(a) {
                            None
                        } else {
                            Some(a.clone())
                        }
                    })
                    .fold(Self::default, |mut acc, a| {
                        acc.insert(a);
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(b))
            },
            || {
                other
                    .par_iter()
                    .filter_map(|a| {
                        if self.contains(a) {
                            None
                        } else {
                            Some(a.clone())
                        }
                    })
                    .fold(Self::default, |mut acc, a| {
                        acc.insert(a);
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(b))
            },
        );
        left.union(right)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Extract the top-level entry references from a HAMT root node.
fn root_entries<A, P: SharedPointerKind>(root: Option<&Node<A, P>>) -> Vec<&Entry<A, P>> {
    match root {
        Some(node) => node.data.iter().collect(),
        None => Vec::new(),
    }
}

/// Splits a vec of entry references for rayon work distribution.
/// If there are multiple entries, splits in half.
/// If there's a single HamtNode entry, expands it and splits its children.
fn split_entries<A, P: SharedPointerKind>(
    mut entries: Vec<&Entry<A, P>>,
) -> (Vec<&Entry<A, P>>, Option<Vec<&Entry<A, P>>>) {
    if entries.len() >= 2 {
        let mid = entries.len() / 2;
        let right = entries.split_off(mid);
        return (entries, Some(right));
    }
    // Single entry — try to expand if it's a HamtNode for deeper parallelism
    if entries.len() == 1 {
        if let Entry::HamtNode(ref child) = entries[0] {
            let child_entries: Vec<_> = child.data.iter().collect();
            if child_entries.len() >= 2 {
                let mid = child_entries.len() / 2;
                let (left, right) = child_entries.split_at(mid);
                return (left.to_vec(), Some(right.to_vec()));
            }
        }
    }
    (entries, None)
}

/// Extract mutable entry references from a HAMT root node.
fn root_entries_mut<A, P: SharedPointerKind>(
    root: Option<&mut Node<A, P>>,
) -> Vec<&mut Entry<A, P>> {
    match root {
        Some(node) => node.data.iter_mut().collect(),
        None => Vec::new(),
    }
}

/// Splits a vec of mutable entry references for rayon work distribution.
/// Same strategy as `split_entries` but with `make_mut` for exclusive ownership.
fn split_entries_mut<A: Clone, P: SharedPointerKind>(
    mut entries: Vec<&mut Entry<A, P>>,
) -> (Vec<&mut Entry<A, P>>, Option<Vec<&mut Entry<A, P>>>) {
    if entries.len() >= 2 {
        let mid = entries.len() / 2;
        let right = entries.split_off(mid);
        return (entries, Some(right));
    }
    // Single entry — try to expand if it's a HamtNode for deeper parallelism
    if entries.len() == 1 {
        let entry = entries.pop().unwrap();
        if let Entry::HamtNode(child_ptr) = entry {
            let child_node = SharedPointer::make_mut(child_ptr);
            let mut child_entries: Vec<_> = child_node.data.iter_mut().collect();
            if child_entries.len() >= 2 {
                let mid = child_entries.len() / 2;
                let right = child_entries.split_off(mid);
                return (child_entries, Some(right));
            }
            return (child_entries, None);
        }
        entries.push(entry);
    }
    (entries, None)
}

#[cfg(test)]
mod test {
    use super::super::map::HashMap;
    use super::super::set::HashSet;
    use ::rayon::iter::{
        IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelExtend, ParallelIterator,
    };

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_sum() {
        let mut map = HashMap::new();
        for i in 0..10_000i64 {
            map.insert(i, i);
        }
        let par_sum: i64 = map.par_iter().map(|(_, &v)| v).sum();
        let seq_sum: i64 = map.iter().map(|(_, &v)| v).sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_count() {
        let mut map = HashMap::new();
        for i in 0..10_000 {
            map.insert(i, i);
        }
        assert_eq!(map.par_iter().count(), 10_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_empty() {
        let map: HashMap<i32, i32> = HashMap::new();
        assert_eq!(map.par_iter().count(), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_collect() {
        let mut map = HashMap::new();
        for i in 0..1_000 {
            map.insert(i, i * 2);
        }
        let mut pairs: Vec<_> = map.par_iter().map(|(&k, &v)| (k, v)).collect();
        pairs.sort();
        for (k, v) in &pairs {
            assert_eq!(*v, k * 2);
        }
        assert_eq!(pairs.len(), 1_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_iter_sum() {
        let mut set = HashSet::new();
        for i in 0..10_000i64 {
            set.insert(i);
        }
        let par_sum: i64 = set.par_iter().copied().sum();
        let seq_sum: i64 = set.iter().copied().sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_iter_count() {
        let mut set = HashSet::new();
        for i in 0..10_000 {
            set.insert(i);
        }
        assert_eq!(set.par_iter().count(), 10_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_iter_empty() {
        let set: HashSet<i32> = HashSet::new();
        assert_eq!(set.par_iter().count(), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_iter_collect() {
        let mut set = HashSet::new();
        for i in 0..1_000 {
            set.insert(i);
        }
        let mut vals: Vec<_> = set.par_iter().copied().collect();
        vals.sort();
        assert_eq!(vals, (0..1_000).collect::<Vec<_>>());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_from_par_iter() {
        let pairs: Vec<(i32, i32)> = (0..10_000).map(|i| (i, i * 3)).collect();
        let map: HashMap<i32, i32> = pairs.par_iter().copied().collect();
        assert_eq!(map.len(), 10_000);
        for i in 0..10_000 {
            assert_eq!(map.get(&i), Some(&(i * 3)));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_extend() {
        let mut map = HashMap::new();
        map.insert(0, 0);
        map.insert(1, 1);
        let extras: Vec<(i32, i32)> = (2..1_000).map(|i| (i, i * 2)).collect();
        map.par_extend(extras.par_iter().copied());
        assert_eq!(map.len(), 1_000);
        assert_eq!(map.get(&0), Some(&0));
        assert_eq!(map.get(&999), Some(&1998));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_from_par_iter() {
        let vals: Vec<i32> = (0..10_000).collect();
        let set: HashSet<i32> = vals.par_iter().copied().collect();
        assert_eq!(set.len(), 10_000);
        for i in 0..10_000 {
            assert!(set.contains(&i));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_mut_double() {
        let mut map = HashMap::new();
        for i in 0..10_000i64 {
            map.insert(i, i);
        }
        map.par_iter_mut().for_each(|(_, v)| *v *= 2);
        for i in 0..10_000i64 {
            assert_eq!(map.get(&i), Some(&(i * 2)));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_mut_count() {
        let mut map = HashMap::new();
        for i in 0..10_000 {
            map.insert(i, i);
        }
        assert_eq!(map.par_iter_mut().count(), 10_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_mut_empty() {
        let mut map: HashMap<i32, i32> = HashMap::new();
        assert_eq!(map.par_iter_mut().count(), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_iter_mut_collect() {
        let mut map = HashMap::new();
        for i in 0..1_000 {
            map.insert(i, i);
        }
        map.par_iter_mut().for_each(|(_, v)| *v += 100);
        let mut pairs: Vec<_> = map.iter().map(|(&k, &v)| (k, v)).collect();
        pairs.sort();
        for (k, v) in &pairs {
            assert_eq!(*v, k + 100);
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_extend() {
        let mut set = HashSet::new();
        set.insert(0);
        set.insert(1);
        let extras: Vec<i32> = (2..1_000).collect();
        set.par_extend(extras.par_iter().copied());
        assert_eq!(set.len(), 1_000);
        assert!(set.contains(&0));
        assert!(set.contains(&999));
    }

    // --- Parallel bulk operations ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_union() {
        let map1: HashMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: HashMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().union(map2.clone());
        let par = map1.par_union(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_intersection() {
        let map1: HashMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: HashMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().intersection(map2.clone());
        let par = map1.par_intersection(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_difference() {
        let map1: HashMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: HashMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().difference(map2.clone());
        let par = map1.par_difference(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_symmetric_difference() {
        let map1: HashMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: HashMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().symmetric_difference(map2.clone());
        let par = map1.par_symmetric_difference(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_union() {
        let set1: HashSet<i32> = (0..5_000).collect();
        let set2: HashSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().union(set2.clone());
        let par = set1.par_union(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_intersection() {
        let set1: HashSet<i32> = (0..5_000).collect();
        let set2: HashSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().intersection(set2.clone());
        let par = set1.par_intersection(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_difference() {
        let set1: HashSet<i32> = (0..5_000).collect();
        let set2: HashSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().difference(set2.clone());
        let par = set1.par_difference(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_symmetric_difference() {
        let set1: HashSet<i32> = (0..5_000).collect();
        let set2: HashSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().symmetric_difference(set2.clone());
        let par = set1.par_symmetric_difference(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_union_empty() {
        let map1: HashMap<i32, i32> = (0..1_000).map(|i| (i, i)).collect();
        let map2: HashMap<i32, i32> = HashMap::new();
        assert_eq!(
            map1.clone().par_union(map2.clone()),
            map1.clone().union(map2.clone())
        );
        assert_eq!(map2.clone().par_union(map1.clone()), map2.union(map1));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_intersection_disjoint() {
        let set1: HashSet<i32> = (0..1_000).collect();
        let set2: HashSet<i32> = (1_000..2_000).collect();
        assert!(set1.par_intersection(set2).is_empty());
    }

    // --- ptr_eq / Merkle fast-path tests ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_union_ptr_eq_fast_path() {
        // When both maps are the same (ptr_eq), par_union returns self unchanged.
        let map: HashMap<i32, i32> = (0..1_000).map(|i| (i, i)).collect();
        let same = map.clone(); // O(1) clone: same root pointer
        assert!(map.ptr_eq(&same));
        let result = map.par_union(same.clone());
        assert_eq!(result.len(), 1_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_difference_ptr_eq_fast_path() {
        // When both maps share the same root, difference is empty.
        let map: HashMap<i32, i32> = (0..1_000).map(|i| (i, i)).collect();
        let same = map.clone();
        assert!(map.ptr_eq(&same));
        assert!(map.par_difference(same).is_empty());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashmap_par_symmetric_difference_ptr_eq_fast_path() {
        let map: HashMap<i32, i32> = (0..1_000).map(|i| (i, i)).collect();
        let same = map.clone();
        assert!(map.ptr_eq(&same));
        assert!(map.par_symmetric_difference(same).is_empty());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hashset_par_difference_ptr_eq_fast_path() {
        let set: HashSet<i32> = (0..1_000).collect();
        let same = set.clone();
        assert!(set.ptr_eq(&same));
        assert!(set.par_difference(same).is_empty());
    }

    #[test]
    fn hashmap_par_filter_keeps_matching() {
        let map: HashMap<i32, i32> = (0..100).map(|i| (i, i * 2)).collect();
        let evens = map.par_filter(|k, _| k % 2 == 0);
        assert_eq!(evens.len(), 50);
        assert!(evens.keys().all(|k| k % 2 == 0));
    }

    #[test]
    fn hashmap_par_filter_empty_input() {
        let map: HashMap<i32, i32> = HashMap::new();
        assert!(map.par_filter(|_, _| true).is_empty());
    }

    #[test]
    fn hashmap_par_filter_none_match() {
        let map: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        assert!(map.par_filter(|_, _| false).is_empty());
    }

    #[test]
    fn hashmap_par_filter_all_match() {
        let map: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let result = map.par_filter(|_, _| true);
        assert_eq!(result, map);
    }

    #[test]
    fn hashmap_par_map_values_doubles() {
        let map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let doubled = map.par_map_values(|v| v * 2);
        for i in 0..100 {
            assert_eq!(doubled.get(&i), Some(&(i * 2)));
        }
    }

    #[test]
    fn hashmap_par_map_values_empty() {
        let map: HashMap<i32, i32> = HashMap::new();
        assert!(map.par_map_values(|v| v * 2).is_empty());
    }

    #[test]
    fn hashmap_par_map_values_type_change() {
        let map: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let stringified: HashMap<i32, String> = map.par_map_values(|v| v.to_string());
        assert_eq!(stringified.get(&0), Some(&"0".to_string()));
        assert_eq!(stringified.get(&42), Some(&"42".to_string()));
    }

    #[test]
    fn hashmap_par_map_values_with_key() {
        let map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let result = map.par_map_values_with_key(|k, v| k + v);
        for i in 0..100_i32 {
            assert_eq!(result.get(&i), Some(&(i + i)));
        }
    }

    #[test]
    fn hashmap_par_map_values_with_key_empty() {
        let map: HashMap<i32, i32> = HashMap::new();
        assert!(map.par_map_values_with_key(|k, v| k + v).is_empty());
    }

    #[test]
    fn hashset_par_filter_keeps_matching() {
        let set: HashSet<i32> = (0..100).collect();
        let evens = set.par_filter(|x| x % 2 == 0);
        assert_eq!(evens.len(), 50);
        assert!(evens.iter().all(|x| x % 2 == 0));
    }

    #[test]
    fn hashset_par_filter_empty_input() {
        let set: HashSet<i32> = HashSet::new();
        assert!(set.par_filter(|_| true).is_empty());
    }

    #[test]
    fn hashset_par_filter_none_match() {
        let set: HashSet<i32> = (0..50).collect();
        assert!(set.par_filter(|_| false).is_empty());
    }

    #[test]
    fn hashset_par_filter_all_match() {
        let set: HashSet<i32> = (0..50).collect();
        let result = set.par_filter(|_| true);
        assert_eq!(result, set);
    }

    // --- tree-native par_map_values correctness vs sequential map_values ---

    #[test]
    fn hashmap_par_map_values_matches_seq() {
        let map: HashMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let seq = map.map_values(|v| v * 3 + 1);
        let par = map.par_map_values(|v| v * 3 + 1);
        assert_eq!(par, seq);
    }

    #[test]
    fn hashmap_par_map_values_type_change_matches_seq() {
        let map: HashMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let seq: HashMap<i32, String> = map.map_values(|v| v.to_string());
        let par: HashMap<i32, String> = map.par_map_values(|v| v.to_string());
        assert_eq!(par, seq);
    }

    #[test]
    fn hashmap_par_map_values_with_key_matches_seq() {
        let map: HashMap<i32, i32> = (0..5_000).map(|i| (i, i * 2)).collect();
        let seq = map.map_values_with_key(|k, v| k + v);
        let par = map.par_map_values_with_key(|k, v| k + v);
        assert_eq!(par, seq);
    }

    #[test]
    fn hashmap_par_map_values_empty_tree_native() {
        let map: HashMap<i32, i32> = HashMap::new();
        let par: HashMap<i32, String> = map.par_map_values(|v| v.to_string());
        assert!(par.is_empty());
    }

    #[test]
    fn hashmap_par_map_values_preserves_size() {
        let n = 10_000;
        let map: HashMap<i32, i32> = (0..n).map(|i| (i, i)).collect();
        let par = map.par_map_values(|v| v + 1);
        assert_eq!(par.len(), n as usize);
        for i in 0..n {
            assert_eq!(par.get(&i), Some(&(i + 1)));
        }
    }
}
