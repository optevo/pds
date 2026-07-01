//! [`FolioTextMap`]: a persistent, path-copy map from keys to UTF-8 text buffers.
//!
//! Each value is an independent [`FolioRope`] stored directly in a
//! `pds::OrdMap`.  `FolioRope::clone()` is already O(1) (it bumps an internal
//! `Arc` strong count), so no additional `Arc` wrapper is needed.
//!
//! # Design
//!
//! The map is a thin newtype over `pds::OrdMap<K, FolioRope<B>>`:
//!
//! - No shared page store, no `Arc` wrapper on values, no custom `Drop`.
//! - `Clone` is O(1): `OrdMap` bumps the root's refcount; each `FolioRope`
//!   clone bumps its internal page-set `Arc` refcount.
//! - `Drop` is automatic: `OrdMap` node refcounts + `FolioRope::Drop`
//!   (`Arc::try_unwrap` on its internal `page_set`) handle everything.
//! - Each rope is fully independent; lifecycle is entirely managed by
//!   `FolioRope`'s own `Drop` impl.
//!
//! # Snapshot semantics
//!
//! `insert` and `remove` return a **new** map snapshot.  The original map is
//! unchanged — all snapshots may coexist and are read-consistently.

use folio_core::{backend::Backend, error::BackendError, store::FolioStore};
use folio_rope::{FolioRope, RopeError};

// ---------------------------------------------------------------------------
// FolioTextMap
// ---------------------------------------------------------------------------

/// A persistent, path-copy map from keys of type `K` to UTF-8 text buffers.
///
/// Values are [`FolioRope`] instances stored directly in a `pds::OrdMap`.
/// Each rope is independent — there is no shared page store between entries.
/// `Clone` is O(1) via `OrdMap`'s structural sharing; `FolioRope::clone()`
/// is also O(1) so no extra `Arc` wrapper is needed.
///
/// Mutation (`insert`, `remove`) is O(log N) for the map update plus O(n)
/// for rope content materialisation.
///
/// # Type parameters
///
/// - `K` — key type; must implement `Clone + Ord`.
/// - `B` — folio backend; defaults to [`folio_core::backend::MemBackend`].
///
/// # Examples
///
/// ```rust
/// use pds_folio::text_map::FolioTextMap;
/// use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
///
/// let map: FolioTextMap<String> = FolioTextMap::new();
/// let store = FolioStore::create(
///     MemBackend::new(4096, 64), 4096, 0, ChecksumKind::Xxh3, true,
/// ).unwrap();
/// let map2 = map.insert("hello".to_string(), store, "world").unwrap();
/// let rope = map2.get(&"hello".to_string()).unwrap();
/// assert_eq!(format!("{rope}"), "world");
/// ```
pub struct FolioTextMap<K, B = folio_core::backend::MemBackend>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
    /// Persistent ordered map: each entry stores a `FolioRope` directly.
    ///
    /// `FolioRope::clone()` is O(1), so `OrdMap`'s path-copy structural sharing
    /// can clone values directly without an additional `Arc` wrapper.
    inner: pds::OrdMap<K, FolioRope<B>>,
}

// ---------------------------------------------------------------------------
// Constructors and operations
// ---------------------------------------------------------------------------

impl<K, B> FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
    /// Creates an empty text map.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use pds_folio::text_map::FolioTextMap;
    ///
    /// let map: FolioTextMap<String> = FolioTextMap::new();
    /// assert!(map.is_empty());
    /// ```
    ///
    /// Time: O(1).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: pds::OrdMap::new(),
        }
    }

    /// Returns the number of entries in the map.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the map contains no entries.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns a clone of the [`FolioRope`] for `key`, or `None` if absent.
    ///
    /// The clone is O(1): it bumps the rope's internal `Arc` page-set refcount
    /// without allocating or copying any folio pages.
    ///
    /// Time: O(log N) for the map lookup; O(1) for the rope clone.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<FolioRope<B>> {
        self.inner.get(key).cloned()
    }

    /// Returns a new map snapshot with `key` mapped to `text`.
    ///
    /// Creates a fresh [`FolioRope`] using the given `store`; the old rope for
    /// `key` (if any) is unaffected and remains live in snapshots that still
    /// reference it.
    ///
    /// The caller supplies a `FolioStore<B>` because the backend type `B` may
    /// not implement [`Default`] (e.g. `MemBackend` requires explicit parameters).
    /// Typically, create the store with:
    ///
    /// ```rust,no_run
    /// # use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
    /// let store = FolioStore::create(
    ///     MemBackend::new(4096, 64), 4096, 0, ChecksumKind::Xxh3, true,
    /// ).unwrap();
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`RopeError`] if the rope insert operation fails.
    ///
    /// Time: O(log N) for the map update; O(n) for rope materialisation.
    pub fn insert(&self, key: K, store: FolioStore<B>, text: &str) -> Result<Self, RopeError> {
        let mut rope = FolioRope::new(store);
        rope.insert(0, text)?;
        let new_inner = self.inner.update(key, rope);
        Ok(Self { inner: new_inner })
    }

    /// Returns a new map snapshot with `key` removed, or `None` if `key` was absent.
    ///
    /// The returned snapshot shares all other entries with the original via
    /// `OrdMap`'s structural sharing.
    ///
    /// Time: O(log N).
    #[must_use]
    pub fn remove(&self, key: &K) -> Option<Self> {
        if !self.inner.contains_key(key) {
            return None;
        }
        Some(Self {
            inner: self.inner.without(key),
        })
    }
}

// ---------------------------------------------------------------------------
// Trait impls
// ---------------------------------------------------------------------------

impl<K, B> Clone for FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
    /// Clones the map in O(1).
    ///
    /// The `OrdMap` root refcount is bumped; each `FolioRope` clone bumps its
    /// internal page-set `Arc` refcount.  No pages are allocated or copied.
    ///
    /// Time: O(1).
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K, B> std::fmt::Debug for FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolioTextMap")
            .field("len", &self.len())
            .finish()
    }
}

impl<K, B> Default for FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
    /// Creates an empty text map.
    ///
    /// Time: O(1).
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};

    type TextMap = FolioTextMap<String, MemBackend>;

    /// Creates a fresh in-memory `FolioStore` for use in `insert` calls.
    fn new_store() -> FolioStore<MemBackend> {
        FolioStore::create(MemBackend::new(4096, 64), 4096, 0, ChecksumKind::Xxh3, true).unwrap()
    }

    #[test]
    fn text_map_empty_len_is_zero() {
        let map = TextMap::new();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
    }

    #[test]
    fn text_map_insert_and_get() {
        let map = TextMap::new();
        let map2 = map.insert("key".to_string(), new_store(), "hello").unwrap();
        let rope = map2.get(&"key".to_string()).unwrap();
        assert_eq!(format!("{rope}"), "hello");
    }

    #[test]
    fn text_map_insert_multiple_keys() {
        let map = TextMap::new();
        let map2 = map.insert("a".to_string(), new_store(), "alpha").unwrap();
        let map3 = map2.insert("b".to_string(), new_store(), "beta").unwrap();
        assert_eq!(map3.len(), 2);
        let a = map3.get(&"a".to_string()).unwrap();
        let b = map3.get(&"b".to_string()).unwrap();
        assert_eq!(format!("{a}"), "alpha");
        assert_eq!(format!("{b}"), "beta");
    }

    #[test]
    fn text_map_get_absent_returns_none() {
        let map = TextMap::new();
        assert!(map.get(&"missing".to_string()).is_none());
    }

    #[test]
    fn text_map_overwrite_key() {
        let map = TextMap::new();
        let map2 = map.insert("k".to_string(), new_store(), "first").unwrap();
        let map3 = map2.insert("k".to_string(), new_store(), "second").unwrap();
        // map3 sees "second"
        let rope = map3.get(&"k".to_string()).unwrap();
        assert_eq!(format!("{rope}"), "second");
        // map2 still sees "first"
        let rope2 = map2.get(&"k".to_string()).unwrap();
        assert_eq!(format!("{rope2}"), "first");
    }

    #[test]
    fn text_map_remove_returns_new_snapshot() {
        let map = TextMap::new();
        let map2 = map.insert("x".to_string(), new_store(), "value").unwrap();
        let map3 = map2.remove(&"x".to_string()).unwrap();
        assert_eq!(map3.len(), 0);
        assert!(map3.get(&"x".to_string()).is_none());
        // map2 still sees the value
        assert!(map2.get(&"x".to_string()).is_some());
    }

    #[test]
    fn text_map_remove_absent_returns_none() {
        let map = TextMap::new();
        assert!(map.remove(&"no_such".to_string()).is_none());
    }

    #[test]
    fn text_map_snapshot_isolation_insert_does_not_affect_original() {
        let map = TextMap::new();
        let map2 = map.insert("k".to_string(), new_store(), "v").unwrap();
        // original map is unaffected
        assert!(map.get(&"k".to_string()).is_none());
        assert!(map2.get(&"k".to_string()).is_some());
    }

    #[test]
    fn text_map_snapshot_isolation_remove_does_not_affect_original() {
        let map = TextMap::new();
        let map2 = map.insert("k".to_string(), new_store(), "v").unwrap();
        let map3 = map2.remove(&"k".to_string()).unwrap();
        // map2 still sees the value
        assert!(map2.get(&"k".to_string()).is_some());
        // map3 does not
        assert!(map3.get(&"k".to_string()).is_none());
    }

    #[test]
    fn text_map_clone_is_independent() {
        let map = TextMap::new();
        let map2 = map.insert("k".to_string(), new_store(), "v").unwrap();
        let map3 = map2.clone();
        assert_eq!(map2.len(), map3.len());
        let r2 = map2.get(&"k".to_string()).unwrap();
        let r3 = map3.get(&"k".to_string()).unwrap();
        assert_eq!(format!("{r2}"), format!("{r3}"));
    }
}
