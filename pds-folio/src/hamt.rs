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
//! A shared `NodeStore<B>` wraps the `FolioStore<B>` and provides
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

use std::collections::HashMap;
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
///
/// # Reference counting
///
/// The `refcounts` table tracks structural sharing across `HamtMap` snapshots.
/// A refcount of 1 is implicit — page IDs absent from the table have exactly
/// one live reference.  Only refcounts ≥ 2 are stored.
#[derive(Debug)]
pub(crate) struct NodeStore<B> {
    /// The underlying folio page store.
    pub(crate) store: FolioStore<B>,
    /// Shared-page refcount table.  Absent = refcount 1 (unique).
    pub(crate) refcounts: HashMap<u64, u32>,
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
    /// # Errors
    ///
    /// Returns [`HamtError::Store`] on folio I/O failure.
    #[allow(dead_code)] // used in future stages
    pub(crate) fn free_node(&mut self, page_id: u64) -> Result<(), HamtError> {
        self.store.free_page(page_id)?;
        Ok(())
    }

    /// Batch-frees a set of folio pages.
    ///
    /// Uses `free_pages` for a single WAL commit rather than one per page.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError::Store`] on folio I/O failure.
    pub(crate) fn free_nodes(&mut self, page_ids: &[u64]) -> Result<(), HamtError> {
        self.store.free_pages(page_ids)?;
        Ok(())
    }

    /// Increments the reference count for `page_id`.
    ///
    /// If `page_id` was at refcount 1 (implicit — absent from the table),
    /// it is inserted with refcount 2.
    pub(crate) fn increment_refcount(&mut self, page_id: u64) {
        let entry = self.refcounts.entry(page_id).or_insert(1);
        *entry += 1;
    }

    /// Decrements the reference count for `page_id`.
    ///
    /// Returns `true` if the page should be freed (refcount reached 0).
    /// After decrementing from 2 → 1 the entry is removed (refcount 1 is implicit).
    pub(crate) fn decrement_refcount(&mut self, page_id: u64) -> bool {
        match self.refcounts.get_mut(&page_id) {
            None => {
                // Implicit refcount 1 — dropping the last reference.
                true
            }
            Some(count) => {
                *count -= 1;
                if *count <= 1 {
                    self.refcounts.remove(&page_id);
                    // If it hit 1, the page is still live (held by one owner).
                    // If it hit 0 that would be a bug — we decrement from ≥2.
                    // We only free when decrementing from implicit 1.
                    false
                } else {
                    false
                }
            }
        }
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
pub struct HamtMap<K = String, V = u64, C = PostcardCodec, B = MemBackend>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
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
            node_store: Arc::new(Mutex::new(NodeStore {
                store,
                refcounts: HashMap::new(),
            })),
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

    /// Returns the folio page ID of the HAMT root node, or `None` for an empty map.
    ///
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
    ///
    /// When a new internal node is written that reuses existing child page IDs,
    /// those children have their refcounts incremented — they are now referenced
    /// by both the old internal node and the new one.  When the old internal node
    /// is freed (during `Drop`), `collect_pages_to_free` will decrement the
    /// children's refcounts back down.
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

                // All children except new_children[idx] are reused from the old
                // internal node — increment their refcounts.
                for (i, &cid) in new_children.iter().enumerate() {
                    if i != idx {
                        store.increment_refcount(cid);
                    }
                }
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

                // All old children are reused — increment their refcounts.
                // The newly inserted child (new_child_id) is not in new_children yet
                // (we just inserted it), but it was freshly allocated so refcount=1.
                // We iterate over all children except the new one.
                for (i, &cid) in new_children.iter().enumerate() {
                    if i != insert_pos {
                        store.increment_refcount(cid);
                    }
                }

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
    /// subtree is now empty.  When the key is absent the original `page_id`
    /// is returned unchanged (no new page allocation).  `removed` is set to
    /// `Some(value)` if the key was found and removed.
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
            DISCRIMINANT_LEAF => Self::remove_from_leaf(store, page_id, &page, key, hash, removed),
            DISCRIMINANT_INTERNAL => {
                Self::remove_from_internal(store, page_id, &page, key, hash, shift, removed)
            }
            d => Err(HamtError::BadDiscriminant(d)),
        }
    }

    /// Removes `key` from a leaf node.
    ///
    /// Returns the original `page_id` unchanged when the key is absent.
    fn remove_from_leaf(
        store: &mut NodeStore<B>,
        page_id: u64,
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

        // Key not found — return original page ID, no allocation needed.
        Ok(Some(page_id))
    }

    /// Removes `key` from an internal node.
    ///
    /// Returns the original `page_id` unchanged when the key is absent.
    fn remove_from_internal(
        store: &mut NodeStore<B>,
        page_id: u64,
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
            // Child not present → key absent, return original page ID unchanged.
            return Ok(Some(page_id));
        };

        let child_id = reader.child_page_id(idx);
        let new_child_opt = Self::remove_recursive(store, child_id, key, hash, new_shift, removed)?;

        if removed.is_none() {
            // Key was absent in the child subtree — return original page ID unchanged.
            return Ok(Some(page_id));
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
                // All remaining children are reused from the old internal node —
                // they are now shared between the old and new internal nodes.
                for &cid in &new_children {
                    store.increment_refcount(cid);
                }
                let new_page = build_internal(new_bitmap, &new_children);
                Ok(Some(store.alloc_node(&new_page)?))
            }
            Some(new_child_id) => {
                // Path-copy: the child at `idx` was replaced.  All other children
                // are reused — increment their refcounts.
                new_children[idx] = new_child_id;
                for (i, &cid) in new_children.iter().enumerate() {
                    if i != idx {
                        store.increment_refcount(cid);
                    }
                }
                let new_page = build_internal(bitmap, &new_children);
                Ok(Some(store.alloc_node(&new_page)?))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Iteration (G.4+)
    // -----------------------------------------------------------------------

    /// Returns an iterator over all `(K, V)` pairs in arbitrary order.
    ///
    /// The iterator reads pages lazily from the folio store.  Individual
    /// elements are returned as `Result<(K, V), HamtError>` to propagate
    /// any I/O or codec errors encountered during traversal.
    ///
    /// # Errors
    ///
    /// Returns [`HamtError`] if the initial root page read fails.
    ///
    /// Time: O(N) total to iterate all entries.
    pub fn iter(&self) -> Result<HamtMapIter<'_, K, V, C, B>, HamtError> {
        let store = self.node_store.lock().expect("mutex not poisoned");
        // Pre-load the initial work stack: start with the root if present.
        let work_stack: Vec<u64> = match self.root {
            None => Vec::new(),
            Some(root_id) => vec![root_id],
        };
        // Release the lock — the iterator will re-acquire it per page.
        drop(store);
        Ok(HamtMapIter {
            node_store: Arc::clone(&self.node_store),
            // work_stack holds page IDs of internal nodes yet to be expanded.
            work_stack,
            // pending_entries holds decoded entries from the current leaf.
            pending_entries: Vec::new(),
            _marker: std::marker::PhantomData,
        })
    }

    // -----------------------------------------------------------------------
    // Reference counting helpers (G.3)
    // -----------------------------------------------------------------------

    /// Increments the refcount of `page_id` (called by `Clone`).
    fn increment_root_refcount(&self) {
        if let Some(root_id) = self.root {
            let mut store = self.node_store.lock().expect("mutex not poisoned");
            store.increment_refcount(root_id);
        }
    }

    /// Iteratively walks the subtree rooted at `page_id`, collecting all page
    /// IDs that should be freed (i.e. their refcount reaches 0 after decrement).
    ///
    /// Uses an explicit work-stack instead of recursion to avoid stack overflow
    /// on deep trees.  Called by `Drop`.
    fn collect_pages_to_free(
        store: &mut NodeStore<B>,
        root_id: u64,
        to_free: &mut Vec<u64>,
    ) -> Result<(), HamtError> {
        // Work-stack of page IDs remaining to process.
        let mut stack: Vec<u64> = vec![root_id];

        while let Some(page_id) = stack.pop() {
            // Decrement this node's refcount.
            let should_free = store.decrement_refcount(page_id);

            if should_free {
                // Read the page to find its children before queuing them.
                let page = store.read_node(page_id)?;
                if page.0[0] == DISCRIMINANT_INTERNAL {
                    let reader = InternalReader::new(&page);
                    let child_count = reader.child_count();
                    for i in 0..child_count {
                        stack.push(reader.child_page_id(i));
                    }
                }
                // Mark this page for batch-free.
                to_free.push(page_id);
            }
            // If should_free is false, the page is still referenced by another
            // snapshot — leave it and its subtree intact.
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Clone — O(1) refcount increment
// ---------------------------------------------------------------------------

impl<K, V, C, B> Clone for HamtMap<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Clones this map snapshot in O(1) by incrementing the root page's
    /// refcount.  Both the original and the clone share all underlying pages.
    fn clone(&self) -> Self {
        self.increment_root_refcount();
        Self {
            node_store: Arc::clone(&self.node_store),
            root: self.root,
            len: self.len,
            _marker: std::marker::PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// Drop — recursive refcount decrement and batch free
// ---------------------------------------------------------------------------

impl<K, V, C, B> Drop for HamtMap<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Drops this map snapshot.
    ///
    /// Decrements refcounts for all reachable pages.  Any page whose refcount
    /// reaches zero is batch-freed via a single `free_pages` call to minimise
    /// WAL commits.  Pages still referenced by other snapshots are left intact.
    fn drop(&mut self) {
        let Some(root_id) = self.root else {
            return; // Empty map — nothing to free.
        };

        let mut store = self.node_store.lock().expect("mutex not poisoned");
        let mut to_free: Vec<u64> = Vec::new();

        // Walk the tree and collect pages whose refcount drops to zero.
        // Ignore errors during drop — we cannot propagate them.
        let _ = HamtMap::<K, V, C, B>::collect_pages_to_free(&mut store, root_id, &mut to_free);

        if !to_free.is_empty() {
            let _ = store.free_nodes(&to_free);
        }
    }
}

// ---------------------------------------------------------------------------
// HamtMapIter — lazy tree traversal
// ---------------------------------------------------------------------------

/// An iterator over `(K, V)` pairs in a [`HamtMap`].
///
/// Traverses the HAMT in an iterative depth-first order using an explicit
/// work-stack.  Leaf entries are buffered in `pending_entries` so that the
/// folio lock is acquired only once per leaf page.
pub struct HamtMapIter<'a, K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Shared reference to the node store.
    node_store: Arc<Mutex<NodeStore<B>>>,
    /// Page IDs yet to be processed (DFS stack — internal nodes pushed last).
    work_stack: Vec<u64>,
    /// Decoded entries from the most recently visited leaf, in reverse order
    /// (so `pop` gives the next entry).
    pending_entries: Vec<(K, V)>,
    /// Phantom lifetime and type markers.
    _marker: std::marker::PhantomData<(&'a (), C)>,
}

impl<K, V, C, B> std::fmt::Debug for HamtMapIter<'_, K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone + std::fmt::Debug,
    V: Serialize + for<'de> Deserialize<'de> + Clone + std::fmt::Debug,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HamtMapIter")
            .field("work_stack_len", &self.work_stack.len())
            .field("pending_entries_len", &self.pending_entries.len())
            .finish()
    }
}

impl<K, V, C, B> Iterator for HamtMapIter<'_, K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    type Item = Result<(K, V), HamtError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Return a buffered entry if one is available.
            if let Some(entry) = self.pending_entries.pop() {
                return Some(Ok(entry));
            }

            // Pop the next page from the work stack.
            let page_id = self.work_stack.pop()?;

            let store = self.node_store.lock().expect("mutex not poisoned");
            let page = match store.read_node(page_id) {
                Ok(p) => p,
                Err(e) => return Some(Err(e)),
            };
            drop(store); // Release the lock before decoding entries.

            match page.0[0] {
                DISCRIMINANT_LEAF => {
                    let reader = LeafReader::new(&page);
                    let count = reader.count();
                    // Decode all entries into `pending_entries` (reversed so pop gives index 0).
                    for i in (0..count).rev() {
                        match reader.get_entry::<K, V, C>(i) {
                            Ok(kv) => self.pending_entries.push(kv),
                            Err(e) => return Some(Err(HamtError::Codec(e))),
                        }
                    }
                    // Loop back to return the first buffered entry.
                }
                DISCRIMINANT_INTERNAL => {
                    let reader = InternalReader::new(&page);
                    let child_count = reader.child_count();
                    // Push children onto the work stack (left-to-right, so first child
                    // is processed first via LIFO).
                    for i in (0..child_count).rev() {
                        self.work_stack.push(reader.child_page_id(i));
                    }
                }
                d => {
                    return Some(Err(HamtError::BadDiscriminant(d)));
                }
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

    // -----------------------------------------------------------------------
    // G.3 — Reference counting and Drop
    // -----------------------------------------------------------------------

    /// Cloning a map shares the underlying pages.  The clone can still read
    /// all entries after the original is dropped.
    #[test]
    fn clone_shares_pages_original_drop_leaves_clone_intact() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("a".to_string(), 1u32).unwrap();
        let m2 = m1.insert("b".to_string(), 2u32).unwrap();

        // Clone m2; m2 and m2_clone share all pages.
        let m2_clone = m2.clone();
        assert_eq!(m2_clone.len(), 2);

        // Drop the original; the clone must still be accessible.
        drop(m2);

        // All entries remain readable via the clone.
        assert_eq!(m2_clone.get(&"a".to_string()).unwrap(), Some(1u32));
        assert_eq!(m2_clone.get(&"b".to_string()).unwrap(), Some(2u32));
    }

    /// After both the original and all clones are dropped, the refcount table
    /// should be empty (all pages freed).
    #[test]
    fn all_clones_dropped_refcounts_empty() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("x".to_string(), 42u32).unwrap();
        let clone1 = m1.clone();
        let clone2 = m1.clone();

        // All three share the same root.
        {
            let store = m1.node_store.lock().unwrap();
            // refcount for root should be 3 (1 implicit + 2 increments = stored as 3).
            // The stored value is `Some(3)` since it is > 1.
            if let Some(root_id) = m1.root {
                assert_eq!(store.refcounts.get(&root_id), Some(&3u32));
            }
        }

        drop(m1);

        // After dropping m1, refcount should decrease to 2 (stored as 2).
        {
            let store = clone1.node_store.lock().unwrap();
            if let Some(root_id) = clone1.root {
                assert_eq!(store.refcounts.get(&root_id), Some(&2u32));
            }
        }

        drop(clone1);

        // After dropping clone1, refcount decreases to 1 — entry removed (implicit).
        {
            let store = clone2.node_store.lock().unwrap();
            if let Some(root_id) = clone2.root {
                assert_eq!(store.refcounts.get(&root_id), None);
            }
        }

        // Drop last clone — the page should be freed.
        drop(clone2);
    }

    /// Multiple clones can be independently read after all intermediate drops.
    #[test]
    fn multiple_snapshots_independent() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("k1".to_string(), 1u32).unwrap();
        let m2 = m1.insert("k2".to_string(), 2u32).unwrap();
        let m3 = m2.insert("k3".to_string(), 3u32).unwrap();

        // Clone intermediate snapshots.
        let snap1 = m1.clone();
        let snap3 = m3.clone();

        // Drop m1, m2, m3 in sequence.
        drop(m1);
        drop(m2);
        drop(m3);

        // snap1 sees only k1.
        assert_eq!(snap1.get(&"k1".to_string()).unwrap(), Some(1u32));
        assert_eq!(snap1.get(&"k2".to_string()).unwrap(), None);

        // snap3 sees all three keys.
        assert_eq!(snap3.get(&"k1".to_string()).unwrap(), Some(1u32));
        assert_eq!(snap3.get(&"k2".to_string()).unwrap(), Some(2u32));
        assert_eq!(snap3.get(&"k3".to_string()).unwrap(), Some(3u32));
    }

    /// Dropping an empty map is a no-op (no pages to free).
    #[test]
    fn drop_empty_map_is_noop() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        drop(map); // Must not panic.
    }

    /// Removing an absent key returns the original snapshot — the remove path
    /// no longer re-allocates pages.
    #[test]
    fn remove_absent_key_no_extra_alloc() {
        let map: HamtMap<String, u32> = HamtMap::new(make_store());
        let m1 = map.insert("a".to_string(), 1u32).unwrap();
        let root_before = m1.root;

        let (m2, removed) = m1.remove(&"absent".to_string()).unwrap();
        assert_eq!(removed, None);
        // The root page ID must be the same — no new page was allocated.
        assert_eq!(m2.root, root_before);
    }
}
