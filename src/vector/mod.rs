// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent vector.
//!
//! This is a sequence of elements in insertion order - if you need a
//! list of things, any kind of list of things, this is what you're
//! looking for.
//!
//! It's implemented as an [RRB vector][rrbpaper] with [smart
//! head/tail chunking][chunkedseq]. In performance terms, this means
//! that practically every operation is O(log n), except push/pop on
//! both sides, which will be O(1) amortised, and O(log n) in the
//! worst case. In practice, the push/pop operations will be
//! blindingly fast, nearly on par with the native
//! [`VecDeque`][VecDeque], and other operations will have decent, if
//! not high, performance, but they all have more or less the same
//! O(log n) complexity, so you don't need to keep their performance
//! characteristics in mind - everything, even splitting and merging,
//! is safe to use and never too slow.
//!
//! ## Performance Notes
//!
//! Because of the head/tail chunking technique, until you push a
//! number of items above double the tree's branching factor (that's
//! `self.len()` = 2 × *k* (where *k* = 64) = 128) on either side, the
//! data structure is still just a handful of arrays, not yet an RRB
//! tree, so you'll see performance and memory characteristics fairly
//! close to [`Vec`][Vec] or [`VecDeque`][VecDeque].
//!
//! This means that the structure always preallocates four chunks of
//! size *k* (*k* being the tree's branching factor), equivalent to a
//! [`Vec`][Vec] with an initial capacity of 256. Beyond that, it will
//! allocate tree nodes of capacity *k* as needed.
//!
//! In addition, vectors start out as single chunks, and only expand into the
//! full data structure once you go past the chunk size. This makes them
//! perform identically to [`Vec`][Vec] at small sizes.
//!
//! [rrbpaper]: https://infoscience.epfl.ch/record/213452/files/rrbvector.pdf
//! [chunkedseq]: http://deepsea.inria.fr/pasl/chunkedseq.pdf
//! [Vec]: https://doc.rust-lang.org/std/vec/struct.Vec.html
//! [VecDeque]: https://doc.rust-lang.org/std/collections/struct.VecDeque.html

#![allow(unsafe_code)] // Vector's focus/spine manipulations require raw pointer arithmetic and unsafe slice ops that cannot be expressed in safe Rust.

use alloc::borrow::ToOwned;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::Sum;
use core::iter::{FromIterator, FusedIterator};
use core::mem::{replace, swap};
use core::ops::{Add, Index, IndexMut, RangeBounds};

use archery::{SharedPointer, SharedPointerKind};
use imbl_sized_chunks::InlineArray;

use crate::config::{MERKLE_HASH_BITS, MERKLE_POSITIVE_EQ_MIN_BITS};
use crate::nodes::chunk::{Chunk, CHUNK_SIZE};
use crate::nodes::rrb::{
    chunk_merkle_hash, hash_element, Node, PopResult, PushResult, SplitResult, MERKLE_PRIME,
};
use crate::shared_ptr::DefaultSharedPtr;
use crate::sort;
use crate::util::{clone_ref, to_range, Side};

use self::VectorInner::{Full, Inline, Single};

mod focus;

pub use self::focus::{Focus, FocusMut};

#[cfg(any(test, feature = "rayon"))]
pub mod rayon;

/// Construct a vector from a sequence of elements.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use pds::Vector;
/// # fn main() {
/// assert_eq!(
///   vector![1, 2, 3],
///   Vector::from(vec![1, 2, 3])
/// );
/// # }
/// ```
#[macro_export]
macro_rules! vector {
    () => { $crate::vector::Vector::new() };

    ( $($x:expr),* ) => {{
        let mut l = $crate::vector::Vector::new();
        $(
            l.push_back($x);
        )*
            l
    }};

    ( $($x:expr ,)* ) => {{
        let mut l = $crate::vector::Vector::new();
        $(
            l.push_back($x);
        )*
            l
    }};
}

/// Type alias for [`GenericVector`] that uses [`DefaultSharedPtr`] as the pointer type.
///
/// [GenericVector]: ./struct.GenericVector.html
/// [DefaultSharedPtr]: ../shared_ptr/type.DefaultSharedPtr.html
pub type Vector<A> = GenericVector<A, DefaultSharedPtr>;

/// A persistent vector.
///
/// This is a sequence of elements in insertion order — if you need a list of
/// things, any kind of list of things, this is what you're looking for.
///
/// It's implemented as an [RRB vector][rrbpaper] with [smart head/tail
/// chunking][chunkedseq]. In performance terms, this means that practically
/// every operation is O(log n), except push/pop on both sides, which will be
/// O(1) amortised, and O(log n) in the worst case. In practice, the push/pop
/// operations will be blindingly fast, nearly on par with the native
/// [`VecDeque`][VecDeque], and other operations will have decent, if not high,
/// performance, but they all have more or less the same O(log n) complexity, so
/// you don't need to keep their performance characteristics in mind —
/// everything, even splitting and merging, is safe to use and never too slow.
///
/// ## Complexity vs Standard Library
///
/// | Operation | `Vector` | `Vec` | `VecDeque` |
/// |---|---|---|---|
/// | `clone` | **O(1)** | O(n) | O(n) |
/// | `eq` (Merkle, same lineage) | **O(1)**† | O(n) | O(n) |
/// | `eq` (fallback) | O(n) | O(n) | O(n) |
/// | `push_front` | **O(1)\*** | O(n) | O(1)\* |
/// | `push_back` | O(1)\* | O(1)\* | O(1)\* |
/// | `pop_front` | **O(1)\*** | O(n) | O(1)\* |
/// | `pop_back` | O(1)\* | O(1)\* | O(1)\* |
/// | `get` / `index` | O(log n) | **O(1)** | **O(1)** |
/// | `set` / `index_mut` | O(log n) | **O(1)** | **O(1)** |
/// | `insert` (middle) | **O(log n)** | O(n) | O(n) |
/// | `remove` (middle) | **O(log n)** | O(n) | O(n) |
/// | `split_at` / `split_off` | **O(log n)** | O(n) | O(n) |
/// | `append` (concatenation) | **O(log n)** | O(n) | O(n) |
/// | `sort` | O(n log n) | O(n log n) | O(n log n) |
/// | `from_iter` | O(n) | O(n) | O(n) |
///
/// **Bold** = asymptotically better than the std alternative.
/// \* = amortised. † = requires [`recompute_merkle`][Self::recompute_merkle]
/// (costs O(k log n) where k = modified nodes since last computation).
///
/// The key advantage is that `clone`, `split`, and `append` are dramatically
/// cheaper due to structural sharing. Two vectors that are clones of each
/// other (or share a common ancestor) share their tree nodes in memory —
/// only modified paths are copied.
///
/// ## Merkle Hashing
///
/// Each internal RRB node maintains a lazy Merkle hash. When you call
/// [`recompute_merkle`][Self::recompute_merkle], only modified subtrees are
/// re-hashed — cached subtrees return instantly. This gives O(k log n) cost
/// where k is the number of nodes modified since the last computation.
///
/// Once both vectors have valid Merkle hashes, equality comparison is O(1):
/// matching hash + matching length = equal. This is position-sensitive —
/// `[a, b]` and `[b, a]` produce different hashes.
///
/// ## Performance Notes
///
/// Because of the head/tail chunking technique, until you push a number of
/// items above double the tree's branching factor (that's `self.len()` = 2 ×
/// *k* (where *k* = 64) = 128) on either side, the data structure is still just
/// a handful of arrays, not yet an RRB tree, so you'll see performance and
/// memory characteristics similar to [`Vec`][Vec] or [`VecDeque`][VecDeque].
///
/// This means that the structure always preallocates four chunks of size *k*
/// (*k* being the tree's branching factor), equivalent to a [`Vec`][Vec] with
/// an initial capacity of 256. Beyond that, it will allocate tree nodes of
/// capacity *k* as needed.
///
/// In addition, vectors start out as single chunks, and only expand into the
/// full data structure once you go past the chunk size. This makes them
/// perform identically to [`Vec`][Vec] at small sizes.
///
/// [rrbpaper]: https://infoscience.epfl.ch/record/213452/files/rrbvector.pdf
/// [chunkedseq]: http://deepsea.inria.fr/pasl/chunkedseq.pdf
/// [Vec]: https://doc.rust-lang.org/std/vec/struct.Vec.html
/// [VecDeque]: https://doc.rust-lang.org/std/collections/struct.VecDeque.html
pub struct GenericVector<A, P: SharedPointerKind> {
    vector: VectorInner<A, P>,
    /// Cached Merkle hash of the entire vector. Position-sensitive:
    /// `[a, b]` and `[b, a]` produce different hashes. Enables O(1)
    /// positive equality when both vectors have valid hashes.
    merkle_hash: u64,
    /// Whether `merkle_hash` is current. Invalidated by any mutation.
    /// Call `recompute_merkle()` to restore.
    merkle_valid: bool,
}

enum VectorInner<A, P: SharedPointerKind> {
    Inline(InlineArray<A, RRB<A, P>>),
    Single(SharedPointer<Chunk<A>, P>),
    Full(RRB<A, P>),
}

#[doc(hidden)]
pub struct RRB<A, P: SharedPointerKind> {
    length: usize,
    middle_level: usize,
    outer_f: SharedPointer<Chunk<A>, P>,
    inner_f: SharedPointer<Chunk<A>, P>,
    middle: SharedPointer<Node<A, P>, P>,
    inner_b: SharedPointer<Chunk<A>, P>,
    outer_b: SharedPointer<Chunk<A>, P>,
}

impl<A, P: SharedPointerKind> Clone for RRB<A, P> {
    fn clone(&self) -> Self {
        RRB {
            length: self.length,
            middle_level: self.middle_level,
            outer_f: self.outer_f.clone(),
            inner_f: self.inner_f.clone(),
            middle: self.middle.clone(),
            inner_b: self.inner_b.clone(),
            outer_b: self.outer_b.clone(),
        }
    }
}

impl<A, P: SharedPointerKind> GenericVector<A, P> {
    /// Wrap a `VectorInner` into a `GenericVector` with invalidated Merkle.
    #[inline]
    fn from_inner(vector: VectorInner<A, P>) -> Self {
        Self {
            vector,
            merkle_hash: 0,
            merkle_valid: false,
        }
    }

    /// True if a vector is a full inline or single chunk, ie. must be promoted
    /// to grow further.
    fn needs_promotion(&self) -> bool {
        match &self.vector {
            // Prevent the inline array from getting bigger than a single chunk. This means that we
            // can always promote `Inline` to `Single`, even when we're configured to have a small
            // chunk size. (TODO: it might be better to just never use `Single` in this situation,
            // but that's a more invasive change.)
            Inline(chunk) => chunk.is_full() || chunk.len() + 1 >= CHUNK_SIZE,
            Single(chunk) => chunk.is_full(),
            _ => false,
        }
    }

    /// Promote an inline to a single.
    fn promote_inline(&mut self) {
        if let Inline(chunk) = &mut self.vector {
            self.vector = Single(SharedPointer::new(chunk.into()));
        }
    }

    /// Promote a single to a full, with the single chunk becoming inner_f, or
    /// promote an inline to a single.
    fn promote_front(&mut self) {
        self.vector = match &mut self.vector {
            Inline(chunk) => Single(SharedPointer::new(chunk.into())),
            Single(chunk) => {
                let chunk = chunk.clone();
                Full(RRB {
                    length: chunk.len(),
                    middle_level: 0,
                    outer_f: SharedPointer::default(),
                    inner_f: chunk,
                    middle: SharedPointer::new(Node::new()),
                    inner_b: SharedPointer::default(),
                    outer_b: SharedPointer::default(),
                })
            }
            Full(_) => return,
        }
    }

    /// Promote a single to a full, with the single chunk becoming inner_b, or
    /// promote an inline to a single.
    fn promote_back(&mut self) {
        self.vector = match &mut self.vector {
            Inline(chunk) => Single(SharedPointer::new(chunk.into())),
            Single(chunk) => {
                let chunk = chunk.clone();
                Full(RRB {
                    length: chunk.len(),
                    middle_level: 0,
                    outer_f: SharedPointer::default(),
                    inner_f: SharedPointer::default(),
                    middle: SharedPointer::new(Node::new()),
                    inner_b: chunk,
                    outer_b: SharedPointer::default(),
                })
            }
            Full(_) => return,
        }
    }

    /// Construct an empty vector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vector: Inline(InlineArray::new()),
            merkle_hash: 0,
            merkle_valid: true,
        }
    }

    /// Get the length of a vector.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::Vector;
    /// let vec: Vector<i64> = vector![1, 2, 3, 4, 5];
    /// assert_eq!(5, vec.len());
    /// ```
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.vector {
            Inline(chunk) => chunk.len(),
            Single(chunk) => chunk.len(),
            Full(tree) => tree.length,
        }
    }

    /// Test whether a vector is empty.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::Vector;
    /// let vec = vector!["Joe", "Mike", "Robert"];
    /// assert_eq!(false, vec.is_empty());
    /// assert_eq!(true, Vector::<i64>::new().is_empty());
    /// ```
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Test whether a vector is currently inlined.
    ///
    /// Vectors small enough that their contents could be stored entirely inside
    /// the space of `std::mem::size_of::<GenericVector<A, P>>()` bytes are stored inline on
    /// the stack instead of allocating any chunks. This method returns `true` if
    /// this vector is currently inlined, or `false` if it currently has chunks allocated
    /// on the heap.
    ///
    /// This may be useful in conjunction with [`ptr_eq()`][ptr_eq], which checks if
    /// two vectors' heap allocations are the same, and thus will never return `true`
    /// for inlined vectors.
    ///
    /// Time: O(1)
    ///
    /// [ptr_eq]: #method.ptr_eq
    #[inline]
    #[must_use]
    pub fn is_inline(&self) -> bool {
        matches!(self.vector, Inline(_))
    }

    /// Test whether two vectors refer to the same content in memory.
    ///
    /// This uses the following rules to determine equality:
    /// * If the two sides are references to the same vector, return true.
    /// * If the two sides are single chunk vectors pointing to the same chunk, return true.
    /// * If the two sides are full trees pointing to the same chunks, return true.
    ///
    /// This would return true if you're comparing a vector to itself, or
    /// if you're comparing a vector to a fresh clone of itself. The exception to this is
    /// if you've cloned an inline array (ie. an array with so few elements they can fit
    /// inside the space a `Vector` allocates for its pointers, so there are no heap allocations
    /// to compare).
    ///
    /// Time: O(1)
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        fn cmp_chunk<A, P: SharedPointerKind>(
            left: &SharedPointer<Chunk<A>, P>,
            right: &SharedPointer<Chunk<A>, P>,
        ) -> bool {
            (left.is_empty() && right.is_empty()) || SharedPointer::ptr_eq(left, right)
        }

        if core::ptr::eq(self, other) {
            return true;
        }

        match (&self.vector, &other.vector) {
            (Single(left), Single(right)) => cmp_chunk(left, right),
            (Full(left), Full(right)) => {
                cmp_chunk(&left.outer_f, &right.outer_f)
                    && cmp_chunk(&left.inner_f, &right.inner_f)
                    && cmp_chunk(&left.inner_b, &right.inner_b)
                    && cmp_chunk(&left.outer_b, &right.outer_b)
                    && ((left.middle.is_empty() && right.middle.is_empty())
                        || SharedPointer::ptr_eq(&left.middle, &right.middle))
            }
            _ => false,
        }
    }

    /// Compute the positional diff between two vectors.
    ///
    /// Returns an iterator of [`DiffItem`] values describing the
    /// element-by-element differences between `self` (old) and `other`
    /// (new). Elements at the same index are compared with
    /// [`PartialEq`]; indices beyond the shorter vector produce
    /// [`DiffItem::Add`] or [`DiffItem::Remove`] items.
    ///
    /// If the two vectors share the same structure (i.e.
    /// [`ptr_eq`][GenericVector::ptr_eq] returns true), the iterator
    /// is empty without traversing any elements.
    ///
    /// When the two vectors share structure (e.g. one was derived from
    /// the other via [`set`][GenericVector::set]), shared leaf chunks
    /// are detected by pointer comparison and skipped in O(1) per chunk.
    /// This makes the effective complexity O(changes × tree_depth) for
    /// structurally-shared vectors, falling back to O(n) for
    /// independently-constructed vectors.
    ///
    /// Time: O(changes × tree_depth) for shared-structure vectors,
    /// O(n) worst case where n = max(self.len(), other.len())
    #[must_use]
    pub fn diff<'a, 'b>(&'a self, other: &'b Self) -> DiffIter<'a, 'b, A, P>
    where
        A: PartialEq,
    {
        let done = self.ptr_eq(other);
        let old_len = self.len();
        let new_len = other.len();
        DiffIter {
            old_focus: self.focus(),
            new_focus: other.focus(),
            old_len,
            new_len,
            index: 0,
            done,
        }
    }

    /// Get an iterator over a vector.
    ///
    /// Time: O(1)
    #[inline]
    #[must_use]
    pub fn iter(&self) -> Iter<'_, A, P> {
        Iter::new(self)
    }

    /// Get an iterator over the leaf nodes of a vector.
    ///
    /// This returns an iterator over the [`Chunk`s][Chunk] at the leaves of the
    /// RRB tree. These are useful for efficient parallelisation of work on
    /// the vector, but should not be used for basic iteration.
    ///
    /// Time: O(1)
    ///
    /// [Chunk]: ../chunk/struct.Chunk.html
    #[inline]
    #[must_use]
    pub fn leaves(&self) -> Chunks<'_, A, P> {
        Chunks::new(self)
    }

    /// Construct a [`Focus`][Focus] for a vector.
    ///
    /// Time: O(1)
    ///
    /// [Focus]: enum.Focus.html
    #[inline]
    #[must_use]
    pub fn focus(&self) -> Focus<'_, A, P> {
        Focus::new(self)
    }

    /// Get a reference to the value at index `index` in a vector.
    ///
    /// Returns `None` if the index is out of bounds.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let vec = vector!["Joe", "Mike", "Robert"];
    /// assert_eq!(Some(&"Robert"), vec.get(2));
    /// assert_eq!(None, vec.get(5));
    /// ```
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&A> {
        if index >= self.len() {
            return None;
        }

        match &self.vector {
            Inline(chunk) => chunk.get(index),
            Single(chunk) => chunk.get(index),
            Full(tree) => {
                let mut local_index = index;

                if local_index < tree.outer_f.len() {
                    return Some(&tree.outer_f[local_index]);
                }
                local_index -= tree.outer_f.len();

                if local_index < tree.inner_f.len() {
                    return Some(&tree.inner_f[local_index]);
                }
                local_index -= tree.inner_f.len();

                if local_index < tree.middle.len() {
                    return Some(tree.middle.index(tree.middle_level, local_index));
                }
                local_index -= tree.middle.len();

                if local_index < tree.inner_b.len() {
                    return Some(&tree.inner_b[local_index]);
                }
                local_index -= tree.inner_b.len();

                Some(&tree.outer_b[local_index])
            }
        }
    }

    /// Get the first element of a vector.
    ///
    /// If the vector is empty, `None` is returned.
    ///
    /// Time: O(log n)
    #[inline]
    #[must_use]
    pub fn front(&self) -> Option<&A> {
        self.get(0)
    }

    /// Get the first element of a vector.
    ///
    /// If the vector is empty, `None` is returned.
    ///
    /// This is an alias for the [`front`][front] method.
    ///
    /// Time: O(log n)
    ///
    /// [front]: #method.front
    #[inline]
    #[must_use]
    pub fn head(&self) -> Option<&A> {
        self.get(0)
    }

    /// Get the last element of a vector.
    ///
    /// If the vector is empty, `None` is returned.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn back(&self) -> Option<&A> {
        if self.is_empty() {
            None
        } else {
            self.get(self.len() - 1)
        }
    }

    /// Get the last element of a vector.
    ///
    /// If the vector is empty, `None` is returned.
    ///
    /// This is an alias for the [`back`][back] method.
    ///
    /// Time: O(log n)
    ///
    /// [back]: #method.back
    #[inline]
    #[must_use]
    pub fn last(&self) -> Option<&A> {
        self.back()
    }

    /// Get the index of a given element in the vector.
    ///
    /// Searches the vector for the first occurrence of a given value,
    /// and returns the index of the value if it's there. Otherwise,
    /// it returns `None`.
    ///
    /// Time: O(n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3, 4, 5];
    /// assert_eq!(Some(2), vec.index_of(&3));
    /// assert_eq!(None, vec.index_of(&31337));
    /// ```
    #[must_use]
    pub fn index_of(&self, value: &A) -> Option<usize>
    where
        A: PartialEq,
    {
        for (index, item) in self.iter().enumerate() {
            if value == item {
                return Some(index);
            }
        }
        None
    }

    /// Test if a given element is in the vector.
    ///
    /// Searches the vector for the first occurrence of a given value,
    /// and returns `true` if it's there. If it's nowhere to be found
    /// in the vector, it returns `false`.
    ///
    /// Time: O(n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3, 4, 5];
    /// assert_eq!(true, vec.contains(&3));
    /// assert_eq!(false, vec.contains(&31337));
    /// ```
    #[inline]
    #[must_use]
    pub fn contains(&self, value: &A) -> bool
    where
        A: PartialEq,
    {
        self.index_of(value).is_some()
    }

    /// Discard all elements from the vector.
    ///
    /// This leaves you with an empty vector, and all elements that
    /// were previously inside it are dropped.
    ///
    /// Time: O(n)
    pub fn clear(&mut self) {
        if !self.is_empty() {
            self.vector = Inline(InlineArray::new());
            self.merkle_hash = 0;
            self.merkle_valid = true;
        }
    }

    /// Binary search a sorted vector for a given element using a comparator
    /// function.
    ///
    /// Assumes the vector has already been sorted using the same comparator
    /// function, eg. by using [`sort_by`][sort_by].
    ///
    /// If the value is found, it returns `Ok(index)` where `index` is the index
    /// of the element. If the value isn't found, it returns `Err(index)` where
    /// `index` is the index at which the element would need to be inserted to
    /// maintain sorted order.
    ///
    /// Time: O(log n)
    ///
    /// [sort_by]: #method.sort_by
    pub fn binary_search_by<F>(&self, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&A) -> Ordering,
    {
        let mut size = self.len();
        if size == 0 {
            return Err(0);
        }
        let mut base = 0;
        while size > 1 {
            let half = size / 2;
            let mid = base + half;
            base = match f(&self[mid]) {
                Ordering::Greater => base,
                _ => mid,
            };
            size -= half;
        }
        match f(&self[base]) {
            Ordering::Equal => Ok(base),
            Ordering::Greater => Err(base),
            Ordering::Less => Err(base + 1),
        }
    }

    /// Binary search a sorted vector for a given element.
    ///
    /// If the value is found, it returns `Ok(index)` where `index` is the index
    /// of the element. If the value isn't found, it returns `Err(index)` where
    /// `index` is the index at which the element would need to be inserted to
    /// maintain sorted order.
    ///
    /// Time: O(log n)
    pub fn binary_search(&self, value: &A) -> Result<usize, usize>
    where
        A: Ord,
    {
        self.binary_search_by(|e| e.cmp(value))
    }

    /// Binary search a sorted vector for a given element with a key extract
    /// function.
    ///
    /// Assumes the vector has already been sorted using the same key extract
    /// function, eg. by using [`sort_by_key`][sort_by_key].
    ///
    /// If the value is found, it returns `Ok(index)` where `index` is the index
    /// of the element. If the value isn't found, it returns `Err(index)` where
    /// `index` is the index at which the element would need to be inserted to
    /// maintain sorted order.
    ///
    /// Time: O(log n)
    ///
    /// [sort_by_key]: #method.sort_by_key
    pub fn binary_search_by_key<B, F>(&self, b: &B, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&A) -> B,
        B: Ord,
    {
        self.binary_search_by(|k| f(k).cmp(b))
    }

    /// Construct a vector with a single value.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::Vector;
    /// let vec  = Vector::unit(1337);
    /// assert_eq!(1, vec.len());
    /// assert_eq!(
    ///   vec.get(0),
    ///   Some(&1337)
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn unit(a: A) -> Self {
        if InlineArray::<A, RRB<A, P>>::CAPACITY > 0 {
            let mut array = InlineArray::new();
            array.push(a);
            Self::from_inner(Inline(array))
        } else {
            let chunk = SharedPointer::new(Chunk::unit(a));
            Self::from_inner(Single(chunk))
        }
    }

    /// Dump the internal RRB tree into graphviz format.
    ///
    /// This method requires the `debug` feature flag.
    #[cfg(any(test, feature = "debug"))]
    pub fn dot<W: std::io::Write>(&self, write: W) -> std::io::Result<()> {
        if let Full(ref tree) = self.vector {
            tree.middle.dot(write)
        } else {
            Ok(())
        }
    }

    /// Verify the internal consistency of a vector.
    ///
    /// This method walks the RRB tree making up the current `Vector`
    /// (if it has one) and verifies that all the invariants hold.
    /// If something is wrong, it will panic.
    ///
    /// This method requires the `debug` feature flag.
    #[cfg(any(test, feature = "debug"))]
    pub fn assert_invariants(&self) {
        if let Full(ref tree) = self.vector {
            tree.assert_invariants();
        }
    }

    /// Returns the height of the middle tree (0 if no tree structure).
    /// Test-only helper for verifying concatenation depth bounds.
    #[cfg(test)]
    fn middle_level(&self) -> usize {
        match &self.vector {
            Inline(_) | Single(_) => 0,
            Full(tree) => tree.middle_level,
        }
    }
}

impl<A: Clone, P: SharedPointerKind> GenericVector<A, P> {
    /// Get a mutable reference to the value at index `index` in a
    /// vector.
    ///
    /// Returns `None` if the index is out of bounds.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector!["Joe", "Mike", "Robert"];
    /// {
    ///     let robert = vec.get_mut(2).unwrap();
    ///     assert_eq!(&mut "Robert", robert);
    ///     *robert = "Bjarne";
    /// }
    /// assert_eq!(vector!["Joe", "Mike", "Bjarne"], vec);
    /// ```
    #[must_use]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut A> {
        if index >= self.len() {
            return None;
        }
        self.merkle_valid = false;

        match &mut self.vector {
            Inline(chunk) => chunk.get_mut(index),
            Single(chunk) => SharedPointer::make_mut(chunk).get_mut(index),
            Full(tree) => {
                let mut local_index = index;

                if local_index < tree.outer_f.len() {
                    let outer_f = SharedPointer::make_mut(&mut tree.outer_f);
                    return Some(&mut outer_f[local_index]);
                }
                local_index -= tree.outer_f.len();

                if local_index < tree.inner_f.len() {
                    let inner_f = SharedPointer::make_mut(&mut tree.inner_f);
                    return Some(&mut inner_f[local_index]);
                }
                local_index -= tree.inner_f.len();

                if local_index < tree.middle.len() {
                    let middle = SharedPointer::make_mut(&mut tree.middle);
                    return Some(middle.index_mut(tree.middle_level, local_index));
                }
                local_index -= tree.middle.len();

                if local_index < tree.inner_b.len() {
                    let inner_b = SharedPointer::make_mut(&mut tree.inner_b);
                    return Some(&mut inner_b[local_index]);
                }
                local_index -= tree.inner_b.len();

                let outer_b = SharedPointer::make_mut(&mut tree.outer_b);
                Some(&mut outer_b[local_index])
            }
        }
    }

    /// Get a mutable reference to the first element of a vector.
    ///
    /// If the vector is empty, `None` is returned.
    ///
    /// Time: O(log n)
    #[inline]
    #[must_use]
    pub fn front_mut(&mut self) -> Option<&mut A> {
        self.get_mut(0)
    }

    /// Get a mutable reference to the last element of a vector.
    ///
    /// If the vector is empty, `None` is returned.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn back_mut(&mut self) -> Option<&mut A> {
        if self.is_empty() {
            None
        } else {
            let len = self.len();
            self.get_mut(len - 1)
        }
    }

    /// Construct a [`FocusMut`][FocusMut] for a vector.
    ///
    /// Time: O(1)
    ///
    /// [FocusMut]: enum.FocusMut.html
    #[inline]
    #[must_use]
    pub fn focus_mut(&mut self) -> FocusMut<'_, A, P> {
        self.merkle_valid = false;
        FocusMut::new(self)
    }

    /// Get a mutable iterator over a vector.
    ///
    /// Time: O(1)
    #[inline]
    #[must_use]
    pub fn iter_mut(&mut self) -> IterMut<'_, A, P> {
        self.merkle_valid = false;
        IterMut::new(self)
    }

    /// Get a mutable iterator over the leaf nodes of a vector.
    //
    /// This returns an iterator over the [`Chunk`s][Chunk] at the leaves of the
    /// RRB tree. These are useful for efficient parallelisation of work on
    /// the vector, but should not be used for basic iteration.
    ///
    /// Time: O(1)
    ///
    /// [Chunk]: ../chunk/struct.Chunk.html
    #[inline]
    #[must_use]
    pub fn leaves_mut(&mut self) -> ChunksMut<'_, A, P> {
        self.merkle_valid = false;
        ChunksMut::new(self)
    }

    /// Create a new vector with the value at index `index` updated.
    ///
    /// Panics if the index is out of bounds.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3];
    /// assert_eq!(vector![1, 5, 3], vec.update(1, 5));
    /// ```
    #[must_use]
    pub fn update(&self, index: usize, value: A) -> Self {
        let mut out = self.clone();
        out[index] = value;
        out
    }

    /// Update the value at index `index` in a vector.
    ///
    /// Returns the previous value at the index.
    ///
    /// Panics if the index is out of bounds.
    ///
    /// Time: O(log n)
    #[inline]
    pub fn set(&mut self, index: usize, value: A) -> A {
        replace(&mut self[index], value)
    }

    /// Swap the elements at indices `i` and `j`.
    ///
    /// Time: O(log n)
    pub fn swap(&mut self, i: usize, j: usize) {
        if i != j {
            // Clone-and-replace: the second IndexMut call may trigger
            // copy-on-write (make_mut) which can invalidate pointers
            // obtained from the first call. The previous raw-pointer
            // implementation was UB (detected by miri). Element clone
            // is trivially cheap compared to the O(log n) tree walk
            // that IndexMut already performs.
            let a = self[i].clone();
            self[i] = replace(&mut self[j], a);
        }
    }

    /// Push a value to the front of a vector.
    ///
    /// Time: O(1)* amortised
    ///
    /// Compare: [`Vec`] has no `push_front`; inserting at index 0 is O(n).
    /// `VecDeque::push_front` is O(1)* amortised.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![5, 6, 7];
    /// vec.push_front(4);
    /// assert_eq!(vector![4, 5, 6, 7], vec);
    /// ```
    pub fn push_front(&mut self, value: A) {
        self.merkle_valid = false;
        if self.needs_promotion() {
            self.promote_back();
        }
        match &mut self.vector {
            Inline(chunk) => {
                chunk.insert(0, value);
            }
            Single(chunk) => SharedPointer::make_mut(chunk).push_front(value),
            Full(tree) => tree.push_front(value),
        }
    }

    /// Push a value to the back of a vector.
    ///
    /// Time: O(1)*
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3];
    /// vec.push_back(4);
    /// assert_eq!(vector![1, 2, 3, 4], vec);
    /// ```
    pub fn push_back(&mut self, value: A) {
        self.merkle_valid = false;
        if self.needs_promotion() {
            self.promote_front();
        }
        match &mut self.vector {
            Inline(chunk) => {
                chunk.push(value);
            }
            Single(chunk) => SharedPointer::make_mut(chunk).push_back(value),
            Full(tree) => tree.push_back(value),
        }
    }

    /// Apply a diff to produce a new vector.
    ///
    /// Takes any iterator of [`DiffItem`] values (such as from
    /// [`diff`][GenericVector::diff]) and applies each change —
    /// `Update` replaces the element at the given index, `Add`
    /// appends an element, and `Remove` truncates at that index.
    ///
    /// The diff items should be in the order produced by
    /// [`diff`][GenericVector::diff]: updates at shared indices,
    /// followed by either additions (appended to the end) or
    /// removals (truncated from the end).
    ///
    /// Time: O(d log n) where d is the number of diff items
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let base = vector![1, 2, 3, 4];
    /// let modified = vector![1, 20, 30, 4, 5];
    /// let diff: Vec<_> = base.diff(&modified).collect();
    /// let patched = base.apply_diff(diff);
    /// assert_eq!(patched, modified);
    /// ```
    #[must_use]
    pub fn apply_diff<'a, 'b, I>(&self, diff: I) -> Self
    where
        I: IntoIterator<Item = DiffItem<'a, 'b, A>>,
        A: PartialEq + 'a + 'b,
    {
        let mut out = self.clone();
        for item in diff {
            match item {
                DiffItem::Update { index, new, .. } => {
                    out.set(index, new.clone());
                }
                DiffItem::Add(_, value) => {
                    out.push_back(value.clone());
                }
                DiffItem::Remove(index, _) => {
                    out.truncate(index);
                    break;
                }
            }
        }
        out
    }

    /// Apply a function at a single index, returning a new vector
    /// with the element at that index replaced by the function's
    /// result. Avoids the get-transform-set pattern.
    ///
    /// Time: O(log n)
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let vec = vector![1, 2, 3, 4];
    /// let updated = vec.adjust(1, |v| v * 10);
    /// assert_eq!(updated, vector![1, 20, 3, 4]);
    /// ```
    #[must_use]
    pub fn adjust<F>(&self, index: usize, f: F) -> Self
    where
        F: FnOnce(&A) -> A,
    {
        let mut out = self.clone();
        let old = &out[index];
        let new_val = f(old);
        out.set(index, new_val);
        out
    }

    /// Split a vector into non-overlapping fixed-size chunks. The
    /// last chunk may contain fewer than `chunk_size` elements.
    ///
    /// Time: O(n log n)
    ///
    /// # Panics
    ///
    /// Panics if `chunk_size` is 0.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let vec = vector![1, 2, 3, 4, 5];
    /// let chunks = vec.chunked(2);
    /// assert_eq!(chunks, vec![vector![1, 2], vector![3, 4], vector![5]]);
    /// ```
    #[must_use]
    pub fn chunked(&self, chunk_size: usize) -> Vec<Self> {
        assert!(chunk_size > 0, "chunk_size must be greater than 0");
        if self.is_empty() {
            return Vec::new();
        }
        let num_chunks = self.len().div_ceil(chunk_size);
        let mut result = Vec::with_capacity(num_chunks);
        let mut remaining = self.clone();
        while remaining.len() > chunk_size {
            let (left, right) = remaining.split_at(chunk_size);
            result.push(left);
            remaining = right;
        }
        result.push(remaining);
        result
    }

    /// Replace a range of elements with the contents of another
    /// vector. Removes `replaced` elements starting at `from` and
    /// inserts all elements from `replacement` at that position.
    ///
    /// Time: O(n log n)
    ///
    /// # Panics
    ///
    /// Panics if `from + replaced > self.len()`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let vec = vector![1, 2, 3, 4, 5];
    /// let replacement = vector![20, 30];
    /// let patched = vec.patch(1, &replacement, 2);
    /// assert_eq!(patched, vector![1, 20, 30, 4, 5]);
    /// ```
    #[must_use]
    pub fn patch(&self, from: usize, replacement: &Self, replaced: usize) -> Self {
        assert!(
            from + replaced <= self.len(),
            "patch range {}..{} out of bounds for vector of length {}",
            from,
            from + replaced,
            self.len()
        );
        let (left, rest) = self.clone().split_at(from);
        let (_, right) = rest.split_at(replaced);
        let mut result = left;
        result.append(replacement.clone());
        result.append(right);
        result
    }

    /// Produce a vector of cumulative results by threading an
    /// accumulator through the elements from left to right.
    ///
    /// The output vector has `self.len() + 1` elements: the initial
    /// state followed by each intermediate result.
    ///
    /// Time: O(n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let vec = vector![1, 2, 3, 4];
    /// let sums = vec.scan_left(0, |acc, x| acc + x);
    /// assert_eq!(sums, vector![0, 1, 3, 6, 10]);
    /// ```
    #[must_use]
    pub fn scan_left<S, F>(&self, init: S, mut f: F) -> GenericVector<S, P>
    where
        S: Clone,
        F: FnMut(&S, &A) -> S,
    {
        let mut result = GenericVector::new();
        let mut acc = init;
        result.push_back(acc.clone());
        for item in self.iter() {
            acc = f(&acc, item);
            result.push_back(acc.clone());
        }
        result
    }

    /// Produce overlapping windows of a given size, advancing by
    /// `step` elements between each window.
    ///
    /// Time: O(n/step × size × log n) — each window is an O(log n)
    /// slice operation.
    ///
    /// # Panics
    ///
    /// Panics if `size` is 0 or `step` is 0.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let vec = vector![1, 2, 3, 4, 5];
    /// let windows = vec.sliding(3, 1);
    /// assert_eq!(windows, vec![
    ///     vector![1, 2, 3],
    ///     vector![2, 3, 4],
    ///     vector![3, 4, 5],
    /// ]);
    /// ```
    #[must_use]
    pub fn sliding(&self, size: usize, step: usize) -> Vec<Self> {
        assert!(size > 0, "window size must be greater than 0");
        assert!(step > 0, "step must be greater than 0");
        if self.len() < size {
            return Vec::new();
        }
        let num_windows = (self.len() - size) / step + 1;
        let mut result = Vec::with_capacity(num_windows);
        let mut start = 0;
        while start + size <= self.len() {
            result.push(self.skip(start).take(size));
            start += step;
        }
        result
    }

    /// Remove the first element from a vector and return it.
    ///
    /// Time: O(1)*
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3];
    /// assert_eq!(Some(1), vec.pop_front());
    /// assert_eq!(vector![2, 3], vec);
    /// ```
    pub fn pop_front(&mut self) -> Option<A> {
        if self.is_empty() {
            None
        } else {
            self.merkle_valid = false;
            match &mut self.vector {
                Inline(chunk) => chunk.remove(0),
                Single(chunk) => Some(SharedPointer::make_mut(chunk).pop_front()),
                Full(tree) => tree.pop_front(),
            }
        }
    }

    /// Remove the last element from a vector and return it.
    ///
    /// Time: O(1)*
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::Vector;
    /// let mut vec = vector![1, 2, 3];
    /// assert_eq!(Some(3), vec.pop_back());
    /// assert_eq!(vector![1, 2], vec);
    /// ```
    pub fn pop_back(&mut self) -> Option<A> {
        if self.is_empty() {
            None
        } else {
            self.merkle_valid = false;
            match &mut self.vector {
                Inline(chunk) => chunk.pop(),
                Single(chunk) => Some(SharedPointer::make_mut(chunk).pop_back()),
                Full(tree) => tree.pop_back(),
            }
        }
    }

    /// Append the vector `other` to the end of the current vector.
    ///
    /// Time: O(log n)
    ///
    /// Compare: [`Vec::append`] is O(n). The RRB tree structure allows
    /// concatenation by linking subtrees rather than copying elements.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3];
    /// vec.append(vector![7, 8, 9]);
    /// assert_eq!(vector![1, 2, 3, 7, 8, 9], vec);
    /// ```
    pub fn append(&mut self, mut other: Self) {
        if other.is_empty() {
            return;
        }

        if self.is_empty() {
            *self = other;
            return;
        }

        self.merkle_valid = false;

        self.promote_inline();
        other.promote_inline();

        let total_length = self
            .len()
            .checked_add(other.len())
            .expect("Vector length overflow");

        match &mut self.vector {
            Inline(_) => unreachable!("inline vecs should have been promoted"),
            Single(left) => {
                match &mut other.vector {
                    Inline(_) => unreachable!("inline vecs should have been promoted"),
                    // If both are single chunks and left has room for right: directly
                    // memcpy right into left
                    Single(ref mut right) if total_length <= CHUNK_SIZE => {
                        SharedPointer::make_mut(left).append(SharedPointer::make_mut(right));
                        return;
                    }
                    // If only left is a single chunk and has room for right: push
                    // right's elements into left
                    _ if total_length <= CHUNK_SIZE => {
                        while let Some(value) = other.pop_front() {
                            SharedPointer::make_mut(left).push_back(value);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            Full(left) => {
                if let Full(mut right) = other.vector {
                    // If left and right are trees with empty middles, left has no back
                    // buffers, and right has no front buffers: copy right's back
                    // buffers over to left
                    if left.middle.is_empty()
                        && right.middle.is_empty()
                        && left.outer_b.is_empty()
                        && left.inner_b.is_empty()
                        && right.outer_f.is_empty()
                        && right.inner_f.is_empty()
                    {
                        left.inner_b = right.inner_b;
                        left.outer_b = right.outer_b;
                        left.length = total_length;
                        return;
                    }
                    // If left and right are trees with empty middles and left's buffers
                    // can fit right's buffers: push right's elements onto left
                    if left.middle.is_empty()
                        && right.middle.is_empty()
                        && total_length <= CHUNK_SIZE * 4
                    {
                        while let Some(value) = right.pop_front() {
                            left.push_back(value);
                        }
                        return;
                    }
                    // Both are full and big: do the full RRB join
                    let inner_b1 = left.inner_b.clone();
                    left.push_middle(Side::Right, inner_b1);
                    let outer_b1 = left.outer_b.clone();
                    left.push_middle(Side::Right, outer_b1);
                    let inner_f2 = right.inner_f.clone();
                    right.push_middle(Side::Left, inner_f2);
                    let outer_f2 = right.outer_f.clone();
                    right.push_middle(Side::Left, outer_f2);

                    let mut middle1 =
                        clone_ref(replace(&mut left.middle, SharedPointer::new(Node::new())));
                    let mut middle2 = clone_ref(right.middle);
                    let normalised_middle = match left.middle_level.cmp(&right.middle_level) {
                        Ordering::Greater => {
                            middle2 = middle2.elevate(left.middle_level - right.middle_level);
                            left.middle_level
                        }
                        Ordering::Less => {
                            middle1 = middle1.elevate(right.middle_level - left.middle_level);
                            right.middle_level
                        }
                        Ordering::Equal => left.middle_level,
                    };
                    let (merged, merged_level) = Node::merge(middle1, middle2, normalised_middle);
                    left.middle = SharedPointer::new(merged);
                    left.middle_level = merged_level;

                    left.inner_b = right.inner_b;
                    left.outer_b = right.outer_b;
                    left.length = total_length;
                    left.prune();
                    return;
                }
            }
        }
        // No optimisations available, and either left, right or both are
        // single: promote both to full and retry
        self.promote_front();
        other.promote_back();
        self.append(other)
    }

    /// Retain only the elements specified by the predicate.
    ///
    /// Remove all elements for which the provided function `f`
    /// returns false from the vector.
    ///
    /// Time: O(n)
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&A) -> bool,
    {
        let len = self.len();
        let mut del = 0;
        {
            let mut focus = self.focus_mut();
            for i in 0..len {
                if !f(focus.index(i)) {
                    del += 1;
                } else if del > 0 {
                    focus.swap(i - del, i);
                }
            }
        }
        if del > 0 {
            let _ = self.split_off(len - del);
        }
    }

    /// Split a vector at a given index.
    ///
    /// Split a vector at a given index, consuming the vector and
    /// returning a pair of the left hand side and the right hand side
    /// of the split.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3, 7, 8, 9];
    /// let (left, right) = vec.split_at(3);
    /// assert_eq!(vector![1, 2, 3], left);
    /// assert_eq!(vector![7, 8, 9], right);
    /// ```
    pub fn split_at(mut self, index: usize) -> (Self, Self) {
        let right = self.split_off(index);
        (self, right)
    }

    /// Split a vector at a given index.
    ///
    /// Split a vector at a given index, leaving the left hand side in
    /// the current vector and returning a new vector containing the
    /// right hand side.
    ///
    /// Time: O(log n)
    ///
    /// Compare: [`Vec::split_off`] is O(n). The RRB tree structure
    /// allows splitting by rearranging subtree pointers.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut left = vector![1, 2, 3, 7, 8, 9];
    /// let right = left.split_off(3);
    /// assert_eq!(vector![1, 2, 3], left);
    /// assert_eq!(vector![7, 8, 9], right);
    /// ```
    #[must_use]
    pub fn split_off(&mut self, index: usize) -> Self {
        assert!(index <= self.len());
        self.merkle_valid = false;

        match &mut self.vector {
            Inline(chunk) => Self::from_inner(Inline(chunk.split_off(index))),
            Single(chunk) => Self::from_inner(Single(SharedPointer::new(
                SharedPointer::make_mut(chunk).split_off(index),
            ))),
            Full(tree) => {
                let mut local_index = index;

                if local_index < tree.outer_f.len() {
                    let of2 = SharedPointer::make_mut(&mut tree.outer_f).split_off(local_index);
                    let right = RRB {
                        length: tree.length - index,
                        middle_level: tree.middle_level,
                        outer_f: SharedPointer::new(of2),
                        inner_f: replace_shared_pointer(&mut tree.inner_f),
                        middle: core::mem::take(&mut tree.middle),
                        inner_b: replace_shared_pointer(&mut tree.inner_b),
                        outer_b: replace_shared_pointer(&mut tree.outer_b),
                    };
                    tree.length = index;
                    tree.middle_level = 0;
                    return Self::from_inner(Full(right));
                }

                local_index -= tree.outer_f.len();

                if local_index < tree.inner_f.len() {
                    let if2 = SharedPointer::make_mut(&mut tree.inner_f).split_off(local_index);
                    let right = RRB {
                        length: tree.length - index,
                        middle_level: tree.middle_level,
                        outer_f: SharedPointer::new(if2),
                        inner_f: SharedPointer::default(),
                        middle: core::mem::take(&mut tree.middle),
                        inner_b: replace_shared_pointer(&mut tree.inner_b),
                        outer_b: replace_shared_pointer(&mut tree.outer_b),
                    };
                    tree.length = index;
                    tree.middle_level = 0;
                    swap(&mut tree.outer_b, &mut tree.inner_f);
                    return Self::from_inner(Full(right));
                }

                local_index -= tree.inner_f.len();

                if local_index < tree.middle.len() {
                    let mut right_middle = tree.middle.clone();
                    let (c1, c2) = {
                        let m1 = SharedPointer::make_mut(&mut tree.middle);
                        let m2 = SharedPointer::make_mut(&mut right_middle);
                        match m1.split(tree.middle_level, Side::Right, local_index) {
                            SplitResult::Dropped(_) => (),
                            SplitResult::OutOfBounds => unreachable!(),
                        };
                        match m2.split(tree.middle_level, Side::Left, local_index) {
                            SplitResult::Dropped(_) => (),
                            SplitResult::OutOfBounds => unreachable!(),
                        };
                        let c1 = match m1.pop_chunk(tree.middle_level, Side::Right) {
                            PopResult::Empty => SharedPointer::default(),
                            PopResult::Done(chunk) => chunk,
                            PopResult::Drained(chunk) => {
                                m1.clear_node();
                                chunk
                            }
                        };
                        let c2 = match m2.pop_chunk(tree.middle_level, Side::Left) {
                            PopResult::Empty => SharedPointer::default(),
                            PopResult::Done(chunk) => chunk,
                            PopResult::Drained(chunk) => {
                                m2.clear_node();
                                chunk
                            }
                        };
                        (c1, c2)
                    };
                    let mut right = RRB {
                        length: tree.length - index,
                        middle_level: tree.middle_level,
                        outer_f: c2,
                        inner_f: SharedPointer::default(),
                        middle: right_middle,
                        inner_b: replace_shared_pointer(&mut tree.inner_b),
                        outer_b: replace(&mut tree.outer_b, c1),
                    };
                    tree.length = index;
                    tree.prune();
                    right.prune();
                    return Self::from_inner(Full(right));
                }

                local_index -= tree.middle.len();

                if local_index < tree.inner_b.len() {
                    let ib2 = SharedPointer::make_mut(&mut tree.inner_b).split_off(local_index);
                    let right = RRB {
                        length: tree.length - index,
                        outer_b: replace_shared_pointer(&mut tree.outer_b),
                        outer_f: SharedPointer::new(ib2),
                        ..RRB::new()
                    };
                    tree.length = index;
                    swap(&mut tree.outer_b, &mut tree.inner_b);
                    return Self::from_inner(Full(right));
                }

                local_index -= tree.inner_b.len();

                let ob2 = SharedPointer::make_mut(&mut tree.outer_b).split_off(local_index);
                tree.length = index;
                Self::from_inner(Single(SharedPointer::new(ob2)))
            }
        }
    }

    /// Construct a vector with `count` elements removed from the
    /// start of the current vector.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn skip(&self, count: usize) -> Self {
        match count {
            0 => self.clone(),
            count if count >= self.len() => Self::new(),
            count => {
                // FIXME can be made more efficient by dropping the unwanted side without constructing it
                self.clone().split_off(count)
            }
        }
    }

    /// Construct a vector of the first `count` elements from the
    /// current vector.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn take(&self, count: usize) -> Self {
        // FIXME can be made more efficient by dropping the unwanted side without constructing it
        let mut left = self.clone();
        let _ = left.split_off(count);
        left
    }

    /// Truncate a vector to the given size.
    ///
    /// Discards all elements in the vector beyond the given length.
    /// Does nothing if `len` is greater or equal to the length of the vector.
    ///
    /// Time: O(log n)
    pub fn truncate(&mut self, len: usize) {
        if len < self.len() {
            // FIXME can be made more efficient by dropping the unwanted side without constructing it
            let _ = self.split_off(len);
        }
    }

    /// Extract a slice from a vector.
    ///
    /// Remove the elements from `start_index` until `end_index` in
    /// the current vector and return the removed slice as a new
    /// vector.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn slice<R>(&mut self, range: R) -> Self
    where
        R: RangeBounds<usize>,
    {
        let r = to_range(&range, self.len());
        if r.start >= r.end || r.start >= self.len() {
            return GenericVector::new();
        }
        let mut middle = self.split_off(r.start);
        let right = middle.split_off(r.end - r.start);
        self.append(right);
        middle
    }

    /// Insert an element into a vector.
    ///
    /// Insert an element at position `index`, shifting all elements
    /// after it to the right.
    ///
    /// ## Performance Note
    ///
    /// While `push_front` and `push_back` are heavily optimised
    /// operations, `insert` in the middle of a vector requires a
    /// split, a push, and an append. Thus, if you want to insert
    /// many elements at the same location, instead of `insert`ing
    /// them one by one, you should rather create a new vector
    /// containing the elements to insert, split the vector at the
    /// insertion point, and append the left hand, the new vector and
    /// the right hand in order.
    ///
    /// Time: O(log n)
    ///
    /// Compare: [`Vec::insert`] is O(n) because it must shift all
    /// subsequent elements. Here, only O(log n) tree nodes are affected.
    pub fn insert(&mut self, index: usize, value: A) {
        self.merkle_valid = false;
        if index == 0 {
            return self.push_front(value);
        }
        if index == self.len() {
            return self.push_back(value);
        }
        assert!(index < self.len());
        if matches!(&self.vector, Inline(_)) && self.needs_promotion() {
            self.promote_inline();
        }
        match &mut self.vector {
            Inline(chunk) => {
                chunk.insert(index, value);
            }
            Single(chunk) if chunk.len() < CHUNK_SIZE => {
                SharedPointer::make_mut(chunk).insert(index, value)
            }
            // TODO a lot of optimisations still possible here
            _ => {
                let right = self.split_off(index);
                self.push_back(value);
                self.append(right);
            }
        }
    }

    /// Remove an element from a vector.
    ///
    /// Remove the element from position 'index', shifting all
    /// elements after it to the left, and return the removed element.
    ///
    /// ## Performance Note
    ///
    /// While `pop_front` and `pop_back` are heavily optimised
    /// operations, `remove` in the middle of a vector requires a
    /// split, a pop, and an append. Thus, if you want to remove many
    /// elements from the same location, instead of `remove`ing them
    /// one by one, it is much better to use [`slice`][slice].
    ///
    /// Time: O(log n)
    ///
    /// [slice]: #method.slice
    pub fn remove(&mut self, index: usize) -> A {
        assert!(index < self.len());
        self.merkle_valid = false;
        match &mut self.vector {
            Inline(chunk) => chunk.remove(index).unwrap(),
            Single(chunk) => SharedPointer::make_mut(chunk).remove(index),
            _ => {
                if index == 0 {
                    return self.pop_front().unwrap();
                }
                if index == self.len() - 1 {
                    return self.pop_back().unwrap();
                }
                // TODO a lot of optimisations still possible here
                let mut right = self.split_off(index);
                let value = right.pop_front().unwrap();
                self.append(right);
                value
            }
        }
    }

    /// Insert an element into a sorted vector.
    ///
    /// Insert an element into a vector in sorted order, assuming the vector is
    /// already in sorted order.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![1, 2, 3, 7, 8, 9];
    /// vec.insert_ord(5);
    /// assert_eq!(vector![1, 2, 3, 5, 7, 8, 9], vec);
    /// ```
    pub fn insert_ord(&mut self, item: A)
    where
        A: Ord,
    {
        match self.binary_search(&item) {
            Ok(index) => self.insert(index, item),
            Err(index) => self.insert(index, item),
        }
    }

    /// Insert an element into a sorted vector using a comparator function.
    ///
    /// Insert an element into a vector in sorted order using the given
    /// comparator function, assuming the vector is already in sorted order.
    ///
    /// Note that the ordering used to sort the vector must logically match
    /// the ordering in the comparison function provided to `insert_ord_by`.
    /// Incompatible definitions of the ordering won't result in memory
    /// unsafety, but will likely result in out-of-order insertions.
    ///
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![9, 8, 7, 3, 2, 1];
    /// vec.insert_ord_by(5, |a, b| a.cmp(b).reverse());
    /// assert_eq!(vector![9, 8, 7, 5, 3, 2, 1], vec);
    ///
    /// // Note that `insert_ord` does not work in this case because it uses
    /// // the default comparison function for the item type.
    /// vec.insert_ord(4);
    /// assert_eq!(vector![4, 9, 8, 7, 5, 3, 2, 1], vec);
    /// ```
    pub fn insert_ord_by<F>(&mut self, item: A, mut f: F)
    where
        F: FnMut(&A, &A) -> Ordering,
    {
        match self.binary_search_by(|scan_item| f(scan_item, &item)) {
            Ok(idx) | Err(idx) => self.insert(idx, item),
        }
    }

    /// Insert an element into a sorted vector where the comparison function
    /// delegates to the Ord implementation for values calculated by a user-
    /// provided function defined on the item type.
    ///
    /// This function assumes the vector is already sorted. If it isn't sorted,
    /// this function may insert the provided value out of order.
    ///
    /// Note that the ordering of the sorted vector must logically match the
    /// `PartialOrd` implementation of the type returned by the passed comparator
    /// function `f`. Incompatible definitions of the ordering won't result in
    /// memory unsafety, but will likely result in out-of-order insertions.
    ///
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::Vector;
    ///
    /// type A = (u8, &'static str);
    ///
    /// let mut vec: Vector<A> = vector![(3, "a"), (1, "c"), (0, "d")];
    ///
    /// // For the sake of this example, let's say that only the second element
    /// // of the A tuple is important in the context of comparison.
    /// vec.insert_ord_by_key((0, "b"), |a| a.1);
    /// assert_eq!(vector![(3, "a"), (0, "b"), (1, "c"), (0, "d")], vec);
    ///
    /// // Note that `insert_ord` does not work in this case because it uses
    /// // the default comparison function for the item type.
    /// vec.insert_ord((0, "e"));
    /// assert_eq!(vector![(3, "a"), (0, "b"), (0, "e"), (1, "c"), (0, "d")], vec);
    /// ```
    pub fn insert_ord_by_key<B, F>(&mut self, item: A, mut f: F)
    where
        B: Ord,
        F: FnMut(&A) -> B,
    {
        match self.binary_search_by_key(&f(&item), |scan_item| f(scan_item)) {
            Ok(idx) | Err(idx) => self.insert(idx, item),
        }
    }

    /// Sort a vector.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![3, 2, 5, 4, 1];
    /// vec.sort();
    /// assert_eq!(vector![1, 2, 3, 4, 5], vec);
    /// ```
    pub fn sort(&mut self)
    where
        A: Ord,
    {
        self.sort_by(Ord::cmp)
    }

    /// Sort a vector using a comparator function.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![3, 2, 5, 4, 1];
    /// vec.sort_by(|left, right| left.cmp(right));
    /// assert_eq!(vector![1, 2, 3, 4, 5], vec);
    /// ```
    pub fn sort_by<F>(&mut self, cmp: F)
    where
        F: Fn(&A, &A) -> Ordering,
    {
        let len = self.len();
        if len > 1 {
            sort::quicksort(self.focus_mut(), &cmp);
        }
    }

    /// Sort a vector using the default comparator, in parallel.
    ///
    /// This is the parallel equivalent of [`sort`][sort]. It collects
    /// elements into a contiguous buffer, sorts in parallel using rayon,
    /// and rebuilds the vector.
    ///
    /// Requires the `rayon` feature flag.
    ///
    /// Time: O(n log n / p) where p is the number of available threads,
    /// plus O(n) for collection and reconstruction.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![3, 2, 5, 4, 1];
    /// vec.par_sort();
    /// assert_eq!(vector![1, 2, 3, 4, 5], vec);
    /// ```
    ///
    /// [sort]: #method.sort
    #[cfg(any(test, feature = "rayon"))]
    pub fn par_sort(&mut self)
    where
        A: Ord + Send,
    {
        self.par_sort_by(Ord::cmp)
    }

    /// Sort a vector using a comparator function, in parallel.
    ///
    /// This is the parallel equivalent of [`sort_by`][sort_by]. It
    /// collects elements into a contiguous buffer, sorts in parallel
    /// using rayon, and rebuilds the vector.
    ///
    /// Requires the `rayon` feature flag.
    ///
    /// Time: O(n log n / p) where p is the number of available threads,
    /// plus O(n) for collection and reconstruction.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// let mut vec = vector![3, 2, 5, 4, 1];
    /// vec.par_sort_by(|left, right| left.cmp(right));
    /// assert_eq!(vector![1, 2, 3, 4, 5], vec);
    /// ```
    ///
    /// [sort_by]: #method.sort_by
    #[cfg(any(test, feature = "rayon"))]
    pub fn par_sort_by<F>(&mut self, cmp: F)
    where
        A: Send,
        F: Fn(&A, &A) -> Ordering + Sync,
    {
        use ::rayon::slice::ParallelSliceMut;
        if self.len() <= 1 {
            return;
        }
        self.merkle_valid = false;
        let mut vec: Vec<A> = core::mem::take(self).into_iter().collect();
        vec.par_sort_unstable_by(|a, b| cmp(a, b));
        *self = vec.into_iter().collect();
    }
}

// Implementation details

impl<A, P: SharedPointerKind> RRB<A, P> {
    fn new() -> Self {
        RRB {
            length: 0,
            middle_level: 0,
            outer_f: SharedPointer::default(),
            inner_f: SharedPointer::default(),
            middle: SharedPointer::new(Node::new()),
            inner_b: SharedPointer::default(),
            outer_b: SharedPointer::default(),
        }
    }

    #[cfg(any(test, feature = "debug"))]
    fn assert_invariants(&self) {
        let ml = self.middle.assert_invariants(self.middle_level);
        assert_eq!(
            self.length,
            self.outer_f.len() + self.inner_f.len() + ml + self.inner_b.len() + self.outer_b.len()
        );
    }
}

impl<A: Clone, P: SharedPointerKind> RRB<A, P> {
    fn prune(&mut self) {
        if self.middle.is_empty() {
            self.middle = SharedPointer::new(Node::new());
            self.middle_level = 0;
        } else {
            while self.middle_level > 0 && self.middle.is_single() {
                // FIXME could be optimised, cloning the node is expensive
                self.middle = SharedPointer::new(self.middle.first_child().clone());
                self.middle_level -= 1;
            }
        }
    }

    fn pop_front(&mut self) -> Option<A> {
        if self.length == 0 {
            return None;
        }
        if self.outer_f.is_empty() {
            if self.inner_f.is_empty() {
                if self.middle.is_empty() {
                    if self.inner_b.is_empty() {
                        swap(&mut self.outer_f, &mut self.outer_b);
                    } else {
                        swap(&mut self.outer_f, &mut self.inner_b);
                    }
                } else {
                    self.outer_f = self.pop_middle(Side::Left).unwrap();
                }
            } else {
                swap(&mut self.outer_f, &mut self.inner_f);
            }
        }
        self.length -= 1;
        let outer_f = SharedPointer::make_mut(&mut self.outer_f);
        Some(outer_f.pop_front())
    }

    fn pop_back(&mut self) -> Option<A> {
        if self.length == 0 {
            return None;
        }
        if self.outer_b.is_empty() {
            if self.inner_b.is_empty() {
                if self.middle.is_empty() {
                    if self.inner_f.is_empty() {
                        swap(&mut self.outer_b, &mut self.outer_f);
                    } else {
                        swap(&mut self.outer_b, &mut self.inner_f);
                    }
                } else {
                    self.outer_b = self.pop_middle(Side::Right).unwrap();
                }
            } else {
                swap(&mut self.outer_b, &mut self.inner_b);
            }
        }
        self.length -= 1;
        let outer_b = SharedPointer::make_mut(&mut self.outer_b);
        Some(outer_b.pop_back())
    }

    fn push_front(&mut self, value: A) {
        if self.outer_f.is_full() {
            swap(&mut self.outer_f, &mut self.inner_f);
            if !self.outer_f.is_empty() {
                let mut chunk = SharedPointer::new(Chunk::new());
                swap(&mut chunk, &mut self.outer_f);
                self.push_middle(Side::Left, chunk);
            }
        }
        self.length = self.length.checked_add(1).expect("Vector length overflow");
        let outer_f = SharedPointer::make_mut(&mut self.outer_f);
        outer_f.push_front(value)
    }

    fn push_back(&mut self, value: A) {
        if self.outer_b.is_full() {
            swap(&mut self.outer_b, &mut self.inner_b);
            if !self.outer_b.is_empty() {
                let mut chunk = SharedPointer::new(Chunk::new());
                swap(&mut chunk, &mut self.outer_b);
                self.push_middle(Side::Right, chunk);
            }
        }
        self.length = self.length.checked_add(1).expect("Vector length overflow");
        let outer_b = SharedPointer::make_mut(&mut self.outer_b);
        outer_b.push_back(value)
    }

    fn push_middle(&mut self, side: Side, chunk: SharedPointer<Chunk<A>, P>) {
        if chunk.is_empty() {
            return;
        }
        let new_middle = {
            let middle = SharedPointer::make_mut(&mut self.middle);
            match middle.push_chunk(self.middle_level, side, chunk) {
                PushResult::Done => return,
                PushResult::Full(chunk, _num_drained) => SharedPointer::new({
                    match side {
                        Side::Left => Node::from_chunk(self.middle_level, chunk)
                            .join_branches(middle.clone(), self.middle_level),
                        Side::Right => middle.clone().join_branches(
                            Node::from_chunk(self.middle_level, chunk),
                            self.middle_level,
                        ),
                    }
                }),
            }
        };
        self.middle_level += 1;
        self.middle = new_middle;
    }

    fn pop_middle(&mut self, side: Side) -> Option<SharedPointer<Chunk<A>, P>> {
        let chunk = {
            let middle = SharedPointer::make_mut(&mut self.middle);
            match middle.pop_chunk(self.middle_level, side) {
                PopResult::Empty => return None,
                PopResult::Done(chunk) => chunk,
                PopResult::Drained(chunk) => {
                    middle.clear_node();
                    self.middle_level = 0;
                    chunk
                }
            }
        };
        Some(chunk)
    }
}

#[inline]
fn replace_shared_pointer<A: Default, P: SharedPointerKind>(
    dest: &mut SharedPointer<A, P>,
) -> SharedPointer<A, P> {
    core::mem::take(dest)
}

// Core traits

impl<A, P: SharedPointerKind> Default for GenericVector<A, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Clone, P: SharedPointerKind> Clone for GenericVector<A, P> {
    /// Clone a vector.
    ///
    /// Time: O(1), or O(n) with a very small, bounded *n* for an inline vector.
    ///
    /// Compare: `Vec::clone` and `VecDeque::clone` are always O(n).
    /// Structural sharing means both the original and the clone share
    /// the same tree nodes in memory until one is modified.
    fn clone(&self) -> Self {
        Self {
            vector: match &self.vector {
                Inline(chunk) => Inline(chunk.clone()),
                Single(chunk) => Single(chunk.clone()),
                Full(tree) => Full(tree.clone()),
            },
            merkle_hash: self.merkle_hash,
            merkle_valid: self.merkle_valid,
        }
    }
}

impl<A, P: SharedPointerKind> GenericVector<A, P> {
    /// Whether the cached Merkle hash is current.
    ///
    /// Returns `false` after any mutation. Call
    /// [`recompute_merkle`][GenericVector::recompute_merkle] to restore validity.
    #[inline]
    #[must_use]
    pub fn merkle_valid(&self) -> bool {
        self.merkle_valid
    }
}

impl<A: Clone + Hash, P: SharedPointerKind> GenericVector<A, P> {
    /// Recompute the Merkle hash from the tree structure.
    ///
    /// The hash is position-sensitive: `[a, b]` and `[b, a]` produce
    /// different hashes. For the `Full` representation this combines
    /// the hashes of all five RRB segments (outer_f, inner_f, middle,
    /// inner_b, outer_b) using the same prime-multiply scheme as the
    /// per-node hashes. The middle tree's hash is computed lazily —
    /// only modified subtrees are recomputed.
    ///
    /// Time: O(k log n) where k is the number of modified nodes since
    /// the last computation. O(1) when no nodes have changed.
    pub fn recompute_merkle(&mut self) {
        if self.merkle_valid {
            return;
        }
        self.merkle_hash = match &self.vector {
            Inline(chunk) => {
                let mut h: u64 = 0;
                for elem in chunk.iter() {
                    h = h
                        .wrapping_mul(MERKLE_PRIME)
                        .wrapping_add(hash_element(elem));
                }
                h
            }
            Single(chunk) => chunk_merkle_hash(chunk),
            Full(tree) => {
                let mut h: u64 = 0;
                // Combine the five RRB segments in order.
                h = h
                    .wrapping_mul(MERKLE_PRIME)
                    .wrapping_add(chunk_merkle_hash(&tree.outer_f));
                h = h
                    .wrapping_mul(MERKLE_PRIME)
                    .wrapping_add(chunk_merkle_hash(&tree.inner_f));
                h = h
                    .wrapping_mul(MERKLE_PRIME)
                    .wrapping_add(tree.middle.merkle_hash());
                h = h
                    .wrapping_mul(MERKLE_PRIME)
                    .wrapping_add(chunk_merkle_hash(&tree.inner_b));
                h = h
                    .wrapping_mul(MERKLE_PRIME)
                    .wrapping_add(chunk_merkle_hash(&tree.outer_b));
                h
            }
        };
        self.merkle_valid = true;
    }

    /// Get the Merkle hash, recomputing if necessary.
    ///
    /// Equivalent to calling [`recompute_merkle`][Self::recompute_merkle]
    /// then reading the cached value, but expressed as a single call.
    ///
    /// Time: O(k log n) amortised — see
    /// [`recompute_merkle`][Self::recompute_merkle].
    #[inline]
    pub fn merkle_hash(&mut self) -> u64 {
        self.recompute_merkle();
        self.merkle_hash
    }
}

impl<A: Debug, P: SharedPointerKind> Debug for GenericVector<A, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_list().entries(self.iter()).finish()
        // match self {
        //     Full(rrb) => {
        //         writeln!(f, "Head: {:?} {:?}", rrb.outer_f, rrb.inner_f)?;
        //         rrb.middle.print(f, 0, rrb.middle_level)?;
        //         writeln!(f, "Tail: {:?} {:?}", rrb.inner_b, rrb.outer_b)
        //     }
        //     Single(_) => write!(f, "nowt"),
        // }
    }
}

impl<A: PartialEq, P: SharedPointerKind> PartialEq for GenericVector<A, P> {
    /// Compare two vectors for equality.
    ///
    /// Time: **O(1)** when both vectors have valid Merkle hashes and
    /// they match (positive equality via [`recompute_merkle`][Self::recompute_merkle]).
    /// O(1) when they are pointer-equal (clones that haven't diverged).
    /// O(n) otherwise (element-by-element comparison).
    ///
    /// Compare: `Vec` and `VecDeque` equality is always O(n).
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        if self.len() != other.len() {
            return false;
        }
        // Positive Merkle check: if both hashes are valid and equal,
        // the vectors are equal without element-wise comparison.
        // Only safe when Merkle hash width ≥ 64 bits (DEC-023).
        if MERKLE_HASH_BITS >= MERKLE_POSITIVE_EQ_MIN_BITS
            && self.merkle_valid
            && other.merkle_valid
            && self.merkle_hash == other.merkle_hash
        {
            return true;
        }
        self.iter().eq(other.iter())
    }
}

impl<A: Eq, P: SharedPointerKind> Eq for GenericVector<A, P> {}

impl<A: PartialOrd, P: SharedPointerKind> PartialOrd for GenericVector<A, P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

impl<A: Ord, P: SharedPointerKind> Ord for GenericVector<A, P> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<A: Hash, P: SharedPointerKind> Hash for GenericVector<A, P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for i in self {
            i.hash(state)
        }
    }
}

impl<A: Clone, P: SharedPointerKind> Sum for GenericVector<A, P> {
    fn sum<I>(it: I) -> Self
    where
        I: Iterator<Item = Self>,
    {
        it.fold(Self::new(), |a, b| a + b)
    }
}

impl<A: Clone, P: SharedPointerKind> Add for GenericVector<A, P> {
    type Output = GenericVector<A, P>;

    /// Concatenate two vectors.
    ///
    /// Time: O(log n)
    fn add(mut self, other: Self) -> Self::Output {
        self.append(other);
        self
    }
}

impl<A: Clone, P: SharedPointerKind> Add for &GenericVector<A, P> {
    type Output = GenericVector<A, P>;

    /// Concatenate two vectors.
    ///
    /// Time: O(log n)
    fn add(self, other: Self) -> Self::Output {
        let mut out = self.clone();
        out.append(other.clone());
        out
    }
}

impl<A: Clone, P: SharedPointerKind> Extend<A> for GenericVector<A, P> {
    /// Add values to the end of a vector by consuming an iterator.
    ///
    /// Time: O(n)
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = A>,
    {
        for item in iter {
            self.push_back(item)
        }
    }
}

impl<A, P: SharedPointerKind> Index<usize> for GenericVector<A, P> {
    type Output = A;
    /// Get a reference to the value at index `index` in the vector.
    ///
    /// Time: O(log n)
    fn index(&self, index: usize) -> &Self::Output {
        match self.get(index) {
            Some(value) => value,
            None => panic!(
                "Vector::index: index out of bounds: {} < {}",
                index,
                self.len()
            ),
        }
    }
}

impl<A: Clone, P: SharedPointerKind> IndexMut<usize> for GenericVector<A, P> {
    /// Get a mutable reference to the value at index `index` in the
    /// vector.
    ///
    /// Time: O(log n)
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.merkle_valid = false;
        match self.get_mut(index) {
            Some(value) => value,
            None => panic!("Vector::index_mut: index out of bounds"),
        }
    }
}

// Conversions

impl<'a, A, P: SharedPointerKind> IntoIterator for &'a GenericVector<A, P> {
    type Item = &'a A;
    type IntoIter = Iter<'a, A, P>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, A: Clone, P: SharedPointerKind> IntoIterator for &'a mut GenericVector<A, P> {
    type Item = &'a mut A;
    type IntoIter = IterMut<'a, A, P>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<A: Clone, P: SharedPointerKind> IntoIterator for GenericVector<A, P> {
    type Item = A;
    type IntoIter = ConsumingIter<A, P>;
    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter::new(self)
    }
}

impl<A: Clone, P: SharedPointerKind> FromIterator<A> for GenericVector<A, P> {
    /// Create a vector from an iterator.
    ///
    /// Time: O(n)
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = A>,
    {
        let mut seq = Self::new();
        for item in iter {
            seq.push_back(item)
        }
        seq
    }
}

impl<A, OA, P1, P2> From<&GenericVector<&A, P2>> for GenericVector<OA, P1>
where
    A: ToOwned<Owned = OA>,
    OA: Borrow<A> + Clone,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(vec: &GenericVector<&A, P2>) -> Self {
        vec.iter().map(|a| (*a).to_owned()).collect()
    }
}

impl<A, const N: usize, P: SharedPointerKind> From<[A; N]> for GenericVector<A, P>
where
    A: Clone,
{
    fn from(arr: [A; N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<A: Clone, P: SharedPointerKind> From<&[A]> for GenericVector<A, P> {
    fn from(slice: &[A]) -> Self {
        slice.iter().cloned().collect()
    }
}

impl<A: Clone, P: SharedPointerKind> From<Vec<A>> for GenericVector<A, P> {
    /// Create a vector from a [`std::vec::Vec`][vec].
    ///
    /// Time: O(n)
    ///
    /// [vec]: https://doc.rust-lang.org/std/vec/struct.Vec.html
    fn from(vec: Vec<A>) -> Self {
        vec.into_iter().collect()
    }
}

impl<A: Clone, P: SharedPointerKind> From<&Vec<A>> for GenericVector<A, P> {
    /// Create a vector from a [`std::vec::Vec`][vec].
    ///
    /// Time: O(n)
    ///
    /// [vec]: https://doc.rust-lang.org/std/vec/struct.Vec.html
    fn from(vec: &Vec<A>) -> Self {
        vec.iter().cloned().collect()
    }
}

// Iterators

/// An iterator over vectors with values of type `A`.
///
/// To obtain one, use [`Vector::iter()`][iter].
///
/// [iter]: type.Vector.html#method.iter
// TODO: we'd like to support Clone even if A is not Clone, but it isn't trivial because
// the TreeFocus variant of Focus does need A to be Clone.
pub struct Iter<'a, A, P: SharedPointerKind> {
    focus: Focus<'a, A, P>,
    front_index: usize,
    back_index: usize,
}

impl<'a, A, P: SharedPointerKind> Iter<'a, A, P> {
    fn new(seq: &'a GenericVector<A, P>) -> Self {
        Iter {
            focus: seq.focus(),
            front_index: 0,
            back_index: seq.len(),
        }
    }

    fn from_focus(focus: Focus<'a, A, P>) -> Self {
        Iter {
            front_index: 0,
            back_index: focus.len(),
            focus,
        }
    }
}

impl<A: Clone, P: SharedPointerKind> Clone for Iter<'_, A, P> {
    fn clone(&self) -> Self {
        Iter {
            focus: self.focus.clone(),
            front_index: self.front_index,
            back_index: self.back_index,
        }
    }
}

impl<'a, A, P: SharedPointerKind + 'a> Iterator for Iter<'a, A, P> {
    type Item = &'a A;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        // SAFETY: Lifetime extension from &mut self to 'a. The Focus holds
        // a reference to tree data with lifetime 'a. get() returns &'a A
        // pointing into that tree, not into the Focus's cache. Each call
        // accesses a distinct index so returned references don't alias.
        // The borrow checker can't prove this because &mut self is shorter
        // than 'a, but the Focus lives for 'a inside the Iter struct.
        let focus: &'a mut Focus<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        let value = focus.get(self.front_index);
        self.front_index += 1;
        value
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.back_index - self.front_index;
        (remaining, Some(remaining))
    }
}

impl<'a, A, P: SharedPointerKind + 'a> DoubleEndedIterator for Iter<'a, A, P> {
    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        self.back_index -= 1;
        // SAFETY: Same as Iter::next — lifetime extension for lending
        // iterator pattern. See SAFETY comment in next() above.
        let focus: &'a mut Focus<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        focus.get(self.back_index)
    }
}

impl<'a, A, P: SharedPointerKind + 'a> ExactSizeIterator for Iter<'a, A, P> {}

impl<'a, A, P: SharedPointerKind + 'a> FusedIterator for Iter<'a, A, P> {}

/// A mutable iterator over vectors with values of type `A`.
///
/// To obtain one, use [`Vector::iter_mut()`][iter_mut].
///
/// [iter_mut]: type.Vector.html#method.iter_mut
pub struct IterMut<'a, A, P: SharedPointerKind> {
    focus: FocusMut<'a, A, P>,
    front_index: usize,
    back_index: usize,
}

impl<'a, A, P: SharedPointerKind> IterMut<'a, A, P> {
    fn from_focus(focus: FocusMut<'a, A, P>) -> Self {
        IterMut {
            front_index: 0,
            back_index: focus.len(),
            focus,
        }
    }
}

impl<'a, A: Clone, P: SharedPointerKind> IterMut<'a, A, P> {
    fn new(seq: &'a mut GenericVector<A, P>) -> Self {
        let focus = seq.focus_mut();
        let len = focus.len();
        IterMut {
            focus,
            front_index: 0,
            back_index: len,
        }
    }
}

impl<'a, A, P: SharedPointerKind> Iterator for IterMut<'a, A, P>
where
    A: 'a + Clone,
{
    type Item = &'a mut A;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        // SAFETY: Same lending iterator pattern as Iter::next. FocusMut
        // uses copy-on-write (make_mut) so each get_mut() returns a &'a mut A
        // into a now-unique tree node. Distinct indices yield non-aliasing
        // mutable references.
        let focus: &'a mut FocusMut<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        let value = focus.get_mut(self.front_index);
        self.front_index += 1;
        value
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.back_index - self.front_index;
        (remaining, Some(remaining))
    }
}

impl<'a, A, P: SharedPointerKind> DoubleEndedIterator for IterMut<'a, A, P>
where
    A: 'a + Clone,
{
    /// Remove and return an element from the back of the iterator.
    ///
    /// Time: O(1)*
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        self.back_index -= 1;
        // SAFETY: Same as IterMut::next — see SAFETY comment above.
        let focus: &'a mut FocusMut<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        focus.get_mut(self.back_index)
    }
}

impl<'a, A: Clone, P: SharedPointerKind> ExactSizeIterator for IterMut<'a, A, P> {}

impl<'a, A: Clone, P: SharedPointerKind> FusedIterator for IterMut<'a, A, P> {}

/// A consuming iterator over vectors with values of type `A`.
pub struct ConsumingIter<A, P: SharedPointerKind> {
    vector: GenericVector<A, P>,
}

impl<A, P: SharedPointerKind> ConsumingIter<A, P> {
    fn new(vector: GenericVector<A, P>) -> Self {
        Self { vector }
    }
}

impl<A: Clone, P: SharedPointerKind> Iterator for ConsumingIter<A, P> {
    type Item = A;

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        self.vector.pop_front()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.vector.len();
        (len, Some(len))
    }
}

impl<A: Clone, P: SharedPointerKind> DoubleEndedIterator for ConsumingIter<A, P> {
    /// Remove and return an element from the back of the iterator.
    ///
    /// Time: O(1)*
    fn next_back(&mut self) -> Option<Self::Item> {
        self.vector.pop_back()
    }
}

impl<A: Clone, P: SharedPointerKind> ExactSizeIterator for ConsumingIter<A, P> {}

impl<A: Clone, P: SharedPointerKind> FusedIterator for ConsumingIter<A, P> {}

/// An iterator over the leaf nodes of a vector.
///
/// To obtain one, use [`Vector::chunks()`][chunks].
///
/// [chunks]: type.Vector.html#method.chunks
pub struct Chunks<'a, A, P: SharedPointerKind> {
    focus: Focus<'a, A, P>,
    front_index: usize,
    back_index: usize,
}

impl<'a, A, P: SharedPointerKind> Chunks<'a, A, P> {
    fn new(seq: &'a GenericVector<A, P>) -> Self {
        Chunks {
            focus: seq.focus(),
            front_index: 0,
            back_index: seq.len(),
        }
    }
}

impl<'a, A, P: SharedPointerKind + 'a> Iterator for Chunks<'a, A, P> {
    type Item = &'a [A];

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        // SAFETY: Same lending iterator pattern as Iter::next —
        // lifetime extension so chunk_at() can return &'a [A].
        let focus: &'a mut Focus<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        let (range, value) = focus.chunk_at(self.front_index);
        self.front_index = range.end;
        Some(value)
    }
}

impl<'a, A, P: SharedPointerKind + 'a> DoubleEndedIterator for Chunks<'a, A, P> {
    /// Remove and return an element from the back of the iterator.
    ///
    /// Time: O(1)*
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        self.back_index -= 1;
        // SAFETY: Same as Chunks::next — see SAFETY comment above.
        let focus: &'a mut Focus<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        let (range, value) = focus.chunk_at(self.back_index);
        self.back_index = range.start;
        Some(value)
    }
}

impl<'a, A, P: SharedPointerKind + 'a> FusedIterator for Chunks<'a, A, P> {}

/// A mutable iterator over the leaf nodes of a vector.
///
/// To obtain one, use [`Vector::chunks_mut()`][chunks_mut].
///
/// [chunks_mut]: type.Vector.html#method.chunks_mut
pub struct ChunksMut<'a, A, P: SharedPointerKind> {
    focus: FocusMut<'a, A, P>,
    front_index: usize,
    back_index: usize,
}

impl<'a, A: Clone, P: SharedPointerKind> ChunksMut<'a, A, P> {
    fn new(seq: &'a mut GenericVector<A, P>) -> Self {
        let len = seq.len();
        ChunksMut {
            focus: seq.focus_mut(),
            front_index: 0,
            back_index: len,
        }
    }
}

impl<'a, A: Clone, P: SharedPointerKind> Iterator for ChunksMut<'a, A, P> {
    type Item = &'a mut [A];

    /// Advance the iterator and return the next value.
    ///
    /// Time: O(1)*
    fn next(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        // SAFETY: Same lending iterator pattern as IterMut::next —
        // lifetime extension so chunk_at() can return &'a mut [A].
        let focus: &'a mut FocusMut<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        let (range, value) = focus.chunk_at(self.front_index);
        self.front_index = range.end;
        Some(value)
    }
}

impl<'a, A: Clone, P: SharedPointerKind> DoubleEndedIterator for ChunksMut<'a, A, P> {
    /// Remove and return an element from the back of the iterator.
    ///
    /// Time: O(1)*
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front_index >= self.back_index {
            return None;
        }
        self.back_index -= 1;
        // SAFETY: Same as ChunksMut::next — see SAFETY comment above.
        let focus: &'a mut FocusMut<'a, A, P> = unsafe { &mut *(&mut self.focus as *mut _) };
        let (range, value) = focus.chunk_at(self.back_index);
        self.back_index = range.start;
        Some(value)
    }
}

impl<'a, A: Clone, P: SharedPointerKind> FusedIterator for ChunksMut<'a, A, P> {}

// Diff

/// An item in a positional diff between two vectors.
///
/// Produced by [`GenericVector::diff`].
#[derive(Debug, PartialEq, Eq)]
pub enum DiffItem<'a, 'b, A> {
    /// An element was added at this index (present in the new vector only).
    Add(usize, &'b A),
    /// An element at this index changed between the two vectors.
    Update {
        /// The index of the changed element.
        index: usize,
        /// The old value.
        old: &'a A,
        /// The new value.
        new: &'b A,
    },
    /// An element was removed from this index (present in the old vector only).
    Remove(usize, &'a A),
}

/// An iterator over the positional differences between two vectors.
///
/// Created by [`GenericVector::diff`].
///
/// Uses chunk-level pointer comparison to skip shared subtrees in O(1)
/// per chunk when two vectors share structure (e.g. one was derived from
/// the other via `set`).
pub struct DiffIter<'a, 'b, A, P: SharedPointerKind> {
    old_focus: Focus<'a, A, P>,
    new_focus: Focus<'b, A, P>,
    old_len: usize,
    new_len: usize,
    index: usize,
    done: bool,
}

impl<'a, 'b, A: PartialEq, P: SharedPointerKind + 'a + 'b> Iterator for DiffIter<'a, 'b, A, P> {
    type Item = DiffItem<'a, 'b, A>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let min_len = self.old_len.min(self.new_len);
        loop {
            if self.index >= min_len {
                // Handle tail: additions or removals from length difference.
                if self.index < self.new_len {
                    // SAFETY: Lifetime extension from &mut self to 'b. The Focus
                    // holds a clone of the vector's tree (Arc-managed). get()
                    // returns a reference into that tree data, which lives for 'b.
                    // Each call accesses a distinct index so returned references
                    // don't alias. Same pattern as Iter::next().
                    let focus: &'b mut Focus<'b, A, P> =
                        unsafe { &mut *(&mut self.new_focus as *mut _) };
                    let val = focus.get(self.index)?;
                    let idx = self.index;
                    self.index += 1;
                    return Some(DiffItem::Add(idx, val));
                }
                if self.index < self.old_len {
                    // SAFETY: Same as above but for lifetime 'a.
                    let focus: &'a mut Focus<'a, A, P> =
                        unsafe { &mut *(&mut self.old_focus as *mut _) };
                    let val = focus.get(self.index)?;
                    let idx = self.index;
                    self.index += 1;
                    return Some(DiffItem::Remove(idx, val));
                }
                self.done = true;
                return None;
            }

            // Get chunks at the current index from both vectors.
            // SAFETY: Lifetime extension — chunk_at returns slices into
            // Arc-managed tree data that lives for 'a/'b. The &mut self
            // borrow on Focus is needed only for internal cache updates,
            // not because the returned data depends on the mutable borrow.
            // Same pattern as Iter::next().
            let old_focus: &'a mut Focus<'a, A, P> =
                unsafe { &mut *(&mut self.old_focus as *mut _) };
            let (old_range, old_chunk) = old_focus.chunk_at(self.index);

            let new_focus: &'b mut Focus<'b, A, P> =
                unsafe { &mut *(&mut self.new_focus as *mut _) };
            let (new_range, new_chunk) = new_focus.chunk_at(self.index);

            // Pointer-equal chunks share the same Arc-managed leaf data —
            // the entire chunk is identical, so skip it.
            if core::ptr::eq(old_chunk, new_chunk) {
                self.index = old_range.end.min(new_range.end);
                continue;
            }

            // Chunks differ — compare the element at the current index.
            let old_val = &old_chunk[self.index - old_range.start];
            let new_val = &new_chunk[self.index - new_range.start];
            let idx = self.index;
            self.index += 1;

            if old_val != new_val {
                return Some(DiffItem::Update {
                    index: idx,
                    old: old_val,
                    new: new_val,
                });
            }
        }
    }
}

impl<'a, 'b, A: PartialEq, P: SharedPointerKind + 'a + 'b> FusedIterator
    for DiffIter<'a, 'b, A, P>
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
    pub use crate::proptest::vector;
}

// Tests

#[cfg(test)]
mod test {
    use super::*;
    use crate::proptest::vector;
    use ::proptest::collection::vec;
    use ::proptest::num::{i32, usize};
    use ::proptest::proptest;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    assert_impl_all!(Vector<i32>: Send, Sync);
    assert_not_impl_any!(Vector<*const i32>: Send, Sync);
    assert_covariant!(Vector<T> in T);

    #[test]
    fn macro_allows_trailing_comma() {
        let vec1 = vector![1, 2, 3];
        let vec2 = vector![1, 2, 3,];
        assert_eq!(vec1, vec2);
    }

    #[test]
    fn indexing() {
        let mut vec: Vector<_> = vector![0, 1, 2, 3, 4, 5];
        vec.push_front(0);
        assert_eq!(0, *vec.get(0).unwrap());
        assert_eq!(0, vec[0]);
    }

    #[test]
    fn test_vector_focus_split_at() {
        for (data, split_points) in [
            (0..0, vec![0]),
            (0..3, vec![0, 1, 2, 3]),
            (0..128, vec![0, 1, 64, 127, 128]),
            #[cfg(not(miri))]
            (0..100_000, vec![0, 1, 50_000, 99_999, 100_000]),
        ] {
            let imbl_vec = Vector::from_iter(data.clone());
            let vec = Vec::from_iter(data);
            let focus = imbl_vec.focus();
            for split_point in split_points {
                let (left, right) = focus.clone().split_at(split_point);
                let (expected_left, expected_right) = vec.split_at(split_point);
                assert_eq!(
                    left.clone().into_iter().copied().collect::<Vec<_>>(),
                    expected_left
                );
                assert_eq!(
                    right.clone().into_iter().copied().collect::<Vec<_>>(),
                    expected_right
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "range out of bounds")]
    fn test_vector_focus_narrow_out_of_range() {
        let vec = Vector::from_iter(0..100);
        _ = vec.focus().narrow(..1000);
    }

    #[test]
    fn test_vector_focus_narrow() {
        macro_rules! testcase {
            ($data:expr, $range:expr) => {{
                let imbl_vector = Vector::<_>::from_iter($data);
                let vec = Vec::from_iter($data);
                let focus = imbl_vector.focus();
                assert_eq!(
                    focus
                        .narrow($range)
                        .into_iter()
                        .copied()
                        .collect::<Vec<_>>(),
                    vec[$range]
                );
            }};
        }
        // exhaustively test small cases
        for len in 0..=3 {
            testcase!(0..len, ..);
            for start in 0..=len {
                testcase!(0..len, start..);
                testcase!(0..len, ..start);
                for end in start..=len {
                    testcase!(0..len, start..end);
                }
            }
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn large_vector_focus() {
        let input = Vector::from_iter(0..100_000);
        let vec = input.clone();
        let mut sum: i64 = 0;
        let mut focus = vec.focus();
        for i in 0..input.len() {
            sum += *focus.index(i);
        }
        let expected: i64 = (0..100_000).sum();
        assert_eq!(expected, sum);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn large_vector_focus_mut() {
        let input = Vector::from_iter(0..100_000);
        let mut vec = input.clone();
        {
            let mut focus = vec.focus_mut();
            for i in 0..input.len() {
                let p = focus.index_mut(i);
                *p += 1;
            }
        }
        let expected: Vector<_> = input.into_iter().map(|i| i + 1).collect();
        assert_eq!(expected, vec);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn issue_55_fwd() {
        let mut l = Vector::new();
        for i in 0..4098 {
            l.append(GenericVector::unit(i));
        }
        l.append(GenericVector::unit(4098));
        assert_eq!(Some(&4097), l.get(4097));
        assert_eq!(Some(&4096), l.get(4096));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn issue_55_back() {
        let mut l = Vector::unit(0);
        for i in 0..4099 {
            let mut tmp = GenericVector::unit(i + 1);
            tmp.append(l);
            l = tmp;
        }
        assert_eq!(Some(&4098), l.get(1));
        assert_eq!(Some(&4097), l.get(2));
        let len = l.len();
        let _ = l.slice(2..len);
    }

    #[test]
    fn issue_55_append() {
        let mut vec1 = Vector::from_iter(0..92);
        let vec2 = GenericVector::from_iter(0..165);
        vec1.append(vec2);
    }

    #[test]
    fn issue_70() {
        // This test assumes that chunks are of size 64.
        if CHUNK_SIZE != 64 {
            return;
        }
        let mut x = Vector::new();
        for _ in 0..262 {
            x.push_back(0);
        }
        for _ in 0..97 {
            x.pop_front();
        }
        for &offset in &[160, 163, 160] {
            x.remove(offset);
        }
        for _ in 0..64 {
            x.push_back(0);
        }
        // At this point middle contains three chunks of size 64, 64 and 1
        // respectively. Previously the next `push_back()` would append another
        // zero-sized chunk to middle even though there is enough space left.
        match x.vector {
            VectorInner::Full(ref tree) => {
                assert_eq!(129, tree.middle.len());
                assert_eq!(3, tree.middle.number_of_children());
            }
            _ => unreachable!(),
        }
        x.push_back(0);
        match x.vector {
            VectorInner::Full(ref tree) => {
                assert_eq!(131, tree.middle.len());
                assert_eq!(3, tree.middle.number_of_children())
            }
            _ => unreachable!(),
        }
        for _ in 0..64 {
            x.push_back(0);
        }
        for _ in x.iter() {}
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn issue_67() {
        let mut l = Vector::unit(4100);
        for i in (0..4099).rev() {
            let mut tmp = GenericVector::unit(i);
            tmp.append(l);
            l = tmp;
        }
        assert_eq!(4100, l.len());
        let len = l.len();
        let tail = l.slice(1..len);
        assert_eq!(1, l.len());
        assert_eq!(4099, tail.len());
        assert_eq!(Some(&0), l.get(0));
        assert_eq!(Some(&1), tail.get(0));
    }

    #[test]
    fn issue_74_simple_size() {
        use crate::nodes::rrb::NODE_SIZE;
        let mut x = Vector::new();
        for _ in 0..(CHUNK_SIZE
            * (
                1 // inner_f
                + (2 * NODE_SIZE) // middle: two full Entry::Nodes (4096 elements each)
                + 1 // inner_b
                + 1
                // outer_b
            ))
        {
            x.push_back(0u32);
        }
        let middle_first_node_start = CHUNK_SIZE;
        let middle_second_node_start = middle_first_node_start + NODE_SIZE * CHUNK_SIZE;
        // This reduces the size of the second node to 4095.
        x.remove(middle_second_node_start);
        // As outer_b is full, this will cause inner_b (length 64) to be pushed
        // to middle. The first element will be merged into the second node, the
        // remaining 63 elements will end up in a new node.
        x.push_back(0u32);
        match x.vector {
            VectorInner::Full(tree) => {
                if CHUNK_SIZE == 64 {
                    assert_eq!(3, tree.middle.number_of_children());
                }
                assert_eq!(
                    2 * NODE_SIZE * CHUNK_SIZE + CHUNK_SIZE - 1,
                    tree.middle.len()
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn issue_77() {
        let mut x = Vector::new();
        for _ in 0..44 {
            x.push_back(0);
        }
        for _ in 0..20 {
            x.insert(0, 0);
        }
        x.insert(1, 0);
        for _ in 0..441 {
            x.push_back(0);
        }
        for _ in 0..58 {
            x.insert(0, 0);
        }
        x.insert(514, 0);
        for _ in 0..73 {
            x.push_back(0);
        }
        for _ in 0..10 {
            x.insert(0, 0);
        }
        x.insert(514, 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn issue_105() {
        let mut v = Vector::<_>::new();

        for i in 0..270_000 {
            v.push_front(i);
        }

        while !v.is_empty() {
            v = v.take(v.len() - 1);
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn issue_107_split_off_causes_overflow() {
        let mut vec = Vector::from_iter(0..4289);
        let mut control = Vec::from_iter(0..4289);
        let chunk = 64;

        while vec.len() >= chunk {
            vec = vec.split_off(chunk);
            control = control.split_off(chunk);
            assert_eq!(vec.len(), control.len());
            assert_eq!(control, vec.iter().cloned().collect::<Vec<_>>());
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn collect_crash() {
        let _vector: Vector<i32> = (0..5953).collect();
        // let _vector: Vector<i32> = (0..16384).collect();
    }

    #[test]
    fn issue_116() {
        let vec = Vector::from_iter(0..300);
        let rev_vec: Vector<_> = vec.clone().into_iter().rev().collect();
        assert_eq!(vec.len(), rev_vec.len());
    }

    #[test]
    fn issue_131() {
        let smol = std::iter::repeat_n(42, 64).collect::<Vector<_>>();
        let mut smol2 = smol.clone();
        assert!(smol.ptr_eq(&smol2));
        smol2.set(63, 420);
        assert!(!smol.ptr_eq(&smol2));

        let huge = std::iter::repeat_n(42, 65).collect::<Vector<_>>();
        let mut huge2 = huge.clone();
        assert!(huge.ptr_eq(&huge2));
        huge2.set(63, 420);
        assert!(!huge.ptr_eq(&huge2));
    }

    #[test]
    fn ptr_eq() {
        const MAX: usize = if cfg!(miri) { 64 } else { 256 };
        for len in 32..MAX {
            let input = std::iter::repeat_n(42, len).collect::<Vector<_>>();
            let mut inp2 = input.clone();
            assert!(input.ptr_eq(&inp2));
            inp2.set(len - 1, 98);
            assert_ne!(inp2.get(len - 1), input.get(len - 1));
            assert!(!input.ptr_eq(&inp2));
        }
    }

    #[test]
    fn partial_eq_ptr_eq_fast_path() {
        // Cloned vectors with shared structure are equal in O(1).
        let v: Vector<i32> = (0..100).collect();
        let v2 = v.clone();
        assert_eq!(v, v2);

        // After mutation, ptr_eq is false but element-wise equality still works.
        let mut v3 = v.clone();
        v3.set(50, 999);
        assert_ne!(v, v3);

        // Empty vectors.
        let empty: Vector<i32> = Vector::new();
        let empty2: Vector<i32> = Vector::new();
        assert_eq!(empty, empty2);

        // Self-comparison.
        assert_eq!(v, v);
    }

    #[test]
    fn diff_identical_vectors() {
        let v: Vector<i32> = (0..100).collect();
        let v2 = v.clone();
        assert_eq!(v.diff(&v2).count(), 0);
    }

    #[test]
    fn diff_ptr_eq_fast_path() {
        // Cloned vectors with shared structure produce no diffs.
        let v: Vector<i32> = (0..100).collect();
        let v2 = v.clone();
        assert!(v.ptr_eq(&v2));
        assert_eq!(v.diff(&v2).count(), 0);
    }

    #[test]
    fn diff_single_update() {
        let v: Vector<i32> = (0..10).collect();
        let mut v2 = v.clone();
        v2.set(5, 99);
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 1);
        assert_eq!(
            diffs[0],
            DiffItem::Update {
                index: 5,
                old: &5,
                new: &99
            }
        );
    }

    #[test]
    fn diff_additions() {
        let v: Vector<i32> = (0..5).collect();
        let v2: Vector<i32> = (0..8).collect();
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0], DiffItem::Add(5, &5));
        assert_eq!(diffs[1], DiffItem::Add(6, &6));
        assert_eq!(diffs[2], DiffItem::Add(7, &7));
    }

    #[test]
    fn diff_removals() {
        let v: Vector<i32> = (0..8).collect();
        let v2: Vector<i32> = (0..5).collect();
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0], DiffItem::Remove(5, &5));
        assert_eq!(diffs[1], DiffItem::Remove(6, &6));
        assert_eq!(diffs[2], DiffItem::Remove(7, &7));
    }

    #[test]
    fn diff_mixed_changes() {
        let v: Vector<i32> = vector![1, 2, 3, 4, 5];
        let v2: Vector<i32> = vector![1, 99, 3, 4, 5, 6];
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 2);
        assert_eq!(
            diffs[0],
            DiffItem::Update {
                index: 1,
                old: &2,
                new: &99
            }
        );
        assert_eq!(diffs[1], DiffItem::Add(5, &6));
    }

    #[test]
    fn diff_empty_vectors() {
        let v: Vector<i32> = Vector::new();
        let v2: Vector<i32> = Vector::new();
        assert_eq!(v.diff(&v2).count(), 0);
    }

    #[test]
    fn diff_from_empty() {
        let v: Vector<i32> = Vector::new();
        let v2: Vector<i32> = (0..3).collect();
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0], DiffItem::Add(0, &0));
        assert_eq!(diffs[1], DiffItem::Add(1, &1));
        assert_eq!(diffs[2], DiffItem::Add(2, &2));
    }

    #[test]
    fn diff_to_empty() {
        let v: Vector<i32> = (0..3).collect();
        let v2: Vector<i32> = Vector::new();
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0], DiffItem::Remove(0, &0));
        assert_eq!(diffs[1], DiffItem::Remove(1, &1));
        assert_eq!(diffs[2], DiffItem::Remove(2, &2));
    }

    #[test]
    fn diff_is_fused() {
        let v: Vector<i32> = vector![1, 2, 3];
        let v2: Vector<i32> = vector![1, 2, 4];
        let mut iter = v.diff(&v2);
        assert!(iter.next().is_some());
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn diff_subtree_skipping() {
        // A vector large enough to span multiple internal chunks.
        // With branching factor 64, a 10000-element vector has ~157 leaf chunks.
        // Modifying one element only affects the chunk containing that element;
        // all other chunks remain pointer-equal and are skipped.
        let v: Vector<i32> = (0..10_000).collect();
        let mut v2 = v.clone();
        v2.set(5000, -1);
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 1);
        assert_eq!(
            diffs[0],
            DiffItem::Update {
                index: 5000,
                old: &5000,
                new: &-1
            }
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn diff_subtree_skipping_multiple_changes() {
        // Multiple changes in different chunks — each changed chunk is
        // detected, all unchanged chunks are skipped.
        let v: Vector<i32> = (0..10_000).collect();
        let mut v2 = v.clone();
        v2.set(100, -1);
        v2.set(5000, -2);
        v2.set(9999, -3);
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 3);
        assert_eq!(
            diffs[0],
            DiffItem::Update {
                index: 100,
                old: &100,
                new: &-1
            }
        );
        assert_eq!(
            diffs[1],
            DiffItem::Update {
                index: 5000,
                old: &5000,
                new: &-2
            }
        );
        assert_eq!(
            diffs[2],
            DiffItem::Update {
                index: 9999,
                old: &9999,
                new: &-3
            }
        );
    }

    #[test]
    fn diff_subtree_skipping_small_vector() {
        // Small vectors (inline/single chunk) still work correctly.
        let v: Vector<i32> = (0..10).collect();
        let mut v2 = v.clone();
        v2.set(3, 99);
        let diffs: Vec<_> = v.diff(&v2).collect();
        assert_eq!(diffs.len(), 1);
        assert_eq!(
            diffs[0],
            DiffItem::Update {
                index: 3,
                old: &3,
                new: &99
            }
        );
    }

    #[test]
    fn apply_diff_roundtrip_updates() {
        let base = vector![1, 2, 3, 4];
        let modified = vector![1, 20, 30, 4];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_roundtrip_additions() {
        let base = vector![1, 2, 3];
        let modified = vector![1, 2, 3, 4, 5];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_roundtrip_removals() {
        let base = vector![1, 2, 3, 4, 5];
        let modified = vector![1, 2, 3];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_roundtrip_mixed() {
        let base = vector![1, 2, 3, 4, 5];
        let modified = vector![1, 20, 30, 4, 5, 6, 7];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_empty_diff() {
        let v = vector![1, 2, 3];
        let patched = v.apply_diff(vec![]);
        assert_eq!(patched, v);
    }

    #[test]
    fn apply_diff_from_empty() {
        let base: Vector<i32> = Vector::new();
        let modified = vector![1, 2, 3];
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_to_empty() {
        let base = vector![1, 2, 3];
        let modified: Vector<i32> = Vector::new();
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_preserves_original() {
        let base = vector![1, 2, 3];
        let modified = vector![1, 20, 30, 4];
        let diff: Vec<_> = base.diff(&modified).collect();
        let _patched = base.apply_diff(diff);
        assert_eq!(base, vector![1, 2, 3]);
    }

    #[test]
    fn adjust_basic() {
        let vec = vector![1, 2, 3, 4];
        let updated = vec.adjust(1, |v| v * 10);
        assert_eq!(updated, vector![1, 20, 3, 4]);
        // Original unchanged
        assert_eq!(vec, vector![1, 2, 3, 4]);
    }

    #[test]
    fn chunked_even() {
        let vec = vector![1, 2, 3, 4];
        let chunks = vec.chunked(2);
        assert_eq!(chunks, vec![vector![1, 2], vector![3, 4]]);
    }

    #[test]
    fn chunked_uneven() {
        let vec = vector![1, 2, 3, 4, 5];
        let chunks = vec.chunked(2);
        assert_eq!(chunks, vec![vector![1, 2], vector![3, 4], vector![5]]);
    }

    #[test]
    fn chunked_larger_than_vec() {
        let vec = vector![1, 2, 3];
        let chunks = vec.chunked(10);
        assert_eq!(chunks, vec![vector![1, 2, 3]]);
    }

    #[test]
    fn chunked_empty() {
        let vec: Vector<i32> = Vector::new();
        let chunks = vec.chunked(3);
        assert!(chunks.is_empty());
    }

    #[test]
    fn patch_replace_middle() {
        let vec = vector![1, 2, 3, 4, 5];
        let replacement = vector![20, 30];
        let patched = vec.patch(1, &replacement, 2);
        assert_eq!(patched, vector![1, 20, 30, 4, 5]);
    }

    #[test]
    fn patch_insert_without_removing() {
        let vec = vector![1, 2, 3];
        let insertion = vector![10, 20];
        let patched = vec.patch(1, &insertion, 0);
        assert_eq!(patched, vector![1, 10, 20, 2, 3]);
    }

    #[test]
    fn patch_remove_without_inserting() {
        let vec = vector![1, 2, 3, 4, 5];
        let empty: Vector<i32> = Vector::new();
        let patched = vec.patch(1, &empty, 2);
        assert_eq!(patched, vector![1, 4, 5]);
    }

    #[test]
    fn patch_at_end() {
        let vec = vector![1, 2, 3];
        let tail = vector![4, 5];
        let patched = vec.patch(3, &tail, 0);
        assert_eq!(patched, vector![1, 2, 3, 4, 5]);
    }

    #[test]
    fn scan_left_prefix_sums() {
        let vec = vector![1, 2, 3, 4];
        let result = vec.scan_left(0, |acc, x| acc + x);
        assert_eq!(result, vector![0, 1, 3, 6, 10]);
    }

    #[test]
    fn scan_left_empty() {
        let vec: Vector<i32> = Vector::new();
        let result = vec.scan_left(42, |acc, x| acc + x);
        assert_eq!(result, vector![42]);
    }

    #[test]
    fn scan_left_single() {
        let vec = vector![5];
        let result = vec.scan_left(0, |acc, x| acc + x);
        assert_eq!(result, vector![0, 5]);
    }

    #[test]
    fn sliding_basic() {
        let vec = vector![1, 2, 3, 4, 5];
        let windows = vec.sliding(3, 1);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], vector![1, 2, 3]);
        assert_eq!(windows[1], vector![2, 3, 4]);
        assert_eq!(windows[2], vector![3, 4, 5]);
    }

    #[test]
    fn sliding_step_two() {
        let vec = vector![1, 2, 3, 4, 5, 6];
        let windows = vec.sliding(2, 2);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], vector![1, 2]);
        assert_eq!(windows[1], vector![3, 4]);
        assert_eq!(windows[2], vector![5, 6]);
    }

    #[test]
    fn sliding_window_equals_len() {
        let vec = vector![1, 2, 3];
        let windows = vec.sliding(3, 1);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], vector![1, 2, 3]);
    }

    #[test]
    fn sliding_window_larger_than_vec() {
        let vec = vector![1, 2];
        let windows = vec.sliding(3, 1);
        assert!(windows.is_empty());
    }

    #[test]
    fn sliding_empty_vec() {
        let vec: Vector<i32> = Vector::new();
        let windows = vec.sliding(2, 1);
        assert!(windows.is_empty());
    }

    #[test]
    fn full_retain() {
        let mut a = Vector::from_iter(0..128);
        let b = Vector::from_iter(128..256);
        a.append(b);
        assert!(matches!(a.vector, Full(_)));
        a.retain(|i| *i % 2 == 0);
        assert_eq!(a.len(), 128);
    }

    proptest! {
        // Miri is slow, so we ignore long-ish tests to keep the test
        // time manageable. For some property tests, it may be worthwhile
        // enabling them in miri with reduced iteration counts.
        #[cfg_attr(miri, ignore)]
        #[test]
        fn iter(ref vec in vec(i32::ANY, 0..1000)) {
            let seq = Vector::from_iter(vec.iter().cloned());
            for (index, item) in seq.iter().enumerate() {
                assert_eq!(&vec[index], item);
            }
            assert_eq!(vec.len(), seq.len());
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn push_front_mut(ref input in vec(i32::ANY, 0..1000)) {
            let mut vector = Vector::new();
            for (count, value) in input.iter().cloned().enumerate() {
                assert_eq!(count, vector.len());
                vector.push_front(value);
                assert_eq!(count + 1, vector.len());
            }
            let input2 = Vec::from_iter(input.iter().rev().cloned());
            assert_eq!(input2, Vec::from_iter(vector.iter().cloned()));
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn push_back_mut(ref input in vec(i32::ANY, 0..1000)) {
            let mut vector = Vector::new();
            for (count, value) in input.iter().cloned().enumerate() {
                assert_eq!(count, vector.len());
                vector.push_back(value);
                assert_eq!(count + 1, vector.len());
            }
            assert_eq!(input, &Vec::from_iter(vector.iter().cloned()));
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn pop_back_mut(ref input in vec(i32::ANY, 0..1000)) {
            let mut vector = Vector::from_iter(input.iter().cloned());
            assert_eq!(input.len(), vector.len());
            for (index, value) in input.iter().cloned().enumerate().rev() {
                match vector.pop_back() {
                    None => panic!("vector emptied unexpectedly"),
                    Some(item) => {
                        assert_eq!(index, vector.len());
                        assert_eq!(value, item);
                    }
                }
            }
            assert_eq!(0, vector.len());
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn pop_front_mut(ref input in vec(i32::ANY, 0..1000)) {
            let mut vector = Vector::from_iter(input.iter().cloned());
            assert_eq!(input.len(), vector.len());
            for (index, value) in input.iter().cloned().rev().enumerate().rev() {
                match vector.pop_front() {
                    None => panic!("vector emptied unexpectedly"),
                    Some(item) => {
                        assert_eq!(index, vector.len());
                        assert_eq!(value, item);
                    }
                }
            }
            assert_eq!(0, vector.len());
        }

        // #[test]
        // fn push_and_pop(ref input in vec(i32::ANY, 0..1000)) {
        //     let mut vector = Vector::new();
        //     for (count, value) in input.iter().cloned().enumerate() {
        //         assert_eq!(count, vector.len());
        //         vector.push_back(value);
        //         assert_eq!(count + 1, vector.len());
        //     }
        //     for (index, value) in input.iter().cloned().rev().enumerate().rev() {
        //         match vector.pop_front() {
        //             None => panic!("vector emptied unexpectedly"),
        //             Some(item) => {
        //                 assert_eq!(index, vector.len());
        //                 assert_eq!(value, item);
        //             }
        //         }
        //     }
        //     assert_eq!(true, vector.is_empty());
        // }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn skip(ref vec in vec(i32::ANY, 1..2000), count in usize::ANY) {
            let count = count % (vec.len() + 1);
            let old = Vector::from_iter(vec.iter().cloned());
            let new = old.skip(count);
            assert_eq!(old.len(), vec.len());
            assert_eq!(new.len(), vec.len() - count);
            for (index, item) in old.iter().enumerate() {
                assert_eq!(& vec[index], item);
            }
            for (index, item) in new.iter().enumerate() {
                assert_eq!(&vec[count + index], item);
            }
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn split_off(ref vec in vec(i32::ANY, 1..2000), split_pos in usize::ANY) {
            let split_index = split_pos % (vec.len() + 1);
            let mut left = Vector::from_iter(vec.iter().cloned());
            let right = left.split_off(split_index);
            assert_eq!(left.len(), split_index);
            assert_eq!(right.len(), vec.len() - split_index);
            for (index, item) in left.iter().enumerate() {
                assert_eq!(& vec[index], item);
            }
            for (index, item) in right.iter().enumerate() {
                assert_eq!(&vec[split_index + index], item);
            }
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn append(ref vec1 in vec(i32::ANY, 0..1000), ref vec2 in vec(i32::ANY, 0..1000)) {
            let mut seq1 = Vector::from_iter(vec1.iter().cloned());
            let seq2 = Vector::from_iter(vec2.iter().cloned());
            assert_eq!(seq1.len(), vec1.len());
            assert_eq!(seq2.len(), vec2.len());
            seq1.append(seq2);
            let mut vec = vec1.clone();
            vec.extend(vec2);
            assert_eq!(seq1.len(), vec.len());
            for (index, item) in seq1.into_iter().enumerate() {
                assert_eq!(vec[index], item);
            }
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn iter_mut(ref input in vector(i32::ANY, 0..10000)) {
            let mut vec = input.clone();
            {
                for p in vec.iter_mut() {
                    *p = p.overflowing_add(1).0;
                }
            }
            let expected: Vector<i32> = input.clone().into_iter().map(|i| i.overflowing_add(1).0).collect();
            assert_eq!(expected, vec);
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn focus(ref input in vector(i32::ANY, 0..10000)) {
            let mut vec = input.clone();
            {
                let mut focus = vec.focus_mut();
                for i in 0..input.len() {
                    let p = focus.index_mut(i);
                    *p = p.overflowing_add(1).0;
                }
            }
            let expected: Vector<i32> = input.clone().into_iter().map(|i| i.overflowing_add(1).0).collect();
            assert_eq!(expected, vec);
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn focus_mut_split(ref input in vector(i32::ANY, 0..10000)) {
            let mut vec = input.clone();

            fn split_down(focus: FocusMut<'_, i32, DefaultSharedPtr>) {
                let len = focus.len();
                if len < 8 {
                    for p in focus {
                        *p = p.overflowing_add(1).0;
                    }
                } else {
                    let (left, right) = focus.split_at(len / 2);
                    split_down(left);
                    split_down(right);
                }
            }

            split_down(vec.focus_mut());

            let expected: Vector<_> = input.clone().into_iter().map(|i| i.overflowing_add(1).0).collect();
            assert_eq!(expected, vec);
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn chunks(ref input in vector(i32::ANY, 0..10000)) {
            let output: Vector<_> = input.leaves().flatten().cloned().collect();
            assert_eq!(input, &output);
            let rev_in: Vector<_> = input.iter().rev().cloned().collect();
            let rev_out: Vector<_> = input.leaves().rev().flat_map(|c| c.iter().rev()).cloned().collect();
            assert_eq!(rev_in, rev_out);
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn chunks_mut(ref mut input_src in vector(i32::ANY, 0..10000)) {
            let mut input = input_src.clone();
            #[allow(clippy::map_clone)] // leaves_mut() yields &mut A; deref via `*v` is clearer than `Clone::clone` here.
            let output: Vector<_> = input.leaves_mut().flatten().map(|v| *v).collect();
            assert_eq!(input, output);
            let rev_in: Vector<_> = input.iter().rev().cloned().collect();
            let rev_out: Vector<_> = input.leaves_mut().rev().flat_map(|c| c.iter().rev()).cloned().collect();
            assert_eq!(rev_in, rev_out);
        }

        // The following two tests are very slow and there are unit tests above
        // which test for regression of issue #55.  It would still be good to
        // run them occasionally.

        // #[test]
        // fn issue55_back(count in 0..10000, slice_at in usize::ANY) {
        //     let count = count as usize;
        //     let slice_at = slice_at % count;
        //     let mut l = Vector::unit(0);
        //     for _ in 0..count {
        //         let mut tmp = Vector::unit(0);
        //         tmp.append(l);
        //         l = tmp;
        //     }
        //     let len = l.len();
        //     l.slice(slice_at..len);
        // }

        // #[test]
        // fn issue55_fwd(count in 0..10000, slice_at in usize::ANY) {
        //     let count = count as usize;
        //     let slice_at = slice_at % count;
        //     let mut l = Vector::new();
        //     for i in 0..count {
        //         l.append(Vector::unit(i));
        //     }
        //     assert_eq!(Some(&slice_at), l.get(slice_at));
        // }
    }

    // Tests targeting unsafe code paths. These exercise boundary conditions
    // that would trigger UB if the safety invariants documented above are
    // violated. Run under miri in CI for UB detection.
    mod unsafe_edge_cases {
        use super::*;

        #[test]
        fn swap_adjacent_elements() {
            let mut v: Vector<i32> = (0..10).collect();
            v.swap(4, 5);
            assert_eq!(v[4], 5);
            assert_eq!(v[5], 4);
        }

        #[test]
        fn swap_first_and_last() {
            let mut v: Vector<i32> = (0..100).collect();
            v.swap(0, 99);
            assert_eq!(v[0], 99);
            assert_eq!(v[99], 0);
        }

        #[test]
        fn swap_same_index_is_noop() {
            let mut v: Vector<i32> = (0..5).collect();
            v.swap(2, 2);
            assert_eq!(v, (0..5).collect::<Vector<_>>());
        }

        #[test]
        fn iter_empty_vector() {
            let v: Vector<i32> = Vector::new();
            assert_eq!(v.iter().count(), 0);
            assert_eq!(v.iter().next(), None);
        }

        #[test]
        fn iter_single_element() {
            let v = Vector::unit(42);
            let items: Vec<_> = v.iter().collect();
            assert_eq!(items, vec![&42]);
        }

        #[test]
        fn iter_at_chunk_boundary() {
            // VECTOR_CHUNK_SIZE is 64 (or 4 with small-chunks). Test at
            // exactly the boundary where the tree gains a new level.
            let v: Vector<i32> = (0..64).collect();
            let items: Vec<_> = v.iter().collect();
            assert_eq!(items.len(), 64);
            assert_eq!(*items[63], 63);

            let v: Vector<i32> = (0..65).collect();
            let items: Vec<_> = v.iter().collect();
            assert_eq!(items.len(), 65);
        }

        #[test]
        fn iter_mut_single_element() {
            let mut v = Vector::unit(1);
            for x in v.iter_mut() {
                *x += 10;
            }
            assert_eq!(v[0], 11);
        }

        #[test]
        fn iter_mut_at_chunk_boundary() {
            let mut v: Vector<i32> = (0..64).collect();
            for x in v.iter_mut() {
                *x *= 2;
            }
            assert_eq!(v[0], 0);
            assert_eq!(v[63], 126);
        }

        #[test]
        fn iter_double_ended_meets_in_middle() {
            let v: Vector<i32> = (0..10).collect();
            let mut it = v.iter();
            assert_eq!(it.next(), Some(&0));
            assert_eq!(it.next_back(), Some(&9));
            assert_eq!(it.next(), Some(&1));
            assert_eq!(it.next_back(), Some(&8));
            // Exhaust remaining
            let rest: Vec<_> = it.collect();
            assert_eq!(rest, vec![&2, &3, &4, &5, &6, &7]);
        }

        #[test]
        fn iter_mut_double_ended() {
            let mut v: Vector<i32> = (0..4).collect();
            let mut it = v.iter_mut();
            *it.next().unwrap() = 100;
            *it.next_back().unwrap() = 300;
            assert_eq!(v[0], 100);
            assert_eq!(v[3], 300);
            assert_eq!(v[1], 1);
            assert_eq!(v[2], 2);
        }

        #[test]
        fn leaves_empty_vector() {
            let v: Vector<i32> = Vector::new();
            assert_eq!(v.leaves().count(), 0);
        }

        #[test]
        fn leaves_single_element() {
            let v = Vector::unit(42);
            let chunks: Vec<_> = v.leaves().collect();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0], &[42]);
        }

        #[test]
        fn leaves_mut_single_element() {
            let mut v = Vector::unit(42);
            for chunk in v.leaves_mut() {
                chunk[0] = 99;
            }
            assert_eq!(v[0], 99);
        }

        #[test]
        fn leaves_double_ended() {
            let v: Vector<i32> = (0..10).collect();
            let mut it = v.leaves();
            let first = it.next().unwrap();
            assert!(!first.is_empty());
            // Double-ended: back chunk should also be non-empty
            if let Some(last) = it.next_back() {
                assert!(!last.is_empty());
            }
        }

        #[test]
        fn focus_get_at_boundaries() {
            let v: Vector<i32> = (0..200).collect();
            let mut focus = v.focus();
            // First element
            assert_eq!(focus.get(0), Some(&0));
            // Last element
            assert_eq!(focus.get(199), Some(&199));
            // Out of bounds
            assert_eq!(focus.get(200), None);
            // Access pattern that crosses chunk boundaries
            assert_eq!(focus.get(63), Some(&63));
            assert_eq!(focus.get(64), Some(&64));
        }

        #[test]
        fn focus_mut_at_boundaries() {
            let mut v: Vector<i32> = (0..200).collect();
            let mut focus = v.focus_mut();
            // Mutate at chunk boundaries
            *focus.get_mut(0).unwrap() = 1000;
            *focus.get_mut(63).unwrap() = 1063;
            *focus.get_mut(64).unwrap() = 1064;
            *focus.get_mut(199).unwrap() = 1199;
            assert_eq!(focus.get_mut(200), None);
            // Verify through the focus
            assert_eq!(*focus.get_mut(0).unwrap(), 1000);
            assert_eq!(*focus.get_mut(199).unwrap(), 1199);
        }

        #[test]
        fn swap_across_chunks() {
            // Swap elements that are in different tree chunks
            let mut v: Vector<i32> = (0..200).collect();
            v.swap(10, 150);
            assert_eq!(v[10], 150);
            assert_eq!(v[150], 10);
        }

        #[test]
        fn iter_shared_structure_isolation() {
            // Clone a vector (shared structure), mutate via iter_mut on
            // one copy, verify the other is unchanged
            let v1: Vector<i32> = (0..100).collect();
            let mut v2 = v1.clone();
            for x in v2.iter_mut() {
                *x += 1000;
            }
            assert_eq!(v1[0], 0);
            assert_eq!(v2[0], 1000);
        }

        // --- Miri-targeted tests ---
        // These specifically exercise patterns where UB is most likely if
        // invariants are violated: aliasing, use-after-free, dangling
        // pointers, and out-of-bounds access through raw pointers.
        // They are NOT marked #[cfg_attr(miri, ignore)] — they are
        // designed to run under miri.

        #[test]
        fn miri_focus_alternating_access_pattern() {
            // Alternating between distant indices forces the Focus to
            // re-target its cached chunk pointer on every access.
            // If the pointer is ever dangling, miri catches it.
            let v: Vector<i32> = (0..500).collect();
            let mut focus = v.focus();
            for i in 0..100 {
                assert_eq!(focus.get(i), Some(&(i as i32)));
                assert_eq!(focus.get(499 - i), Some(&(499 - i as i32)));
            }
        }

        #[test]
        fn miri_focus_mut_alternating_access() {
            // Same pattern with mutable focus — exercises make_mut
            // copy-on-write on each chunk re-target.
            let mut v: Vector<i32> = (0..500).collect();
            let mut focus = v.focus_mut();
            for i in 0..50 {
                *focus.get_mut(i).unwrap() += 1;
                *focus.get_mut(499 - i).unwrap() += 1;
            }
            // Verify mutations stuck
            assert_eq!(*focus.get_mut(0).unwrap(), 1);
            assert_eq!(*focus.get_mut(499).unwrap(), 500);
        }

        #[test]
        fn miri_focus_mut_then_read_original() {
            // Create shared structure, mutate through FocusMut, then
            // read the original. Tests that copy-on-write correctly
            // separates the two collections (no aliasing of mutated
            // data through the original's pointers).
            let v1: Vector<i32> = (0..200).collect();
            let mut v2 = v1.clone();
            {
                let mut focus = v2.focus_mut();
                for i in 0..200 {
                    *focus.get_mut(i).unwrap() *= -1;
                }
            }
            // v1 must be untouched — its tree nodes were not mutated
            for i in 0..200 {
                assert_eq!(v1[i], i as i32);
                assert_eq!(v2[i], -(i as i32));
            }
        }

        #[test]
        fn miri_iter_mut_drop_midway() {
            // Create a mutable iterator, consume some elements, then
            // drop the iterator. Tests that partial iteration doesn't
            // leave dangling state in the Focus.
            let mut v: Vector<i32> = (0..100).collect();
            {
                let mut it = v.iter_mut();
                *it.next().unwrap() = 999;
                *it.next().unwrap() = 998;
                // Drop iterator without exhausting it
            }
            assert_eq!(v[0], 999);
            assert_eq!(v[1], 998);
            assert_eq!(v[2], 2); // unchanged
        }

        #[test]
        fn miri_swap_many_cross_chunk() {
            // Many swaps across different chunks — each swap takes raw
            // pointers from IndexMut. If any pointer is invalid, miri
            // catches the access.
            let mut v: Vector<i32> = (0..300).collect();
            for i in 0..150 {
                v.swap(i, 299 - i);
            }
            for i in 0..300 {
                assert_eq!(v[i], (299 - i) as i32);
            }
        }

        #[test]
        fn miri_leaves_mut_interleaved_with_access() {
            // Mutate through leaves_mut, then access through normal
            // indexing. Tests that the AtomicPtr in FocusMut doesn't
            // leave stale pointers.
            let mut v: Vector<i32> = (0..200).collect();
            for chunk in v.leaves_mut() {
                for val in chunk.iter_mut() {
                    *val += 1000;
                }
            }
            for i in 0..200 {
                assert_eq!(v[i], i as i32 + 1000);
            }
        }

        #[test]
        fn miri_multiple_focus_mut_sequential() {
            // Create and drop multiple FocusMut instances on the same
            // vector. Each should get a valid, non-aliasing view.
            let mut v: Vector<i32> = (0..100).collect();
            {
                let mut f = v.focus_mut();
                *f.get_mut(0).unwrap() = 10;
            }
            {
                let mut f = v.focus_mut();
                *f.get_mut(1).unwrap() = 20;
            }
            assert_eq!(v[0], 10);
            assert_eq!(v[1], 20);
        }

        /// Verify that repeated concatenation produces bounded tree height.
        /// This is the regression test for issue #35: with Stucki's algorithm,
        /// vectors of ~40K elements reached height 7 after repeated concatenation
        /// (expected height 3 with branching factor 64).
        #[test]
        fn concat_depth_bounded() {
            use crate::nodes::chunk::CHUNK_SIZE;
            let chunk_count = 100;
            let chunk_size = CHUNK_SIZE; // 64 or 4 depending on small-chunks

            // Build a vector by repeatedly concatenating small vectors
            let mut vec = Vector::new();
            for i in 0..chunk_count {
                let chunk: Vector<usize> = (i * chunk_size..(i + 1) * chunk_size).collect();
                vec.append(chunk);
            }

            let total = chunk_count * chunk_size;
            assert_eq!(vec.len(), total);

            // Verify correctness: all elements present in order
            for (idx, val) in vec.iter().enumerate() {
                assert_eq!(idx, *val);
            }

            // Verify invariants
            vec.assert_invariants();

            // Check tree height is bounded by O(log_m(n))
            // For n elements and branching factor m, max height = ceil(log_m(n)) + 1
            // (the +1 accounts for the leaf level)
            let max_height = {
                let mut h = 0;
                let mut capacity = 1_usize;
                while capacity < total {
                    capacity = capacity.saturating_mul(CHUNK_SIZE);
                    h += 1;
                }
                h + 1 // extra margin for relaxed nodes
            };
            let actual_height = vec.middle_level();
            assert!(
                actual_height <= max_height,
                "tree height {} exceeds expected bound {} for {} elements (branching factor {})",
                actual_height,
                max_height,
                total,
                CHUNK_SIZE,
            );
        }

        /// Verify that repeated equal-sized concatenation maintains bounded height.
        /// This is the pathological case: building 40K elements from 40 chunks
        /// of 1000 elements via repeated append.
        #[cfg(not(miri))]
        #[test]
        fn concat_depth_equal_sized() {
            use crate::nodes::chunk::CHUNK_SIZE;
            let chunk_count = 40;
            let elements_per_chunk = 1000;

            let mut vec = Vector::new();
            for i in 0..chunk_count {
                let chunk: Vector<usize> =
                    (i * elements_per_chunk..(i + 1) * elements_per_chunk).collect();
                vec.append(chunk);
            }

            let total = chunk_count * elements_per_chunk;
            assert_eq!(vec.len(), total);

            // Verify correctness
            for (idx, val) in vec.iter().enumerate() {
                assert_eq!(idx, *val);
            }

            vec.assert_invariants();

            // Height bound: ceil(log_m(n)) + 1
            let max_height = {
                let mut h = 0;
                let mut capacity = 1_usize;
                while capacity < total {
                    capacity = capacity.saturating_mul(CHUNK_SIZE);
                    h += 1;
                }
                h + 1
            };
            let actual_height = vec.middle_level();
            assert!(
                actual_height <= max_height,
                "tree height {} exceeds expected bound {} for {} elements (branching factor {})",
                actual_height,
                max_height,
                total,
                CHUNK_SIZE,
            );
        }
    }

    // --- Merkle hash tests ---

    #[test]
    fn merkle_empty_vector_is_valid() {
        let v: Vector<i32> = Vector::new();
        assert!(v.merkle_valid());
    }

    #[test]
    fn merkle_new_vector_invalidated_after_push() {
        let mut v: Vector<i32> = Vector::new();
        v.push_back(1);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_recompute_makes_valid() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        assert!(!v.merkle_valid());
        v.recompute_merkle();
        assert!(v.merkle_valid());
    }

    #[test]
    fn merkle_identical_vectors_same_hash() {
        let mut a: Vector<i32> = vector![1, 2, 3, 4, 5];
        let mut b: Vector<i32> = vector![1, 2, 3, 4, 5];
        a.recompute_merkle();
        b.recompute_merkle();
        assert_eq!(a.merkle_hash, b.merkle_hash);
    }

    #[test]
    fn merkle_different_vectors_different_hash() {
        let mut a: Vector<i32> = vector![1, 2, 3];
        let mut b: Vector<i32> = vector![1, 2, 4];
        a.recompute_merkle();
        b.recompute_merkle();
        assert_ne!(a.merkle_hash, b.merkle_hash);
    }

    #[test]
    fn merkle_order_sensitive() {
        let mut a: Vector<i32> = vector![1, 2, 3];
        let mut b: Vector<i32> = vector![3, 2, 1];
        a.recompute_merkle();
        b.recompute_merkle();
        assert_ne!(a.merkle_hash, b.merkle_hash);
    }

    #[test]
    fn merkle_clone_preserves_hash() {
        let mut a: Vector<i32> = vector![1, 2, 3];
        a.recompute_merkle();
        let b = a.clone();
        assert!(b.merkle_valid());
        assert_eq!(a.merkle_hash, b.merkle_hash);
    }

    #[test]
    fn merkle_invalidated_by_set() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.set(1, 20);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_push_front() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.push_front(0);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_push_back() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.push_back(4);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_pop_front() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.pop_front();
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_pop_back() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.pop_back();
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_append() {
        let mut a: Vector<i32> = vector![1, 2, 3];
        a.recompute_merkle();
        a.append(vector![4, 5]);
        assert!(!a.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_split_off() {
        let mut v: Vector<i32> = vector![1, 2, 3, 4, 5];
        v.recompute_merkle();
        let right = v.split_off(3);
        assert!(!v.merkle_valid());
        assert!(!right.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_insert() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.insert(1, 10);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_remove() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.remove(1);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_get_mut() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        let _ = v.get_mut(1);
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_iter_mut() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        {
            let _ = v.iter_mut();
        }
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_focus_mut() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        {
            let _ = v.focus_mut();
        }
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_invalidated_by_sort() {
        let mut v: Vector<i32> = vector![3, 1, 2];
        v.recompute_merkle();
        v.sort();
        assert!(!v.merkle_valid());
    }

    #[test]
    fn merkle_clear_sets_valid_zero() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        v.recompute_merkle();
        v.clear();
        assert!(v.merkle_valid());
        assert_eq!(v.merkle_hash, 0);
    }

    #[test]
    fn merkle_positive_equality() {
        // Two separately constructed identical vectors with valid
        // Merkle hashes should compare equal via the Merkle shortcut.
        let mut a: Vector<i32> = vector![10, 20, 30, 40, 50];
        let mut b: Vector<i32> = vector![10, 20, 30, 40, 50];
        a.recompute_merkle();
        b.recompute_merkle();
        assert_eq!(a, b);
    }

    #[test]
    fn merkle_hash_method_recomputes() {
        let mut v: Vector<i32> = vector![1, 2, 3];
        assert!(!v.merkle_valid());
        let h = v.merkle_hash();
        assert!(v.merkle_valid());
        assert_ne!(h, 0); // non-trivial hash
    }

    #[test]
    fn merkle_stable_across_recomputation() {
        let mut v: Vector<i32> = vector![1, 2, 3, 4, 5];
        let h1 = v.merkle_hash();
        v.merkle_valid = false; // force recomputation
        let h2 = v.merkle_hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn merkle_large_vector() {
        // Test with a vector large enough to use the Full representation.
        let mut a: Vector<i32> = (0..10000).collect();
        let mut b: Vector<i32> = (0..10000).collect();
        let ha = a.merkle_hash();
        let hb = b.merkle_hash();
        assert_eq!(ha, hb);

        // Mutate one element — hashes should diverge.
        b.set(5000, -1);
        b.recompute_merkle();
        assert_ne!(ha, b.merkle_hash);
    }

    proptest! {
        #[test]
        fn merkle_proptest_identical_vectors(ref v in vector(i32::ANY, 0..1000)) {
            let mut a = v.clone();
            let mut b = v.clone();
            a.recompute_merkle();
            b.recompute_merkle();
            assert_eq!(a.merkle_hash, b.merkle_hash);
        }

        #[test]
        fn merkle_proptest_mutation_changes_hash(ref v in vector(i32::ANY, 2..500)) {
            let mut original = v.clone();
            original.recompute_merkle();
            let orig_hash = original.merkle_hash;

            let mut modified = v.clone();
            // Flip the first element to something different.
            let first = modified[0];
            modified.set(0, first.wrapping_add(1));
            modified.recompute_merkle();

            // Collision is theoretically possible but astronomically
            // unlikely for a 64-bit hash.
            assert_ne!(orig_hash, modified.merkle_hash);
        }
    }

    #[test]
    fn from_array() {
        let v: Vector<i32> = [1, 2, 3].into();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 1);
        assert_eq!(v[2], 3);
    }

    #[test]
    fn from_slice() {
        let s: &[i32] = &[10, 20, 30];
        let v: Vector<i32> = s.into();
        assert_eq!(v.len(), 3);
        assert_eq!(v[1], 20);
    }

    #[test]
    fn from_std_vec() {
        let v: Vector<i32> = vec![5, 6, 7].into();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 5);
    }

    #[test]
    fn hash_equal_vectors_same_hash() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(v: &Vector<i32>) -> u64 {
            let mut h = DefaultHasher::new();
            v.hash(&mut h);
            h.finish()
        }
        let a: Vector<i32> = vector![1, 2, 3];
        let b: Vector<i32> = vector![1, 2, 3];
        assert_eq!(hash_of(&a), hash_of(&b));
        let c: Vector<i32> = vector![3, 2, 1];
        // Order matters for vector hash.
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn sum_vectors() {
        let vecs: Vec<Vector<i32>> = vec![vector![1, 2], vector![3], vector![4, 5]];
        let total: Vector<i32> = vecs.into_iter().sum();
        assert_eq!(total.len(), 5);
        assert_eq!(total[0], 1);
        assert_eq!(total[4], 5);
    }

    #[test]
    fn add_ref_vectors() {
        let a: Vector<i32> = vector![1, 2];
        let b: Vector<i32> = vector![3, 4];
        let c = &a + &b;
        assert_eq!(c.len(), 4);
        assert_eq!(c[2], 3);
    }

    #[test]
    fn extend_appends() {
        let mut v: Vector<i32> = vector![1, 2];
        v.extend(vec![3, 4, 5]);
        assert_eq!(v.len(), 5);
        assert_eq!(v[4], 5);
    }

    #[test]
    fn partial_ord_and_ord() {
        let a: Vector<i32> = vector![1, 2, 3];
        let b: Vector<i32> = vector![1, 2, 4];
        let c: Vector<i32> = vector![1, 2, 3];
        assert!(a < b);
        assert!(b > a);
        assert_eq!(a.partial_cmp(&c), Some(core::cmp::Ordering::Equal));
        assert_eq!(a.cmp(&c), core::cmp::Ordering::Equal);
        assert_eq!(a.cmp(&b), core::cmp::Ordering::Less);
        // Shorter vector is less than longer with same prefix.
        let d: Vector<i32> = vector![1, 2];
        assert!(d < a);
    }
}
