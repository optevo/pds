//! Folio-backed persistent hash array mapped trie (HAMT).
//!
//! [`HamtMap<K, V, C, B>`] is a persistent hash map stored in a
//! [`folio_core::store::FolioStore`].  All mutations return a new `HamtMap`
//! instance with an updated root — the original is unchanged (path-copy
//! semantics).
//!
//! # Storage model
//!
//! Each HAMT node occupies one folio page.  The 512-byte [`HamtNodePage`]
//! payload is written into the page's data section.  The remaining page bytes
//! (header, checksum) are managed by folio's page format.
//!
//! A shared [`NodeStore<B>`] wraps the `FolioStore<B>` and provides
//! typed node read/write operations.  Multiple `HamtMap` snapshots (created
//! by successive inserts or removes) share the same `NodeStore` via
//! `Arc<Mutex<NodeStore<B>>>`.
//!
//! # Path-copy
//!
//! Inserting or removing an entry creates `O(log N)` new pages (one per
//! HAMT level on the path from root to the affected leaf) and returns a new
//! `HamtMap` with the new root page ID.  Unchanged subtrees are not copied;
//! their page IDs are reused in the new version.  Pages from the old version
//! are **not freed** — that is deferred to G.3 (reference counting).
//!
//! # Codec
//!
//! The `C: Codec` type parameter controls how keys and values are serialised
//! into leaf node byte arrays.  See [`crate::codec`] for built-in options.
//!
//! # Type parameters
//!
//! - `K` — key type; must be `Serialize + Hash + Eq + Clone`
//! - `V` — value type; must be `Serialize + DeserializeOwned + Clone`
//! - `C` — codec; defaults to [`crate::codec::PostcardCodec`]
//! - `B` — folio backend; defaults to [`folio_core::backend::MemBackend`]

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use folio_core::{
    backend::{Backend, MemBackend},
    error::BackendError,
    page::PageType,
    store::FolioStore,
};
use serde::{Deserialize, Serialize};

use crate::{
    codec::{Codec, CodecError, PostcardCodec},
    node::{
        build_internal, HamtNodePage, InternalReader, LeafBuilder, LeafReader, BRANCH_BITS,
        BRANCH_MASK, DISCRIMINANT_INTERNAL, DISCRIMINANT_LEAF, LEAF_CAP,
    },
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`HamtMap`] operations.
#[derive(Debug, thiserror::Error)]
pub enum HamtError {
    /// A folio store operation failed.
    #[error("folio store error: {0}")]
    Store(#[from] folio_core::error::Error),

    /// Codec encoding or decoding failed.
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    /// A page contained an unexpected discriminant byte.
    #[error("unexpected page discriminant {0:#x}")]
    BadDiscriminant(u8),
}

// ---------------------------------------------------------------------------
// NodeStore — typed page operations
// ---------------------------------------------------------------------------

/// A thin wrapper around [`FolioStore`] that reads and writes [`HamtNodePage`]
/// values as folio page payloads.
#[derive(Debug)]
pub(crate) struct NodeStore<B> {
    /// The underlying folio page store.
    pub(crate) store: FolioStore<B>,
}

impl<B: Backend<Error = BackendError>> NodeStore<B> {
    /// Allocates a new folio page and writes `page` as its payload.
    ///
    /// Returns the folio page ID of the new node.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError::Store`] on folio I/O failure.
    pub(crate) fn alloc_node(&mut self, page: &HamtNodePage) -> Result<u64, HamtError> {
        let page_id = self.store.alloc_page(PageType::Data)?;
        self.store.write_page_data(page_id, &page.0)?;
        Ok(page_id)
    }

    /// Reads the [`HamtNodePage`] stored at `page_id`.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError::Store`] if reading the page fails.
    pub(crate) fn read_node(&self, page_id: u64) -> Result<HamtNodePage, HamtError> {
        let data = self.store.read_page_data(page_id)?;
        let mut page = HamtNodePage::default();
        let len = data.len().min(crate::node::PAGE_BYTES);
        page.0[..len].copy_from_slice(&data[..len]);
        Ok(page)
    }

    /// Frees the folio page at `page_id`.
    ///
    /// No-op if `page_id` is `u64::MAX` (sentinel for no page).
    ///
    /// # Errors
    ///
    /// Returns [`HamtError::Store`] on folio I/O failure.
    #[allow(dead_code)] // used in G.3
    pub(crate) fn free_node(&mut self, page_id: u64) -> Result<(), HamtError> {
        self.store.free_page(page_id)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HamtMap
// ---------------------------------------------------------------------------

/// A persistent, folio-backed hash map.
///
/// Every mutating operation (`insert`, `remove`) returns a new `HamtMap`
/// with an updated root, leaving the original unchanged.  Shared subtrees
/// are not copied; only the path from root to the modified leaf is duplicated
/// (O(log N) pages per operation).
///
/// # Usage
///
/// ```no_run
/// use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
/// use pds_folio::hamt::HamtMap;
///
/// let backend = MemBackend::new(4096, 64);
/// let store = FolioStore::create(backend, 4096, 64, ChecksumKind::Xxh3, true).unwrap();
/// let map: HamtMap = HamtMap::new(store);
/// let map2 = map.insert("key".to_string(), 42u64).unwrap();
/// assert_eq!(map.get(&"key".to_string()).unwrap(), None);
/// assert_eq!(map2.get(&"key".to_string()).unwrap(), Some(42u64));
/// ```
#[derive(Debug)]
pub struct HamtMap<K = String, V = u64, C = PostcardCodec, B = MemBackend> {
    /// Shared node store.
    node_store: Arc<Mutex<NodeStore<B>>>,
    /// Root page ID, or `None` for an empty map.
    root: Option<u64>,
    /// Number of entries in the map.
    len: usize,
    /// Zero-sized markers for the type parameters.
    _marker: std::marker::PhantomData<(K, V, C)>,
}

impl<K, V, C, B> HamtMap<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Creates a new empty map backed by `store`.
    ///
    /// Takes ownership of the folio store.  Multiple map snapshots share the
    /// same store via an internal `Arc<Mutex<…>>`.
    #[must_use]
    pub fn new(store: FolioStore<B>) -> Self {
        Self {
            node_store: Arc::new(Mutex::new(NodeStore { store })),
            root: None,
            len: 0,
            _marker: std::marker::PhantomData,
        }
    }

    /// Creates a map snapshot that shares `node_store` and has the given
    /// `root` page ID and `len`.  Used internally by insert/remove.
    fn with_root(node_store: Arc<Mutex<NodeStore<B>>>, root: Option<u64>, len: usize) -> Self {
        Self {
            node_store,
            root,
            len,
            _marker: std::marker::PhantomData,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Returns the number of entries in the map.
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Tests whether the map is empty.
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Tests whether the map contains `key`.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N).
    pub fn contains_key(&self, key: &K) -> Result<bool, HamtError> {
        Ok(self.get(key)?.is_some())
    }

    // -----------------------------------------------------------------------
    // get
    // -----------------------------------------------------------------------

    /// Returns the value associated with `key`, or `None` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N).
    pub fn get(&self, key: &K) -> Result<Option<V>, HamtError> {
        let hash = hash_key(key);
        let store = self.node_store.lock().expect("mutex not poisoned");

        let Some(mut page_id) = self.root else {
            return Ok(None);
        };

        let mut shift = 0u32;
        loop {
            let page = store.read_node(page_id)?;
            match page.0[0] {
                DISCRIMINANT_LEAF => {
                    let reader = LeafReader::new(&page);
                    for i in 0..reader.count() {
                        if reader.key_hash(i) != hash {
                            continue;
                        }
                        let (k, v) = reader.get_entry::<K, V, C>(i)?;
                        if k == *key {
                            return Ok(Some(v));
                        }
                    }
                    return Ok(None);
                }
                DISCRIMINANT_INTERNAL => {
                    let reader = InternalReader::new(&page);
                    let bit_pos = (hash >> shift) as u32 & BRANCH_MASK;
                    shift += BRANCH_BITS;
                    match reader.child_index(bit_pos) {
                        None => return Ok(None),
                        Some(idx) => {
                            page_id = reader.child_page_id(idx);
                        }
                    }
                }
                d => return Err(HamtError::BadDiscriminant(d)),
            }
        }
    }

    // -----------------------------------------------------------------------
    // insert
    // -----------------------------------------------------------------------

    /// Returns a new map with `(key, value)` inserted.
    ///
    /// If `key` already exists, its value is replaced.  The original map is
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N) page writes.
    pub fn insert(&self, key: K, value: V) -> Result<Self, HamtError> {
        let hash = hash_key(&key);
        let mut store = self.node_store.lock().expect("mutex not poisoned");

        let (new_root, delta) = match self.root {
            None => {
                // Empty map: create a single leaf.
                let mut builder = LeafBuilder::new();
                builder.push_framed::<K, V, C>(hash, &key, &value)?;
                let page = builder.finish();
                let page_id = store.alloc_node(&page)?;
                (page_id, 1i64)
            }
            Some(root_id) => {
                let mut delta = 0i64;
                let new_root =
                    Self::insert_recursive(&mut store, root_id, &key, &value, hash, 0, &mut delta)?;
                (new_root, delta)
            }
        };

        let new_len = (self.len as i64 + delta) as usize;
        Ok(Self::with_root(
            Arc::clone(&self.node_store),
            Some(new_root),
            new_len,
        ))
    }

    /// Recursive path-copy insert.
    ///
    /// Returns the page ID of the new root of the modified subtree.
    /// `delta` is incremented by 1 if a new entry was inserted, 0 if overwritten.
    fn insert_recursive(
        store: &mut NodeStore<B>,
        page_id: u64,
        key: &K,
        value: &V,
        hash: u64,
        shift: u32,
        delta: &mut i64,
    ) -> Result<u64, HamtError> {
        let page = store.read_node(page_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                Self::insert_into_leaf(store, &page, key, value, hash, shift, delta)
            }
            DISCRIMINANT_INTERNAL => {
                Self::insert_into_internal(store, &page, key, value, hash, shift, delta)
            }
            d => Err(HamtError::BadDiscriminant(d)),
        }
    }

    /// Inserts into (or replaces in) a leaf node.
    fn insert_into_leaf(
        store: &mut NodeStore<B>,
        page: &HamtNodePage,
        key: &K,
        value: &V,
        hash: u64,
        shift: u32,
        delta: &mut i64,
    ) -> Result<u64, HamtError> {
        let reader = LeafReader::new(page);
        let count = reader.count();

        // Check for existing key (update).
        for i in 0..count {
            if reader.key_hash(i) == hash {
                let (existing_key, _) = reader.get_entry::<K, V, C>(i)?;
                if existing_key == *key {
                    // Overwrite: rebuild leaf with updated value, same count.
                    let mut builder = LeafBuilder::new();
                    for j in 0..count {
                        let (k, v) = reader.get_entry::<K, V, C>(j)?;
                        if j == i {
                            builder.push_framed::<K, V, C>(hash, key, value)?;
                        } else {
                            let h = reader.key_hash(j);
                            builder.push_framed::<K, V, C>(h, &k, &v)?;
                        }
                    }
                    // delta stays 0 (update, not insert)
                    let new_page = builder.finish();
                    return store.alloc_node(&new_page);
                }
            }
        }

        // New key — try to append to the leaf.
        if count < LEAF_CAP {
            // There might be room. Try appending.
            let mut builder = LeafBuilder::new();
            for i in 0..count {
                let (k, v) = reader.get_entry::<K, V, C>(i)?;
                let h = reader.key_hash(i);
                builder.push_framed::<K, V, C>(h, &k, &v)?;
            }
            match builder.push_framed::<K, V, C>(hash, key, value) {
                Ok(()) => {
                    *delta += 1;
                    let new_page = builder.finish();
                    return store.alloc_node(&new_page);
                }
                Err(_) => {
                    // Data section full even though count < LEAF_CAP; fall through to split.
                }
            }
        }

        // Leaf is full — split into an internal node.
        // Create a new internal node that dispatches between the old leaf entries
        // (by their (hash >> shift) bits) and the new entry.
        Self::split_leaf_and_insert(store, page, key, value, hash, shift, delta)
    }

    /// Splits a full leaf into an internal node and inserts the new entry.
    fn split_leaf_and_insert(
        store: &mut NodeStore<B>,
        leaf_page: &HamtNodePage,
        key: &K,
        value: &V,
        hash: u64,
        shift: u32,
        delta: &mut i64,
    ) -> Result<u64, HamtError> {
        let reader = LeafReader::new(leaf_page);
        let count = reader.count();

        // Collect all existing entries including the new one.
        let mut entries: Vec<(u64, K, V)> = Vec::with_capacity(count + 1);
        for i in 0..count {
            let h = reader.key_hash(i);
            let (k, v) = reader.get_entry::<K, V, C>(i)?;
            entries.push((h, k, v));
        }
        entries.push((hash, key.clone(), value.clone()));
        *delta += 1;

        // Group entries by their bit at this HAMT level.
        Self::build_trie_from_entries(store, entries, shift)
    }

    /// Recursively builds a HAMT subtrie from a list of (hash, key, value) entries.
    ///
    /// All entries share the same hash prefix up to `shift` bits.
    fn build_trie_from_entries(
        store: &mut NodeStore<B>,
        entries: Vec<(u64, K, V)>,
        shift: u32,
    ) -> Result<u64, HamtError> {
        if entries.is_empty() {
            // Should not happen — caller ensures non-empty.
            unreachable!("build_trie_from_entries called with empty entries");
        }

        // If all entries fit in one leaf (no split needed), create a leaf.
        if entries.len() <= LEAF_CAP {
            let mut builder = LeafBuilder::new();
            let mut fits = true;
            for (h, k, v) in &entries {
                if builder.push_framed::<K, V, C>(*h, k, v).is_err() {
                    fits = false;
                    break;
                }
            }
            if fits {
                let page = builder.finish();
                return store.alloc_node(&page);
            }
        }

        // Partition entries by their bit_pos at this level.
        let mut groups: std::collections::HashMap<u32, Vec<(u64, K, V)>> =
            std::collections::HashMap::new();
        for (h, k, v) in entries {
            let bit_pos = (h >> shift) as u32 & BRANCH_MASK;
            groups.entry(bit_pos).or_default().push((h, k, v));
        }

        // For each group, recursively build a child subtrie.
        let mut bitmap: u32 = 0;
        let mut child_ids_sorted: Vec<(u32, u64)> = Vec::new();

        for (bit_pos, group_entries) in groups {
            bitmap |= 1 << bit_pos;
            let child_id =
                Self::build_trie_from_entries(store, group_entries, shift + BRANCH_BITS)?;
            child_ids_sorted.push((bit_pos, child_id));
        }

        // Sort by bit_pos to match compressed child array order.
        child_ids_sorted.sort_unstable_by_key(|(bp, _)| *bp);
        let children: Vec<u64> = child_ids_sorted.into_iter().map(|(_, id)| id).collect();

        let page = build_internal(bitmap, &children);
        store.alloc_node(&page)
    }

    /// Inserts into an internal node by recursing into the appropriate child.
    fn insert_into_internal(
        store: &mut NodeStore<B>,
        page: &HamtNodePage,
        key: &K,
        value: &V,
        hash: u64,
        shift: u32,
        delta: &mut i64,
    ) -> Result<u64, HamtError> {
        let reader = InternalReader::new(page);
        let bitmap = reader.bitmap();
        let bit_pos = (hash >> shift) as u32 & BRANCH_MASK;
        let new_shift = shift + BRANCH_BITS;

        let child_count = reader.child_count();
        let mut new_children: Vec<u64> =
            (0..child_count).map(|i| reader.child_page_id(i)).collect();

        match reader.child_index(bit_pos) {
            Some(idx) => {
                // Recurse into existing child.
                let child_id = reader.child_page_id(idx);
                let new_child_id =
                    Self::insert_recursive(store, child_id, key, value, hash, new_shift, delta)?;
                new_children[idx] = new_child_id;
            }
            None => {
                // New child branch: create a single-entry leaf.
                let mut builder = LeafBuilder::new();
                builder.push_framed::<K, V, C>(hash, key, value)?;
                let leaf_page = builder.finish();
                let new_child_id = store.alloc_node(&leaf_page)?;

                // Insert into compressed children array at the correct position.
                let insert_pos = (bitmap & ((1 << bit_pos) - 1)).count_ones() as usize;
                new_children.insert(insert_pos, new_child_id);
                let new_bitmap = bitmap | (1 << bit_pos);

                *delta += 1;
                let new_page = build_internal(new_bitmap, &new_children);
                return store.alloc_node(&new_page);
            }
        }

        let new_page = build_internal(bitmap, &new_children);
        store.alloc_node(&new_page)
    }

    // -----------------------------------------------------------------------
    // remove
    // -----------------------------------------------------------------------

    /// Returns a new map with `key` removed.
    ///
    /// If `key` is absent, returns the original map (as a new snapshot with
    /// the same root).  The removed value is returned alongside.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] on folio I/O or codec failure.
    ///
    /// Time: O(log N) page writes.
    pub fn remove(&self, key: &K) -> Result<(Self, Option<V>), HamtError> {
        let hash = hash_key(key);
        let mut store = self.node_store.lock().expect("mutex not poisoned");

        let Some(root_id) = self.root else {
            return Ok((Self::with_root(Arc::clone(&self.node_store), None, 0), None));
        };

        let mut removed_value: Option<V> = None;
        let new_root_opt =
            Self::remove_recursive(&mut store, root_id, key, hash, 0, &mut removed_value)?;

        if removed_value.is_none() {
            // Key was absent; return self (same snapshot).
            return Ok((
                Self::with_root(Arc::clone(&self.node_store), self.root, self.len),
                None,
            ));
        }

        let new_len = self.len.saturating_sub(1);
        Ok((
            Self::with_root(Arc::clone(&self.node_store), new_root_opt, new_len),
            removed_value,
        ))
    }

    /// Recursive path-copy remove.
    ///
    /// Returns `Some(new_page_id)` for the updated subtree, or `None` if the
    /// subtree is now empty.  `removed` is set to `Some(value)` if the key
    /// was found and removed.
    fn remove_recursive(
        store: &mut NodeStore<B>,
        page_id: u64,
        key: &K,
        hash: u64,
        shift: u32,
        removed: &mut Option<V>,
    ) -> Result<Option<u64>, HamtError> {
        let page = store.read_node(page_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => Self::remove_from_leaf(store, &page, key, hash, removed),
            DISCRIMINANT_INTERNAL => {
                Self::remove_from_internal(store, &page, key, hash, shift, removed)
            }
            d => Err(HamtError::BadDiscriminant(d)),
        }
    }

    /// Removes `key` from a leaf node.
    fn remove_from_leaf(
        store: &mut NodeStore<B>,
        page: &HamtNodePage,
        key: &K,
        hash: u64,
        removed: &mut Option<V>,
    ) -> Result<Option<u64>, HamtError> {
        let reader = LeafReader::new(page);
        let count = reader.count();

        for i in 0..count {
            if reader.key_hash(i) != hash {
                continue;
            }
            let (k, v) = reader.get_entry::<K, V, C>(i)?;
            if k != *key {
                continue;
            }
            // Found — remove entry i.
            *removed = Some(v);
            if count == 1 {
                // Leaf becomes empty → subtree is gone.
                return Ok(None);
            }
            // Rebuild leaf without entry i.
            let mut builder = LeafBuilder::new();
            for j in 0..count {
                if j == i {
                    continue;
                }
                let (k2, v2) = reader.get_entry::<K, V, C>(j)?;
                let h2 = reader.key_hash(j);
                builder.push_framed::<K, V, C>(h2, &k2, &v2)?;
            }
            let new_page = builder.finish();
            let new_id = store.alloc_node(&new_page)?;
            return Ok(Some(new_id));
        }

        // Key not found.
        Ok(Some(page_id_from_page(store, page)?))
    }

    /// Removes `key` from an internal node.
    fn remove_from_internal(
        store: &mut NodeStore<B>,
        page: &HamtNodePage,
        key: &K,
        hash: u64,
        shift: u32,
        removed: &mut Option<V>,
    ) -> Result<Option<u64>, HamtError> {
        let reader = InternalReader::new(page);
        let bitmap = reader.bitmap();
        let bit_pos = (hash >> shift) as u32 & BRANCH_MASK;
        let new_shift = shift + BRANCH_BITS;

        let child_count = reader.child_count();
        let Some(idx) = reader.child_index(bit_pos) else {
            // Child not present → key absent, nothing to remove.
            return Ok(Some(page_id_of_current(store, page)?));
        };

        let child_id = reader.child_page_id(idx);
        let new_child_opt = Self::remove_recursive(store, child_id, key, hash, new_shift, removed)?;

        if removed.is_none() {
            // Key was absent in the child subtree.
            return Ok(Some(page_id_of_current(store, page)?));
        }

        let mut new_children: Vec<u64> =
            (0..child_count).map(|i| reader.child_page_id(i)).collect();

        match new_child_opt {
            None => {
                // Child subtree is now empty — remove from bitmap and children.
                new_children.remove(idx);
                let new_bitmap = bitmap & !(1 << bit_pos);
                if new_children.is_empty() {
                    return Ok(None);
                }
                let new_page = build_internal(new_bitmap, &new_children);
                Ok(Some(store.alloc_node(&new_page)?))
            }
            Some(new_child_id) => {
                new_children[idx] = new_child_id;
                let new_page = build_internal(bitmap, &new_children);
                Ok(Some(store.alloc_node(&new_page)?))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Computes a 64-bit hash of `key`.
fn hash_key<K: Hash>(key: &K) -> u64 {
    // Use std's DefaultHasher. For production use, a stronger hasher
    // (e.g. xxHash3) would be preferable; this is sufficient for G.2 tests.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Re-allocates the page to obtain a stable page ID.
///
/// Used when a leaf/internal node is returned unchanged (key absent) and we
/// need to return a page ID.  Since the caller already holds the page ID we
/// just return it directly — but we need access to the original ID.
///
/// This helper is used only in remove when the key is absent; it allocates
/// a new copy of the page and returns the new ID.
fn page_id_from_page<B: Backend<Error = BackendError>>(
    store: &mut NodeStore<B>,
    page: &HamtNodePage,
) -> Result<u64, HamtError> {
    // Allocate a new copy of the unchanged page.
    // This is wasteful but correct for G.2 (no path sharing on unchanged subtrees).
    // G.3 will improve this with ref-counted sharing.
    store.alloc_node(page)
}

/// Same as `page_id_from_page` — see its doc comment.
fn page_id_of_current<B: Backend<Error = BackendError>>(
    store: &mut NodeStore<B>,
    page: &HamtNodePage,
) -> Result<u64, HamtError> {
    store.alloc_node(page)
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

    // -----------------------------------------------------------------------
    // PostcardCodec (String keys)
    // -----------------------------------------------------------------------

    #[test]
    fn empty_map_is_empty() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(map.get(&"foo".to_string()).unwrap(), None);
        assert!(!map.contains_key(&"foo".to_string()).unwrap());
    }

    #[test]
    fn single_insert_and_get() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let map2 = map.insert("hello".to_string(), 42u32).unwrap();
        assert_eq!(map2.len(), 1);
        assert!(!map2.is_empty());
        assert_eq!(map2.get(&"hello".to_string()).unwrap(), Some(42u32));
        assert_eq!(map2.get(&"world".to_string()).unwrap(), None);
        // Original unchanged.
        assert!(map.is_empty());
    }

    #[test]
    fn multiple_inserts_postcard() {
        let map: HamtMap<String, u64> = HamtMap::new(make_store());
        let mut current = map;
        for i in 0u64..32 {
            current = current.insert(format!("key{i}"), i * 10).unwrap();
        }
        assert_eq!(current.len(), 32);
        for i in 0u64..32 {
            let val = current.get(&format!("key{i}")).unwrap();
            assert_eq!(val, Some(i * 10), "missing key{i}");
        }
    }

    #[test]
    fn overwrite_existing_key() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("k".to_string(), 1u32).unwrap();
        let m2 = m1.insert("k".to_string(), 99u32).unwrap();
        assert_eq!(m1.get(&"k".to_string()).unwrap(), Some(1u32));
        assert_eq!(m2.get(&"k".to_string()).unwrap(), Some(99u32));
        assert_eq!(m1.len(), 1);
        assert_eq!(m2.len(), 1); // overwrite, not insert
    }

    #[test]
    fn remove_present_key() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("a".to_string(), 10u32).unwrap();
        let m2 = m1.insert("b".to_string(), 20u32).unwrap();
        let (m3, removed) = m2.remove(&"a".to_string()).unwrap();
        assert_eq!(removed, Some(10u32));
        assert_eq!(m3.len(), 1);
        assert_eq!(m3.get(&"a".to_string()).unwrap(), None);
        assert_eq!(m3.get(&"b".to_string()).unwrap(), Some(20u32));
        // Original unchanged.
        assert_eq!(m2.get(&"a".to_string()).unwrap(), Some(10u32));
    }

    #[test]
    fn remove_absent_key() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("a".to_string(), 1u32).unwrap();
        let (m2, removed) = m1.remove(&"b".to_string()).unwrap();
        assert_eq!(removed, None);
        assert_eq!(m2.len(), 1);
        assert_eq!(m2.get(&"a".to_string()).unwrap(), Some(1u32));
    }

    #[test]
    fn remove_all_entries_produces_empty_map() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("x".to_string(), 1u32).unwrap();
        let (m2, _) = m1.remove(&"x".to_string()).unwrap();
        assert!(m2.is_empty());
        assert_eq!(m2.root, None);
    }

    // -----------------------------------------------------------------------
    // PodCodec (u64 keys and values)
    // -----------------------------------------------------------------------

    #[test]
    fn pod_codec_insert_and_get_u64() {
        let map: HamtMap<u64, u64, PodCodec> = HamtMap::new(make_store());
        let mut current = map;
        for i in 0u64..16 {
            current = current.insert(i, i * i).unwrap();
        }
        assert_eq!(current.len(), 16);
        for i in 0u64..16 {
            assert_eq!(current.get(&i).unwrap(), Some(i * i));
        }
    }

    #[test]
    fn pod_codec_overwrite_and_remove() {
        let map: HamtMap<u64, u64, PodCodec> = HamtMap::new(make_store());
        let m1 = map.insert(100u64, 1u64).unwrap();
        let m2 = m1.insert(100u64, 999u64).unwrap();
        assert_eq!(m2.get(&100u64).unwrap(), Some(999u64));
        let (m3, v) = m2.remove(&100u64).unwrap();
        assert_eq!(v, Some(999u64));
        assert!(m3.is_empty());
    }

    // -----------------------------------------------------------------------
    // contains_key
    // -----------------------------------------------------------------------

    #[test]
    fn contains_key_is_consistent_with_get() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("present".to_string(), 1u32).unwrap();
        assert!(m1.contains_key(&"present".to_string()).unwrap());
        assert!(!m1.contains_key(&"absent".to_string()).unwrap());
    }
}
