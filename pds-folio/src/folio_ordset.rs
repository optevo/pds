//! Folio-backed persistent ordered set — a thin newtype over [`FolioOrdMap`].
//!
//! [`FolioOrdSet<A, C, B>`] wraps `FolioOrdMap<A, (), C, B>`, delegating all
//! operations to the underlying map.  The unit value `()` is zero-sized and
//! encodes to zero bytes with [`crate::codec::PodCodec`], so there is no space
//! overhead over a plain key store.

use std::ops::RangeBounds;

use crate::{
    codec::{PodCodec, ValueCodec},
    folio_ordmap::{FolioOrdMap, OrdMapError},
};
use folio_core::{
    backend::{Backend, MemBackend},
    error::BackendError,
    store::FolioStore,
};

/// A persistent, folio-backed ordered set.
///
/// Wraps [`FolioOrdMap<A, (), C, B>`].  All mutations return a new
/// `FolioOrdSet` snapshot, leaving the original unchanged.
///
/// # Type parameters
///
/// - `A` — element type; must be `Ord + Clone`
/// - `C` — codec; defaults to [`PodCodec`]
/// - `B` — folio backend; defaults to [`MemBackend`]
#[derive(Debug)]
pub struct FolioOrdSet<A = (), C = PodCodec, B = MemBackend>
where
    A: Ord + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    inner: FolioOrdMap<A, (), C, B>,
}

impl<A, C, B> FolioOrdSet<A, C, B>
where
    A: Ord + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    /// Creates a new, empty `FolioOrdSet` backed by `store`.
    pub fn new(store: FolioStore<B>) -> Self {
        Self {
            inner: FolioOrdMap::new(store),
        }
    }

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Tests whether `value` is a member.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn contains(&self, value: &A) -> Result<bool, OrdMapError> {
        self.inner.contains_key(value)
    }

    /// Returns a new set with `value` inserted.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn insert(&self, value: A) -> Result<Self, OrdMapError> {
        let inner = self.inner.insert(value, ())?;
        Ok(Self { inner })
    }

    /// Returns a new set with `value` removed.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn remove(&self, value: &A) -> Result<Self, OrdMapError> {
        let (inner, _) = self.inner.remove(value)?;
        Ok(Self { inner })
    }

    /// Returns the smallest element, or `None` if empty.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn first(&self) -> Result<Option<A>, OrdMapError> {
        self.inner.first().map(|opt| opt.map(|(k, _)| k))
    }

    /// Returns the largest element, or `None` if empty.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn last(&self) -> Result<Option<A>, OrdMapError> {
        self.inner.last().map(|opt| opt.map(|(k, _)| k))
    }

    /// Returns all elements in `bounds`, in ascending order.
    ///
    /// Time: O(log N + k) for k results.
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn range<R: RangeBounds<A>>(&self, bounds: R) -> Result<Vec<A>, OrdMapError> {
        self.inner
            .range(bounds)
            .map(|pairs| pairs.into_iter().map(|(k, _)| k).collect())
    }

    /// Returns all elements in ascending order.
    ///
    /// Time: O(N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn iter(&self) -> Result<Vec<A>, OrdMapError> {
        self.range(..)
    }
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

impl<A, C, B> Clone for FolioOrdSet<A, C, B>
where
    A: Ord + Clone,
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
// PersistentCollection + PersistentOrdSet trait impls
// ---------------------------------------------------------------------------

impl<A, C, B> pds::traits::PersistentCollection for FolioOrdSet<A, C, B>
where
    A: Ord + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
}

impl<A, C, B> pds::traits::PersistentOrdSet<A> for FolioOrdSet<A, C, B>
where
    A: Ord + Clone,
    C: ValueCodec<A> + ValueCodec<()>,
    B: Backend<Error = BackendError>,
{
    fn contains(&self, value: &A) -> bool {
        FolioOrdSet::contains(self, value)
            .expect("FolioOrdSet::contains failed in PersistentOrdSet::contains")
    }

    fn insert(&self, value: A) -> Self {
        FolioOrdSet::insert(self, value)
            .expect("FolioOrdSet::insert failed in PersistentOrdSet::insert")
    }

    fn remove(&self, value: &A) -> Self {
        FolioOrdSet::remove(self, value)
            .expect("FolioOrdSet::remove failed in PersistentOrdSet::remove")
    }

    fn len(&self) -> usize {
        FolioOrdSet::len(self)
    }

    fn first(&self) -> Option<A> {
        FolioOrdSet::first(self).expect("FolioOrdSet::first failed in PersistentOrdSet::first")
    }

    fn last(&self) -> Option<A> {
        FolioOrdSet::last(self).expect("FolioOrdSet::last failed in PersistentOrdSet::last")
    }

    fn range<R: RangeBounds<A>>(&self, bounds: R) -> impl Iterator<Item = A> + '_ {
        FolioOrdSet::range(self, bounds)
            .expect("FolioOrdSet::range failed in PersistentOrdSet::range")
            .into_iter()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::PodCodec;
    use folio_core::{backend::MemBackend, checksum::ChecksumKind};

    fn make_store() -> FolioStore<MemBackend> {
        let backend = MemBackend::new(4096, 512);
        FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    type TestSet = FolioOrdSet<u32, PodCodec, MemBackend>;

    fn empty_set() -> TestSet {
        FolioOrdSet::new(make_store())
    }

    #[test]
    fn empty_set_properties() {
        let s = empty_set();
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
        assert!(!s.contains(&1).unwrap());
        assert_eq!(s.first().unwrap(), None);
        assert_eq!(s.last().unwrap(), None);
    }

    #[test]
    fn insert_and_contains() {
        let s = empty_set().insert(5u32).unwrap();
        assert_eq!(s.len(), 1);
        assert!(s.contains(&5).unwrap());
        assert!(!s.contains(&6).unwrap());
    }

    #[test]
    fn insert_multiple() {
        let s = empty_set()
            .insert(3u32)
            .unwrap()
            .insert(1u32)
            .unwrap()
            .insert(2u32)
            .unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s.first().unwrap(), Some(1));
        assert_eq!(s.last().unwrap(), Some(3));
        assert_eq!(s.iter().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn remove_present() {
        let s = empty_set().insert(1u32).unwrap().insert(2u32).unwrap();
        let s2 = s.remove(&1).unwrap();
        assert_eq!(s2.len(), 1);
        assert!(!s2.contains(&1).unwrap());
        assert!(s2.contains(&2).unwrap());
    }

    #[test]
    fn remove_absent() {
        let s = empty_set().insert(1u32).unwrap();
        let s2 = s.remove(&99).unwrap();
        assert_eq!(s2.len(), 1);
        assert!(s2.contains(&1).unwrap());
    }

    #[test]
    fn snapshot_isolation() {
        let s0 = empty_set().insert(1u32).unwrap();
        let s1 = s0.insert(2u32).unwrap();
        assert!(!s0.contains(&2).unwrap());
        assert!(s1.contains(&2).unwrap());
    }

    #[test]
    fn range_query() {
        let s = empty_set()
            .insert(1u32)
            .unwrap()
            .insert(3u32)
            .unwrap()
            .insert(5u32)
            .unwrap()
            .insert(7u32)
            .unwrap();
        assert_eq!(s.range(3u32..=6u32).unwrap(), vec![3, 5]);
    }

    #[test]
    fn persistent_ord_set_trait() {
        fn pos_insert<S: pds::traits::PersistentOrdSet<u32>>(empty: S) {
            let s = empty.insert(1).insert(2);
            assert!(s.contains(&1));
            assert!(s.contains(&2));
            assert!(!s.contains(&3));
        }

        fn pos_remove<S: pds::traits::PersistentOrdSet<u32>>(empty: S) {
            let s = empty.insert(1).insert(2);
            let s2 = s.remove(&1);
            assert!(!s2.contains(&1));
            assert!(s2.contains(&2));
        }

        fn pos_first_last<S: pds::traits::PersistentOrdSet<u32>>(empty: S) {
            let s = empty.insert(5).insert(1).insert(3);
            assert_eq!(s.first(), Some(1));
            assert_eq!(s.last(), Some(5));
        }

        fn pos_range<S: pds::traits::PersistentOrdSet<u32>>(empty: S) {
            let s = empty.insert(1).insert(2).insert(3).insert(4);
            let v: Vec<_> = s.range(2u32..=3u32).collect();
            assert_eq!(v, vec![2, 3]);
        }

        pos_insert(FolioOrdSet::<u32>::new(make_store()));
        pos_remove(FolioOrdSet::<u32>::new(make_store()));
        pos_first_last(FolioOrdSet::<u32>::new(make_store()));
        pos_range(FolioOrdSet::<u32>::new(make_store()));
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    /// Clone then drop the original — the clone must still be readable.
    #[test]
    fn clone_and_drop_original_leaves_clone_intact() {
        let s = empty_set()
            .insert(1u32)
            .unwrap()
            .insert(2u32)
            .unwrap()
            .insert(3u32)
            .unwrap();
        let snap = s.clone();
        drop(s);
        assert_eq!(snap.len(), 3);
        assert!(snap.contains(&1).unwrap());
        assert!(snap.contains(&2).unwrap());
        assert!(snap.contains(&3).unwrap());
    }

    /// first() and last() on a single-element set both return that element.
    #[test]
    fn first_last_single_element() {
        let s = empty_set().insert(42u32).unwrap();
        assert_eq!(s.first().unwrap(), Some(42));
        assert_eq!(s.last().unwrap(), Some(42));
    }

    /// Inserting a duplicate does not change the set.
    #[test]
    fn insert_duplicate_is_noop() {
        let s = empty_set().insert(7u32).unwrap();
        let s2 = s.insert(7u32).unwrap();
        assert_eq!(s2.len(), 1);
        assert!(s2.contains(&7).unwrap());
    }

    /// Two independent chains from the same base remain independent.
    #[test]
    fn two_chains_from_same_base_are_independent() {
        let base = empty_set().insert(1u32).unwrap();
        let a = base.insert(2u32).unwrap();
        let b = base.insert(100u32).unwrap();
        assert!(a.contains(&2).unwrap());
        assert!(!a.contains(&100).unwrap());
        assert!(b.contains(&100).unwrap());
        assert!(!b.contains(&2).unwrap());
        assert!(a.contains(&1).unwrap());
        assert!(b.contains(&1).unwrap());
    }

    /// Removing all elements one by one ends with an empty set.
    #[test]
    fn remove_all_elements_produces_empty() {
        let s = empty_set()
            .insert(1u32)
            .unwrap()
            .insert(2u32)
            .unwrap()
            .insert(3u32)
            .unwrap();
        let s = s.remove(&2).unwrap();
        let s = s.remove(&1).unwrap();
        let s = s.remove(&3).unwrap();
        assert!(s.is_empty());
        assert_eq!(s.first().unwrap(), None);
        assert_eq!(s.last().unwrap(), None);
    }
}
