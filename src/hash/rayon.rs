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

use crate::config::HASH_LEVEL_SIZE as HASH_SHIFT;
use crate::nodes::hamt::{Entry, Node};

const HASH_WIDTH: usize = 2_usize.pow(HASH_SHIFT as u32);
const SMALL_NODE_WIDTH: usize = HASH_WIDTH / 2;

// ---------------------------------------------------------------------------
// HashMap
// ---------------------------------------------------------------------------

use super::map::GenericHashMap;

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

/// Iterate all (K, V) pairs reachable from a single Entry, yielding (&K, &V).
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

/// Create a mutable DFS iterator from a single Entry, yielding (&K, &mut V).
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
    /// Construct the union of two maps in parallel.
    ///
    /// Values from `self` take precedence for keys present in both maps.
    /// Parallel speedup comes from filtering the smaller map's elements
    /// against the larger map concurrently.
    ///
    /// This is the parallel equivalent of [`union`][GenericHashMap::union].
    ///
    /// Time: O(n log n / p) where p is the thread pool size
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
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

    /// Construct the intersection of two maps in parallel, keeping
    /// values from `self`.
    ///
    /// This is the parallel equivalent of
    /// [`intersection`][GenericHashMap::intersection].
    ///
    /// Time: O(min(n, m) log max(n, m) / p)
    #[must_use]
    pub fn par_intersection(self, other: Self) -> Self {
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

    /// Construct the relative complement (self − other) in parallel:
    /// elements in `self` whose keys are not in `other`.
    ///
    /// This is the parallel equivalent of
    /// [`difference`][GenericHashMap::difference].
    ///
    /// Time: O(n log m / p)
    #[must_use]
    pub fn par_difference(self, other: Self) -> Self {
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

    /// Construct the symmetric difference of two maps in parallel:
    /// elements present in exactly one of the two maps.
    ///
    /// This is the parallel equivalent of
    /// [`symmetric_difference`][GenericHashMap::symmetric_difference].
    ///
    /// Time: O((n + m) log max(n, m) / p)
    #[must_use]
    pub fn par_symmetric_difference(self, other: Self) -> Self {
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

/// Iterate all A values reachable from a single Entry<Value<A>, P>.
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
    /// Construct the union of two sets in parallel.
    ///
    /// This is the parallel equivalent of [`union`][GenericHashSet::union].
    ///
    /// Time: O(n log n / p)
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
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

    /// Construct the intersection of two sets in parallel.
    ///
    /// This is the parallel equivalent of
    /// [`intersection`][GenericHashSet::intersection].
    ///
    /// Time: O(min(n, m) log max(n, m) / p)
    #[must_use]
    pub fn par_intersection(self, other: Self) -> Self {
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

    /// Construct the relative complement (self − other) in parallel:
    /// elements in `self` not in `other`.
    ///
    /// This is the parallel equivalent of
    /// [`difference`][GenericHashSet::difference].
    ///
    /// Time: O(n log m / p)
    #[must_use]
    pub fn par_difference(self, other: Self) -> Self {
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

    /// Construct the symmetric difference of two sets in parallel:
    /// elements in exactly one of the two sets.
    ///
    /// This is the parallel equivalent of
    /// [`symmetric_difference`][GenericHashSet::symmetric_difference].
    ///
    /// Time: O((n + m) log max(n, m) / p)
    #[must_use]
    pub fn par_symmetric_difference(self, other: Self) -> Self {
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

/// Split a vec of entry references for rayon work distribution.
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

/// Split a vec of mutable entry references for rayon work distribution.
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
}
