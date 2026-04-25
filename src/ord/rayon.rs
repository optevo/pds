// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Parallel iterators for OrdMap and OrdSet.
//!
//! These are only available when using the `rayon` feature flag.

use ::rayon::iter::plumbing::{bridge_unindexed, Folder, UnindexedConsumer, UnindexedProducer};
use ::rayon::iter::{
    FromParallelIterator, IntoParallelIterator, IntoParallelRefIterator, ParallelExtend,
    ParallelIterator,
};

use archery::SharedPointerKind;

use crate::nodes::btree::{Children, Leaf, Node};

// ---------------------------------------------------------------------------
// OrdMap
// ---------------------------------------------------------------------------

use super::map::GenericOrdMap;

impl<'a, K, V, P> IntoParallelRefIterator<'a> for GenericOrdMap<K, V, P>
where
    K: Ord + Send + Sync + 'a,
    V: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);
    type Iter = ParIterMap<'a, K, V>;

    fn par_iter(&'a self) -> Self::Iter {
        ParIterMap {
            leaves: collect_leaves(&self.root),
        }
    }
}

/// A parallel iterator over the entries of a [`GenericOrdMap`].
pub struct ParIterMap<'a, K, V> {
    leaves: Vec<&'a Leaf<K, V>>,
}

impl<'a, K, V> ParallelIterator for ParIterMap<'a, K, V>
where
    K: Send + Sync + 'a,
    V: Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        bridge_unindexed(
            MapProducer {
                leaves: self.leaves,
            },
            consumer,
        )
    }
}

struct MapProducer<'a, K, V> {
    leaves: Vec<&'a Leaf<K, V>>,
}

impl<'a, K, V> UnindexedProducer for MapProducer<'a, K, V>
where
    K: Send + Sync + 'a,
    V: Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);

    fn split(self) -> (Self, Option<Self>) {
        if self.leaves.len() >= 2 {
            let mid = self.leaves.len() / 2;
            let mut left = self.leaves;
            let right = left.split_off(mid);
            (
                MapProducer { leaves: left },
                Some(MapProducer { leaves: right }),
            )
        } else {
            (self, None)
        }
    }

    fn fold_with<F>(self, folder: F) -> F
    where
        F: Folder<Self::Item>,
    {
        let iter = self
            .leaves
            .into_iter()
            .flat_map(|leaf| leaf.keys.iter().map(|(k, v)| (k, v)));
        folder.consume_iter(iter)
    }
}

impl<K, V, P> FromParallelIterator<(K, V)> for GenericOrdMap<K, V, P>
where
    K: Ord + Clone + Send + Sync,
    V: Clone + Send + Sync,
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

impl<K, V, P> ParallelExtend<(K, V)> for GenericOrdMap<K, V, P>
where
    K: Ord + Clone + Send + Sync,
    V: Clone + Send + Sync,
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
// OrdSet
// ---------------------------------------------------------------------------

use super::set::GenericOrdSet;

impl<'a, A, P> IntoParallelRefIterator<'a> for GenericOrdSet<A, P>
where
    A: Ord + Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = &'a A;
    type Iter = ParIterSet<'a, A>;

    fn par_iter(&'a self) -> Self::Iter {
        ParIterSet {
            leaves: collect_leaves(&self.map.root),
        }
    }
}

/// A parallel iterator over the elements of a [`GenericOrdSet`].
pub struct ParIterSet<'a, A> {
    leaves: Vec<&'a Leaf<A, ()>>,
}

impl<'a, A> ParallelIterator for ParIterSet<'a, A>
where
    A: Send + Sync + 'a,
{
    type Item = &'a A;

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        bridge_unindexed(
            SetProducer {
                leaves: self.leaves,
            },
            consumer,
        )
    }
}

struct SetProducer<'a, A> {
    leaves: Vec<&'a Leaf<A, ()>>,
}

impl<'a, A> UnindexedProducer for SetProducer<'a, A>
where
    A: Send + Sync + 'a,
{
    type Item = &'a A;

    fn split(self) -> (Self, Option<Self>) {
        if self.leaves.len() >= 2 {
            let mid = self.leaves.len() / 2;
            let mut left = self.leaves;
            let right = left.split_off(mid);
            (
                SetProducer { leaves: left },
                Some(SetProducer { leaves: right }),
            )
        } else {
            (self, None)
        }
    }

    fn fold_with<F>(self, folder: F) -> F
    where
        F: Folder<Self::Item>,
    {
        let iter = self
            .leaves
            .into_iter()
            .flat_map(|leaf| leaf.keys.iter().map(|(a, _)| a));
        folder.consume_iter(iter)
    }
}

impl<A, P> FromParallelIterator<A> for GenericOrdSet<A, P>
where
    A: Ord + Clone + Send + Sync,
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

impl<A, P> ParallelExtend<A> for GenericOrdSet<A, P>
where
    A: Ord + Clone + Send + Sync,
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
// Shared helpers
// ---------------------------------------------------------------------------

/// Collect all leaf references from a B+ tree into a Vec (in order).
fn collect_leaves<K, V, P: SharedPointerKind>(root: &Option<Node<K, V, P>>) -> Vec<&Leaf<K, V>> {
    match root {
        None => Vec::new(),
        Some(Node::Leaf(leaf)) => {
            if leaf.keys.is_empty() {
                Vec::new()
            } else {
                vec![leaf.as_ref()]
            }
        }
        Some(Node::Branch(branch)) => {
            let mut leaves = Vec::new();
            push_leaves(&mut leaves, branch.as_ref());
            leaves
        }
    }
}

/// Recursively collect leaf references from a branch node.
fn push_leaves<'a, K, V, P: SharedPointerKind>(
    out: &mut Vec<&'a Leaf<K, V>>,
    branch: &'a crate::nodes::btree::Branch<K, V, P>,
) {
    match &branch.children {
        Children::Leaves { leaves } => {
            out.extend(
                leaves
                    .iter()
                    .filter(|l| !l.keys.is_empty())
                    .map(|l| l.as_ref()),
            );
        }
        Children::Branches { branches, .. } => {
            for child in branches.iter() {
                push_leaves(out, child.as_ref());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::super::map::OrdMap;
    use super::super::set::OrdSet;
    use ::rayon::iter::{IntoParallelRefIterator, ParallelExtend, ParallelIterator};

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_iter_sum() {
        let map: OrdMap<i64, i64> = (0..10_000i64).map(|i| (i, i)).collect();
        let par_sum: i64 = map.par_iter().map(|(_, &v)| v).sum();
        let seq_sum: i64 = map.iter().map(|(_, &v)| v).sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_iter_count() {
        let map: OrdMap<i32, i32> = (0..10_000).map(|i| (i, i)).collect();
        assert_eq!(map.par_iter().count(), 10_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_iter_empty() {
        let map: OrdMap<i32, i32> = OrdMap::new();
        assert_eq!(map.par_iter().count(), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_iter_collect() {
        let map: OrdMap<i32, i32> = (0..1_000).map(|i| (i, i * 2)).collect();
        let mut pairs: Vec<_> = map.par_iter().map(|(&k, &v)| (k, v)).collect();
        pairs.sort();
        for (k, v) in &pairs {
            assert_eq!(*v, k * 2);
        }
        assert_eq!(pairs.len(), 1_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_from_par_iter() {
        let pairs: Vec<(i32, i32)> = (0..10_000).map(|i| (i, i * 3)).collect();
        let map: OrdMap<i32, i32> = pairs.par_iter().copied().collect();
        assert_eq!(map.len(), 10_000);
        for i in 0..10_000 {
            assert_eq!(map.get(&i), Some(&(i * 3)));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_extend() {
        let mut map: OrdMap<i32, i32> = OrdMap::new();
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
    fn ordset_par_iter_sum() {
        let set: OrdSet<i64> = (0..10_000i64).collect();
        let par_sum: i64 = set.par_iter().copied().sum();
        let seq_sum: i64 = set.iter().copied().sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_iter_count() {
        let set: OrdSet<i32> = (0..10_000).collect();
        assert_eq!(set.par_iter().count(), 10_000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_iter_empty() {
        let set: OrdSet<i32> = OrdSet::new();
        assert_eq!(set.par_iter().count(), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_iter_collect() {
        let set: OrdSet<i32> = (0..1_000).collect();
        let mut vals: Vec<_> = set.par_iter().copied().collect();
        vals.sort();
        assert_eq!(vals, (0..1_000).collect::<Vec<_>>());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_from_par_iter() {
        let vals: Vec<i32> = (0..10_000).collect();
        let set: OrdSet<i32> = vals.par_iter().copied().collect();
        assert_eq!(set.len(), 10_000);
        for i in 0..10_000 {
            assert!(set.contains(&i));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_extend() {
        let mut set: OrdSet<i32> = OrdSet::new();
        set.insert(0);
        set.insert(1);
        let extras: Vec<i32> = (2..1_000).collect();
        set.par_extend(extras.par_iter().copied());
        assert_eq!(set.len(), 1_000);
        assert!(set.contains(&0));
        assert!(set.contains(&999));
    }
}
