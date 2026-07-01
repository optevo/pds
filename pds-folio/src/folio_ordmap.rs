//! Folio-backed persistent ordered map using a B+ tree.
//!
//! [`FolioOrdMap<K, V, C, B>`] is a persistent ordered map stored in a
//! [`folio_core::store::FolioStore`].  All mutations return a new
//! `FolioOrdMap` with an updated root, leaving the original unchanged
//! (path-copy semantics).  Unchanged subtrees are shared across snapshots
//! via a reference-count table.
//!
//! # Tree structure
//!
//! The map is stored as a B+ tree:
//! - Internal nodes hold separator keys and child page IDs.
//! - Leaf nodes hold sorted K-V pairs.
//! - `BTREE_ORDER = 32`: max separator keys per internal; max entries per leaf.
//!
//! # Range queries
//!
//! Range queries perform a recursive in-order tree walk filtered by the bounds.
//! The `next_leaf` pointer in leaf pages is not yet used for range optimisation
//! (planned post-G.11).
//!
//! # Codec
//!
//! The `C: Codec` type parameter controls how keys and values are serialised.
//! See [`crate::codec`] for built-in options.

use std::{
    ops::RangeBounds,
    sync::{Arc, Mutex},
};

use crate::{
    btree::{
        build_internal_node, BTreeNodePage, InternalReader, LeafBuilder, LeafReader, BTREE_ORDER,
        DISCRIMINANT_INTERNAL, DISCRIMINANT_LEAF,
    },
    codec::{CodecError, PodCodec, ValueCodec},
};
use folio_collections::refcount::PageRefcount;
use folio_core::{
    backend::{Backend, MemBackend},
    error::BackendError,
    page::PageType,
    store::FolioStore,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`FolioOrdMap`] operations.
#[derive(Debug, thiserror::Error)]
pub enum OrdMapError {
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
// OrdMapNodeStore — typed page operations
// ---------------------------------------------------------------------------

/// Shared storage layer for [`FolioOrdMap`] snapshots.
///
/// Multiple snapshots share one `OrdMapNodeStore` via
/// `Arc<Mutex<OrdMapNodeStore<B>>>`.  Reference counting tracks structural
/// sharing across path-copy mutations.
#[derive(Debug)]
pub(crate) struct OrdMapNodeStore<B> {
    /// The underlying folio page store.
    store: FolioStore<B>,
    /// Shared-page refcount table.  Absent = implicit refcount 1.
    refcounts: PageRefcount,
}

impl<B: Backend<Error = BackendError>> OrdMapNodeStore<B> {
    fn alloc_node(&mut self, page: &BTreeNodePage) -> Result<u64, OrdMapError> {
        let page_id = self.store.alloc_page(PageType::Data)?;
        self.store.write_page_data(page_id, &page.0)?;
        Ok(page_id)
    }

    fn read_node(&self, page_id: u64) -> Result<BTreeNodePage, OrdMapError> {
        let data = self.store.read_page_data(page_id)?;
        let mut page = BTreeNodePage::default();
        let len = data.len().min(512);
        page.0[..len].copy_from_slice(&data[..len]);
        Ok(page)
    }

    fn free_nodes(&mut self, page_ids: &[u64]) -> Result<(), OrdMapError> {
        self.store.free_pages(page_ids)?;
        Ok(())
    }

    fn increment_refcount(&mut self, page_id: u64) {
        self.refcounts.inc(page_id);
    }

    /// Decrements the refcount.  Returns `true` if the page should be freed.
    fn decrement_refcount(&mut self, page_id: u64) -> bool {
        self.refcounts.dec(page_id) == 0
    }
}

// ---------------------------------------------------------------------------
// FolioOrdMap
// ---------------------------------------------------------------------------

/// A persistent, folio-backed ordered map.
///
/// All mutating operations return a new `FolioOrdMap` snapshot; the original
/// is unchanged.  Shared subtrees are not copied — only the path from root to
/// the modified leaf is duplicated (O(log N) pages per operation).
///
/// # Type parameters
///
/// - `K` — key type; must be `Serialize + DeserializeOwned + Ord + Clone`
/// - `V` — value type; must be `Serialize + DeserializeOwned + Clone`
/// - `C` — codec; defaults to [`crate::codec::PodCodec`]
/// - `B` — folio backend; defaults to [`MemBackend`]
#[derive(Debug)]
pub struct FolioOrdMap<K = u64, V = u64, C = PodCodec, B = MemBackend>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
    store: Arc<Mutex<OrdMapNodeStore<B>>>,
    /// Root page ID, or `None` for an empty map.
    root: Option<u64>,
    /// Tree height: 1 = single leaf, 2 = internal + leaves, etc.  0 = empty.
    height: usize,
    /// Number of key-value pairs.
    len: usize,
    _phantom: std::marker::PhantomData<(K, V, C)>,
}

impl<K, V, C, B> FolioOrdMap<K, V, C, B>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
    /// Creates a new, empty `FolioOrdMap` backed by `store`.
    pub fn new(store: FolioStore<B>) -> Self {
        Self {
            store: Arc::new(Mutex::new(OrdMapNodeStore {
                store,
                refcounts: PageRefcount::new(),
            })),
            root: None,
            height: 0,
            len: 0,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Returns the number of key-value pairs.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Tests whether the map is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    // ---- Internal helpers ----

    fn with_root(&self, root: Option<u64>, height: usize, len: usize) -> Self {
        Self {
            store: Arc::clone(&self.store),
            root,
            height,
            len,
            _phantom: std::marker::PhantomData,
        }
    }

    fn read_page(&self, page_id: u64) -> Result<BTreeNodePage, OrdMapError> {
        self.store.lock().unwrap().read_node(page_id)
    }

    fn alloc_page(&self, page: &BTreeNodePage) -> Result<u64, OrdMapError> {
        self.store.lock().unwrap().alloc_node(page)
    }

    // ---- get ----

    /// Returns a clone of the value for `key`, or `None`.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn get(&self, key: &K) -> Result<Option<V>, OrdMapError> {
        let Some(root_id) = self.root else {
            return Ok(None);
        };
        self.get_in_node(root_id, key)
    }

    fn get_in_node(&self, node_id: u64, key: &K) -> Result<Option<V>, OrdMapError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                for i in 0..reader.count() {
                    let (k, rest) = reader.decode_key::<K, C>(i)?;
                    if &k == key {
                        let v: V = C::decode(rest)?;
                        return Ok(Some(v));
                    }
                    if &k > key {
                        break; // sorted; key not present
                    }
                }
                Ok(None)
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let child_idx = reader.find_child::<K, C>(key)?;
                let child_id = reader.child_page_id(child_idx);
                self.get_in_node(child_id, key)
            }
            d => Err(OrdMapError::BadDiscriminant(d)),
        }
    }

    /// Tests whether `key` is present.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn contains_key(&self, key: &K) -> Result<bool, OrdMapError> {
        self.get(key).map(|v| v.is_some())
    }

    // ---- first / last ----

    /// Returns the smallest key-value pair, or `None` if empty.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn first(&self) -> Result<Option<(K, V)>, OrdMapError> {
        let Some(root_id) = self.root else {
            return Ok(None);
        };
        self.leftmost_entry(root_id)
    }

    fn leftmost_entry(&self, node_id: u64) -> Result<Option<(K, V)>, OrdMapError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                if reader.count() == 0 {
                    return Ok(None);
                }
                let (k, v) = reader.decode_kv::<K, V, C>(0)?;
                Ok(Some((k, v)))
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let child_id = reader.child_page_id(0);
                self.leftmost_entry(child_id)
            }
            d => Err(OrdMapError::BadDiscriminant(d)),
        }
    }

    /// Returns the largest key-value pair, or `None` if empty.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn last(&self) -> Result<Option<(K, V)>, OrdMapError> {
        let Some(root_id) = self.root else {
            return Ok(None);
        };
        self.rightmost_entry(root_id)
    }

    fn rightmost_entry(&self, node_id: u64) -> Result<Option<(K, V)>, OrdMapError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                let count = reader.count();
                if count == 0 {
                    return Ok(None);
                }
                let (k, v) = reader.decode_kv::<K, V, C>(count - 1)?;
                Ok(Some((k, v)))
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let child_id = reader.child_page_id(reader.child_count() - 1);
                self.rightmost_entry(child_id)
            }
            d => Err(OrdMapError::BadDiscriminant(d)),
        }
    }

    // ---- insert ----

    /// Returns a new map with `key` → `value` inserted (or updated).
    ///
    /// If `key` is already present, its value is replaced.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn insert(&self, key: K, value: V) -> Result<Self, OrdMapError> {
        if self.root.is_none() {
            // Tree is empty — create a single leaf.
            let mut builder = LeafBuilder::new();
            builder.push_encoded::<K, V, C>(&key, &value)?;
            let leaf_id = self.alloc_page(&builder.finish())?;
            return Ok(self.with_root(Some(leaf_id), 1, 1));
        }

        let root_id = self.root.unwrap();
        let was_present = self.contains_key(&key)?;
        let result = self.insert_into(root_id, self.height, &key, &value)?;

        let new_len = if was_present { self.len } else { self.len + 1 };

        match result {
            InsertResult::Updated(new_root) => {
                Ok(self.with_root(Some(new_root), self.height, new_len))
            }
            InsertResult::Split {
                left,
                right,
                separator,
            } => {
                // Root split — grow the tree by one level.
                let page = build_internal_node::<K, C>(&[left, right], &[separator]);
                let new_root = self.alloc_page(&page)?;
                // left was the original root (now shared as left child).
                self.store.lock().unwrap().increment_refcount(left);
                Ok(self.with_root(Some(new_root), self.height + 1, new_len))
            }
        }
    }

    /// Recursive path-copy insert.
    ///
    /// Returns [`InsertResult::Updated`] if no split occurred, or
    /// [`InsertResult::Split`] if the node overflowed and was split.
    fn insert_into(
        &self,
        node_id: u64,
        height: usize,
        key: &K,
        value: &V,
    ) -> Result<InsertResult<K>, OrdMapError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => self.insert_into_leaf(&page, key, value),
            DISCRIMINANT_INTERNAL => self.insert_into_internal(&page, height, key, value),
            d => Err(OrdMapError::BadDiscriminant(d)),
        }
    }

    fn insert_into_leaf(
        &self,
        page: &BTreeNodePage,
        key: &K,
        value: &V,
    ) -> Result<InsertResult<K>, OrdMapError> {
        let reader = LeafReader::new(page);
        let count = reader.count();

        // Decode all existing entries and merge in the new key-value pair.
        let mut entries: Vec<(K, V)> = Vec::with_capacity(count + 1);
        let mut inserted = false;
        for i in 0..count {
            let (k, v) = reader.decode_kv::<K, V, C>(i)?;
            if !inserted && key < &k {
                entries.push((key.clone(), value.clone()));
                inserted = true;
            }
            if &k == key {
                // Overwrite: replace value, skip original.
                entries.push((key.clone(), value.clone()));
                inserted = true;
                continue;
            }
            entries.push((k, v));
        }
        if !inserted {
            entries.push((key.clone(), value.clone()));
        }

        // Try to write all entries into a single leaf.
        let mut builder = LeafBuilder::new();
        let mut all_fit = true;
        for (k, v) in &entries {
            if builder.push_encoded::<K, V, C>(k, v).is_err() {
                all_fit = false;
                break;
            }
        }

        if all_fit {
            let new_id = self.alloc_page(&builder.finish())?;
            return Ok(InsertResult::Updated(new_id));
        }

        // Leaf overflowed — split at midpoint.
        let mid = entries.len() / 2;
        let mut left_b = LeafBuilder::new();
        for (k, v) in &entries[..mid] {
            left_b
                .push_encoded::<K, V, C>(k, v)
                .expect("left split leaf overflow");
        }
        let mut right_b = LeafBuilder::new();
        for (k, v) in &entries[mid..] {
            right_b
                .push_encoded::<K, V, C>(k, v)
                .expect("right split leaf overflow");
        }
        let separator = entries[mid].0.clone();
        let left_id = self.alloc_page(&left_b.finish())?;
        let right_id = self.alloc_page(&right_b.finish())?;
        Ok(InsertResult::Split {
            left: left_id,
            right: right_id,
            separator,
        })
    }

    fn insert_into_internal(
        &self,
        page: &BTreeNodePage,
        height: usize,
        key: &K,
        value: &V,
    ) -> Result<InsertResult<K>, OrdMapError> {
        let reader = InternalReader::new(page);
        let child_idx = reader.find_child::<K, C>(key)?;
        let child_id = reader.child_page_id(child_idx);
        let child_count = reader.child_count();
        let sep_count = reader.count();

        // Collect current children and separators.
        let mut children: Vec<u64> = (0..child_count).map(|i| reader.child_page_id(i)).collect();
        let mut separators: Vec<K> = (0..sep_count)
            .map(|i| reader.decode_separator_key::<K, C>(i))
            .collect::<Result<_, _>>()?;

        let result = self.insert_into(child_id, height - 1, key, value)?;

        match result {
            InsertResult::Updated(new_child) => {
                children[child_idx] = new_child;
                let new_page = build_internal_node::<K, C>(&children, &separators);
                let new_id = self.alloc_page(&new_page)?;
                // Share all siblings (children other than the updated one).
                let mut store = self.store.lock().unwrap();
                for (i, &cid) in children.iter().enumerate() {
                    if i != child_idx {
                        store.increment_refcount(cid);
                    }
                }
                Ok(InsertResult::Updated(new_id))
            }
            InsertResult::Split {
                left,
                right,
                separator: new_sep,
            } => {
                // Replace child_idx with `left`; insert `right` at child_idx+1;
                // insert `new_sep` into separators at position child_idx.
                children[child_idx] = left;
                children.insert(child_idx + 1, right);
                separators.insert(child_idx, new_sep);

                if separators.len() <= BTREE_ORDER {
                    // Internal node absorbed the split without itself splitting.
                    let new_page = build_internal_node::<K, C>(&children, &separators);
                    let new_id = self.alloc_page(&new_page)?;
                    // Increment refcounts for pre-existing children (not left or right,
                    // which are freshly allocated from the recursive split).
                    let mut store = self.store.lock().unwrap();
                    for (i, &cid) in children.iter().enumerate() {
                        // child_idx = left (new), child_idx+1 = right (new).
                        if i != child_idx && i != child_idx + 1 {
                            store.increment_refcount(cid);
                        }
                    }
                    Ok(InsertResult::Updated(new_id))
                } else {
                    // This internal node is also full — split it.
                    // children has child_count+1 entries, separators has sep_count+1 entries.
                    // Split point: push up separators[mid]; left gets [0..mid], right gets [mid+1..].
                    let mid = separators.len() / 2;
                    let push_up = separators[mid].clone();
                    let left_children = &children[..=mid];
                    let left_seps = &separators[..mid];
                    let right_children = &children[mid + 1..];
                    let right_seps = &separators[mid + 1..];
                    let left_page = build_internal_node::<K, C>(left_children, left_seps);
                    let right_page = build_internal_node::<K, C>(right_children, right_seps);
                    let left_id = self.alloc_page(&left_page)?;
                    let right_id = self.alloc_page(&right_page)?;
                    // All pre-existing children (those that came from the old internal node,
                    // not the two new ones from the recursive split) are now distributed
                    // across the new left and right internal nodes.  They are shared between
                    // the old internal (not yet freed) and new internals → increment refcounts.
                    let new_left = children[child_idx]; // == left from recursive split
                    let new_right = children[child_idx + 1]; // == right from recursive split
                    let mut store = self.store.lock().unwrap();
                    for &cid in &children {
                        if cid != new_left && cid != new_right {
                            store.increment_refcount(cid);
                        }
                    }
                    Ok(InsertResult::Split {
                        left: left_id,
                        right: right_id,
                        separator: push_up,
                    })
                }
            }
        }
    }

    // ---- remove ----

    /// Returns a new map with `key` removed, plus the evicted value (if any).
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn remove(&self, key: &K) -> Result<(Self, Option<V>), OrdMapError> {
        let Some(root_id) = self.root else {
            return Ok((self.clone(), None));
        };

        let (new_root_opt, evicted) = self.remove_from(root_id, key)?;

        if evicted.is_none() {
            // Key was not present — return a clone.
            return Ok((self.clone(), None));
        }

        let new_len = self.len - 1;
        match new_root_opt {
            None => Ok((self.with_root(None, 0, 0), evicted)),
            Some(new_root) => {
                let (collapsed_root, collapsed_height) =
                    self.maybe_collapse_root(new_root, self.height)?;
                Ok((
                    self.with_root(Some(collapsed_root), collapsed_height, new_len),
                    evicted,
                ))
            }
        }
    }

    /// Recursive path-copy remove.
    ///
    /// Returns `(None, value)` if the node became empty after remove, or
    /// `(Some(new_node_id), value)` otherwise.  `value` is `None` if not found.
    fn remove_from(&self, node_id: u64, key: &K) -> Result<(Option<u64>, Option<V>), OrdMapError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                let count = reader.count();
                let mut entries: Vec<(K, V)> = Vec::with_capacity(count);
                let mut evicted: Option<V> = None;

                for i in 0..count {
                    let (k, v) = reader.decode_kv::<K, V, C>(i)?;
                    if &k == key {
                        evicted = Some(v);
                    } else {
                        entries.push((k, v));
                    }
                }

                if evicted.is_none() {
                    return Ok((Some(node_id), None));
                }

                if entries.is_empty() {
                    return Ok((None, evicted));
                }

                let mut builder = LeafBuilder::new();
                for (k, v) in &entries {
                    builder
                        .push_encoded::<K, V, C>(k, v)
                        .expect("rebuilt leaf overflow after remove");
                }
                let new_id = self.alloc_page(&builder.finish())?;
                Ok((Some(new_id), evicted))
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let child_idx = reader.find_child::<K, C>(key)?;
                let child_id = reader.child_page_id(child_idx);
                let child_count = reader.child_count();
                let sep_count = reader.count();

                let (new_child_opt, evicted) = self.remove_from(child_id, key)?;

                if evicted.is_none() {
                    return Ok((Some(node_id), None));
                }

                let mut children: Vec<u64> =
                    (0..child_count).map(|i| reader.child_page_id(i)).collect();
                let mut separators: Vec<K> = (0..sep_count)
                    .map(|i| reader.decode_separator_key::<K, C>(i))
                    .collect::<Result<_, _>>()?;

                match new_child_opt {
                    Some(new_child) => {
                        // Child shrank but didn't disappear.
                        children[child_idx] = new_child;
                        let new_page = build_internal_node::<K, C>(&children, &separators);
                        let new_id = self.alloc_page(&new_page)?;
                        let mut store = self.store.lock().unwrap();
                        for (i, &cid) in children.iter().enumerate() {
                            if i != child_idx {
                                store.increment_refcount(cid);
                            }
                        }
                        Ok((Some(new_id), evicted))
                    }
                    None => {
                        // Child became empty — remove it from children array.
                        children.remove(child_idx);
                        // Remove the associated separator key.
                        // Separator i sits between children[i] and children[i+1].
                        // When removing child at child_idx:
                        //   - If child_idx < sep_count, remove separator at child_idx.
                        //   - If child_idx == child_count-1, remove separator at child_idx-1.
                        if !separators.is_empty() {
                            let sep_idx = child_idx.min(separators.len() - 1);
                            separators.remove(sep_idx);
                        }

                        if children.is_empty() {
                            return Ok((None, evicted));
                        }

                        let new_page = build_internal_node::<K, C>(&children, &separators);
                        let new_id = self.alloc_page(&new_page)?;
                        let mut store = self.store.lock().unwrap();
                        for &cid in &children {
                            store.increment_refcount(cid);
                        }
                        Ok((Some(new_id), evicted))
                    }
                }
            }
            d => Err(OrdMapError::BadDiscriminant(d)),
        }
    }

    /// If `root_id` is an internal node with a single child, unwrap to that
    /// child and reduce height (applied repeatedly).
    fn maybe_collapse_root(
        &self,
        root_id: u64,
        height: usize,
    ) -> Result<(u64, usize), OrdMapError> {
        if height <= 1 {
            return Ok((root_id, height));
        }
        let page = self.read_page(root_id)?;
        if page.0[0] == DISCRIMINANT_INTERNAL {
            let reader = InternalReader::new(&page);
            if reader.child_count() == 1 {
                let child_id = reader.child_page_id(0);
                return self.maybe_collapse_root(child_id, height - 1);
            }
        }
        Ok((root_id, height))
    }

    // ---- range ----

    /// Returns all key-value pairs with keys in `bounds`, in ascending order.
    ///
    /// Uses a recursive in-order tree walk.  Time: O(log N + k) for k results.
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn range<R: RangeBounds<K>>(&self, bounds: R) -> Result<Vec<(K, V)>, OrdMapError> {
        let Some(root_id) = self.root else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        self.range_in_node(root_id, &bounds, &mut result)?;
        Ok(result)
    }

    fn range_in_node<R: RangeBounds<K>>(
        &self,
        node_id: u64,
        bounds: &R,
        result: &mut Vec<(K, V)>,
    ) -> Result<(), OrdMapError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                for i in 0..reader.count() {
                    let (k, v) = reader.decode_kv::<K, V, C>(i)?;
                    if bounds.contains(&k) {
                        result.push((k, v));
                    }
                }
                Ok(())
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let child_count = reader.child_count();
                let sep_count = reader.count();
                for child_idx in 0..child_count {
                    // Prune: skip subtrees whose entire key range falls outside bounds.
                    // A child subtree [left_sep, right_sep) contains no in-bounds keys if:
                    //   - right_sep ≤ lower_bound, or
                    //   - left_sep ≥ upper_bound.
                    // left_sep = separator[child_idx - 1] if child_idx > 0.
                    // right_sep = separator[child_idx] if child_idx < sep_count.
                    let maybe_max_key: Option<K> = if child_idx < sep_count {
                        Some(reader.decode_separator_key::<K, C>(child_idx)?)
                    } else {
                        None
                    };
                    let maybe_min_key: Option<K> = if child_idx > 0 {
                        Some(reader.decode_separator_key::<K, C>(child_idx - 1)?)
                    } else {
                        None
                    };

                    // Prune if subtree's max < start_bound.
                    if let Some(ref max_k) = maybe_max_key {
                        match bounds.start_bound() {
                            std::ops::Bound::Included(start) if max_k < start => continue,
                            std::ops::Bound::Excluded(start) if max_k <= start => continue,
                            _ => {}
                        }
                    }
                    // Prune if subtree's min >= end_bound.
                    if let Some(ref min_k) = maybe_min_key {
                        match bounds.end_bound() {
                            std::ops::Bound::Included(end) if min_k > end => continue,
                            std::ops::Bound::Excluded(end) if min_k >= end => continue,
                            _ => {}
                        }
                    }

                    let child_id = reader.child_page_id(child_idx);
                    self.range_in_node(child_id, bounds, result)?;
                }
                Ok(())
            }
            d => Err(OrdMapError::BadDiscriminant(d)),
        }
    }

    /// Returns all key-value pairs in ascending key order.
    ///
    /// Time: O(N).
    ///
    /// # Errors
    ///
    /// Returns [`OrdMapError`] on folio I/O or codec failure.
    pub fn iter(&self) -> Result<Vec<(K, V)>, OrdMapError> {
        self.range(..)
    }
}

// ---------------------------------------------------------------------------
// InsertResult — internal helper
// ---------------------------------------------------------------------------

/// Result of a recursive insert into a B+ tree node.
enum InsertResult<K> {
    /// Node was updated without splitting.
    Updated(u64),
    /// Node overflowed — split into two nodes with a separator key to push up.
    Split {
        /// Left node page ID.
        left: u64,
        /// Right node page ID.
        right: u64,
        /// Separator key to push up to the parent.
        separator: K,
    },
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

impl<K, V, C, B> Clone for FolioOrdMap<K, V, C, B>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
    /// Clones the map in O(1) by sharing the root page.
    ///
    /// The shared root has its refcount incremented.
    fn clone(&self) -> Self {
        if let Some(root_id) = self.root {
            self.store.lock().unwrap().increment_refcount(root_id);
        }
        Self {
            store: Arc::clone(&self.store),
            root: self.root,
            height: self.height,
            len: self.len,
            _phantom: std::marker::PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------

impl<K, V, C, B> Drop for FolioOrdMap<K, V, C, B>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
    fn drop(&mut self) {
        let Some(root_id) = self.root else {
            return;
        };
        // Collect all pages reachable from root, decrement refcounts, free those
        // that reach zero.
        let pages_to_free = self.collect_drop_pages(root_id);
        if !pages_to_free.is_empty() {
            let mut store = self.store.lock().unwrap();
            let _ = store.free_nodes(&pages_to_free);
        }
    }
}

impl<K, V, C, B> FolioOrdMap<K, V, C, B>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
    /// Iterative DFS to collect pages to free on drop.
    fn collect_drop_pages(&self, root_id: u64) -> Vec<u64> {
        let mut to_free = Vec::new();
        let mut stack = vec![root_id];

        while let Some(node_id) = stack.pop() {
            let should_free = self.store.lock().unwrap().decrement_refcount(node_id);
            if should_free {
                to_free.push(node_id);
                // Push children onto the stack.
                if let Ok(page) = self.store.lock().unwrap().read_node(node_id) {
                    if page.0[0] == DISCRIMINANT_INTERNAL {
                        let reader = InternalReader::new(&page);
                        for i in 0..reader.child_count() {
                            stack.push(reader.child_page_id(i));
                        }
                    }
                }
            }
        }

        to_free
    }
}

// ---------------------------------------------------------------------------
// PersistentCollection + PersistentOrdMap trait impls
// ---------------------------------------------------------------------------

impl<K, V, C, B> pds::traits::PersistentCollection for FolioOrdMap<K, V, C, B>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
}

impl<K, V, C, B> pds::traits::PersistentOrdMap<K, V> for FolioOrdMap<K, V, C, B>
where
    K: Ord + Clone,
    V: Clone,
    C: ValueCodec<K> + ValueCodec<V>,
    B: Backend<Error = BackendError>,
{
    fn get_cloned(&self, key: &K) -> Option<V> {
        FolioOrdMap::get(self, key)
            .expect("FolioOrdMap::get failed in PersistentOrdMap::get_cloned")
    }

    fn insert(&self, key: K, value: V) -> Self {
        FolioOrdMap::insert(self, key, value)
            .expect("FolioOrdMap::insert failed in PersistentOrdMap::insert")
    }

    fn remove(&self, key: &K) -> (Self, Option<V>) {
        FolioOrdMap::remove(self, key)
            .expect("FolioOrdMap::remove failed in PersistentOrdMap::remove")
    }

    fn len(&self) -> usize {
        FolioOrdMap::len(self)
    }

    fn contains_key(&self, key: &K) -> bool {
        FolioOrdMap::contains_key(self, key)
            .expect("FolioOrdMap::contains_key failed in PersistentOrdMap::contains_key")
    }

    fn first(&self) -> Option<(K, V)> {
        FolioOrdMap::first(self).expect("FolioOrdMap::first failed in PersistentOrdMap::first")
    }

    fn last(&self) -> Option<(K, V)> {
        FolioOrdMap::last(self).expect("FolioOrdMap::last failed in PersistentOrdMap::last")
    }

    fn range<R: RangeBounds<K>>(&self, bounds: R) -> impl Iterator<Item = (K, V)> + '_ {
        FolioOrdMap::range(self, bounds)
            .expect("FolioOrdMap::range failed in PersistentOrdMap::range")
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

    type TestMap = FolioOrdMap<u32, u32, PodCodec, MemBackend>;

    fn empty_map() -> TestMap {
        FolioOrdMap::new(make_store())
    }

    #[test]
    fn empty_map_properties() {
        let m = empty_map();
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
        assert_eq!(m.get(&1).unwrap(), None);
        assert!(!m.contains_key(&1).unwrap());
        assert_eq!(m.first().unwrap(), None);
        assert_eq!(m.last().unwrap(), None);
    }

    #[test]
    fn insert_single() {
        let m = empty_map().insert(1u32, 10u32).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&1).unwrap(), Some(10));
        assert_eq!(m.get(&2).unwrap(), None);
        assert_eq!(m.first().unwrap(), Some((1, 10)));
        assert_eq!(m.last().unwrap(), Some((1, 10)));
    }

    #[test]
    fn insert_multiple_sorted_order() {
        let m = empty_map()
            .insert(3u32, 30u32)
            .unwrap()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(2u32, 20u32)
            .unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&1).unwrap(), Some(10));
        assert_eq!(m.get(&2).unwrap(), Some(20));
        assert_eq!(m.get(&3).unwrap(), Some(30));
        assert_eq!(m.first().unwrap(), Some((1, 10)));
        assert_eq!(m.last().unwrap(), Some((3, 30)));
    }

    #[test]
    fn overwrite_existing_key() {
        let m = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(1u32, 99u32)
            .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&1).unwrap(), Some(99));
    }

    #[test]
    fn remove_present_key() {
        let m = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(2u32, 20u32)
            .unwrap();
        let (m2, evicted) = m.remove(&1).unwrap();
        assert_eq!(evicted, Some(10));
        assert_eq!(m2.len(), 1);
        assert_eq!(m2.get(&1).unwrap(), None);
        assert_eq!(m2.get(&2).unwrap(), Some(20));
    }

    #[test]
    fn remove_absent_key() {
        let m = empty_map().insert(1u32, 10u32).unwrap();
        let (m2, evicted) = m.remove(&99).unwrap();
        assert_eq!(evicted, None);
        assert_eq!(m2.len(), 1);
    }

    #[test]
    fn remove_from_empty() {
        let m = empty_map();
        let (m2, evicted) = m.remove(&1).unwrap();
        assert_eq!(evicted, None);
        assert_eq!(m2.len(), 0);
    }

    #[test]
    fn snapshot_isolation_insert() {
        let m0 = empty_map().insert(1u32, 10u32).unwrap();
        let m1 = m0.insert(2u32, 20u32).unwrap();
        assert_eq!(m0.len(), 1);
        assert_eq!(m0.get(&2).unwrap(), None);
        assert_eq!(m1.len(), 2);
        assert_eq!(m1.get(&2).unwrap(), Some(20));
    }

    #[test]
    fn snapshot_isolation_remove() {
        let m0 = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(2u32, 20u32)
            .unwrap();
        let (m1, _) = m0.remove(&1).unwrap();
        assert_eq!(m0.get(&1).unwrap(), Some(10));
        assert_eq!(m1.get(&1).unwrap(), None);
    }

    #[test]
    fn insert_many_keys_cross_leaf_boundary() {
        // Push more than BTREE_ORDER entries to force leaf splits.
        let n = BTREE_ORDER + 10;
        let mut m = empty_map();
        for i in 0..n {
            m = m.insert(i as u32, (i as u32) * 2).unwrap();
        }
        assert_eq!(m.len(), n);
        for i in 0..n {
            assert_eq!(
                m.get(&(i as u32)).unwrap(),
                Some((i as u32) * 2),
                "key {i} not found"
            );
        }
    }

    #[test]
    fn insert_large_n_all_present() {
        // Force multiple levels of splits.
        let n = BTREE_ORDER * BTREE_ORDER / 2;
        let mut m = empty_map();
        for i in (0..n).rev() {
            m = m.insert(i as u32, i as u32).unwrap();
        }
        assert_eq!(m.len(), n);
        for i in 0..n {
            assert_eq!(
                m.get(&(i as u32)).unwrap(),
                Some(i as u32),
                "key {i} missing after large insert"
            );
        }
    }

    fn make_store_n(pages: u64) -> FolioStore<MemBackend> {
        let backend = folio_core::backend::MemBackend::new(4096, pages);
        folio_core::store::FolioStore::create(
            backend,
            4096,
            pages,
            folio_core::checksum::ChecksumKind::Xxh3,
            true,
        )
        .unwrap()
    }

    #[test]
    fn insert_sequential_past_internal_full() {
        // Regression test: sequential inserts must succeed past the point where the root
        // internal node fills (BTREE_ORDER=32 separator keys, 33 children).  A layout bug
        // previously caused child[32] to overlap the key_offsets region, corrupting the
        // rightmost child page ID and panicking at insert #530.
        let mut m: FolioOrdMap<u32, u32, PodCodec, MemBackend> =
            FolioOrdMap::new(make_store_n(16256));
        for i in 0..600u32 {
            m = m.insert(i, i * 3).unwrap_or_else(|e| {
                panic!(
                    "insert {i} failed (len={}, height={}): {e:?}",
                    m.len(),
                    m.height
                )
            });
        }
        assert_eq!(m.len(), 600);
        // Verify all keys can be retrieved.
        for i in 0..600u32 {
            assert_eq!(
                m.get(&i).unwrap(),
                Some(i * 3),
                "key {i} missing after sequential insert past internal-full"
            );
        }
    }

    #[test]
    fn range_query_basic() {
        let m = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(3u32, 30u32)
            .unwrap()
            .insert(5u32, 50u32)
            .unwrap()
            .insert(7u32, 70u32)
            .unwrap();
        let pairs = m.range(3u32..=6u32).unwrap();
        assert_eq!(pairs, vec![(3, 30), (5, 50)]);
    }

    #[test]
    fn range_all() {
        let m = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(2u32, 20u32)
            .unwrap()
            .insert(3u32, 30u32)
            .unwrap();
        let pairs = m.iter().unwrap();
        assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    fn range_empty_result() {
        let m = empty_map().insert(1u32, 10u32).unwrap();
        let pairs = m.range(5u32..=10u32).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn first_last_many_keys() {
        let mut m = empty_map();
        for i in (0..20u32).rev() {
            m = m.insert(i, i * 3).unwrap();
        }
        assert_eq!(m.first().unwrap(), Some((0, 0)));
        assert_eq!(m.last().unwrap(), Some((19, 57)));
    }

    #[test]
    fn contains_key_basic() {
        let m = empty_map().insert(42u32, 0u32).unwrap();
        assert!(m.contains_key(&42).unwrap());
        assert!(!m.contains_key(&43).unwrap());
    }

    // --- PersistentOrdMap trait tests ---

    fn pom_get_insert<M: pds::traits::PersistentOrdMap<u32, u32>>(empty: M) {
        let m = empty.insert(10, 100);
        assert_eq!(m.get_cloned(&10), Some(100));
        assert_eq!(m.get_cloned(&11), None);
    }

    fn pom_remove<M: pds::traits::PersistentOrdMap<u32, u32>>(empty: M) {
        let m = empty.insert(1, 10).insert(2, 20);
        let (m2, evicted) = m.remove(&1);
        assert_eq!(evicted, Some(10));
        assert!(!m2.contains_key(&1));
    }

    fn pom_first_last<M: pds::traits::PersistentOrdMap<u32, u32>>(empty: M) {
        let m = empty.insert(3, 30).insert(1, 10).insert(2, 20);
        assert_eq!(m.first(), Some((1, 10)));
        assert_eq!(m.last(), Some((3, 30)));
    }

    fn pom_range<M: pds::traits::PersistentOrdMap<u32, u32>>(empty: M) {
        let m = empty
            .insert(1, 10)
            .insert(2, 20)
            .insert(3, 30)
            .insert(4, 40);
        let v: Vec<_> = m.range(2u32..=3u32).collect();
        assert_eq!(v, vec![(2, 20), (3, 30)]);
    }

    #[test]
    fn persistent_ord_map_trait() {
        pom_get_insert(FolioOrdMap::<u32, u32>::new(make_store()));
        pom_remove(FolioOrdMap::<u32, u32>::new(make_store()));
        pom_first_last(FolioOrdMap::<u32, u32>::new(make_store()));
        pom_range(FolioOrdMap::<u32, u32>::new(make_store()));
    }

    #[test]
    fn clone_is_independent() {
        let m0 = empty_map().insert(1u32, 10u32).unwrap();
        let m1 = m0.clone().insert(2u32, 20u32).unwrap();
        assert_eq!(m0.len(), 1);
        assert_eq!(m0.get(&2).unwrap(), None);
        assert_eq!(m1.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Edge cases: range with exclusive bounds, two chains, error Display
    // -----------------------------------------------------------------------

    /// range() with an exclusive upper bound excludes the boundary element.
    #[test]
    fn range_exclusive_upper_bound() {
        let m = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(3u32, 30u32)
            .unwrap()
            .insert(5u32, 50u32)
            .unwrap();
        // Exclusive upper bound excludes 5.
        let pairs = m.range(1u32..5u32).unwrap();
        assert_eq!(pairs, vec![(1, 10), (3, 30)]);
    }

    /// range() from an exclusive lower bound excludes the start element.
    #[test]
    fn range_exclusive_lower_bound() {
        let m = empty_map()
            .insert(1u32, 10u32)
            .unwrap()
            .insert(3u32, 30u32)
            .unwrap()
            .insert(5u32, 50u32)
            .unwrap();
        use std::ops::Bound;
        let pairs = m
            .range((Bound::Excluded(1u32), Bound::Included(5u32)))
            .unwrap();
        assert_eq!(pairs, vec![(3, 30), (5, 50)]);
    }

    /// Two independent chains from the same base remain independent.
    #[test]
    fn two_chains_from_same_base_are_independent() {
        let base = empty_map().insert(1u32, 10u32).unwrap();

        let a = base
            .insert(2u32, 20u32)
            .unwrap()
            .insert(3u32, 30u32)
            .unwrap();
        let b = base.insert(100u32, 1000u32).unwrap();

        // Chain A sees only its own keys.
        assert_eq!(a.get(&2).unwrap(), Some(20u32));
        assert_eq!(a.get(&100).unwrap(), None);

        // Chain B sees only its own keys.
        assert_eq!(b.get(&100).unwrap(), Some(1000u32));
        assert_eq!(b.get(&2).unwrap(), None);

        // Both see the shared base key.
        assert_eq!(a.get(&1).unwrap(), Some(10u32));
        assert_eq!(b.get(&1).unwrap(), Some(10u32));
    }

    /// In-order scan of a large map returns keys in sorted order.
    #[test]
    fn iter_returns_keys_in_sorted_order() {
        let n = BTREE_ORDER * 2; // forces internal-node splits
        let mut m = empty_map();
        // Insert in reverse order to exercise split paths thoroughly.
        for i in (0..n).rev() {
            m = m.insert(i as u32, (i as u32) * 3).unwrap();
        }
        let pairs = m.iter().unwrap();
        assert_eq!(pairs.len(), n);
        for (idx, (k, v)) in pairs.iter().enumerate() {
            assert_eq!(*k, idx as u32, "key ordering wrong at position {idx}");
            assert_eq!(*v, idx as u32 * 3);
        }
    }

    /// OrdMapError::BadDiscriminant Display format.
    #[test]
    fn bad_discriminant_error_formats() {
        let err = OrdMapError::BadDiscriminant(0xCC);
        let s = format!("{err}");
        assert!(
            s.contains("0xcc") || s.contains("204") || s.contains("CC"),
            "unexpected format: {s}"
        );
    }

    /// Cloning a map snapshot and then dropping the original leaves the clone intact.
    #[test]
    fn clone_and_drop_original_leaves_clone_intact() {
        let mut m = empty_map();
        for i in 0..20u32 {
            m = m.insert(i, i * 2).unwrap();
        }
        let snap = m.clone();
        drop(m);
        assert_eq!(snap.len(), 20);
        for i in 0..20u32 {
            assert_eq!(snap.get(&i).unwrap(), Some(i * 2));
        }
    }

    /// Remove all entries one by one from a large map ends with empty.
    #[test]
    fn remove_all_entries_produces_empty_map() {
        let n = BTREE_ORDER + 5;
        let mut m = empty_map();
        for i in 0..n {
            m = m.insert(i as u32, (i as u32) * 2).unwrap();
        }
        for i in 0..n {
            let (new_m, v) = m.remove(&(i as u32)).unwrap();
            assert_eq!(v, Some((i as u32) * 2), "wrong evicted value for key {i}");
            m = new_m;
        }
        assert!(m.is_empty());
        assert_eq!(m.first().unwrap(), None);
        assert_eq!(m.last().unwrap(), None);
    }
}
