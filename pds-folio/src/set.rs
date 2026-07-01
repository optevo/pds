//! Folio-backed persistent hash set.
//!
//! [`HamtSet<A, C, B>`] is a thin wrapper over [`HamtMap<A, (), C, B>`].
//! All mutations return a new `HamtSet` snapshot; the original is unchanged.
//! Structural sharing and reference counting are inherited from [`HamtMap`].
//!
//! # Type parameters
//!
//! - `A` — element type; must be `Hash + Eq + Clone`
//! - `C` — codec; defaults to [`crate::codec::PodCodec`]
//! - `B` — folio backend; defaults to [`folio_core::backend::MemBackend`]

use folio_core::{backend::MemBackend, error::BackendError, store::FolioStore};
use std::hash::Hash;

use folio_core::backend::Backend;

use crate::{
    codec::{PodCodec, ValueCodec},
    hamt::{HamtError, HamtMap},
};

// ---------------------------------------------------------------------------
// HamtSet
// ---------------------------------------------------------------------------

/// A persistent, folio-backed hash set.
///
/// Thin wrapper over [`HamtMap<A, (), C, B>`].  Every mutating operation
/// returns a new `HamtSet` snapshot, leaving the original unchanged.
///
/// # Examples
///
/// ```no_run
/// use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
/// use pds_folio::set::HamtSet;
///
/// let backend = MemBackend::new(4096, 64);
/// let store = FolioStore::create(backend, 4096, 64, ChecksumKind::Xxh3, true).unwrap();
/// let s: HamtSet<u32> = HamtSet::new(store);
/// let s2 = s.insert(42u32).unwrap();
/// assert!(!s2.is_empty());
/// assert!(s2.contains(&42u32).unwrap());
/// assert!(!s2.contains(&99u32).unwrap());
/// ```
#[derive(Debug)]
pub struct HamtSet<A = u32, C = PodCodec, B = MemBackend>
where
    A: Hash + Eq + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    /// Inner map with unit values.
    inner: HamtMap<A, (), C, B>,
}

impl<A, C, B> HamtSet<A, C, B>
where
    A: Hash + Eq + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Creates a new empty set backed by `store`.
    ///
    /// Takes ownership of the folio store.  Multiple set snapshots share the
    /// same store via an internal `Arc<Mutex<…>>`.
    #[must_use]
    pub fn new(store: FolioStore<B>) -> Self {
        Self {
            inner: HamtMap::new(store),
        }
    }

    /// Wraps an existing `HamtMap<A, (), C, B>` as a `HamtSet`.
    fn from_inner(inner: HamtMap<A, (), C, B>) -> Self {
        Self { inner }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Returns the number of elements in the set.
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Tests whether the set contains `value`.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N).
    pub fn contains(&self, value: &A) -> Result<bool, HamtError> {
        self.inner.contains_key(value)
    }

    // -----------------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------------

    /// Returns a new set with `value` inserted.
    ///
    /// If `value` is already present, the set is unchanged (returns an
    /// identical snapshot).  The original set is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N).
    pub fn insert(&self, value: A) -> Result<Self, HamtError> {
        let new_inner = self.inner.insert(value, ())?;
        Ok(Self::from_inner(new_inner))
    }

    /// Returns a new set with `value` removed, plus a flag indicating whether
    /// the value was present.
    ///
    /// If `value` is absent, the returned set is an identical snapshot.
    /// The original set is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N).
    pub fn remove(&self, value: &A) -> Result<(Self, bool), HamtError> {
        let (new_inner, removed) = self.inner.remove(value)?;
        Ok((Self::from_inner(new_inner), removed.is_some()))
    }

    // -----------------------------------------------------------------------
    // Set operations
    // -----------------------------------------------------------------------

    /// Returns a new set containing all elements from both `self` and `other`.
    ///
    /// Iterates `other` and inserts each element into a clone of `self`.
    /// Elements already present are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(N log N) where N = `other.len()`.
    pub fn union(&self, other: &Self) -> Result<Self, HamtError> {
        let mut result = self.clone();
        for value in other.iter()? {
            let value = value?;
            result = result.insert(value)?;
        }
        Ok(result)
    }

    /// Returns a new set containing only elements present in both `self` and
    /// `other`.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(N log N) where N = min(self.len(), other.len()).
    pub fn intersection(&self, other: &Self) -> Result<Self, HamtError> {
        let mut result = self.clone();
        for value in self.iter()? {
            let value = value?;
            if !other.contains(&value)? {
                (result, _) = result.remove(&value)?;
            }
        }
        Ok(result)
    }

    /// Returns a new set containing elements in `self` that are not in `other`.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(N log N) where N = `self.len()`.
    pub fn difference(&self, other: &Self) -> Result<Self, HamtError> {
        let mut result = self.clone();
        for value in self.iter()? {
            let value = value?;
            if other.contains(&value)? {
                (result, _) = result.remove(&value)?;
            }
        }
        Ok(result)
    }

    /// Returns a new set containing elements in exactly one of `self` or `other`.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(N log N).
    pub fn symmetric_difference(&self, other: &Self) -> Result<Self, HamtError> {
        let in_self_not_other = self.difference(other)?;
        let in_other_not_self = other.difference(self)?;
        in_self_not_other.union(&in_other_not_self)
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    /// Returns an iterator over all elements in arbitrary order.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] if the initial page scan fails.  Individual
    /// iteration steps also return `Result<A, HamtError>`.
    ///
    /// Time: O(N) total to iterate all elements.
    pub fn iter(&self) -> Result<HamtSetIter<'_, A, C, B>, HamtError> {
        Ok(HamtSetIter {
            inner: self.inner.iter()?,
        })
    }
}

// ---------------------------------------------------------------------------
// Clone — delegated to HamtMap
// ---------------------------------------------------------------------------

impl<A, C, B> Clone for HamtSet<A, C, B>
where
    A: Hash + Eq + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

/// An iterator over the elements of a [`HamtSet`].
pub struct HamtSetIter<'a, A, C, B>
where
    A: Hash + Eq + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    /// Underlying map iterator (key=A, value=()).
    inner: crate::hamt::HamtMapIter<'a, A, (), C, B>,
}

impl<A, C, B> Iterator for HamtSetIter<'_, A, C, B>
where
    A: Hash + Eq + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    type Item = Result<A, HamtError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|r| r.map(|(k, _)| k))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::PodCodec;
    use folio_core::{checksum::ChecksumKind, store::FolioStore};

    fn make_store() -> FolioStore<MemBackend> {
        let backend = MemBackend::new(4096, 256);
        FolioStore::create(backend, 4096, 256, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    #[test]
    fn empty_set_is_empty() {
        let s: HamtSet<u32, PodCodec> = HamtSet::new(make_store());
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(!s.contains(&42u32).unwrap());
    }

    #[test]
    fn single_insert_and_contains() {
        let s: HamtSet<u32, PodCodec> = HamtSet::new(make_store());
        let s2 = s.insert(1u32).unwrap();
        assert_eq!(s2.len(), 1);
        assert!(!s2.is_empty());
        assert!(s2.contains(&1u32).unwrap());
        assert!(!s2.contains(&2u32).unwrap());
        // Original unchanged.
        assert!(s.is_empty());
    }

    #[test]
    fn insert_duplicate_does_not_grow_set() {
        let s: HamtSet<u32, PodCodec> = HamtSet::new(make_store());
        let s1 = s.insert(1u32).unwrap();
        let s2 = s1.insert(1u32).unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s2.len(), 1); // No duplicate.
    }

    #[test]
    fn remove_present_element() {
        let s: HamtSet<u32, PodCodec> = HamtSet::new(make_store());
        let s1 = s.insert(1u32).unwrap();
        let s2 = s1.insert(2u32).unwrap();
        let (s3, removed) = s2.remove(&1u32).unwrap();
        assert!(removed);
        assert_eq!(s3.len(), 1);
        assert!(!s3.contains(&1u32).unwrap());
        assert!(s3.contains(&2u32).unwrap());
    }

    #[test]
    fn remove_absent_element() {
        let s: HamtSet<u32, PodCodec> = HamtSet::new(make_store());
        let s1 = s.insert(10u32).unwrap();
        let (s2, removed) = s1.remove(&99u32).unwrap();
        assert!(!removed);
        assert_eq!(s2.len(), 1);
    }

    #[test]
    fn remove_all_produces_empty_set() {
        let s: HamtSet<u32, PodCodec> = HamtSet::new(make_store());
        let s1 = s.insert(7u32).unwrap();
        let (s2, removed) = s1.remove(&7u32).unwrap();
        assert!(removed);
        assert!(s2.is_empty());
    }

    #[test]
    fn multiple_inserts_pod_codec() {
        let s: HamtSet<u64, PodCodec> = HamtSet::new(make_store());
        let mut current = s;
        for i in 0u64..16 {
            current = current.insert(i).unwrap();
        }
        assert_eq!(current.len(), 16);
        for i in 0u64..16 {
            assert!(current.contains(&i).unwrap());
        }
    }

    // -----------------------------------------------------------------------
    // Set operations (String keys require the serde feature for PostcardCodec)
    // -----------------------------------------------------------------------

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;
        use crate::codec::PostcardCodec;

        fn make_string_set(elems: &[&str]) -> HamtSet<String, PostcardCodec> {
            let s: HamtSet<String, PostcardCodec> = HamtSet::new(make_store());
            let mut current = s;
            for &e in elems {
                current = current.insert(e.to_string()).unwrap();
            }
            current
        }

        /// Collects a `HamtSet<String, PostcardCodec>` into a sorted `Vec<String>` for comparison.
        fn collect_sorted(s: &HamtSet<String, PostcardCodec>) -> Vec<String> {
            let mut v: Vec<String> = s.iter().unwrap().map(|r| r.unwrap()).collect();
            v.sort();
            v
        }

        #[test]
        fn union_is_superset() {
            let a = make_string_set(&["a", "b", "c"]);
            let b = make_string_set(&["b", "c", "d"]);
            let u = a.union(&b).unwrap();
            let mut got = collect_sorted(&u);
            got.sort();
            assert_eq!(got, vec!["a", "b", "c", "d"]);
        }

        #[test]
        fn union_with_empty_is_identity() {
            let a = make_string_set(&["x", "y"]);
            let empty: HamtSet<String, PostcardCodec> = HamtSet::new(make_store());
            let u = a.union(&empty).unwrap();
            assert_eq!(collect_sorted(&u), vec!["x", "y"]);
        }

        #[test]
        fn intersection_gives_common_elements() {
            let a = make_string_set(&["a", "b", "c"]);
            let b = make_string_set(&["b", "c", "d"]);
            let i = a.intersection(&b).unwrap();
            assert_eq!(collect_sorted(&i), vec!["b", "c"]);
        }

        #[test]
        fn intersection_with_disjoint_is_empty() {
            let a = make_string_set(&["a", "b"]);
            let b = make_string_set(&["c", "d"]);
            let i = a.intersection(&b).unwrap();
            assert!(i.is_empty());
        }

        #[test]
        fn difference_removes_common() {
            let a = make_string_set(&["a", "b", "c"]);
            let b = make_string_set(&["b", "c", "d"]);
            let d = a.difference(&b).unwrap();
            assert_eq!(collect_sorted(&d), vec!["a"]);
        }

        #[test]
        fn symmetric_difference_is_exclusive_elements() {
            let a = make_string_set(&["a", "b", "c"]);
            let b = make_string_set(&["b", "c", "d"]);
            let sd = a.symmetric_difference(&b).unwrap();
            assert_eq!(collect_sorted(&sd), vec!["a", "d"]);
        }

        #[test]
        fn clone_and_drop_refcounting() {
            let s = make_string_set(&["x", "y"]);
            let s_clone = s.clone();
            drop(s);
            // Clone must still work after original is dropped.
            assert!(s_clone.contains(&"x".to_string()).unwrap());
            assert!(s_clone.contains(&"y".to_string()).unwrap());
        }
    }
}
