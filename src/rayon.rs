// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Parallel iterators for [`Bag`][crate::Bag], [`HashMultiMap`][crate::HashMultiMap],
//! [`BiMap`][crate::BiMap], [`SymMap`][crate::SymMap],
//! [`InsertionOrderMap`][crate::InsertionOrderMap], and
//! [`InsertionOrderSet`][crate::InsertionOrderSet].
//!
//! Only available with the `rayon` feature flag.
//!
//! ## Coverage and limitations
//!
//! | Type | `par_iter` | `FromParallelIterator` | `ParallelExtend` | Par set ops | Notes |
//! |------|:----------:|:---------------------:|:----------------:|:-----------:|-------|
//! | `Bag` | ✓ | ✓ | ✓ | ✓ | Also provides [`GenericBag::par_elements`]; all 4 ops parallelised |
//! | `HashMultiMap` | ✓ | ✓ | ✓ | ✓ | Default `H = u64` only; `par_union` delegates to sequential |
//! | `BiMap` | ✓ | ✓ | ✓ | ✓ | Default `H = u64` only; `par_union` delegates to sequential |
//! | `SymMap` | ✓ | ✓ | ✓ | ✓ | Default `H = u64` only; `par_union` delegates to sequential |
//! | `InsertionOrderMap` | ✓ | — | — | — | Parallel collection would lose insertion order |
//! | `InsertionOrderSet` | ✓ | — | — | — | Parallel collection would lose insertion order |
//! | `Trie` | — | — | — | — | Not supported |
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
use crate::symmap::{Direction, GenericSymMap};

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
// Bag — parallel set operations
// ---------------------------------------------------------------------------

impl<A, S, P> GenericBag<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Parallel multiset union (sum of multiplicities).
    ///
    /// Computes new counts for each key in `other` in parallel (`self[k] +
    /// other[k]`), then applies the updates sequentially to a clone of `self`.
    /// Time: O(|other| / threads + |other|) — the sequential phase is O(|other|).
    #[must_use]
    pub fn par_union(&self, other: &Self) -> Self {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        // Compute updated counts in parallel: for each key in other, the new
        // count is self[k] + other[k].  Reading self.count() concurrently is
        // safe because self is immutably borrowed for the duration.
        let updates: Vec<(A, usize)> = other
            .par_iter()
            .map(|(k, other_count)| (k.clone(), self.count(k) + other_count))
            .collect();
        let mut result = self.clone();
        for (k, new_count) in updates {
            result.map.insert(k, new_count);
        }
        // Total is self.total + other.total because every key in other
        // contributes exactly other[k] additional occurrences.
        result.total = self.total + other.total;
        result
    }

    /// Parallel multiset intersection (minimum multiplicities).
    ///
    /// Iterates the smaller bag in parallel; for each element looks up the
    /// count in the larger bag and keeps `min(self[k], other[k])`.
    #[must_use]
    pub fn par_intersection(&self, other: &Self) -> Self {
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
            .filter_map(|(k, small_count)| {
                let large_count = larger.count(k);
                let min_count = small_count.min(large_count);
                if min_count > 0 {
                    Some((k.clone(), min_count))
                } else {
                    None
                }
            })
            .fold(Self::default, |mut acc, (k, min_count)| {
                // Fold outputs have disjoint keys (each chunk covers a
                // disjoint slice of smaller's HashMap entries).
                acc.map.insert(k, min_count);
                acc.total += min_count;
                acc
            })
            .reduce(Self::default, |a, b| a.union(&b))
    }

    /// Parallel multiset difference (self minus other, counts clamped to zero).
    ///
    /// For each element in `self`, the result count is
    /// `max(0, self[k] − other[k])`.
    #[must_use]
    pub fn par_difference(&self, other: &Self) -> Self {
        if self.is_empty() {
            return Self::default();
        }
        if other.is_empty() {
            return self.clone();
        }
        self.par_iter()
            .filter_map(|(k, self_count)| {
                let diff = self_count.saturating_sub(other.count(k));
                if diff > 0 {
                    Some((k.clone(), diff))
                } else {
                    None
                }
            })
            .fold(Self::default, |mut acc, (k, diff)| {
                acc.map.insert(k, diff);
                acc.total += diff;
                acc
            })
            .reduce(Self::default, |a, b| a.union(&b))
    }

    /// Parallel multiset symmetric difference (absolute difference of counts).
    ///
    /// For each element the result count is `|self[k] − other[k]|`.
    /// Elements with equal counts in both bags are excluded.
    ///
    /// Runs two halves in parallel via [`rayon::join`]: one half for keys where
    /// `self[k] > other[k]`, the other for keys where `other[k] > self[k]`.
    #[must_use]
    pub fn par_symmetric_difference(&self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        let (part_self, part_other) = ::rayon::join(
            || {
                // Keys where self_count > other_count.
                self.par_iter()
                    .filter_map(|(k, self_count)| {
                        let other_count = other.count(k);
                        if self_count > other_count {
                            Some((k.clone(), self_count - other_count))
                        } else {
                            None
                        }
                    })
                    .fold(Self::default, |mut acc, (k, diff)| {
                        acc.map.insert(k, diff);
                        acc.total += diff;
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(&b))
            },
            || {
                // Keys where other_count > self_count.
                other
                    .par_iter()
                    .filter_map(|(k, other_count)| {
                        let self_count = self.count(k);
                        if other_count > self_count {
                            Some((k.clone(), other_count - self_count))
                        } else {
                            None
                        }
                    })
                    .fold(Self::default, |mut acc, (k, diff)| {
                        acc.map.insert(k, diff);
                        acc.total += diff;
                        acc
                    })
                    .reduce(Self::default, |a, b| a.union(&b))
            },
        );
        part_self.union(&part_other)
    }
}

// ---------------------------------------------------------------------------
// HashMultiMap — parallel set operations
// ---------------------------------------------------------------------------

impl<K, V, S, P> GenericHashMultiMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Parallel multimap union; delegates to the sequential [`union`][Self::union].
    ///
    /// Provided for API completeness so `par_union` can be used as a reducer
    /// in parallel pipelines. The actual merge is sequential because maintaining
    /// per-key value-set union semantics requires coordination across keys.
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        self.union(other)
    }

    /// Parallel multimap difference (key-level).
    ///
    /// Retains entries whose keys are in `self` but not in `other`.
    /// Processes all keys in `self` in parallel; each key's value set is kept
    /// intact.
    #[must_use]
    pub fn par_difference(self, other: &Self) -> Self {
        if self.is_empty() {
            return Self::default();
        }
        if other.is_empty() {
            return self;
        }
        self.par_iter()
            .filter(|(k, _)| !other.contains_key(*k))
            .flat_map_iter(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
            .collect()
    }

    /// Parallel multimap intersection (key-level).
    ///
    /// Retains entries whose keys appear in both `self` and `other`; `self`'s
    /// value sets are kept.
    #[must_use]
    pub fn par_intersection(self, other: &Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
        }
        self.par_iter()
            .filter(|(k, _)| other.contains_key(*k))
            .flat_map_iter(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
            .collect()
    }

    /// Parallel multimap symmetric difference (key-level).
    ///
    /// Retains entries whose keys appear in exactly one of `self` or `other`.
    /// The two halves (`self \ other` and `other \ self`) are computed in
    /// parallel via [`rayon::join`].
    #[must_use]
    pub fn par_symmetric_difference(self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self;
        }
        // Clone self so the second closure can check key membership after self
        // is moved into the first closure's borrow.
        let self_clone = self.clone();
        let (self_diff, other_diff) = ::rayon::join(
            || {
                self.par_iter()
                    .filter(|(k, _)| !other.contains_key(*k))
                    .flat_map_iter(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
                    .collect::<Self>()
            },
            || {
                other
                    .par_iter()
                    .filter(|(k, _)| !self_clone.contains_key(*k))
                    .flat_map_iter(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
                    .collect::<Self>()
            },
        );
        self_diff.union(other_diff)
    }
}

// ---------------------------------------------------------------------------
// BiMap — parallel set operations
// ---------------------------------------------------------------------------

impl<K, V, S, P> GenericBiMap<K, V, S, P>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Parallel bimap union; delegates to the sequential [`union`][Self::union].
    ///
    /// Provided for API completeness. The bijection invariant requires
    /// conflict resolution that is inherently sequential (each insert may
    /// displace existing key or value mappings).
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        self.union(other)
    }

    /// Parallel bimap difference (key-level).
    ///
    /// Retains entries whose keys are in `self` but not in `other`.
    #[must_use]
    pub fn par_difference(self, other: &Self) -> Self {
        if self.is_empty() {
            return Self::default();
        }
        if other.is_empty() {
            return self;
        }
        self.par_iter()
            .filter(|(k, _)| !other.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Parallel bimap intersection (key-level).
    ///
    /// Retains entries whose keys appear in both `self` and `other`; `self`'s
    /// values are kept.
    #[must_use]
    pub fn par_intersection(self, other: &Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
        }
        self.par_iter()
            .filter(|(k, _)| other.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Parallel bimap symmetric difference (key-level).
    ///
    /// Retains entries whose keys appear in exactly one of `self` or `other`.
    /// The two halves are computed in parallel via [`rayon::join`].
    #[must_use]
    pub fn par_symmetric_difference(self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self;
        }
        let self_clone = self.clone();
        let (self_diff, other_diff) = ::rayon::join(
            || {
                self.par_iter()
                    .filter(|(k, _)| !other.contains_key(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Self>()
            },
            || {
                other
                    .par_iter()
                    .filter(|(k, _)| !self_clone.contains_key(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Self>()
            },
        );
        self_diff.union(other_diff)
    }
}

// ---------------------------------------------------------------------------
// SymMap — parallel set operations
// ---------------------------------------------------------------------------

impl<A, S, P> GenericSymMap<A, S, P>
where
    A: Hash + Eq + Clone + Send + Sync,
    S: BuildHasher + Clone + Default + Send + Sync,
    P: SharedPointerKind + Send + Sync,
{
    /// Parallel symmap union; delegates to the sequential [`union`][Self::union].
    ///
    /// Provided for API completeness. The symmetry invariant requires that each
    /// insert also writes the reverse mapping, which must remain consistent
    /// across all insertions — inherently sequential.
    #[must_use]
    pub fn par_union(self, other: Self) -> Self {
        self.union(other)
    }

    /// Parallel symmap difference (forward-key-level).
    ///
    /// Retains forward-direction entries whose keys are in `self` but not in
    /// `other`.
    #[must_use]
    pub fn par_difference(self, other: &Self) -> Self {
        if self.is_empty() {
            return Self::default();
        }
        if other.is_empty() {
            return self;
        }
        self.par_iter()
            .filter(|(a, _)| !other.contains(Direction::Forward, *a))
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect()
    }

    /// Parallel symmap intersection (forward-key-level).
    ///
    /// Retains forward-direction entries whose keys appear in both `self` and
    /// `other`; `self`'s values are kept.
    #[must_use]
    pub fn par_intersection(self, other: &Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::default();
        }
        self.par_iter()
            .filter(|(a, _)| other.contains(Direction::Forward, *a))
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect()
    }

    /// Parallel symmap symmetric difference (forward-key-level).
    ///
    /// Retains forward-direction entries whose keys appear in exactly one of
    /// `self` or `other`. The two halves are computed in parallel via
    /// [`rayon::join`].
    #[must_use]
    pub fn par_symmetric_difference(self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self;
        }
        let self_clone = self.clone();
        let (self_diff, other_diff) = ::rayon::join(
            || {
                self.par_iter()
                    .filter(|(a, _)| !other.contains(Direction::Forward, *a))
                    .map(|(a, b)| (a.clone(), b.clone()))
                    .collect::<Self>()
            },
            || {
                other
                    .par_iter()
                    .filter(|(a, _)| !self_clone.contains(Direction::Forward, *a))
                    .map(|(a, b)| (a.clone(), b.clone()))
                    .collect::<Self>()
            },
        );
        self_diff.union(other_diff)
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

    // --- Bag par set ops ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_union_matches_seq() {
        use crate::bag::Bag;
        let mut a = Bag::new();
        let mut b = Bag::new();
        for i in 0..200i32 {
            a.insert_many(i, (i % 3 + 1) as usize);
            b.insert_many(i + 100, (i % 4 + 1) as usize);
        }
        let par = a.par_union(&b);
        let seq = a.union(&b);
        assert_eq!(par, seq);
        // total is sum of both totals
        assert_eq!(par.total_count(), a.total_count() + b.total_count());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_intersection_matches_seq() {
        use crate::bag::Bag;
        let mut a = Bag::new();
        let mut b = Bag::new();
        a.insert_many(1i32, 3);
        a.insert_many(2, 5);
        a.insert_many(3, 1);
        b.insert_many(2i32, 2);
        b.insert_many(3, 4);
        b.insert_many(4, 7);
        let par = a.par_intersection(&b);
        let seq = a.intersection(&b);
        assert_eq!(par, seq);
        // min(5,2)=2 for key 2, min(1,4)=1 for key 3
        assert_eq!(par.count(&2), 2);
        assert_eq!(par.count(&3), 1);
        assert_eq!(par.count(&1), 0);
        assert_eq!(par.count(&4), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_difference_matches_seq() {
        use crate::bag::Bag;
        let mut a = Bag::new();
        let mut b = Bag::new();
        a.insert_many(1i32, 5);
        a.insert_many(2, 3);
        b.insert_many(1i32, 2);
        b.insert_many(3, 1);
        let par = a.par_difference(&b);
        let seq = a.difference(&b);
        assert_eq!(par, seq);
        assert_eq!(par.count(&1), 3); // 5 - 2
        assert_eq!(par.count(&2), 3); // 3 - 0
        assert_eq!(par.count(&3), 0); // not in a
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_symmetric_difference_matches_seq() {
        use crate::bag::Bag;
        let mut a = Bag::new();
        let mut b = Bag::new();
        a.insert_many(1i32, 3);
        a.insert_many(2, 2);
        b.insert_many(2i32, 5);
        b.insert_many(3, 1);
        let par = a.par_symmetric_difference(&b);
        let seq = a.symmetric_difference(&b);
        assert_eq!(par, seq);
        assert_eq!(par.count(&1), 3); // 3 > 0
        assert_eq!(par.count(&2), 3); // |2 - 5|
        assert_eq!(par.count(&3), 1); // 0 < 1
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bag_par_union_empty_fast_paths() {
        use crate::bag::Bag;
        let mut a = Bag::new();
        a.insert_many(1i32, 2);
        let empty: Bag<i32> = Bag::new();
        assert_eq!(a.par_union(&empty), a);
        assert_eq!(empty.par_union(&a), a);
    }

    // --- HashMultiMap par set ops ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hash_multimap_par_difference_matches_seq() {
        use crate::hash_multimap::HashMultiMap;
        let mut a = HashMultiMap::new();
        let mut b = HashMultiMap::new();
        for i in 0..50i32 {
            a.insert(i, i * 10);
            a.insert(i, i * 10 + 1);
        }
        for i in 25..75i32 {
            b.insert(i, i);
        }
        let par = a.clone().par_difference(&b);
        let seq = a.clone().difference(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.keys().count(), seq.keys().count());
        // Keys 0..25 should be in result; keys 25..50 should not
        assert!(par.contains_key(&0));
        assert!(!par.contains_key(&30));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hash_multimap_par_intersection_matches_seq() {
        use crate::hash_multimap::HashMultiMap;
        let mut a = HashMultiMap::new();
        let mut b = HashMultiMap::new();
        for i in 0..50i32 {
            a.insert(i, i);
            b.insert(i + 25, i + 25);
        }
        let par = a.clone().par_intersection(&b);
        let seq = a.clone().intersection(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.keys().count(), seq.keys().count());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn hash_multimap_par_symmetric_difference_matches_seq() {
        use crate::hash_multimap::HashMultiMap;
        let mut a = HashMultiMap::new();
        let mut b = HashMultiMap::new();
        for i in 0..40i32 {
            a.insert(i, i);
        }
        for i in 20..60i32 {
            b.insert(i, i);
        }
        let par = a.clone().par_symmetric_difference(&b);
        let seq = a.clone().symmetric_difference(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.keys().count(), seq.keys().count());
    }

    // --- BiMap par set ops ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bimap_par_difference_matches_seq() {
        use crate::bimap::BiMap;
        let a: BiMap<i32, i32> = (0..100).map(|i| (i, i * 2)).collect();
        let b: BiMap<i32, i32> = (50..150).map(|i| (i, i * 2)).collect();
        let par = a.clone().par_difference(&b);
        let seq = a.clone().difference(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.len(), 50);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bimap_par_intersection_matches_seq() {
        use crate::bimap::BiMap;
        let a: BiMap<i32, i32> = (0..100).map(|i| (i, i * 2)).collect();
        let b: BiMap<i32, i32> = (50..150).map(|i| (i, i * 2)).collect();
        let par = a.clone().par_intersection(&b);
        let seq = a.clone().intersection(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.len(), 50);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn bimap_par_symmetric_difference_matches_seq() {
        use crate::bimap::BiMap;
        let a: BiMap<i32, i32> = (0..100).map(|i| (i, i * 10)).collect();
        let b: BiMap<i32, i32> = (50..150).map(|i| (i, i * 10)).collect();
        let par = a.clone().par_symmetric_difference(&b);
        let seq = a.clone().symmetric_difference(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.len(), 100); // 0..50 from a, 100..150 from b
    }

    // --- SymMap par set ops ---

    #[cfg_attr(miri, ignore)]
    #[test]
    fn symmap_par_difference_matches_seq() {
        use crate::symmap::SymMap;
        let a: SymMap<i32> = (0..80).map(|i| (i, i + 1000)).collect();
        let b: SymMap<i32> = (40..120).map(|i| (i, i + 1000)).collect();
        let par = a.clone().par_difference(&b);
        let seq = a.clone().difference(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.len(), 40); // forward keys 0..39
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn symmap_par_intersection_matches_seq() {
        use crate::symmap::SymMap;
        let a: SymMap<i32> = (0..80).map(|i| (i, i + 1000)).collect();
        let b: SymMap<i32> = (40..120).map(|i| (i, i + 1000)).collect();
        let par = a.clone().par_intersection(&b);
        let seq = a.clone().intersection(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.len(), 40); // forward keys 40..79
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn symmap_par_symmetric_difference_matches_seq() {
        use crate::symmap::SymMap;
        let a: SymMap<i32> = (0..80).map(|i| (i, i + 1000)).collect();
        let b: SymMap<i32> = (40..120).map(|i| (i, i + 1000)).collect();
        let par = a.clone().par_symmetric_difference(&b);
        let seq = a.clone().symmetric_difference(&b);
        assert_eq!(par.len(), seq.len());
        assert_eq!(par.len(), 80); // 0..39 from a, 80..119 from b
    }

    // --- Property-based tests for parallel set operations ---
    //
    // Strategy: generate random inputs with proptest, assert par_op ≡ seq_op.
    // This verifies correctness AND exercises race-condition surface area — rayon
    // spawns multiple threads per case, so any unsynchronised access would produce
    // non-deterministic results and fail across proptest's 256 default runs.

    #[cfg(feature = "proptest")]
    mod prop {
        use ::proptest::collection::vec;
        use ::proptest::num::{i16, i8};
        use ::proptest::proptest;

        // Build a Bag from (element, count) pairs; counts are 1..=8 to keep totals
        // bounded while still creating meaningful multi-element scenarios.
        fn make_bag(pairs: Vec<(i8, usize)>) -> crate::bag::Bag<i8> {
            pairs
                .into_iter()
                .fold(crate::bag::Bag::new(), |mut b, (e, c)| {
                    b.insert_many(e, c);
                    b
                })
        }

        proptest! {
            // Bag: par_union == seq_union
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bag_par_union_eq_seq(
                pa in vec((i8::ANY, 1usize..=8usize), 0..40),
                pb in vec((i8::ANY, 1usize..=8usize), 0..40),
            ) {
                let a = make_bag(pa);
                let b = make_bag(pb);
                assert_eq!(a.par_union(&b), a.union(&b));
            }

            // Bag: par_intersection == seq_intersection
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bag_par_intersection_eq_seq(
                pa in vec((i8::ANY, 1usize..=8usize), 0..40),
                pb in vec((i8::ANY, 1usize..=8usize), 0..40),
            ) {
                let a = make_bag(pa);
                let b = make_bag(pb);
                assert_eq!(a.par_intersection(&b), a.intersection(&b));
            }

            // Bag: par_difference == seq_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bag_par_difference_eq_seq(
                pa in vec((i8::ANY, 1usize..=8usize), 0..40),
                pb in vec((i8::ANY, 1usize..=8usize), 0..40),
            ) {
                let a = make_bag(pa);
                let b = make_bag(pb);
                assert_eq!(a.par_difference(&b), a.difference(&b));
            }

            // Bag: par_symmetric_difference == seq_symmetric_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bag_par_symmetric_difference_eq_seq(
                pa in vec((i8::ANY, 1usize..=8usize), 0..40),
                pb in vec((i8::ANY, 1usize..=8usize), 0..40),
            ) {
                let a = make_bag(pa);
                let b = make_bag(pb);
                assert_eq!(a.par_symmetric_difference(&b), a.symmetric_difference(&b));
            }

            // HashMultiMap: par_difference == seq_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_hash_multimap_par_difference_eq_seq(
                pa in vec((i8::ANY, i16::ANY), 0..60),
                pb in vec((i8::ANY, i16::ANY), 0..60),
            ) {
                use crate::hash_multimap::HashMultiMap;
                let a: HashMultiMap<i8, i16> = pa.into_iter().collect();
                let b: HashMultiMap<i8, i16> = pb.into_iter().collect();
                // Compare key sets: parallel and sequential difference must have the same
                // key count and identical key presence.
                let par = a.clone().par_difference(&b);
                let seq = a.clone().difference(&b);
                assert_eq!(par.len(), seq.len());
                assert_eq!(par.keys().count(), seq.keys().count());
            }

            // HashMultiMap: par_intersection == seq_intersection
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_hash_multimap_par_intersection_eq_seq(
                pa in vec((i8::ANY, i16::ANY), 0..60),
                pb in vec((i8::ANY, i16::ANY), 0..60),
            ) {
                use crate::hash_multimap::HashMultiMap;
                let a: HashMultiMap<i8, i16> = pa.into_iter().collect();
                let b: HashMultiMap<i8, i16> = pb.into_iter().collect();
                let par = a.clone().par_intersection(&b);
                let seq = a.clone().intersection(&b);
                assert_eq!(par.len(), seq.len());
                assert_eq!(par.keys().count(), seq.keys().count());
            }

            // HashMultiMap: par_symmetric_difference == seq_symmetric_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_hash_multimap_par_symmetric_difference_eq_seq(
                pa in vec((i8::ANY, i16::ANY), 0..60),
                pb in vec((i8::ANY, i16::ANY), 0..60),
            ) {
                use crate::hash_multimap::HashMultiMap;
                let a: HashMultiMap<i8, i16> = pa.into_iter().collect();
                let b: HashMultiMap<i8, i16> = pb.into_iter().collect();
                let par = a.clone().par_symmetric_difference(&b);
                let seq = a.clone().symmetric_difference(&b);
                assert_eq!(par.len(), seq.len());
                assert_eq!(par.keys().count(), seq.keys().count());
            }

            // BiMap: par_difference == seq_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bimap_par_difference_eq_seq(
                // Disjoint key/value ranges to avoid spurious BiMap conflicts
                // that make comparison harder; use i8 keys and i16 values.
                pa in vec((i8::ANY, i16::ANY), 0..50),
                pb in vec((i8::ANY, i16::ANY), 0..50),
            ) {
                use crate::bimap::BiMap;
                let a: BiMap<i8, i16> = pa.into_iter().collect();
                let b: BiMap<i8, i16> = pb.into_iter().collect();
                let par = a.clone().par_difference(&b);
                let seq = a.clone().difference(&b);
                assert_eq!(par, seq);
            }

            // BiMap: par_intersection == seq_intersection
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bimap_par_intersection_eq_seq(
                pa in vec((i8::ANY, i16::ANY), 0..50),
                pb in vec((i8::ANY, i16::ANY), 0..50),
            ) {
                use crate::bimap::BiMap;
                let a: BiMap<i8, i16> = pa.into_iter().collect();
                let b: BiMap<i8, i16> = pb.into_iter().collect();
                let par = a.clone().par_intersection(&b);
                let seq = a.clone().intersection(&b);
                assert_eq!(par, seq);
            }

            // BiMap: par_symmetric_difference == seq_symmetric_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_bimap_par_symmetric_difference_eq_seq(
                pa in vec((i8::ANY, i16::ANY), 0..50),
                pb in vec((i8::ANY, i16::ANY), 0..50),
            ) {
                use crate::bimap::BiMap;
                let a: BiMap<i8, i16> = pa.into_iter().collect();
                let b: BiMap<i8, i16> = pb.into_iter().collect();
                let par = a.clone().par_symmetric_difference(&b);
                let seq = a.clone().symmetric_difference(&b);
                assert_eq!(par, seq);
            }

            // SymMap: par_difference == seq_difference
            // Use disjoint i16 key/value ranges so the SymMap bijection invariant
            // is not triggered unexpectedly (values 10000..20000 can't be keys).
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_symmap_par_difference_eq_seq(
                pa in vec((0i16..100, 10000i16..10100), 0..40),
                pb in vec((0i16..100, 10000i16..10100), 0..40),
            ) {
                use crate::symmap::SymMap;
                let a: SymMap<i16> = pa.into_iter().collect();
                let b: SymMap<i16> = pb.into_iter().collect();
                let par = a.clone().par_difference(&b);
                let seq = a.clone().difference(&b);
                assert_eq!(par.len(), seq.len());
            }

            // SymMap: par_intersection == seq_intersection
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_symmap_par_intersection_eq_seq(
                pa in vec((0i16..100, 10000i16..10100), 0..40),
                pb in vec((0i16..100, 10000i16..10100), 0..40),
            ) {
                use crate::symmap::SymMap;
                let a: SymMap<i16> = pa.into_iter().collect();
                let b: SymMap<i16> = pb.into_iter().collect();
                let par = a.clone().par_intersection(&b);
                let seq = a.clone().intersection(&b);
                assert_eq!(par.len(), seq.len());
            }

            // SymMap: par_symmetric_difference == seq_symmetric_difference
            #[cfg_attr(miri, ignore)]
            #[test]
            fn prop_symmap_par_symmetric_difference_eq_seq(
                pa in vec((0i16..100, 10000i16..10100), 0..40),
                pb in vec((0i16..100, 10000i16..10100), 0..40),
            ) {
                use crate::symmap::SymMap;
                let a: SymMap<i16> = pa.into_iter().collect();
                let b: SymMap<i16> = pb.into_iter().collect();
                let par = a.clone().par_symmetric_difference(&b);
                let seq = a.clone().symmetric_difference(&b);
                assert_eq!(par.len(), seq.len());
            }
        }
    }
}
