// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Parallel iterators for HashMap and HashSet.
//!
//! These are only available when using the `rayon` feature flag.

use std::hash::Hash;

use ::rayon::iter::plumbing::{bridge_unindexed, Folder, UnindexedConsumer, UnindexedProducer};
use ::rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use archery::SharedPointerKind;

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
    Hamt(
        imbl_sized_chunks::sparse_chunk::Iter<'a, Entry<(K, V), P>, HASH_WIDTH>,
    ),
    /// Iterating kv-pairs in a SmallSimdNode
    SmallSimd(
        imbl_sized_chunks::sparse_chunk::Iter<
            'a,
            ((K, V), crate::nodes::hamt::HashBits),
            SMALL_NODE_WIDTH,
        >,
    ),
    /// Iterating kv-pairs in a LargeSimdNode
    LargeSimd(
        imbl_sized_chunks::sparse_chunk::Iter<
            'a,
            ((K, V), crate::nodes::hamt::HashBits),
            HASH_WIDTH,
        >,
    ),
    /// Iterating kv-pairs in a CollisionNode
    Collision(std::slice::Iter<'a, (K, V)>),
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
    Hamt(
        imbl_sized_chunks::sparse_chunk::Iter<'a, Entry<Value<A>, P>, HASH_WIDTH>,
    ),
    SmallSimd(
        imbl_sized_chunks::sparse_chunk::Iter<
            'a,
            (Value<A>, crate::nodes::hamt::HashBits),
            SMALL_NODE_WIDTH,
        >,
    ),
    LargeSimd(
        imbl_sized_chunks::sparse_chunk::Iter<
            'a,
            (Value<A>, crate::nodes::hamt::HashBits),
            HASH_WIDTH,
        >,
    ),
    Collision(std::slice::Iter<'a, Value<A>>),
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
                self.stack
                    .push(SetIterFrame::Collision(coll.data.iter()));
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

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Extract the top-level entry references from a HAMT root node.
fn root_entries<'a, A, P: SharedPointerKind>(
    root: Option<&'a Node<A, P>>,
) -> Vec<&'a Entry<A, P>> {
    match root {
        Some(node) => node.data.iter().collect(),
        None => Vec::new(),
    }
}

/// Split a vec of entry references for rayon work distribution.
/// If there are multiple entries, splits in half.
/// If there's a single HamtNode entry, expands it and splits its children.
fn split_entries<'a, A, P: SharedPointerKind>(
    mut entries: Vec<&'a Entry<A, P>>,
) -> (Vec<&'a Entry<A, P>>, Option<Vec<&'a Entry<A, P>>>) {
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

#[cfg(test)]
mod test {
    use super::super::map::HashMap;
    use super::super::set::HashSet;
    use ::rayon::iter::{IntoParallelRefIterator, ParallelIterator};

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
}
