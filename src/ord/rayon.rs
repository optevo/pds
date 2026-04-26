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

use archery::{SharedPointer, SharedPointerKind};
use imbl_sized_chunks::Chunk;

use crate::nodes::btree::{Branch, Children, Leaf, Node};

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
// OrdMap — tree-native par_map_values helper
//
// Similar to the HAMT version: walking the B+ tree directly avoids O(n log n)
// insert-based reconstruction and replaces it with O(n) leaf traversal.
//
// Structure:
//  - Leaf nodes: dense (K, V) arrays → transform values, clone keys.
//  - Branch nodes: separator keys (unchanged) + children (recurse).
//
// The root-level branch children are processed in parallel via rayon; child
// branches recurse sequentially (tree depth is typically 2–4).
// ---------------------------------------------------------------------------

/// Parallel map-values on a B+ tree node. Forks at the top-level children
/// of a Branch; Leaf nodes and deeper children are handled sequentially.
fn par_map_values_ord_node<K, V, V2, P, F>(node: &Node<K, V, P>, f: &F) -> Node<K, V2, P>
where
    K: Clone + Ord + Send + Sync,
    V: Clone + Send + Sync,
    V2: Clone + Send + Sync,
    P: SharedPointerKind + Send + Sync,
    F: Fn(&K, &V) -> V2 + Send + Sync,
{
    match node {
        // Leaf: no parallelism to exploit; delegate to the sequential helper.
        Node::Leaf(_) => node.map_values(f),
        Node::Branch(branch) => {
            let new_children = match &branch.children {
                Children::Leaves { leaves } => {
                    // Process all leaf children in parallel.
                    let new_vec: Vec<SharedPointer<Leaf<K, V2>, P>> = leaves
                        .as_slice()
                        .par_iter()
                        .map(|leaf_ptr| SharedPointer::new(leaf_ptr.map_values(f)))
                        .collect();
                    let mut chunk = Chunk::new();
                    for l in new_vec {
                        chunk.push_back(l);
                    }
                    Children::Leaves { leaves: chunk }
                }
                Children::Branches { branches, level } => {
                    // Process all branch children in parallel; each recurses
                    // sequentially for deeper levels.
                    let new_vec: Vec<SharedPointer<Branch<K, V2, P>, P>> = branches
                        .as_slice()
                        .par_iter()
                        .map(|b_ptr| SharedPointer::new(b_ptr.map_values(f)))
                        .collect();
                    let mut chunk = Chunk::new();
                    for b in new_vec {
                        chunk.push_back(b);
                    }
                    Children::Branches {
                        branches: chunk,
                        level: *level,
                    }
                }
            };
            Node::Branch(SharedPointer::new(Branch {
                keys: branch.keys.clone(),
                children: new_children,
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// OrdMap — parallel bulk operations
// ---------------------------------------------------------------------------

impl<K, V, P> GenericOrdMap<K, V, P>
where
    K: Ord + Clone + Send + Sync,
    V: Clone + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Construct the union of two maps in parallel.
    ///
    /// Values from `self` take precedence for keys present in both maps.
    ///
    /// This is the parallel equivalent of [`union`][GenericOrdMap::union].
    ///
    /// Time: O(n log n / p)
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        let to_add: Self = other
            .par_iter()
            .filter_map(|(k, v)| {
                if self.contains_key(k) {
                    None
                } else {
                    Some((k.clone(), v.clone()))
                }
            })
            .fold(Self::default, |mut acc, (k, v)| {
                acc.insert(k, v);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b));
        self.union(to_add)
    }

    /// Construct the intersection of two maps in parallel, keeping values from `self`.
    ///
    /// This is the parallel equivalent of [`intersection`][GenericOrdMap::intersection].
    ///
    /// Time: O(min(n, m) log max(n, m) / p)
    #[must_use]
    pub fn par_intersection(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
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
                    acc.insert(k, v);
                    acc
                })
                .reduce(Self::default, |a, b| a.union(b))
        } else {
            other
                .par_iter()
                .filter_map(|(k, _)| self.get(k).map(|v| (k.clone(), v.clone())))
                .fold(Self::default, |mut acc, (k, v)| {
                    acc.insert(k, v);
                    acc
                })
                .reduce(Self::default, |a, b| a.union(b))
        }
    }

    /// Construct the relative complement (self − other) in parallel:
    /// keys in `self` not present in `other`.
    ///
    /// This is the parallel equivalent of [`difference`][GenericOrdMap::difference].
    ///
    /// Time: O(n log m / p)
    #[must_use]
    pub fn par_difference(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return self;
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
                acc.insert(k, v);
                acc
            })
            .reduce(Self::default, |a, b| a.union(b))
    }

    /// Construct the symmetric difference of two maps in parallel:
    /// keys present in exactly one of the two maps.
    ///
    /// This is the parallel equivalent of
    /// [`symmetric_difference`][GenericOrdMap::symmetric_difference].
    ///
    /// Time: O((n + m) log max(n, m) / p)
    #[must_use]
    pub fn par_symmetric_difference(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
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
                        acc.insert(k, v);
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
                        acc.insert(k, v);
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(b))
            },
        );
        left.union(right)
    }

    /// Return a new map keeping only entries that satisfy the predicate.
    ///
    /// Evaluates `f(&key, &value)` for every entry in parallel; returns a new
    /// map containing only those where `f` returns `true`. The original map
    /// is unchanged.
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

    /// Return a new map with values transformed by `f`, applied in parallel.
    ///
    /// Keys are unchanged; each value is replaced by `f(&value)`. Equivalent
    /// to the sequential [`map_values`][Self::map_values] but evaluated in
    /// parallel.
    ///
    /// **Implementation-optimised:** walks the B+ tree directly, processing
    /// leaf children in parallel via rayon. Because keys are not modified, the
    /// tree topology (separator keys, node structure) is preserved without
    /// re-insertion or re-sorting. This is O(n / p) vs O(n / p + n log n)
    /// for a collect-based approach.
    ///
    /// Time: O(n / p)
    #[must_use]
    pub fn par_map_values<V2, F>(&self, f: F) -> GenericOrdMap<K, V2, P>
    where
        V2: Clone + Send + Sync,
        F: Fn(&V) -> V2 + Sync + Send,
    {
        if self.is_empty() {
            return GenericOrdMap::default();
        }
        let g = |_k: &K, v: &V| f(v);
        let new_root = self
            .root
            .as_ref()
            .map(|node| par_map_values_ord_node(node, &g));
        GenericOrdMap {
            root: new_root,
            size: self.size,
        }
    }

    /// Return a new map with values transformed by `f(key, value)`, applied in
    /// parallel.
    ///
    /// Each entry's value is replaced by `f(&key, &value)`. Equivalent to
    /// the sequential [`map_values_with_key`][Self::map_values_with_key].
    ///
    /// **Implementation-optimised:** walks the B+ tree directly, processing
    /// leaf children in parallel via rayon. Because keys are not modified, the
    /// tree topology (separator keys, node structure) is preserved without
    /// re-insertion or re-sorting. This is O(n / p) vs O(n / p + n log n)
    /// for a collect-based approach.
    ///
    /// Time: O(n / p)
    #[must_use]
    pub fn par_map_values_with_key<V2, F>(&self, f: F) -> GenericOrdMap<K, V2, P>
    where
        V2: Clone + Send + Sync,
        F: Fn(&K, &V) -> V2 + Sync + Send,
    {
        if self.is_empty() {
            return GenericOrdMap::default();
        }
        let new_root = self
            .root
            .as_ref()
            .map(|node| par_map_values_ord_node(node, &f));
        GenericOrdMap {
            root: new_root,
            size: self.size,
        }
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
// OrdSet — parallel bulk operations
// ---------------------------------------------------------------------------

impl<A, P> GenericOrdSet<A, P>
where
    A: Ord + Clone + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Construct the union of two sets in parallel.
    ///
    /// This is the parallel equivalent of [`union`][GenericOrdSet::union].
    ///
    /// Time: O(n log n / p)
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        let to_add: Self = other
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
        self.union(to_add)
    }

    /// Construct the intersection of two sets in parallel.
    ///
    /// This is the parallel equivalent of [`intersection`][GenericOrdSet::intersection].
    ///
    /// Time: O(min(n, m) log max(n, m) / p)
    #[must_use]
    pub fn par_intersection(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
        }
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
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
    /// This is the parallel equivalent of [`difference`][GenericOrdSet::difference].
    ///
    /// Time: O(n log m / p)
    #[must_use]
    pub fn par_difference(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return self;
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

    /// Construct the symmetric difference of two sets in parallel:
    /// elements in exactly one of the two sets.
    ///
    /// This is the parallel equivalent of
    /// [`symmetric_difference`][GenericOrdSet::symmetric_difference].
    ///
    /// Time: O((n + m) log max(n, m) / p)
    #[must_use]
    pub fn par_symmetric_difference(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
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

    /// Return a new set keeping only elements that satisfy the predicate.
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

    // --- Parallel bulk operations ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_union() {
        let map1: OrdMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: OrdMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().union(map2.clone());
        let par = map1.par_union(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_intersection() {
        let map1: OrdMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: OrdMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().intersection(map2.clone());
        let par = map1.par_intersection(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_difference() {
        let map1: OrdMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: OrdMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().difference(map2.clone());
        let par = map1.par_difference(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_symmetric_difference() {
        let map1: OrdMap<i32, i32> = (0..5_000).map(|i| (i, i)).collect();
        let map2: OrdMap<i32, i32> = (2_500..7_500).map(|i| (i, i * 10)).collect();
        let seq = map1.clone().symmetric_difference(map2.clone());
        let par = map1.par_symmetric_difference(map2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordmap_par_union_empty() {
        let map: OrdMap<i32, i32> = (0..1_000).map(|i| (i, i)).collect();
        let empty: OrdMap<i32, i32> = OrdMap::new();
        assert_eq!(map.clone().par_union(empty.clone()), map.clone());
        assert_eq!(empty.clone().par_union(map.clone()), map);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_union() {
        let set1: OrdSet<i32> = (0..5_000).collect();
        let set2: OrdSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().union(set2.clone());
        let par = set1.par_union(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_intersection() {
        let set1: OrdSet<i32> = (0..5_000).collect();
        let set2: OrdSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().intersection(set2.clone());
        let par = set1.par_intersection(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_difference() {
        let set1: OrdSet<i32> = (0..5_000).collect();
        let set2: OrdSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().difference(set2.clone());
        let par = set1.par_difference(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_symmetric_difference() {
        let set1: OrdSet<i32> = (0..5_000).collect();
        let set2: OrdSet<i32> = (2_500..7_500).collect();
        let seq = set1.clone().symmetric_difference(set2.clone());
        let par = set1.par_symmetric_difference(set2);
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn ordset_par_intersection_disjoint() {
        let set1: OrdSet<i32> = (0..1_000).collect();
        let set2: OrdSet<i32> = (1_000..2_000).collect();
        assert!(set1.par_intersection(set2).is_empty());
    }

    #[test]
    fn ordmap_par_filter_keeps_matching() {
        let map: OrdMap<i32, i32> = (0..100).map(|i| (i, i * 2)).collect();
        let evens = map.par_filter(|k, _| k % 2 == 0);
        assert_eq!(evens.len(), 50);
        assert!(evens.keys().all(|k| k % 2 == 0));
    }

    #[test]
    fn ordmap_par_filter_empty() {
        let map: OrdMap<i32, i32> = OrdMap::new();
        assert!(map.par_filter(|_, _| true).is_empty());
    }

    #[test]
    fn ordmap_par_filter_none_match() {
        let map: OrdMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        assert!(map.par_filter(|_, _| false).is_empty());
    }

    #[test]
    fn ordmap_par_filter_all_match() {
        let map: OrdMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        assert_eq!(map.par_filter(|_, _| true), map);
    }

    #[test]
    fn ordmap_par_map_values_doubles() {
        let map: OrdMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let doubled = map.par_map_values(|v| v * 2);
        for i in 0..100 {
            assert_eq!(doubled.get(&i), Some(&(i * 2)));
        }
    }

    #[test]
    fn ordmap_par_map_values_empty() {
        let map: OrdMap<i32, i32> = OrdMap::new();
        assert!(map.par_map_values(|v| v * 2).is_empty());
    }

    #[test]
    fn ordmap_par_map_values_type_change() {
        let map: OrdMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let strings: OrdMap<i32, String> = map.par_map_values(|v| v.to_string());
        assert_eq!(strings.get(&42), Some(&"42".to_string()));
    }

    #[test]
    fn ordmap_par_map_values_with_key() {
        let map: OrdMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let result = map.par_map_values_with_key(|k, v| k + v);
        for i in 0..100_i32 {
            assert_eq!(result.get(&i), Some(&(i + i)));
        }
    }

    #[test]
    fn ordmap_par_map_values_with_key_empty() {
        let map: OrdMap<i32, i32> = OrdMap::new();
        assert!(map.par_map_values_with_key(|k, v| k + v).is_empty());
    }

    #[test]
    fn ordset_par_filter_keeps_matching() {
        let set: OrdSet<i32> = (0..100).collect();
        let evens = set.par_filter(|x| x % 2 == 0);
        assert_eq!(evens.len(), 50);
        assert!(evens.iter().all(|x| x % 2 == 0));
    }

    #[test]
    fn ordset_par_filter_empty() {
        let set: OrdSet<i32> = OrdSet::new();
        assert!(set.par_filter(|_| true).is_empty());
    }

    #[test]
    fn ordset_par_filter_none_match() {
        let set: OrdSet<i32> = (0..50).collect();
        assert!(set.par_filter(|_| false).is_empty());
    }

    #[test]
    fn ordset_par_filter_all_match() {
        let set: OrdSet<i32> = (0..50).collect();
        assert_eq!(set.par_filter(|_| true), set);
    }
}
