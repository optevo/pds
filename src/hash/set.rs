// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! An unordered set.
//!
//! An immutable hash set using [hash array mapped tries] [1].
//!
//! Most operations on this set are O(log<sub>x</sub> n) for a
//! suitably high *x* that it should be nearly O(1) for most sets.
//! Because of this, it's a great choice for a generic set as long as
//! you don't mind that values will need to implement
//! [`Hash`][std::hash::Hash] and [`Eq`][std::cmp::Eq].
//!
//! Values will have a predictable order based on the hasher
//! being used. Unless otherwise specified, this will be the standard
//! [`RandomState`][std::collections::hash_map::RandomState] hasher.
//!
//! [1]: https://en.wikipedia.org/wiki/Hash_array_mapped_trie
//! [std::cmp::Eq]: https://doc.rust-lang.org/std/cmp/trait.Eq.html
//! [std::hash::Hash]: https://doc.rust-lang.org/std/hash/trait.Hash.html
//! [std::collections::hash_map::RandomState]: https://doc.rust-lang.org/std/collections/hash_map/struct.RandomState.html

use alloc::borrow::ToOwned;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::borrow::Borrow;
#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::{FromIterator, FusedIterator, Sum};
use core::ops::{Add, Deref, Mul};

use archery::{SharedPointer, SharedPointerKind};
use equivalent::Equivalent;

use crate::config::{MERKLE_HASH_BITS, MERKLE_POSITIVE_EQ_MIN_BITS};
use crate::hash_width::HashWidth;
use crate::hashmap::next_hasher_id;
use crate::nodes::hamt::{
    hash_key, Drain as NodeDrain, Entry as NodeEntry, HashValue, Iter as NodeIter, Node,
    HASH_WIDTH,
};
use crate::ordset::GenericOrdSet;
#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::shared_ptr::DefaultSharedPtr;
use crate::GenericVector;

/// Construct a set from a sequence of values.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use pds::HashSet;
/// # fn main() {
/// assert_eq!(
///   hashset![1, 2, 3],
///   HashSet::from(vec![1, 2, 3])
/// );
/// # }
/// ```
#[macro_export]
macro_rules! hashset {
    () => { $crate::hashset::HashSet::new() };

    ( $($x:expr),* ) => {{
        let mut l = $crate::hashset::HashSet::new();
        $(
            l.insert($x);
        )*
            l
    }};

    ( $($x:expr ,)* ) => {{
        let mut l = $crate::hashset::HashSet::new();
        $(
            l.insert($x);
        )*
            l
    }};
}

/// Type alias for [`GenericHashSet`] that uses [`std::hash::RandomState`] as the default hasher and [`DefaultSharedPtr`] as the pointer type.
///
/// [GenericHashSet]: ./struct.GenericHashSet.html
/// [`std::hash::RandomState`]: https://doc.rust-lang.org/stable/std/collections/hash_map/struct.RandomState.html
/// [DefaultSharedPtr]: ../shared_ptr/type.DefaultSharedPtr.html
#[cfg(feature = "std")]
pub type HashSet<A> = GenericHashSet<A, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericHashSet`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type HashSet<A> = GenericHashSet<A, foldhash::fast::RandomState, DefaultSharedPtr>;

/// An unordered set backed by a [hash array mapped trie][1].
///
/// ## Complexity vs Standard Library
///
/// | Operation | `HashSet` | [`std::HashSet`] |
/// |---|---|---|
/// | `clone` | **O(1)** | O(n) |
/// | `eq` (Merkle, same lineage) | **O(1)**† | O(n) |
/// | `eq` (different lineage) | O(n) | O(n) |
/// | `contains` / `get` | O(log₃₂ n) ≈ O(1) | O(1) |
/// | `insert` | O(log₃₂ n) ≈ O(1) | O(1)\* |
/// | `remove` | O(log₃₂ n) ≈ O(1) | O(1) |
/// | `union` / `intersection` | O(n + m) | O(n + m) |
/// | `is_subset` | O(n log₃₂ m) | O(n) |
/// | `from_iter` | O(n log₃₂ n) ≈ O(n) | O(n) |
///
/// **Bold** = asymptotically better than the std alternative.
/// \* = amortised. † = requires both sets to share a hasher instance
/// (common ancestor via `clone`).
///
/// The O(log₃₂ n) operations are *effectively* O(1) for practical sizes:
/// log₃₂(1 billion) < 7.
///
/// The key advantage is `clone` in O(1) via structural sharing. Two sets
/// from a common ancestor share all unmodified subtries in memory.
///
/// ## Merkle Hashing
///
/// Each HAMT node maintains a commutative Merkle hash — a fingerprint of
/// all elements in the subtrie. Sets with the same hasher, same size, and
/// matching Merkle hash are treated as equal without element-by-element
/// comparison (O(1)). The false-positive rate (~2⁻⁶⁴) is below DRAM
/// bit-flip rates.
///
/// For sets with independent hashers (no shared ancestor), equality falls
/// back to O(n) element-by-element comparison.
///
/// Values must implement [`Hash`][std::hash::Hash] and [`Eq`][std::cmp::Eq].
///
/// [`std::HashSet`]: https://doc.rust-lang.org/std/collections/struct.HashSet.html
/// [1]: https://en.wikipedia.org/wiki/Hash_array_mapped_trie
/// [std::cmp::Eq]: https://doc.rust-lang.org/std/cmp/trait.Eq.html
/// [std::hash::Hash]: https://doc.rust-lang.org/std/hash/trait.Hash.html
/// [std::collections::hash_map::RandomState]: https://doc.rust-lang.org/std/collections/hash_map/struct.RandomState.html
pub struct GenericHashSet<A, S, P: SharedPointerKind, H: HashWidth = u64> {
    pub(crate) hasher: S,
    /// Identifies the hasher lineage — see GenericHashMap::hasher_id.
    pub(crate) hasher_id: u64,
    pub(crate) root: Option<SharedPointer<Node<Value<A>, P, H>, P>>,
    pub(crate) size: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
/// Internal wrapper for hash set elements. This type is an
/// implementation detail used by [`HashSetInternPool`](crate::intern::HashSetInternPool)
/// — prefer the type alias over naming this type directly.
#[doc(hidden)]
pub struct Value<A>(pub(crate) A);

impl<A> Deref for Value<A> {
    type Target = A;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// FIXME lacking specialisation, we can't simply implement `HashValue`
// for `A`, we have to use the `Value<A>` indirection.
impl<A> HashValue for Value<A>
where
    A: Hash + Eq,
{
    type Key = A;

    fn extract_key(&self) -> &Self::Key {
        &self.0
    }

    fn ptr_eq(&self, _other: &Self) -> bool {
        false
    }
}

impl<A, S, P, H: HashWidth> GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    /// Construct a set with a single value.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// # use std::sync::Arc;
    /// let set = HashSet::unit(123);
    /// assert!(set.contains(&123));
    /// ```
    #[inline]
    #[must_use]
    pub fn unit(a: A) -> Self {
        GenericHashSet::new().update(a)
    }
}

impl<A, S, P: SharedPointerKind, H: HashWidth> GenericHashSet<A, S, P, H> {
    /// Construct an empty set.
    #[must_use]
    pub fn new() -> Self
    where
        S: Default,
    {
        Self::default()
    }

    /// Test whether a set is empty.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// assert!(
    ///   !hashset![1, 2, 3].is_empty()
    /// );
    /// assert!(
    ///   HashSet::<i32>::new().is_empty()
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the size of a set.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// assert_eq!(3, hashset![1, 2, 3].len());
    /// ```
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.size
    }

    /// Test whether two sets refer to the same content in memory.
    ///
    /// This is true if the two sides are references to the same set,
    /// or if the two sets refer to the same root node.
    ///
    /// This would return true if you're comparing a set to itself, or
    /// if you're comparing a set to a fresh clone of itself.
    ///
    /// Time: O(1)
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (Some(a), Some(b)) => SharedPointer::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// Construct an empty hash set using the provided hasher.
    #[inline]
    #[must_use]
    pub fn with_hasher(hasher: S) -> Self {
        GenericHashSet {
            size: 0,
            root: None,
            hasher,
            hasher_id: next_hasher_id(),
        }
    }

    /// Get a reference to the set's [`BuildHasher`][BuildHasher].
    ///
    /// [BuildHasher]: https://doc.rust-lang.org/std/hash/trait.BuildHasher.html
    #[must_use]
    pub fn hasher(&self) -> &S {
        &self.hasher
    }

    /// Construct an empty hash set using the same hasher as the current hash set.
    #[inline]
    #[must_use]
    pub fn new_from<A2>(&self) -> GenericHashSet<A2, S, P, H>
    where
        A2: Hash + Eq + Clone,
        S: Clone,
    {
        GenericHashSet {
            size: 0,
            root: None,
            hasher: self.hasher.clone(),
            hasher_id: self.hasher_id,
        }
    }

    /// Discard all elements from the set.
    ///
    /// This leaves you with an empty set, and all elements that
    /// were previously inside it are dropped.
    ///
    /// Time: O(n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::HashSet;
    /// let mut set = hashset![1, 2, 3];
    /// set.clear();
    /// assert!(set.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.root = None;
        self.size = 0;
    }

    /// Get an iterator over the values in a hash set.
    ///
    /// Please note that the order is consistent between sets using
    /// the same hasher, but no other ordering guarantee is offered.
    /// Items will not come out in insertion order or sort order.
    /// They will, however, come out in the same order every time for
    /// the same set.
    #[must_use]
    pub fn iter(&self) -> Iter<'_, A, P, H> {
        Iter {
            it: NodeIter::new(self.root.as_deref(), self.size),
        }
    }
}

impl<A, S, P, H: HashWidth> GenericHashSet<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn test_eq<S2: BuildHasher, P2: SharedPointerKind>(
        &self,
        other: &GenericHashSet<A, S2, P2, H>,
    ) -> bool {
        if self.len() != other.len() {
            return false;
        }
        // Fast path: if both roots point to the same allocation, the sets
        // are identical. See HashMap::test_eq for rationale.
        match (&self.root, &other.root) {
            (None, None) => return true,
            (Some(a), Some(b)) => {
                let a_ptr = &**a as *const _ as *const ();
                let b_ptr = &**b as *const _ as *const ();
                if a_ptr == b_ptr {
                    return true;
                }
                // Merkle check: when both sets share the same hasher instance
                // (common ancestor via clone), their key hashes are computed
                // identically and Merkle hashes are directly comparable.
                //
                // Negative: different Merkle → definitely not equal.
                // Positive: same Merkle + same size → equal with probability
                // 1 - 2^-64 (≈5.4e-20). This false positive rate is far below
                // hardware error rates (~1e-15 per bit-hour for unprotected
                // DRAM), so we treat Merkle match as equality — same reasoning
                // as treating GUIDs as unique despite collision possibility.
                if self.hasher_id == other.hasher_id {
                    // Negative check is always safe: different Merkle → not equal.
                    if a.merkle_hash != b.merkle_hash {
                        return false;
                    }
                    // Positive check: only safe when hash width ≥ 64 bits (DEC-023).
                    if MERKLE_HASH_BITS >= MERKLE_POSITIVE_EQ_MIN_BITS {
                        return true;
                    }
                }
            }
            _ => {}
        }
        // Lengths are equal and sets have no duplicates, so if every element
        // of self is in other, the sets must be identical.
        for value in self.iter() {
            if !other.contains(value) {
                return false;
            }
        }
        true
    }

    /// Test if a value is part of a set.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        if let Some(root) = &self.root {
            root.get(hash_key(&self.hasher, value), 0, value).is_some()
        } else {
            false
        }
    }

    /// Test whether a set is a subset of another set, meaning that
    /// all values in our set must also be in the other set.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn is_subset<RS>(&self, other: RS) -> bool
    where
        RS: Borrow<Self>,
    {
        let o = other.borrow();
        self.iter().all(|a| o.contains(a))
    }

    /// Test whether a set is a proper subset of another set, meaning
    /// that all values in our set must also be in the other set. A
    /// proper subset must also be smaller than the other set.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn is_proper_subset<RS>(&self, other: RS) -> bool
    where
        RS: Borrow<Self>,
    {
        self.len() != other.borrow().len() && self.is_subset(other)
    }

    /// Compute the diff between two hash sets.
    ///
    /// Returns an iterator of [`DiffItem`] values describing the
    /// differences between `self` (old) and `other` (new). Values
    /// present only in `self` produce [`DiffItem::Remove`], values
    /// present only in `other` produce [`DiffItem::Add`].
    ///
    /// If the two sets share the same root (i.e.
    /// [`ptr_eq`][GenericHashSet::ptr_eq] returns true), the iterator
    /// is empty without traversing any elements.
    ///
    /// When both sets are same-lineage (same `hasher_id`), same size,
    /// and their root Merkle hashes match, the diff is known to be empty
    /// without any tree traversal. This fires after `insert`/`remove`
    /// that leave the set unchanged, or after a round-trip through
    /// insert+remove that restores the original key set.
    ///
    /// ## Performance tip
    ///
    /// For independently-constructed sets with high content overlap,
    /// call `intern` (requires the `hash-intern` feature) on both before
    /// diffing. After interning, content-equal subtrees share the same
    /// allocation and are skipped in O(1), reducing diff complexity from
    /// O(n + m) to O(changes × depth).
    ///
    /// When the two sets share structure (one was derived from the other
    /// via insert/remove), shared subtrees are detected via pointer
    /// comparison and skipped in O(1), reducing complexity to
    /// O(changes × tree_depth). For independently-constructed sets with
    /// different hasher states, falls back to O(n + m).
    #[must_use]
    pub fn diff<'a, 'b>(&'a self, other: &'b Self) -> DiffIter<'a, 'b, A, S, P, H> {
        let mut diffs = Vec::new();
        if !self.ptr_eq(other) {
            // Root Merkle fast-path: same-lineage sets (same hasher_id) with the
            // same size and matching root Merkle hash are almost certainly equal.
            // For sets, node Merkle covers all keys (no values that could silently
            // differ), so the root Merkle is a full content fingerprint. Same
            // probabilistic argument as PartialEq (DEC-023): false positive ≈ 2^-64.
            if self.len() == other.len()
                && self.hasher_id == other.hasher_id
                && MERKLE_HASH_BITS >= MERKLE_POSITIVE_EQ_MIN_BITS
            {
                if let (Some(a), Some(b)) = (&self.root, &other.root) {
                    if a.merkle_hash == b.merkle_hash {
                        return DiffIter {
                            diffs,
                            index: 0,
                            _phantom: core::marker::PhantomData,
                        };
                    }
                }
            }
            match (&self.root, &other.root) {
                (Some(old_root), Some(new_root)) => {
                    if !SharedPointer::ptr_eq(old_root, new_root) {
                        if set_hashers_compatible(&self.hasher, &other.hasher) {
                            set_diff_hamt_nodes(old_root, new_root, &mut diffs);
                        } else {
                            set_diff_iterate_and_lookup(self, other, &mut diffs);
                        }
                    }
                }
                (Some(_), None) => {
                    for v in self.iter() {
                        diffs.push(DiffItem::Remove(v));
                    }
                }
                (None, Some(_)) => {
                    for v in other.iter() {
                        diffs.push(DiffItem::Add(v));
                    }
                }
                (None, None) => {}
            }
        }
        DiffIter {
            diffs,
            index: 0,
            _phantom: core::marker::PhantomData,
        }
    }
}

// Mutating methods that need A: Clone for copy-on-write but NOT S: Clone.
impl<A, S, P, H: HashWidth> GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Insert a value into a set.
    ///
    /// Time: O(log n)
    #[inline]
    pub fn insert(&mut self, a: A) -> Option<A> {
        let hash = hash_key(&self.hasher, &a);
        let root = SharedPointer::make_mut(self.root.get_or_insert_with(Default::default));
        match root.insert(hash, 0, Value(a)) {
            None => {
                self.size += 1;
                None
            }
            Some(Value(old_value)) => Some(old_value),
        }
    }

    /// Remove a value from a set if it exists.
    ///
    /// Time: O(log n)
    pub fn remove<Q>(&mut self, value: &Q) -> Option<A>
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        let root = SharedPointer::make_mut(self.root.get_or_insert_with(Default::default));
        let result = root.remove(hash_key(&self.hasher, value), 0, value);
        if result.is_some() {
            self.size -= 1;
        }
        result.map(|v| v.0)
    }

    /// Filter out values from a set which don't satisfy a predicate.
    ///
    /// This is slightly more efficient than filtering using an
    /// iterator, in that it doesn't need to rehash the retained
    /// values, but it still needs to reconstruct the entire tree
    /// structure of the set.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::HashSet;
    /// let mut set = hashset![1, 2, 3];
    /// set.retain(|v| *v > 1);
    /// let expected = hashset![2, 3];
    /// assert_eq!(expected, set);
    /// ```
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&A) -> bool,
    {
        let Some(root) = &mut self.root else {
            return;
        };
        let old_root = root.clone();
        let root = SharedPointer::make_mut(root);
        for (value, hash) in NodeIter::new(Some(&old_root), self.size) {
            if !f(value) && root.remove(hash, 0, &**value).is_some() {
                self.size -= 1;
            }
        }
    }

    /// Split a set into two sets, where the first contains values
    /// that satisfy the predicate and the second contains values
    /// that do not.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let set = hashset!{1, 2, 3, 4, 5};
    /// let (evens, odds) = set.partition(|v| v % 2 == 0);
    /// assert_eq!(evens, hashset!{2, 4});
    /// assert_eq!(odds, hashset!{1, 3, 5});
    /// ```
    #[must_use]
    pub fn partition<F>(&self, mut f: F) -> (Self, Self)
    where
        S: Default,
        F: FnMut(&A) -> bool,
    {
        let mut left = Self::new();
        let mut right = Self::new();
        for a in self.iter() {
            if f(a) {
                left.insert(a.clone());
            } else {
                right.insert(a.clone());
            }
        }
        (left, right)
    }

    /// Construct the union of two sets.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let set1 = hashset!{1, 2};
    /// let set2 = hashset!{2, 3};
    /// let expected = hashset!{1, 2, 3};
    /// assert_eq!(expected, set1.union(set2));
    /// ```
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let (mut to_mutate, to_consume) = if self.len() >= other.len() {
            (self, other)
        } else {
            (other, self)
        };
        for value in to_consume {
            to_mutate.insert(value);
        }
        to_mutate
    }

    /// Construct the union of multiple sets.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn unions<I>(i: I) -> Self
    where
        I: IntoIterator<Item = Self>,
        S: Default,
    {
        i.into_iter().fold(Self::default(), Self::union)
    }

    /// Construct the symmetric difference between two sets.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let set1 = hashset!{1, 2};
    /// let set2 = hashset!{2, 3};
    /// let expected = hashset!{1, 3};
    /// assert_eq!(expected, set1.symmetric_difference(set2));
    /// ```
    #[must_use]
    pub fn symmetric_difference(mut self, other: Self) -> Self {
        for value in other {
            if self.remove(&value).is_none() {
                self.insert(value);
            }
        }
        self
    }

    /// Construct the relative complement between two sets, that is the set
    /// of values in `self` that do not occur in `other`.
    ///
    /// Time: O(m log n) where m is the size of the other set
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let set1 = hashset!{1, 2};
    /// let set2 = hashset!{2, 3};
    /// let expected = hashset!{1};
    /// assert_eq!(expected, set1.relative_complement(set2));
    /// ```
    #[must_use]
    pub fn relative_complement(mut self, other: Self) -> Self {
        for value in other {
            let _ = self.remove(&value);
        }
        self
    }
}

// Methods that clone self or create new sets (persistent API).
// Previously required S: Clone; now S is behind SharedPointer so clone is free.
impl<A, S, P, H: HashWidth> GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Apply a diff to produce a new set.
    ///
    /// Takes any iterator of [`DiffItem`] values (such as from
    /// [`diff`][GenericHashSet::diff]) and applies each change —
    /// `Add` inserts values, `Remove` removes values.
    ///
    /// Time: O(d log n) where d is the number of diff items
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let base = hashset!{1, 2, 3};
    /// let modified = hashset!{2, 3, 4};
    /// let diff: Vec<_> = base.diff(&modified).collect();
    /// let patched = base.apply_diff(diff);
    /// assert_eq!(patched, modified);
    /// ```
    #[must_use]
    pub fn apply_diff<'a, 'b, I>(&self, diff: I) -> Self
    where
        I: IntoIterator<Item = DiffItem<'a, 'b, A>>,
        A: 'a + 'b,
    {
        let mut out = self.clone();
        for item in diff {
            match item {
                DiffItem::Add(a) => {
                    out.insert(a.clone());
                }
                DiffItem::Remove(a) => {
                    out.remove(a);
                }
            }
        }
        out
    }

    /// Construct a new set from the current set with the given value
    /// added.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// # use std::sync::Arc;
    /// let set = hashset![123];
    /// assert_eq!(
    ///   set.update(456),
    ///   hashset![123, 456]
    /// );
    /// ```
    #[must_use]
    pub fn update(&self, a: A) -> Self {
        let mut out = self.clone();
        out.insert(a);
        out
    }

    /// Construct a new set with the given value removed if it's in
    /// the set.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn without<Q>(&self, value: &Q) -> Self
    where
        Q: Hash + Equivalent<A> + ?Sized,
    {
        let mut out = self.clone();
        out.remove(value);
        out
    }

    /// Keep only values that are in the given set.
    ///
    /// Time: O(n log m) where n = self.len(), m = other.len()
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let set = hashset!{1, 2, 3, 4, 5};
    /// let keep = hashset!{2, 4, 6};
    /// assert_eq!(set.restrict(&keep), hashset!{2, 4});
    /// ```
    #[must_use]
    pub fn restrict(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.retain(|a| other.contains(a));
        out
    }

    /// Construct the intersection of two sets.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let set1 = hashset!{1, 2};
    /// let set2 = hashset!{2, 3};
    /// let expected = hashset!{2};
    /// assert_eq!(expected, set1.intersection(set2));
    /// ```
    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        let mut out = self.new_from();
        for value in other {
            if self.contains(&value) {
                out.insert(value);
            }
        }
        out
    }
}

// Methods that need A: Hash + Eq but not A: Clone
impl<A, S, P, H: HashWidth> GenericHashSet<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Check whether two sets share no elements.
    ///
    /// Time: O(n) — iterates the smaller set and checks each element
    /// against the larger set.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashset::HashSet;
    /// let a = hashset!{1, 2, 3};
    /// let b = hashset!{4, 5, 6};
    /// let c = hashset!{3, 4, 5};
    /// assert!(a.disjoint(&b));
    /// assert!(!a.disjoint(&c));
    /// ```
    #[must_use]
    pub fn disjoint(&self, other: &Self) -> bool {
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };
        smaller.iter().all(|a| !larger.contains(a))
    }
}

// Core traits

impl<A, S, P: SharedPointerKind, H: HashWidth> Clone for GenericHashSet<A, S, P, H>
where
    S: Clone,
    P: SharedPointerKind,
{
    /// Clone a set.
    ///
    /// Time: O(1), plus a cheap hasher clone.
    #[inline]
    fn clone(&self) -> Self {
        GenericHashSet {
            hasher: self.hasher.clone(),
            hasher_id: self.hasher_id,
            root: self.root.clone(),
            size: self.size,
        }
    }
}

#[cfg(feature = "hash-intern")]
impl<A, S, P, H: HashWidth> GenericHashSet<A, S, P, H>
where
    A: Clone + PartialEq,
    P: SharedPointerKind,
{
    /// Intern the internal HAMT nodes of this set into the given pool.
    ///
    /// See [`GenericHashMap::intern`](crate::hashmap::GenericHashMap::intern) for details on how interning works.
    ///
    /// ## Performance tip — diff across independently-constructed sets
    ///
    /// After interning two sets built from the same data (even if
    /// constructed independently), content-equal subtrees share the same
    /// allocation. Subsequent `diff` calls reduce from O(n + m) to
    /// O(changes × depth) because shared subtrees are skipped in O(1)
    /// via pointer comparison.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "hash-intern")]
    /// # {
    /// use pds::HashSet;
    /// use pds::intern::HashSetInternPool;
    ///
    /// let mut pool = HashSetInternPool::new();
    /// let mut set: HashSet<i32> = (0..100).collect();
    /// set.intern(&mut pool);
    /// # }
    /// ```
    pub fn intern(&mut self, pool: &mut crate::intern::HashSetInternPool<A, P, H>) {
        if let Some(root) = &mut self.root {
            let node = SharedPointer::make_mut(root);
            for entry in node.data.iter_mut() {
                entry.intern(pool);
            }
            *root = pool.intern_hamt(root.clone());
        }
    }
}

impl<A, S1, P1, S2, P2, H: HashWidth> PartialEq<GenericHashSet<A, S2, P2, H>> for GenericHashSet<A, S1, P1, H>
where
    A: Hash + Eq,
    S1: BuildHasher,
    S2: BuildHasher,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn eq(&self, other: &GenericHashSet<A, S2, P2, H>) -> bool {
        self.test_eq(other)
    }
}

impl<A, S, P, H: HashWidth> Eq for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
}

impl<A, S, P, H: HashWidth> Hash for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        self.len().hash(state);
        // Order-independent: wrapping_add of per-element hashes.
        let mut combined: u64 = 0;
        for a in self.iter() {
            let mut h = crate::util::FnvHasher::new();
            a.hash(&mut h);
            combined = combined.wrapping_add(h.finish());
        }
        combined.hash(state);
    }
}

impl<A, S, P, H: HashWidth> Default for GenericHashSet<A, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    fn default() -> Self {
        GenericHashSet {
            hasher: S::default(),
            hasher_id: next_hasher_id(),
            root: None,
            size: 0,
        }
    }
}

impl<A, S, P, H: HashWidth> Add for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = GenericHashSet<A, S, P, H>;

    fn add(self, other: Self) -> Self::Output {
        self.union(other)
    }
}

impl<A, S, P, H: HashWidth> Mul for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = GenericHashSet<A, S, P, H>;

    fn mul(self, other: Self) -> Self::Output {
        self.intersection(other)
    }
}

impl<A, S, P, H: HashWidth> Add for &GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = GenericHashSet<A, S, P, H>;

    fn add(self, other: Self) -> Self::Output {
        self.clone().union(other.clone())
    }
}

impl<A, S, P, H: HashWidth> Mul for &GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Output = GenericHashSet<A, S, P, H>;

    fn mul(self, other: Self) -> Self::Output {
        self.clone().intersection(other.clone())
    }
}

impl<A, S, P: SharedPointerKind, H: HashWidth> Sum for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn sum<I>(it: I) -> Self
    where
        I: Iterator<Item = Self>,
    {
        it.fold(Self::default(), |a, b| a + b)
    }
}

impl<A, S, R, P: SharedPointerKind, H: HashWidth> Extend<R> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone + From<R>,
    S: BuildHasher,
{
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = R>,
    {
        for value in iter {
            self.insert(From::from(value));
        }
    }
}

impl<A, S, P, H: HashWidth> Debug for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Debug,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_set().entries(self.iter()).finish()
    }
}

// Iterators

/// An iterator over the elements of a set.
pub struct Iter<'a, A, P: SharedPointerKind, H: HashWidth = u64> {
    it: NodeIter<'a, Value<A>, P, H>,
}

// We impl Clone instead of deriving it, because we want Clone even if K and V aren't.
impl<'a, A, P: SharedPointerKind, H: HashWidth> Clone for Iter<'a, A, P, H> {
    fn clone(&self) -> Self {
        Iter {
            it: self.it.clone(),
        }
    }
}

impl<'a, A, P, H: HashWidth> Iterator for Iter<'a, A, P, H>
where
    A: 'a,
    P: SharedPointerKind,
{
    type Item = &'a A;

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|(v, _)| &v.0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, A, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for Iter<'a, A, P, H> {}

impl<'a, A, P: SharedPointerKind, H: HashWidth> FusedIterator for Iter<'a, A, P, H> {}

/// A consuming iterator over the elements of a set.
pub struct ConsumingIter<A, P, H = u64>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
    H: HashWidth,
{
    it: NodeDrain<Value<A>, P, H>,
}

impl<A, P, H: HashWidth> Iterator for ConsumingIter<A, P, H>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
    type Item = A;

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|(v, _)| v.0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<A, P, H: HashWidth> ExactSizeIterator for ConsumingIter<A, P, H>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
}

impl<A, P, H: HashWidth> FusedIterator for ConsumingIter<A, P, H>
where
    A: Hash + Eq + Clone,
    P: SharedPointerKind,
{
}

// Iterator conversions

impl<A, RA, S, P, H: HashWidth> FromIterator<RA> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone + From<RA>,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from_iter<T>(i: T) -> Self
    where
        T: IntoIterator<Item = RA>,
    {
        let mut set = Self::default();
        for value in i {
            set.insert(From::from(value));
        }
        set
    }
}

impl<'a, A, S, P, H: HashWidth> IntoIterator for &'a GenericHashSet<A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = &'a A;
    type IntoIter = Iter<'a, A, P, H>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<A, S, P, H: HashWidth> IntoIterator for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = A;
    type IntoIter = ConsumingIter<Self::Item, P, H>;

    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            it: NodeDrain::new(self.root, self.size),
        }
    }
}

// Conversions

impl<A, OA, SA, SB, P1, P2, H: HashWidth> From<&GenericHashSet<&A, SA, P1, H>> for GenericHashSet<OA, SB, P2, H>
where
    A: ToOwned<Owned = OA> + Hash + Equivalent<A> + ?Sized,
    OA: Hash + Eq + Clone,
    SA: BuildHasher,
    SB: BuildHasher + Default,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(set: &GenericHashSet<&A, SA, P1, H>) -> Self {
        set.iter().map(|a| (*a).to_owned()).collect()
    }
}

impl<A, S, const N: usize, P, H: HashWidth> From<[A; N]> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, A, S, P, H: HashWidth> From<&'a [A]> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<A, S, P, H: HashWidth> From<Vec<A>> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(vec: Vec<A>) -> Self {
        vec.into_iter().collect()
    }
}

impl<A, S, P, H: HashWidth> From<&Vec<A>> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(vec: &Vec<A>) -> Self {
        vec.iter().cloned().collect()
    }
}

impl<A, S, P1, P2> From<GenericVector<A, P2>> for GenericHashSet<A, S, P1>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(vector: GenericVector<A, P2>) -> Self {
        vector.into_iter().collect()
    }
}

impl<A, S, P1, P2> From<&GenericVector<A, P2>> for GenericHashSet<A, S, P1>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(vector: &GenericVector<A, P2>) -> Self {
        vector.iter().cloned().collect()
    }
}

#[cfg(feature = "std")]
impl<A, S, P, H: HashWidth> From<std::collections::HashSet<A>> for GenericHashSet<A, S, P, H>
where
    A: Eq + Hash + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(hash_set: std::collections::HashSet<A>) -> Self {
        hash_set.into_iter().collect()
    }
}

#[cfg(feature = "std")]
impl<A, S, P, H: HashWidth> From<&std::collections::HashSet<A>> for GenericHashSet<A, S, P, H>
where
    A: Eq + Hash + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(hash_set: &std::collections::HashSet<A>) -> Self {
        hash_set.iter().cloned().collect()
    }
}

impl<A, S, P, H: HashWidth> From<&BTreeSet<A>> for GenericHashSet<A, S, P, H>
where
    A: Hash + Eq + Clone,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(btree_set: &BTreeSet<A>) -> Self {
        btree_set.iter().cloned().collect()
    }
}

impl<A, S, P1, P2> From<GenericOrdSet<A, P2>> for GenericHashSet<A, S, P1>
where
    A: Ord + Hash + Eq + Clone,
    S: BuildHasher + Default,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(ordset: GenericOrdSet<A, P2>) -> Self {
        ordset.into_iter().collect()
    }
}

impl<A, S, P1, P2> From<&GenericOrdSet<A, P2>> for GenericHashSet<A, S, P1>
where
    A: Ord + Hash + Eq + Clone,
    S: BuildHasher + Default,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(ordset: &GenericOrdSet<A, P2>) -> Self {
        ordset.into_iter().cloned().collect()
    }
}

// Diff

/// An item in a diff between two hash sets.
///
/// Produced by [`GenericHashSet::diff`].
#[derive(Debug, PartialEq, Eq)]
pub enum DiffItem<'a, 'b, A> {
    /// This value was added (present in new set only).
    Add(&'b A),
    /// This value was removed (present in old set only).
    Remove(&'a A),
}

impl<A> Clone for DiffItem<'_, '_, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A> Copy for DiffItem<'_, '_, A> {}

/// Check whether two BuildHasher instances produce the same hash output.
fn set_hashers_compatible<S: BuildHasher>(a: &S, b: &S) -> bool {
    use core::hash::Hasher;
    let mut ha = a.build_hasher();
    ha.write_u64(0x517c_c1b7_2722_0a95);
    let mut hb = b.build_hasher();
    hb.write_u64(0x517c_c1b7_2722_0a95);
    ha.finish() == hb.finish()
}

/// Walk two HAMT nodes for sets, collecting diffs.
fn set_diff_hamt_nodes<'a, 'b, A, P, H: HashWidth>(
    old_node: &'a Node<Value<A>, P, H>,
    new_node: &'b Node<Value<A>, P, H>,
    diffs: &mut Vec<DiffItem<'a, 'b, A>>,
) where
    A: Eq,
    P: SharedPointerKind,
{
    // Per-node Merkle pruning: same Merkle hash → same key set in this
    // subtree. Safe for sets because node Merkle is a full content
    // fingerprint (no values to silently differ). This catches content-
    // equal subtrees that aren't pointer-equal, e.g. after independent
    // construction or a persistence round-trip.
    if MERKLE_HASH_BITS >= MERKLE_POSITIVE_EQ_MIN_BITS
        && old_node.merkle_hash == new_node.merkle_hash
    {
        return;
    }
    for i in 0..HASH_WIDTH {
        match (old_node.data.get(i), new_node.data.get(i)) {
            (None, None) => {}
            (Some(old_entry), None) => {
                let mut vals = Vec::new();
                old_entry.collect_values(&mut vals);
                for v in vals {
                    diffs.push(DiffItem::Remove(&v.0));
                }
            }
            (None, Some(new_entry)) => {
                let mut vals = Vec::new();
                new_entry.collect_values(&mut vals);
                for v in vals {
                    diffs.push(DiffItem::Add(&v.0));
                }
            }
            (Some(old_entry), Some(new_entry)) => {
                if old_entry.ptr_eq(new_entry) {
                    continue;
                }
                set_diff_entries(old_entry, new_entry, diffs);
            }
        }
    }
}

/// Compare two HAMT entries for sets.
fn set_diff_entries<'a, 'b, A, P, H: HashWidth>(
    old_entry: &'a NodeEntry<Value<A>, P, H>,
    new_entry: &'b NodeEntry<Value<A>, P, H>,
    diffs: &mut Vec<DiffItem<'a, 'b, A>>,
) where
    A: Eq,
    P: SharedPointerKind,
{
    match (old_entry, new_entry) {
        (NodeEntry::HamtNode(old_node), NodeEntry::HamtNode(new_node)) => {
            set_diff_hamt_nodes(old_node, new_node, diffs);
        }
        (NodeEntry::Value(old_val, _), NodeEntry::Value(new_val, _)) => {
            if old_val.0 != new_val.0 {
                diffs.push(DiffItem::Remove(&old_val.0));
                diffs.push(DiffItem::Add(&new_val.0));
            }
        }
        _ => {
            let mut old_vals: Vec<&'a Value<A>> = Vec::new();
            let mut new_vals: Vec<&'b Value<A>> = Vec::new();
            old_entry.collect_values(&mut old_vals);
            new_entry.collect_values(&mut new_vals);
            for old in &old_vals {
                if !new_vals.iter().any(|new| old.0 == new.0) {
                    diffs.push(DiffItem::Remove(&old.0));
                }
            }
            for new in &new_vals {
                if !old_vals.iter().any(|old| old.0 == new.0) {
                    diffs.push(DiffItem::Add(&new.0));
                }
            }
        }
    }
}

/// Fallback diff using iterate-and-lookup for sets with incompatible hashers.
fn set_diff_iterate_and_lookup<'a, 'b, A, S, P, H: HashWidth>(
    old_set: &'a GenericHashSet<A, S, P, H>,
    new_set: &'b GenericHashSet<A, S, P, H>,
    diffs: &mut Vec<DiffItem<'a, 'b, A>>,
) where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    for v in old_set.iter() {
        if !new_set.contains(v) {
            diffs.push(DiffItem::Remove(v));
        }
    }
    for v in new_set.iter() {
        if !old_set.contains(v) {
            diffs.push(DiffItem::Add(v));
        }
    }
}

/// An iterator over the differences between two hash sets.
///
/// Created by [`GenericHashSet::diff`].
///
/// Uses a simultaneous HAMT tree walk with pointer-based subtree
/// skipping, matching the [`HashMap`][crate::HashMap] diff strategy.
pub struct DiffIter<'a, 'b, A, S, P: SharedPointerKind, H: HashWidth = u64> {
    diffs: Vec<DiffItem<'a, 'b, A>>,
    index: usize,
    _phantom: core::marker::PhantomData<fn(&S, &P, &H)>,
}

impl<'a, 'b, A, S, P, H: HashWidth> Iterator for DiffIter<'a, 'b, A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = DiffItem<'a, 'b, A>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.diffs.len() {
            let item = self.diffs[self.index];
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.diffs.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl<A, S, P, H: HashWidth> ExactSizeIterator for DiffIter<'_, '_, A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
}

impl<A, S, P, H: HashWidth> FusedIterator for DiffIter<'_, '_, A, S, P, H>
where
    A: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
}

// Proptest
#[cfg(any(test, feature = "proptest"))]
#[doc(hidden)]
pub mod proptest {
    #[deprecated(
        since = "14.3.0",
        note = "proptest strategies have moved to pds::proptest"
    )]
    pub use crate::proptest::hash_set;
}

#[cfg(test)]
mod test {
    use super::proptest::*;
    use super::*;
    use crate::test::LolHasher;
    use ::proptest::num::i16;
    use ::proptest::proptest;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use core::hash::BuildHasherDefault;

    assert_impl_all!(HashSet<i32>: Send, Sync);
    assert_not_impl_any!(HashSet<*const i32>: Send, Sync);
    assert_covariant!(HashSet<T> in T);

    #[test]
    fn insert_failing() {
        let mut set: GenericHashSet<i16, BuildHasherDefault<LolHasher>, DefaultSharedPtr> =
            Default::default();
        set.insert(14658);
        assert_eq!(1, set.len());
        set.insert(-19198);
        assert_eq!(2, set.len());
    }

    #[test]
    fn match_strings_with_string_slices() {
        let mut set: HashSet<String> = From::from(&hashset!["foo", "bar"]);
        set = set.without("bar");
        assert!(!set.contains("bar"));
        set.remove("foo");
        assert!(!set.contains("foo"));
    }

    #[test]
    fn macro_allows_trailing_comma() {
        let set1 = hashset! {"foo", "bar"};
        let set2 = hashset! {
            "foo",
            "bar",
        };
        assert_eq!(set1, set2);
    }

    #[test]
    fn issue_60_drain_iterator_memory_corruption() {
        use crate::test::MetroHashBuilder;
        for i in 0..1000 {
            let mut lhs = vec![0, 1, 2];
            lhs.sort_unstable();

            let hasher = MetroHashBuilder::new(i);
            let mut iset: GenericHashSet<_, MetroHashBuilder, DefaultSharedPtr> =
                GenericHashSet::with_hasher(hasher);
            for &i in &lhs {
                iset.insert(i);
            }

            let mut rhs: Vec<_> = iset.clone().into_iter().collect();
            rhs.sort_unstable();

            if lhs != rhs {
                println!("iteration: {}", i);
                println!("seed: {}", hasher.seed());
                println!("lhs: {}: {:?}", lhs.len(), &lhs);
                println!("rhs: {}: {:?}", rhs.len(), &rhs);
                panic!();
            }
        }
    }

    #[test]
    fn partial_eq_ptr_eq_fast_path() {
        // Cloned sets with shared structure are equal in O(1).
        let set: HashSet<i32> = (0..100).collect();
        let set2 = set.clone();
        assert_eq!(set, set2);

        // After mutation, ptr_eq is false but element-wise equality still works.
        let mut set3 = set.clone();
        set3.insert(999);
        assert_ne!(set, set3);

        // Empty sets.
        let empty: HashSet<i32> = HashSet::new();
        let empty2: HashSet<i32> = HashSet::new();
        assert_eq!(empty, empty2);

        // Self-comparison.
        assert_eq!(set, set);
    }

    #[test]
    fn partial_eq_merkle_negative_check() {
        // Cloned sets share a hasher. Merkle negative check should catch
        // inequality without full tree traversal.
        let mut a: HashSet<i32> = (0..1000).collect();
        let b = a.clone();
        assert_eq!(a, b);

        // Modify a — different Merkle hash, same hasher
        a.insert(9999);
        assert_ne!(a, b);

        // Remove to make same length but different elements
        a.remove(&0);
        assert_eq!(a.len(), b.len()); // same length
        assert_ne!(a, b); // Merkle catches this
    }

    #[test]
    fn diff_identical_sets() {
        let set: HashSet<i32> = (0..50).collect();
        let set2 = set.clone();
        assert_eq!(set.diff(&set2).count(), 0);
    }

    #[test]
    fn diff_ptr_eq_fast_path() {
        let set: HashSet<i32> = (0..50).collect();
        let set2 = set.clone();
        assert!(set.ptr_eq(&set2));
        assert_eq!(set.diff(&set2).count(), 0);
    }

    #[test]
    fn diff_additions() {
        let set1: HashSet<i32> = HashSet::new();
        let set2: HashSet<i32> = (0..3).collect();
        let diffs: Vec<_> = set1.diff(&set2).collect();
        assert_eq!(diffs.len(), 3);
        assert!(diffs.iter().all(|d| matches!(d, DiffItem::Add(_))));
    }

    #[test]
    fn diff_removals() {
        let set1: HashSet<i32> = (0..3).collect();
        let set2: HashSet<i32> = HashSet::new();
        let diffs: Vec<_> = set1.diff(&set2).collect();
        assert_eq!(diffs.len(), 3);
        assert!(diffs.iter().all(|d| matches!(d, DiffItem::Remove(_))));
    }

    #[test]
    fn diff_mixed() {
        let set1: HashSet<i32> = (0..5).collect();
        let mut set2 = set1.clone();
        set2.remove(&0);
        set2.insert(10);
        let diffs: Vec<_> = set1.diff(&set2).collect();
        assert_eq!(diffs.len(), 2);
        let mut adds = 0;
        let mut removes = 0;
        for d in &diffs {
            match d {
                DiffItem::Add(_) => adds += 1,
                DiffItem::Remove(_) => removes += 1,
            }
        }
        assert_eq!(adds, 1);
        assert_eq!(removes, 1);
    }

    #[test]
    fn diff_empty_sets() {
        let set1: HashSet<i32> = HashSet::new();
        let set2: HashSet<i32> = HashSet::new();
        assert_eq!(set1.diff(&set2).count(), 0);
    }

    #[test]
    fn diff_is_fused() {
        let mut set1: HashSet<i32> = HashSet::new();
        set1.insert(1);
        let set2: HashSet<i32> = HashSet::new();
        let mut iter = set1.diff(&set2);
        assert!(iter.next().is_some());
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[test]
    fn diff_root_merkle_fast_path_equal_sets() {
        // Cloned sets share hasher_id and have the same root Merkle hash.
        // The root Merkle fast-path should fire and return an empty diff.
        let mut set = HashSet::new();
        for i in 0..500 {
            set.insert(i);
        }
        let other = set.clone();
        assert_eq!(set.diff(&other).count(), 0);
    }

    #[test]
    fn diff_root_merkle_fast_path_after_insert_remove() {
        // Insert + remove restores the original key set and Merkle hash.
        // The resulting set is equal to the original, diff should be empty.
        let mut set1 = HashSet::new();
        for i in 0..200 {
            set1.insert(i);
        }
        let set2 = {
            let mut s = set1.clone();
            s.insert(9999);
            s.remove(&9999);
            s
        };
        assert_eq!(set1.diff(&set2).count(), 0);
    }

    #[test]
    fn diff_merkle_subtree_pruning_correctness() {
        // Two large sets that differ in only a few elements. After a clone +
        // mutation, some subtrees are content-equal but not pointer-equal.
        // The per-node Merkle pruning should skip those, yielding the same
        // result as a full tree walk.
        let mut set1 = HashSet::new();
        for i in 0..1000 {
            set1.insert(i);
        }
        let mut set2 = set1.clone();
        set2.remove(&42);
        set2.insert(9999);

        let diffs: Vec<_> = set1.diff(&set2).collect();
        assert_eq!(diffs.len(), 2);
    }

    #[test]
    fn apply_diff_roundtrip() {
        let base = hashset! {1, 2, 3};
        let modified = hashset! {2, 3, 4};
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_empty_diff() {
        let set = hashset! {1, 2, 3};
        let patched = set.apply_diff(vec![]);
        assert_eq!(patched, set);
    }

    #[test]
    fn apply_diff_from_empty() {
        let base: HashSet<i32> = HashSet::new();
        let modified = hashset! {1, 2, 3};
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_to_empty() {
        let base = hashset! {1, 2, 3};
        let modified: HashSet<i32> = HashSet::new();
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn partition_basic() {
        let set = hashset! {1, 2, 3, 4, 5};
        let (evens, odds) = set.partition(|v| v % 2 == 0);
        assert_eq!(evens, hashset! {2, 4});
        assert_eq!(odds, hashset! {1, 3, 5});
    }

    #[test]
    fn disjoint_basic() {
        let a = hashset! {1, 2, 3};
        let b = hashset! {4, 5, 6};
        let c = hashset! {3, 4, 5};
        assert!(a.disjoint(&b));
        assert!(!a.disjoint(&c));
    }

    #[test]
    fn disjoint_empty() {
        let a = hashset! {1, 2};
        let b: HashSet<i32> = HashSet::new();
        assert!(a.disjoint(&b));
        assert!(b.disjoint(&a));
    }

    #[test]
    fn restrict_basic() {
        let set = hashset! {1, 2, 3, 4, 5};
        let keep = hashset! {2, 4, 6};
        assert_eq!(set.restrict(&keep), hashset! {2, 4});
    }

    proptest! {
        #[test]
        fn proptest_a_set(ref s in hash_set(".*", 10..100)) {
            assert!(s.len() < 100);
            assert!(s.len() >= 10);
        }
    }

    // --- Coverage gap tests ---

    #[test]
    fn new_unit_is_empty_len() {
        let empty: HashSet<i32> = HashSet::new();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let single = HashSet::unit(42);
        assert!(!single.is_empty());
        assert_eq!(single.len(), 1);
        assert!(single.contains(&42));
    }

    #[test]
    fn clear() {
        let mut set = hashset! {1, 2, 3};
        set.clear();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn hasher_and_new_from() {
        let set: HashSet<i32> = HashSet::new();
        let _h = set.hasher(); // just verify it's accessible
        let set2: HashSet<String> = set.new_from();
        assert!(set2.is_empty());
    }

    #[test]
    fn is_subset_is_proper_subset() {
        let a = hashset! {1, 2, 3};
        let b = hashset! {1, 2, 3, 4, 5};
        let c = hashset! {1, 2, 3};
        assert!(a.is_subset(&b));
        assert!(a.is_subset(&c));
        assert!(a.is_proper_subset(&b));
        assert!(!a.is_proper_subset(&c));
        assert!(!b.is_subset(&a));
    }

    #[test]
    fn union_basic() {
        let a = hashset! {1, 2, 3};
        let b = hashset! {3, 4, 5};
        let u = a.union(b);
        assert_eq!(u.len(), 5);
        for i in 1..=5 {
            assert!(u.contains(&i));
        }
    }

    #[test]
    fn unions_multiple() {
        let sets = vec![hashset! {1, 2}, hashset! {2, 3}, hashset! {3, 4}];
        let u = HashSet::unions(sets);
        assert_eq!(u.len(), 4);

        let empty: Vec<HashSet<i32>> = vec![];
        assert!(HashSet::unions(empty).is_empty());
    }

    #[test]
    fn symmetric_difference_basic() {
        let a = hashset! {1, 2, 3};
        let b = hashset! {2, 3, 4};
        let sd = a.symmetric_difference(b);
        assert_eq!(sd.len(), 2);
        assert!(sd.contains(&1));
        assert!(sd.contains(&4));
    }

    #[test]
    fn relative_complement_basic() {
        let a = hashset! {1, 2, 3, 4, 5};
        let b = hashset! {2, 4};
        let rc = a.relative_complement(b);
        assert_eq!(rc.len(), 3);
        assert!(rc.contains(&1));
        assert!(rc.contains(&3));
        assert!(rc.contains(&5));
    }

    #[test]
    fn intersection_basic() {
        let a = hashset! {1, 2, 3, 4};
        let b = hashset! {2, 4, 6};
        let i = a.intersection(b);
        assert_eq!(i.len(), 2);
        assert!(i.contains(&2));
        assert!(i.contains(&4));
    }

    #[test]
    fn update_persistent() {
        let set = hashset! {1, 2, 3};
        let updated = set.update(4);
        assert_eq!(updated.len(), 4);
        assert!(updated.contains(&4));
        assert_eq!(set.len(), 3); // original unchanged
    }

    #[test]
    fn without_persistent() {
        let set = hashset! {1, 2, 3};
        let smaller = set.without(&2);
        assert_eq!(smaller.len(), 2);
        assert!(!smaller.contains(&2));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn insert_remove_returns() {
        let mut set = hashset! {1, 2, 3};
        assert_eq!(set.insert(4), None);
        assert_eq!(set.insert(2), Some(2));
        assert_eq!(set.remove(&3), Some(3));
        assert_eq!(set.remove(&99), None);
    }

    #[test]
    fn retain_basic() {
        let mut set = hashset! {1, 2, 3, 4, 5};
        set.retain(|v| v % 2 != 0);
        assert_eq!(set.len(), 3);
        assert!(set.contains(&1));
        assert!(set.contains(&3));
        assert!(set.contains(&5));
    }

    #[test]
    fn add_mul_operators() {
        let a = hashset! {1, 2};
        let b = hashset! {2, 3};
        let union: HashSet<i32> = a + b;
        assert_eq!(union.len(), 3);

        let a = hashset! {1, 2, 3};
        let b = hashset! {2, 3, 4};
        let inter: HashSet<i32> = a * b;
        assert_eq!(inter.len(), 2);
    }

    #[test]
    fn sum_trait() {
        let sets = vec![hashset! {1, 2}, hashset! {2, 3}, hashset! {3, 4}];
        let union: HashSet<i32> = sets.into_iter().sum();
        assert_eq!(union.len(), 4);
    }

    #[test]
    fn from_conversions() {
        // From<Vec>
        let set: HashSet<i32> = HashSet::from(vec![3, 1, 2, 1]);
        assert_eq!(set.len(), 3);

        // From<std::HashSet>
        let std_set: std::collections::HashSet<i32> = [1, 2, 3].into_iter().collect();
        let set: HashSet<i32> = HashSet::from(std_set);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn extend_trait() {
        let mut set = hashset! {1, 2};
        set.extend(vec![3, 4, 5]);
        assert_eq!(set.len(), 5);
    }

    #[test]
    fn into_iterator() {
        let set = hashset! {1, 2, 3};
        let mut items: Vec<i32> = set.into_iter().collect();
        items.sort();
        assert_eq!(items, vec![1, 2, 3]);
    }

    #[test]
    fn iter_fused_and_exact_size() {
        let set = hashset! {1, 2, 3, 4, 5};
        let mut it = set.iter();
        assert_eq!(it.len(), 5);
        it.next();
        assert_eq!(it.len(), 4);
        while it.next().is_some() {}
        assert_eq!(it.len(), 0);
        assert_eq!(it.next(), None);
        assert_eq!(it.next(), None);
    }

    #[test]
    fn debug_display() {
        let set = hashset! {1, 2, 3};
        let debug = format!("{:?}", set);
        assert!(debug.contains('1'));
    }

    #[test]
    fn large_set_operations() {
        let n = 500;
        let a: HashSet<i32> = (0..n).collect();
        let b: HashSet<i32> = (n / 2..n + n / 2).collect();

        let u = a.clone().union(b.clone());
        assert_eq!(u.len() as i32, n + n / 2);

        let i = a.clone().intersection(b.clone());
        assert_eq!(i.len() as i32, n / 2);

        let rc = a.clone().relative_complement(b.clone());
        assert_eq!(rc.len() as i32, n / 2);

        let sd = a.symmetric_difference(b);
        assert_eq!(sd.len() as i32, n);
    }

    #[test]
    fn from_ordset() {
        let os = ordset! {3, 1, 2};
        let hs: HashSet<i32> = HashSet::from(os);
        assert_eq!(hs.len(), 3);
        assert!(hs.contains(&1));
        assert!(hs.contains(&2));
        assert!(hs.contains(&3));
    }

    #[test]
    fn hash_order_independent() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(s: &HashSet<i32>) -> u64 {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        }
        let mut a = HashSet::new();
        a.insert(1); a.insert(2); a.insert(3);
        let mut b = HashSet::new();
        b.insert(3); b.insert(1); b.insert(2); // different insertion order
        assert_eq!(hash_of(&a), hash_of(&b));
    }
}
