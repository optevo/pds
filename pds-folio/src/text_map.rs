//! [`FolioTextMap`]: a persistent, path-copy map from keys to UTF-8 text buffers.
//!
//! Each value is an independent [`FolioRope`] owned exclusively by its map entry and
//! wrapped in `Arc` so that `OrdMap`'s structural sharing can clone values in O(1).
//!
//! # Design
//!
//! The map is a thin newtype over `pds::OrdMap<K, Arc<FolioRope<B>>>`:
//!
//! - No shared page store, no `Mutex`, no custom `Drop`.
//! - `Clone` is O(1): `OrdMap` bumps the root's refcount; each `Arc<FolioRope<B>>`
//!   bumps the rope's refcount.
//! - `Drop` is automatic: `OrdMap` node refcounts + `Arc<FolioRope<B>>` refcounts
//!   handle everything.
//! - Each rope is fully independent; lifecycle is entirely managed by `Arc`.
//!
//! # Snapshot semantics
//!
//! `insert` and `remove` return a **new** map snapshot.  The original map is
//! unchanged — all snapshots may coexist and are read-consistently.

use std::sync::Arc;

use folio_core::{
    backend::Backend, checksum::ChecksumKind, error::BackendError, store::FolioStore,
};
use folio_rope::{FolioRope, RopeError};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`FolioTextMap`] operations.
#[derive(Debug, thiserror::Error)]
pub enum TextMapError {
    /// A rope operation failed.
    #[error("rope error: {0}")]
    Rope(#[from] RopeError),
}

// ---------------------------------------------------------------------------
// FolioTextMap
// ---------------------------------------------------------------------------

/// A persistent, path-copy map from keys of type `K` to UTF-8 text buffers.
///
/// Values are [`FolioRope`] instances wrapped in `Arc`.  Each rope is
/// independent — there is no shared page store between entries.  `Clone` is
/// O(1) via `OrdMap`'s structural sharing.  Mutation (`insert`, `remove`) is
/// O(log N) for the map update plus O(n) for rope content materialisation.
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
///
/// let map: FolioTextMap<String> = FolioTextMap::new();
/// let map2 = map.insert("hello".to_string(), "world").unwrap();
/// let rope = map2.get(&"hello".to_string()).unwrap();
/// assert_eq!(format!("{rope}"), "world");
/// ```
pub struct FolioTextMap<K, B = folio_core::backend::MemBackend>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
    /// Persistent ordered map: each entry stores an `Arc`-wrapped `FolioRope`.
    inner: pds::OrdMap<K, Arc<FolioRope<B>>>,
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

impl<K, B> FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError> + Default,
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
}

// ---------------------------------------------------------------------------
// Read operations (no B: Default required)
// ---------------------------------------------------------------------------

impl<K, B> FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError>,
{
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

    /// Returns the `Arc<FolioRope<B>>` for `key`, or `None` if absent.
    ///
    /// The returned `Arc` keeps the rope alive independently of the map.
    ///
    /// Time: O(log N).
    #[must_use]
    pub fn get(&self, key: &K) -> Option<Arc<FolioRope<B>>> {
        self.inner.get(key).cloned()
    }
}

// ---------------------------------------------------------------------------
// Mutation operations (require B: Default to create stores)
// ---------------------------------------------------------------------------

impl<K, B> FolioTextMap<K, B>
where
    K: Clone + Ord,
    B: Backend<Error = BackendError> + Default,
{
    /// Returns a new map snapshot with `key` mapped to `text`.
    ///
    /// Creates a fresh `FolioRope` for the value; the old rope for `key` (if
    /// any) is unaffected and remains live in snapshots that still reference it.
    ///
    /// # Errors
    ///
    /// Returns [`TextMapError::Rope`] if the rope insert operation fails.
    ///
    /// Time: O(log N) for the map update; O(n) for rope materialisation.
    pub fn insert(&self, key: K, text: &str) -> Result<Self, TextMapError> {
        // Each rope has its own independent store — no shared state.
        let store = FolioStore::create(B::default(), 4096, 0, ChecksumKind::Xxh3, true)
            .expect("FolioStore::create with default backend must succeed");
        let mut rope = FolioRope::new(store);
        rope.insert(0, text)?;
        let new_inner = self.inner.update(key, Arc::new(rope));
        Ok(Self { inner: new_inner })
    }

    /// Returns a new map snapshot with `key` removed, or `None` if `key` was absent.
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
    /// The `OrdMap` root refcount is bumped; each `Arc<FolioRope<B>>` refcount
    /// is bumped.  No pages are allocated or copied.
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
    B: Backend<Error = BackendError> + Default,
{
    /// Creates an empty text map backed by a default backend.
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
    use folio_core::backend::MemBackend;

    type TextMap = FolioTextMap<String, MemBackend>;

    #[test]
    fn text_map_empty_len_is_zero() {
        let map = TextMap::new();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
    }

    #[test]
    fn text_map_insert_and_get() {
        let map = TextMap::new();
        let map2 = map.insert("key".to_string(), "hello").unwrap();
        let rope = map2.get(&"key".to_string()).unwrap();
        assert_eq!(format!("{rope}"), "hello");
    }

    #[test]
    fn text_map_insert_multiple_keys() {
        let map = TextMap::new();
        let map2 = map.insert("a".to_string(), "alpha").unwrap();
        let map3 = map2.insert("b".to_string(), "beta").unwrap();
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
        let map2 = map.insert("k".to_string(), "first").unwrap();
        let map3 = map2.insert("k".to_string(), "second").unwrap();
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
        let map2 = map.insert("x".to_string(), "value").unwrap();
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
        let map2 = map.insert("k".to_string(), "v").unwrap();
        // original map is unaffected
        assert!(map.get(&"k".to_string()).is_none());
        assert!(map2.get(&"k".to_string()).is_some());
    }

    #[test]
    fn text_map_snapshot_isolation_remove_does_not_affect_original() {
        let map = TextMap::new();
        let map2 = map.insert("k".to_string(), "v").unwrap();
        let map3 = map2.remove(&"k".to_string()).unwrap();
        // map2 still sees the value
        assert!(map2.get(&"k".to_string()).is_some());
        // map3 does not
        assert!(map3.get(&"k".to_string()).is_none());
    }

    #[test]
    fn text_map_clone_is_independent() {
        let map = TextMap::new();
        let map2 = map.insert("k".to_string(), "v").unwrap();
        let map3 = map2.clone();
        assert_eq!(map2.len(), map3.len());
        let r2 = map2.get(&"k".to_string()).unwrap();
        let r3 = map3.get(&"k".to_string()).unwrap();
        assert_eq!(format!("{r2}"), format!("{r3}"));
    }

    #[test]
    fn text_map_debug_format() {
        let map = TextMap::new();
        let dbg = format!("{map:?}");
        assert!(dbg.contains("FolioTextMap"));
    }
}
