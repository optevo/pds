// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Parallel iterators for newer collection types.
//!
//! These are only available when using the `rayon` feature flag.
//!
//! Parallel iterators for [`HashMap`][crate::HashMap], [`HashSet`][crate::HashSet],
//! [`OrdMap`][crate::OrdMap], [`OrdSet`][crate::OrdSet], and [`Vector`][crate::Vector]
//! live in their respective sub-modules (`hash::rayon`, `ord::rayon`, `vector::rayon`).

use core::hash::{BuildHasher, Hash};

use ::rayon::iter::plumbing::UnindexedConsumer;
use ::rayon::iter::{
    FromParallelIterator, IntoParallelIterator, IntoParallelRefIterator, ParallelExtend,
    ParallelIterator,
};

use archery::SharedPointerKind;

use crate::bag::GenericBag;
use crate::bimap::GenericBiMap;
use crate::hash_multimap::GenericHashMultiMap;
use crate::hash_width::HashWidth;
use crate::hashset::GenericHashSet;
use crate::insertion_order_map::GenericInsertionOrderMap;
use crate::insertion_order_set::GenericInsertionOrderSet;
use crate::symmap::GenericSymMap;

// ---------------------------------------------------------------------------
// Bag
// ---------------------------------------------------------------------------

/// A parallel iterator over `(&A, usize)` pairs from a [`GenericBag`].
///
/// Yields each distinct element alongside its count. To iterate each element
/// once per occurrence (expanding counts), use [`GenericBag::par_elements`].
pub struct BagParIter<'a, A, P: SharedPointerKind> {
    inner: crate::hash::rayon::ParIterMap<'a, A, usize, P>,
}

impl<'a, A, P> ParallelIterator for BagParIter<'a, A, P>
where
    A: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a A, usize);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        // Copy the count out of the reference so the item matches the
        // sequential `iter()` signature of `(&A, usize)`.
        self.inner
            .map(|(a, count)| (a, *count))
            .drive_unindexed(consumer)
    }
}

impl<'a, A, S, P> IntoParallelRefIterator<'a> for GenericBag<A, S, P>
where
    A: Hash + Eq + Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a A, usize);
    type Iter = BagParIter<'a, A, P>;

    fn par_iter(&'a self) -> Self::Iter {
        BagParIter {
            inner: self.map.par_iter(),
        }
    }
}

impl<A, S, P> GenericBag<A, S, P>
where
    A: Hash + Eq + Send + Sync,
    S: Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Parallel iterator that yields each element once per occurrence.
    ///
    /// Unlike [`par_iter`][IntoParallelRefIterator::par_iter], which yields
    /// `(&A, usize)` pairs (one entry per distinct element), this expands each
    /// element its count times — treating the bag as a flat multiset.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "std", feature = "rayon"))] {
    /// use pds::Bag;
    /// use rayon::iter::ParallelIterator;
    ///
    /// let mut bag = Bag::new();
    /// bag.insert_many('a', 3);
    /// bag.insert_many('b', 2);
    ///
    /// let mut chars: Vec<char> = bag.par_elements().copied().collect();
    /// chars.sort();
    /// assert_eq!(chars, vec!['a', 'a', 'a', 'b', 'b']);
    /// # }
    /// ```
    pub fn par_elements(&self) -> impl ParallelIterator<Item = &A> + '_ {
        self.par_iter()
            .flat_map(|(a, count)| ::rayon::iter::repeat_n(a, count))
    }
}

impl<A, S, P> FromParallelIterator<A> for GenericBag<A, S, P>
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
            .fold(Self::default, |mut bag, a| {
                bag.insert(a);
                bag
            })
            .reduce(Self::default, |a, b| a.union(&b))
    }
}

impl<A, S, P> ParallelExtend<A> for GenericBag<A, S, P>
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
        *self = self.union(&collected);
    }
}

// ---------------------------------------------------------------------------
// HashMultiMap
// ---------------------------------------------------------------------------

impl<'a, K, V, S, P> IntoParallelRefIterator<'a> for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Send + Sync + 'a,
    V: Hash + Eq + Send + Sync + 'a,
    S: BuildHasher + Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a GenericHashSet<V, S, P>);
    // Delegate directly to the underlying map's ParIterMap — the item type
    // matches since par_iter over GenericHashMap<K, HashSet<V>> yields
    // (&K, &HashSet<V>) pairs.
    type Iter = crate::hash::rayon::ParIterMap<'a, K, GenericHashSet<V, S, P>, P>;

    fn par_iter(&'a self) -> Self::Iter {
        self.map.par_iter()
    }
}

impl<K, V, S, P> FromParallelIterator<(K, V)> for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn from_par_iter<I>(par_iter: I) -> Self
    where
        I: IntoParallelIterator<Item = (K, V)>,
    {
        par_iter
            .into_par_iter()
            .fold(Self::default, |mut m, (k, v)| {
                m.insert(k, v);
                m
            })
            .reduce(Self::default, |a, b| a.union(b))
    }
}

impl<K, V, S, P> ParallelExtend<(K, V)> for GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Eq + Clone + Send + Sync,
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
// BiMap
// ---------------------------------------------------------------------------

impl<'a, K, V, S, P> IntoParallelRefIterator<'a> for GenericBiMap<K, V, S, P>
where
    K: Hash + Eq + Send + Sync + 'a,
    V: Hash + Eq + Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);
    // Delegate to the forward map — its ParIterMap already yields (&K, &V) pairs.
    type Iter = crate::hash::rayon::ParIterMap<'a, K, V, P>;

    fn par_iter(&'a self) -> Self::Iter {
        self.forward.par_iter()
    }
}

impl<K, V, S, P> FromParallelIterator<(K, V)> for GenericBiMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn from_par_iter<I>(par_iter: I) -> Self
    where
        I: IntoParallelIterator<Item = (K, V)>,
    {
        par_iter
            .into_par_iter()
            .fold(Self::default, |mut m, (k, v)| {
                m.insert(k, v);
                m
            })
            .reduce(Self::default, |mut a, b| {
                a.extend(b);
                a
            })
    }
}

impl<K, V, S, P> ParallelExtend<(K, V)> for GenericBiMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn par_extend<I>(&mut self, par_iter: I)
    where
        I: IntoParallelIterator<Item = (K, V)>,
    {
        let collected: Self = par_iter.into_par_iter().collect();
        self.extend(collected);
    }
}

// ---------------------------------------------------------------------------
// SymMap
// ---------------------------------------------------------------------------

impl<'a, A, S, P> IntoParallelRefIterator<'a> for GenericSymMap<A, S, P>
where
    A: Hash + Eq + Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
{
    type Item = (&'a A, &'a A);
    // Delegate to the forward map — yields (&A, &A) pairs for each edge.
    type Iter = crate::hash::rayon::ParIterMap<'a, A, A, P>;

    fn par_iter(&'a self) -> Self::Iter {
        self.forward.par_iter()
    }
}

impl<A, S, P> FromParallelIterator<(A, A)> for GenericSymMap<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn from_par_iter<I>(par_iter: I) -> Self
    where
        I: IntoParallelIterator<Item = (A, A)>,
    {
        par_iter
            .into_par_iter()
            .fold(Self::default, |mut m, (a, b)| {
                m.insert(a, b);
                m
            })
            .reduce(Self::default, |mut a, b| {
                a.extend(b);
                a
            })
    }
}

impl<A, S, P> ParallelExtend<(A, A)> for GenericSymMap<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    fn par_extend<I>(&mut self, par_iter: I)
    where
        I: IntoParallelIterator<Item = (A, A)>,
    {
        let collected: Self = par_iter.into_par_iter().collect();
        self.extend(collected);
    }
}

// ---------------------------------------------------------------------------
// InsertionOrderMap — read-only parallel iteration only
//
// FromParallelIterator and ParallelExtend are intentionally absent: parallel
// collection destroys insertion order. Use the sequential FromIterator/Extend
// impls when insertion order must be preserved.
// ---------------------------------------------------------------------------

/// A parallel iterator over `(&K, &V)` pairs from a [`GenericInsertionOrderMap`].
///
/// Note: parallel iteration does not preserve insertion order. Each worker
/// thread processes a subset of the underlying B+ tree leaves.
pub struct InsertionOrderMapParIter<'a, K, V> {
    inner: crate::ord::rayon::ParIterMap<'a, usize, (K, V)>,
}

impl<'a, K, V> ParallelIterator for InsertionOrderMapParIter<'a, K, V>
where
    K: Send + Sync + 'a,
    V: Send + Sync + 'a,
{
    type Item = (&'a K, &'a V);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        // Map (&usize, &(K, V)) → (&K, &V) by projecting out the index key.
        self.inner
            .map(|(_, kv)| (&kv.0, &kv.1))
            .drive_unindexed(consumer)
    }
}

impl<'a, K, V, S, P, H> IntoParallelRefIterator<'a> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Send + Sync + 'a,
    V: Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
    H: HashWidth + 'a,
{
    type Item = (&'a K, &'a V);
    type Iter = InsertionOrderMapParIter<'a, K, V>;

    fn par_iter(&'a self) -> Self::Iter {
        InsertionOrderMapParIter {
            inner: self.entries.par_iter(),
        }
    }
}

// ---------------------------------------------------------------------------
// InsertionOrderSet — read-only parallel iteration only
//
// Same rationale as InsertionOrderMap: FromParallelIterator/ParallelExtend
// are absent because they would lose insertion order.
// ---------------------------------------------------------------------------

/// A parallel iterator over `&A` references from a [`GenericInsertionOrderSet`].
///
/// Note: parallel iteration does not preserve insertion order.
pub struct InsertionOrderSetParIter<'a, A> {
    inner: InsertionOrderMapParIter<'a, A, ()>,
}

impl<'a, A> ParallelIterator for InsertionOrderSetParIter<'a, A>
where
    A: Send + Sync + 'a,
{
    type Item = &'a A;

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        // Strip the unit value from the (&A, &()) pair.
        self.inner.map(|(a, _)| a).drive_unindexed(consumer)
    }
}

impl<'a, A, S, P, H> IntoParallelRefIterator<'a> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Send + Sync + 'a,
    S: Send + Sync + 'a,
    P: SharedPointerKind + Send + Sync + 'a,
    H: HashWidth + 'a,
{
    type Item = &'a A;
    type Iter = InsertionOrderSetParIter<'a, A>;

    fn par_iter(&'a self) -> Self::Iter {
        InsertionOrderSetParIter {
            inner: self.map.par_iter(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ::rayon::iter::{IntoParallelRefIterator, ParallelExtend, ParallelIterator};

    // --- Bag ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_iter_sum_matches_seq() {
        use crate::bag::Bag;
        let mut bag = Bag::new();
        for i in 0..1_000i64 {
            bag.insert_many(i, (i % 5 + 1) as usize);
        }
        let par: i64 = bag.par_iter().map(|(&v, count)| v * count as i64).sum();
        let seq: i64 = bag.iter().map(|(&v, count)| v * count as i64).sum();
        assert_eq!(par, seq);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_elements_count() {
        use crate::bag::Bag;
        let mut bag = Bag::new();
        bag.insert_many(1i32, 3);
        bag.insert_many(2i32, 2);
        assert_eq!(bag.par_elements().count(), 5);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_from_par_iter() {
        use crate::bag::Bag;
        let items = vec![1i32, 2, 1, 3, 2, 1];
        let bag: Bag<i32> = items.into_par_iter().collect();
        assert_eq!(bag.count(&1), 3);
        assert_eq!(bag.count(&2), 2);
        assert_eq!(bag.count(&3), 1);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_extend() {
        use crate::bag::Bag;
        let mut bag: Bag<i32> = vec![1, 2, 1].into_iter().collect();
        bag.par_extend(vec![1i32, 3, 3].into_par_iter());
        assert_eq!(bag.count(&1), 3);
        assert_eq!(bag.count(&2), 1);
        assert_eq!(bag.count(&3), 2);
    }

    // --- HashMultiMap ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hash_multimap_par_iter_count() {
        use crate::hash_multimap::HashMultiMap;
        let mut m = HashMultiMap::new();
        m.insert("a", 1);
        m.insert("a", 2);
        m.insert("b", 3);
        // par_iter yields one entry per key
        assert_eq!(m.par_iter().count(), 2);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hash_multimap_par_iter_total_values() {
        use crate::hash_multimap::HashMultiMap;
        let mut m = HashMultiMap::new();
        for i in 0..100i32 {
            m.insert(i % 10, i);
        }
        // Each key holds a HashSet; count the total values across all sets.
        let par_total: usize = m.par_iter().map(|(_, set)| set.len()).sum();
        let seq_total: usize = m.iter_sets().map(|(_, set)| set.len()).sum();
        assert_eq!(par_total, seq_total);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hash_multimap_from_par_iter() {
        use crate::hash_multimap::HashMultiMap;
        let pairs: Vec<(i32, i32)> = (0..100).map(|i| (i % 10, i)).collect();
        let m: HashMultiMap<i32, i32> = pairs.into_par_iter().collect();
        // len() returns total key-value pairs; keys().count() returns distinct keys
        assert_eq!(m.len(), 100);
        assert_eq!(m.keys().count(), 10);
    }

    // --- BiMap ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bimap_par_iter_count() {
        use crate::bimap::BiMap;
        let m: BiMap<i32, &str> = vec![(1, "a"), (2, "b"), (3, "c")].into_iter().collect();
        assert_eq!(m.par_iter().count(), 3);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bimap_par_iter_matches_seq() {
        use crate::bimap::BiMap;
        let m: BiMap<i32, i32> = (0..500).map(|i| (i, i * 2)).collect();
        let par_sum: i32 = m.par_iter().map(|(&k, _)| k).sum();
        let seq_sum: i32 = m.iter().map(|(&k, _)| k).sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bimap_from_par_iter() {
        use crate::bimap::BiMap;
        let pairs: Vec<(i32, i32)> = (0..1_000).map(|i| (i, i * 10)).collect();
        let m: BiMap<i32, i32> = pairs.into_par_iter().collect();
        assert_eq!(m.len(), 1_000);
        assert_eq!(m.get_by_key(&5), Some(&50));
        assert_eq!(m.get_by_value(&50), Some(&5));
    }

    // --- SymMap ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn symmap_par_iter_count() {
        use crate::symmap::SymMap;
        let mut m = SymMap::new();
        m.insert(1i32, 2);
        m.insert(3, 4);
        // forward map only — half the total pairs
        assert_eq!(m.par_iter().count(), 2);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn symmap_from_par_iter() {
        use crate::symmap::Direction;
        use crate::symmap::SymMap;
        let pairs: Vec<(i32, i32)> = (0..500).map(|i| (i, i + 500)).collect();
        let m: SymMap<i32> = pairs.into_par_iter().collect();
        assert_eq!(m.len(), 500);
        assert_eq!(m.get(Direction::Forward, &0), Some(&500));
        assert_eq!(m.get(Direction::Backward, &500), Some(&0));
    }

    // --- InsertionOrderMap ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn insertion_order_map_par_iter_sum_matches_seq() {
        use crate::insertion_order_map::InsertionOrderMap;
        let m: InsertionOrderMap<i32, i32> = (0..1_000).map(|i| (i, i * 2)).collect();
        let par_sum: i32 = m.par_iter().map(|(_, &v)| v).sum();
        let seq_sum: i32 = m.iter().map(|(_, v)| *v).sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn insertion_order_map_par_iter_count() {
        use crate::insertion_order_map::InsertionOrderMap;
        let m: InsertionOrderMap<i32, i32> = (0..500).map(|i| (i, i)).collect();
        assert_eq!(m.par_iter().count(), 500);
    }

    // --- InsertionOrderSet ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn insertion_order_set_par_iter_sum_matches_seq() {
        use crate::insertion_order_set::InsertionOrderSet;
        let s: InsertionOrderSet<i32> = (0..1_000).collect();
        let par_sum: i32 = s.par_iter().copied().sum();
        let seq_sum: i32 = s.iter().copied().sum();
        assert_eq!(par_sum, seq_sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn insertion_order_set_par_iter_count() {
        use crate::insertion_order_set::InsertionOrderSet;
        let s: InsertionOrderSet<i32> = (0..500).collect();
        assert_eq!(s.par_iter().count(), 500);
    }
}
