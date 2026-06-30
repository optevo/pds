//! Folio-backed persistent vector using an RRB-tree node layout.
//!
//! [`FolioVector<A, C, B>`] is a persistent vector stored in a
//! [`folio_core::store::FolioStore`].  All mutations return a new `FolioVector`
//! instance with an updated root — the original is unchanged (path-copy
//! semantics).
//!
//! # Storage model
//!
//! Each RRB-tree node occupies one folio page.  The 512-byte
//! [`VectorNodePage`] payload is written into the page's data section.
//! Multiple `FolioVector` snapshots share the same `Arc<Mutex<VectorNodeStore<B>>>`.
//!
//! # Tree structure
//!
//! The vector is stored as a complete or nearly-complete tree of depth ≥ 1.
//!
//! - An **empty** vector has `root = None` and `len = 0`.
//! - A vector with ≤ [`BRANCHING_FACTOR`] elements has a single leaf at the root.
//! - Larger vectors have a tree of internal nodes with leaves at the bottom.
//!
//! `depth` is the height of the tree: 1 = single leaf, 2 = internal + leaves, etc.
//!
//! # Codec
//!
//! The `C: Codec` type parameter controls how elements are serialised into leaf
//! node byte arrays.  See [`crate::codec`] for built-in options.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use folio_core::{
    backend::{Backend, MemBackend},
    error::BackendError,
    page::PageType,
    store::FolioStore,
};
use serde::{Deserialize, Serialize};

use crate::{
    codec::{Codec, CodecError, PostcardCodec},
    vector::{
        build_internal, InternalReader, LeafBuilder, LeafReader, VectorNodePage, BRANCHING_FACTOR,
        DISCRIMINANT_INTERNAL, DISCRIMINANT_LEAF,
    },
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`FolioVector`] operations.
#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    /// A folio store operation failed.
    #[error("folio store error: {0}")]
    Store(#[from] folio_core::error::Error),

    /// Codec encoding or decoding failed.
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    /// A page contained an unexpected discriminant byte.
    #[error("unexpected page discriminant {0:#x}")]
    BadDiscriminant(u8),

    /// Index was out of bounds.
    #[error("index {0} out of bounds (len={1})")]
    OutOfBounds(usize, usize),
}

// ---------------------------------------------------------------------------
// VectorNodeStore — typed page operations
// ---------------------------------------------------------------------------

/// A thin wrapper around [`FolioStore`] for reading/writing [`VectorNodePage`] values.
///
/// Multiple `FolioVector` snapshots share one `VectorNodeStore` via
/// `Arc<Mutex<VectorNodeStore<B>>>`.  Reference counting tracks structural sharing.
#[derive(Debug)]
pub(crate) struct VectorNodeStore<B> {
    /// The underlying folio page store.
    pub(crate) store: FolioStore<B>,
    /// Shared-page refcount table.  Absent = refcount 1 (unique).
    pub(crate) refcounts: HashMap<u64, u32>,
}

impl<B: Backend<Error = BackendError>> VectorNodeStore<B> {
    /// Allocates a new folio page and writes `page` as its payload.
    pub(crate) fn alloc_node(&mut self, page: &VectorNodePage) -> Result<u64, VectorError> {
        let page_id = self.store.alloc_page(PageType::Data)?;
        self.store.write_page_data(page_id, &page.0)?;
        Ok(page_id)
    }

    /// Reads the [`VectorNodePage`] stored at `page_id`.
    pub(crate) fn read_node(&self, page_id: u64) -> Result<VectorNodePage, VectorError> {
        let data = self.store.read_page_data(page_id)?;
        let mut page = VectorNodePage::default();
        let len = data.len().min(crate::vector::PAGE_BYTES);
        page.0[..len].copy_from_slice(&data[..len]);
        Ok(page)
    }

    /// Batch-frees a set of folio pages.
    pub(crate) fn free_nodes(&mut self, page_ids: &[u64]) -> Result<(), VectorError> {
        self.store.free_pages(page_ids)?;
        Ok(())
    }

    /// Increments the reference count for `page_id`.
    pub(crate) fn increment_refcount(&mut self, page_id: u64) {
        let entry = self.refcounts.entry(page_id).or_insert(1);
        *entry += 1;
    }

    /// Decrements the reference count for `page_id`.
    ///
    /// Returns `true` if the page should be freed (was last unique reference).
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
                    false
                } else {
                    false
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FolioVector
// ---------------------------------------------------------------------------

/// A persistent, folio-backed vector.
///
/// Every mutating operation (`push_back`, `push_front`, `update`, etc.) returns
/// a new `FolioVector` with an updated root, leaving the original unchanged.
/// Shared subtrees are not copied; only the path from root to the modified
/// leaf is duplicated (O(log_32 N) pages per operation).
///
/// # Type parameters
///
/// - `A` — element type; must be `Serialize + DeserializeOwned + Clone`
/// - `C` — codec; defaults to [`PostcardCodec`]
/// - `B` — folio backend; defaults to [`MemBackend`]
#[derive(Debug)]
pub struct FolioVector<A = (), C = PostcardCodec, B = MemBackend>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Shared node store.
    store: Arc<Mutex<VectorNodeStore<B>>>,
    /// Root page ID, or `None` for an empty vector.
    root: Option<u64>,
    /// Number of elements.
    len: usize,
    /// Tree depth: 1 = single leaf, 2 = internal + leaves, etc.
    /// 0 means empty (root = None).
    depth: usize,
    _phantom: std::marker::PhantomData<(A, C)>,
}

impl<A, C, B> FolioVector<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Creates a new empty `FolioVector` backed by `store`.
    ///
    /// All snapshots created from this vector share the same underlying store.
    #[must_use]
    pub fn new(store: FolioStore<B>) -> Self {
        let node_store = VectorNodeStore {
            store,
            refcounts: HashMap::new(),
        };
        Self {
            store: Arc::new(Mutex::new(node_store)),
            root: None,
            len: 0,
            depth: 0,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Tests whether the vector is empty.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    // ---------------------------------------------------------------------------
    // Internal helpers
    // ---------------------------------------------------------------------------

    /// Returns a copy of `self` with fields updated.
    fn with_root(&self, root: Option<u64>, len: usize, depth: usize) -> Self {
        Self {
            store: Arc::clone(&self.store),
            root,
            len,
            depth,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Reads a node page from the store.
    fn read_page(&self, page_id: u64) -> Result<VectorNodePage, VectorError> {
        self.store.lock().unwrap().read_node(page_id)
    }

    /// Allocates and writes a node page, returning its ID.
    fn alloc_page(&self, page: &VectorNodePage) -> Result<u64, VectorError> {
        self.store.lock().unwrap().alloc_node(page)
    }

    // ---------------------------------------------------------------------------
    // get
    // ---------------------------------------------------------------------------

    /// Returns a clone of the element at `index`, or `None` if out of bounds.
    ///
    /// Time: O(log_32 N).
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn get(&self, index: usize) -> Result<Option<A>, VectorError> {
        if index >= self.len {
            return Ok(None);
        }
        let root_id = match self.root {
            Some(id) => id,
            None => return Ok(None),
        };
        let elem = self.get_at_node(root_id, index)?;
        Ok(Some(elem))
    }

    /// Navigates from `node_id` to find element at `pos`.
    ///
    /// `pos` is relative to the start of this subtree.  The discriminant byte
    /// determines whether this is a leaf or internal node.
    fn get_at_node(&self, node_id: u64, pos: usize) -> Result<A, VectorError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                if pos >= reader.count() {
                    // Shouldn't happen if tree is consistent.
                    return Err(VectorError::OutOfBounds(pos, reader.count()));
                }
                let elem = reader.get_entry::<A, C>(pos)?;
                Ok(elem)
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                match reader.find_child(pos) {
                    Some((child_idx, local_pos)) => {
                        let child_id = reader.child_page_id(child_idx);
                        self.get_at_node(child_id, local_pos)
                    }
                    None => Err(VectorError::OutOfBounds(pos, self.len)),
                }
            }
            d => Err(VectorError::BadDiscriminant(d)),
        }
    }

    // ---------------------------------------------------------------------------
    // update
    // ---------------------------------------------------------------------------

    /// Returns a new vector with element at `index` replaced by `value`.
    ///
    /// Returns the original vector unchanged if `index` is out of bounds.
    ///
    /// Time: O(log_32 N).
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn update(&self, index: usize, value: A) -> Result<Self, VectorError> {
        if index >= self.len {
            return Ok(self.with_root(self.root, self.len, self.depth));
        }
        let root_id = match self.root {
            Some(id) => id,
            None => return Ok(self.with_root(None, 0, 0)),
        };
        let new_root = self.update_at_node(root_id, index, &value)?;
        Ok(self.with_root(Some(new_root), self.len, self.depth))
    }

    /// Path-copy update: returns a new node ID that is a copy of `node_id` with
    /// the element at `pos` replaced.
    fn update_at_node(&self, node_id: u64, pos: usize, value: &A) -> Result<u64, VectorError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                let count = reader.count();
                // Rebuild the leaf with the updated element.
                let mut builder = LeafBuilder::new();
                for i in 0..count {
                    if i == pos {
                        builder.push_encoded::<A, C>(value)?;
                    } else {
                        let bytes = reader.entry_bytes(i);
                        let elem = C::decode::<A>(bytes)?;
                        builder.push_encoded::<A, C>(&elem)?;
                    }
                }
                let new_page = builder.finish();
                self.alloc_page(&new_page)
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let (child_idx, local_pos) = reader
                    .find_child(pos)
                    .ok_or(VectorError::OutOfBounds(pos, self.len))?;
                let child_id = reader.child_page_id(child_idx);
                let new_child_id = self.update_at_node(child_id, local_pos, value)?;
                // Rebuild the internal node with the updated child.
                let count = reader.count();
                let mut children: Vec<u64> = (0..count).map(|i| reader.child_page_id(i)).collect();
                let cum_sizes: Vec<u32> = (0..count).map(|i| reader.cumulative_size(i)).collect();
                children[child_idx] = new_child_id;
                let new_page = build_internal(&children, &cum_sizes);
                let new_id = self.alloc_page(&new_page)?;
                // All unchanged children are now shared between old and new internal nodes —
                // increment their refcounts.
                {
                    let mut store = self.store.lock().unwrap();
                    for (i, &cid) in children.iter().enumerate() {
                        if i != child_idx {
                            store.increment_refcount(cid);
                        }
                    }
                }
                Ok(new_id)
            }
            d => Err(VectorError::BadDiscriminant(d)),
        }
    }

    // ---------------------------------------------------------------------------
    // push_back
    // ---------------------------------------------------------------------------

    /// Returns a new vector with `value` appended at the end.
    ///
    /// Time: O(log_32 N) amortised.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn push_back(&self, value: A) -> Result<Self, VectorError> {
        match self.root {
            None => {
                // Empty vector — create the first leaf.
                let mut builder = LeafBuilder::new();
                builder.push_encoded::<A, C>(&value)?;
                let leaf_id = self.alloc_page(&builder.finish())?;
                Ok(self.with_root(Some(leaf_id), 1, 1))
            }
            Some(root_id) => {
                let (new_root, overflowed) = self.push_back_into(root_id, self.depth, &value)?;
                if overflowed {
                    // Tree is full at current depth — grow by one level.
                    // Create a new internal node with the old root and the overflow subtree
                    // as children.
                    let overflow_tree = self.build_single_element_subtree(&value, self.depth)?;
                    let old_size = self.len as u32;
                    let new_size = (self.len + 1) as u32;
                    let new_root_page =
                        build_internal(&[root_id, overflow_tree], &[old_size, new_size]);
                    let new_root_id = self.alloc_page(&new_root_page)?;
                    // Increment refcount for old root (now shared by parent).
                    self.store.lock().unwrap().increment_refcount(root_id);
                    Ok(self.with_root(Some(new_root_id), self.len + 1, self.depth + 1))
                } else {
                    Ok(self.with_root(Some(new_root), self.len + 1, self.depth))
                }
            }
        }
    }

    /// Attempts to push `value` into the subtree rooted at `node_id` at `depth`.
    ///
    /// Returns `(new_node_id, overflowed)`.  If `overflowed` is `true`, the subtree
    /// was full and the element was not inserted — the caller must handle overflow.
    fn push_back_into(
        &self,
        node_id: u64,
        depth: usize,
        value: &A,
    ) -> Result<(u64, bool), VectorError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                if reader.count() >= BRANCHING_FACTOR {
                    // Leaf is full — signal overflow to parent.
                    return Ok((node_id, true));
                }
                // Append to a copy of this leaf.
                let count = reader.count();
                let mut builder = LeafBuilder::new();
                for i in 0..count {
                    let bytes = reader.entry_bytes(i);
                    let elem = C::decode::<A>(bytes)?;
                    builder.push_encoded::<A, C>(&elem)?;
                }
                builder.push_encoded::<A, C>(value)?;
                let new_page = builder.finish();
                let new_id = self.alloc_page(&new_page)?;
                Ok((new_id, false))
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let count = reader.count();
                // Try to push into the last child.
                let last_idx = count - 1;
                let last_child = reader.child_page_id(last_idx);
                let (new_child, overflowed) = self.push_back_into(last_child, depth - 1, value)?;

                if !overflowed {
                    // Update this internal node with the new last child.
                    let mut children: Vec<u64> =
                        (0..count).map(|i| reader.child_page_id(i)).collect();
                    let mut cum_sizes: Vec<u32> =
                        (0..count).map(|i| reader.cumulative_size(i)).collect();
                    children[last_idx] = new_child;
                    // Only the last cumulative size changes.
                    cum_sizes[last_idx] += 1;
                    let new_page = build_internal(&children, &cum_sizes);
                    let new_id = self.alloc_page(&new_page)?;
                    // All children except the replaced last one are now shared between
                    // the old internal node and this new one — increment their refcounts.
                    {
                        let mut store = self.store.lock().unwrap();
                        for (i, &cid) in children.iter().enumerate() {
                            if i != last_idx {
                                store.increment_refcount(cid);
                            }
                        }
                    }
                    return Ok((new_id, false));
                }

                // Last child overflowed. Can we add a new child here?
                if count < BRANCHING_FACTOR {
                    // Build a new subtree of depth-1 containing just the new element.
                    let new_child_root = self.build_single_element_subtree(value, depth - 1)?;
                    let total_before = reader.cumulative_size(last_idx) as usize;
                    let mut children: Vec<u64> =
                        (0..count).map(|i| reader.child_page_id(i)).collect();
                    let mut cum_sizes: Vec<u32> =
                        (0..count).map(|i| reader.cumulative_size(i)).collect();
                    children.push(new_child_root);
                    cum_sizes.push((total_before + 1) as u32);
                    let new_page = build_internal(&children, &cum_sizes);
                    let new_id = self.alloc_page(&new_page)?;
                    // All old children (0..count) are shared between old and new internal.
                    // The new last child (new_child_root) is freshly allocated — no increment.
                    {
                        let mut store = self.store.lock().unwrap();
                        for &cid in children.iter().take(count) {
                            store.increment_refcount(cid);
                        }
                    }
                    Ok((new_id, false))
                } else {
                    // This internal node is also full — overflow upward.
                    Ok((node_id, true))
                }
            }
            d => Err(VectorError::BadDiscriminant(d)),
        }
    }

    /// Builds a subtree of height `depth` containing only `value` (all the way to a leaf).
    ///
    /// depth=1 → a single leaf with one element
    /// depth=2 → an internal with one child leaf containing one element
    /// ...
    fn build_single_element_subtree(&self, value: &A, depth: usize) -> Result<u64, VectorError> {
        if depth == 1 {
            let mut builder = LeafBuilder::new();
            builder.push_encoded::<A, C>(value)?;
            return self.alloc_page(&builder.finish());
        }
        let child_id = self.build_single_element_subtree(value, depth - 1)?;
        let page = build_internal(&[child_id], &[1u32]);
        self.alloc_page(&page)
    }

    // ---------------------------------------------------------------------------
    // pop_back
    // ---------------------------------------------------------------------------

    /// Returns a new vector with the last element removed, and that element.
    ///
    /// Returns `(self_clone, None)` if the vector is empty.
    ///
    /// Time: O(log_32 N).
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn pop_back(&self) -> Result<(Self, Option<A>), VectorError> {
        let root_id = match self.root {
            None => return Ok((self.with_root(None, 0, 0), None)),
            Some(id) => id,
        };

        let (new_root_opt, elem) = self.pop_back_from(root_id)?;

        if new_root_opt.is_none() {
            // Tree is now empty.
            return Ok((self.with_root(None, 0, 0), Some(elem)));
        }

        let new_root = new_root_opt.unwrap();

        // If root is an internal with one child, collapse depth.
        let (collapsed_root, new_depth) = self.collapse_root(new_root, self.depth)?;

        Ok((
            self.with_root(Some(collapsed_root), self.len - 1, new_depth),
            Some(elem),
        ))
    }

    /// Removes the last element from the subtree at `node_id`.
    ///
    /// Returns `(new_node_id_or_none, removed_elem)`.
    /// `None` means the node is now empty and should be deleted.
    fn pop_back_from(&self, node_id: u64) -> Result<(Option<u64>, A), VectorError> {
        let page = self.read_page(node_id)?;
        match page.0[0] {
            DISCRIMINANT_LEAF => {
                let reader = LeafReader::new(&page);
                let count = reader.count();
                // Remove last element.
                let last = reader.get_entry::<A, C>(count - 1)?;
                if count == 1 {
                    // Leaf is now empty.
                    return Ok((None, last));
                }
                // Rebuild leaf without last element.
                let mut builder = LeafBuilder::new();
                for i in 0..count - 1 {
                    let bytes = reader.entry_bytes(i);
                    let elem = C::decode::<A>(bytes)?;
                    builder.push_encoded::<A, C>(&elem)?;
                }
                let new_page = builder.finish();
                let new_id = self.alloc_page(&new_page)?;
                Ok((Some(new_id), last))
            }
            DISCRIMINANT_INTERNAL => {
                let reader = InternalReader::new(&page);
                let count = reader.count();
                let last_idx = count - 1;
                let last_child = reader.child_page_id(last_idx);
                let (new_child_opt, elem) = self.pop_back_from(last_child)?;

                if let Some(new_child) = new_child_opt {
                    // Update this node with the shrunken last child.
                    let mut children: Vec<u64> =
                        (0..count).map(|i| reader.child_page_id(i)).collect();
                    let mut cum_sizes: Vec<u32> =
                        (0..count).map(|i| reader.cumulative_size(i)).collect();
                    children[last_idx] = new_child;
                    cum_sizes[last_idx] -= 1;
                    let new_page = build_internal(&children, &cum_sizes);
                    let new_id = self.alloc_page(&new_page)?;
                    // All unchanged children are shared between old and new internal nodes.
                    {
                        let mut store = self.store.lock().unwrap();
                        for (i, &cid) in children.iter().enumerate() {
                            if i != last_idx {
                                store.increment_refcount(cid);
                            }
                        }
                    }
                    Ok((Some(new_id), elem))
                } else {
                    // Last child is now empty.
                    if count == 1 {
                        // This node is now empty too.
                        Ok((None, elem))
                    } else {
                        // Remove the last child from this node.
                        let children: Vec<u64> =
                            (0..last_idx).map(|i| reader.child_page_id(i)).collect();
                        let cum_sizes: Vec<u32> =
                            (0..last_idx).map(|i| reader.cumulative_size(i)).collect();
                        let new_page = build_internal(&children, &cum_sizes);
                        let new_id = self.alloc_page(&new_page)?;
                        // All remaining children are shared between old and new internal nodes.
                        {
                            let mut store = self.store.lock().unwrap();
                            for &cid in &children {
                                store.increment_refcount(cid);
                            }
                        }
                        Ok((Some(new_id), elem))
                    }
                }
            }
            d => Err(VectorError::BadDiscriminant(d)),
        }
    }

    /// If root is an internal node with exactly one child, unwrap to that child and
    /// reduce depth by 1 (repeatedly if needed).
    fn collapse_root(&self, root_id: u64, depth: usize) -> Result<(u64, usize), VectorError> {
        if depth <= 1 {
            return Ok((root_id, depth));
        }
        let page = self.read_page(root_id)?;
        if page.0[0] != DISCRIMINANT_INTERNAL {
            return Ok((root_id, depth));
        }
        let reader = InternalReader::new(&page);
        if reader.count() == 1 {
            let child_id = reader.child_page_id(0);
            return self.collapse_root(child_id, depth - 1);
        }
        Ok((root_id, depth))
    }

    // ---------------------------------------------------------------------------
    // push_front
    // ---------------------------------------------------------------------------

    /// Returns a new vector with `value` prepended at the front.
    ///
    /// Implemented as `split_at(0)` + prepend logic: build a 1-element vector,
    /// then concatenate with self.
    ///
    /// Time: O(N) — re-indexes all elements (concat is O(N) for simple impl).
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn push_front(&self, value: A) -> Result<Self, VectorError> {
        // Build a 1-element vector, then concat with self.
        let singleton = self.with_root(None, 0, 0).push_back(value)?;
        singleton.concat(self)
    }

    // ---------------------------------------------------------------------------
    // pop_front
    // ---------------------------------------------------------------------------

    /// Returns a new vector with the first element removed, and that element.
    ///
    /// Returns `(self_clone, None)` if the vector is empty.
    ///
    /// Time: O(N) — implemented via split_at.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn pop_front(&self) -> Result<(Self, Option<A>), VectorError> {
        if self.is_empty() {
            return Ok((self.with_root(None, 0, 0), None));
        }
        let first = self.get(0)?;
        let (_, rest) = self.split_at(1)?;
        Ok((rest, first))
    }

    // ---------------------------------------------------------------------------
    // concat
    // ---------------------------------------------------------------------------

    /// Returns a new vector that is the concatenation of `self` and `other`.
    ///
    /// Implemented by iterating `other` and pushing elements back into a clone
    /// of `self`.
    ///
    /// Time: O(M log N) where M is `other.len()`.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn concat(&self, other: &Self) -> Result<Self, VectorError> {
        let mut result = self.with_root(self.root, self.len, self.depth);
        // Clone root refcount if non-empty.
        if let Some(root_id) = self.root {
            self.store.lock().unwrap().increment_refcount(root_id);
        }
        for i in 0..other.len {
            let elem = other.get(i)?.expect("index in range");
            result = result.push_back(elem)?;
        }
        Ok(result)
    }

    // ---------------------------------------------------------------------------
    // split_at
    // ---------------------------------------------------------------------------

    /// Splits the vector at `index`, returning `(left, right)`.
    ///
    /// `left` contains elements `0..index`, `right` contains elements `index..len`.
    ///
    /// Time: O(N) — rebuilds both halves.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] on folio I/O or codec failure.
    pub fn split_at(&self, index: usize) -> Result<(Self, Self), VectorError> {
        let index = index.min(self.len);
        let mut left = self.with_root(None, 0, 0);
        let mut right = self.with_root(None, 0, 0);
        for i in 0..index {
            let elem = self.get(i)?.expect("index in range");
            left = left.push_back(elem)?;
        }
        for i in index..self.len {
            let elem = self.get(i)?.expect("index in range");
            right = right.push_back(elem)?;
        }
        Ok((left, right))
    }

    // ---------------------------------------------------------------------------
    // iter
    // ---------------------------------------------------------------------------

    /// Returns an iterator over cloned elements.
    ///
    /// Time per element: O(log_32 N).
    ///
    /// # Errors
    ///
    /// The iterator is infallible — elements are returned as `A`.
    /// Internal errors cause panics.
    #[must_use]
    pub fn iter(&self) -> FolioVectorIter<'_, A, C, B> {
        FolioVectorIter {
            vector: self,
            index: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

impl<A, C, B> Clone for FolioVector<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    fn clone(&self) -> Self {
        if let Some(root_id) = self.root {
            self.store.lock().unwrap().increment_refcount(root_id);
        }
        Self {
            store: Arc::clone(&self.store),
            root: self.root,
            len: self.len,
            depth: self.depth,
            _phantom: std::marker::PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------

impl<A, C, B> Drop for FolioVector<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    fn drop(&mut self) {
        if let Some(root_id) = self.root {
            let mut store = self.store.lock().unwrap();
            // Collect pages to free via iterative DFS.
            let mut pages_to_free: Vec<u64> = Vec::new();
            let mut stack: Vec<(u64, usize)> = vec![(root_id, self.depth)];
            while let Some((page_id, depth)) = stack.pop() {
                let should_free = store.decrement_refcount(page_id);
                if should_free {
                    pages_to_free.push(page_id);
                    if depth > 1 {
                        // Read the page to find children.
                        if let Ok(page) = store.read_node(page_id) {
                            if page.0[0] == DISCRIMINANT_INTERNAL {
                                let reader = InternalReader::new(&page);
                                for i in 0..reader.count() {
                                    stack.push((reader.child_page_id(i), depth - 1));
                                }
                            }
                        }
                    }
                }
            }
            if !pages_to_free.is_empty() {
                let _ = store.free_nodes(&pages_to_free);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FolioVectorIter
// ---------------------------------------------------------------------------

/// An iterator over cloned elements of a [`FolioVector`].
pub struct FolioVectorIter<'a, A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Reference to the vector being iterated.
    vector: &'a FolioVector<A, C, B>,
    /// Current index.
    index: usize,
}

impl<'a, A, C, B> Iterator for FolioVectorIter<'a, A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    type Item = A;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.vector.len {
            return None;
        }
        let elem = self.vector.get(self.index).expect("in-range get").unwrap();
        self.index += 1;
        Some(elem)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.vector.len - self.index;
        (remaining, Some(remaining))
    }
}

// ---------------------------------------------------------------------------
// PersistentCollection + PersistentVector trait impls
// ---------------------------------------------------------------------------

impl<A, C, B> pds::traits::PersistentCollection for FolioVector<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
}

impl<A, C, B> pds::traits::PersistentVector<A> for FolioVector<A, C, B>
where
    A: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    fn get(&self, index: usize) -> Option<A> {
        FolioVector::get(self, index).expect("FolioVector::get failed in PersistentVector::get")
    }

    fn push_back(&self, value: A) -> Self {
        FolioVector::push_back(self, value)
            .expect("FolioVector::push_back failed in PersistentVector::push_back")
    }

    fn push_front(&self, value: A) -> Self {
        FolioVector::push_front(self, value)
            .expect("FolioVector::push_front failed in PersistentVector::push_front")
    }

    fn update(&self, index: usize, value: A) -> Self {
        FolioVector::update(self, index, value)
            .expect("FolioVector::update failed in PersistentVector::update")
    }

    fn pop_back(&self) -> (Self, Option<A>) {
        FolioVector::pop_back(self)
            .expect("FolioVector::pop_back failed in PersistentVector::pop_back")
    }

    fn pop_front(&self) -> (Self, Option<A>) {
        FolioVector::pop_front(self)
            .expect("FolioVector::pop_front failed in PersistentVector::pop_front")
    }

    fn concat(&self, other: &Self) -> Self {
        FolioVector::concat(self, other)
            .expect("FolioVector::concat failed in PersistentVector::concat")
    }

    fn split_at(&self, index: usize) -> (Self, Self) {
        FolioVector::split_at(self, index)
            .expect("FolioVector::split_at failed in PersistentVector::split_at")
    }

    fn len(&self) -> usize {
        FolioVector::len(self)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{PodCodec, PostcardCodec};
    use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
    use pds::traits::PersistentVector;

    fn make_store() -> FolioStore<MemBackend> {
        let backend = MemBackend::new(4096, 512);
        FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    // --- Basic operations ---

    #[test]
    fn empty_vector_is_empty() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        assert!(v.is_empty());
        assert_eq!(v.len(), 0);
        assert_eq!(v.get(0).unwrap(), None);
    }

    #[test]
    fn single_push_back_and_get() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v2 = v.push_back(42u64).unwrap();
        assert_eq!(v2.len(), 1);
        assert!(!v2.is_empty());
        assert_eq!(v2.get(0).unwrap(), Some(42u64));
        assert_eq!(v2.get(1).unwrap(), None);
        // Original unchanged.
        assert!(v.is_empty());
    }

    #[test]
    fn multiple_push_backs() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..10u64 {
            v = v.push_back(i).unwrap();
        }
        assert_eq!(v.len(), 10);
        for i in 0..10u64 {
            assert_eq!(v.get(i as usize).unwrap(), Some(i));
        }
    }

    #[test]
    fn push_back_across_leaf_boundary() {
        // Push BRANCHING_FACTOR + 1 elements to exercise leaf splitting.
        let mut v: FolioVector<u32, PostcardCodec, MemBackend> = FolioVector::new(make_store());
        let n = BRANCHING_FACTOR + 5;
        for i in 0..n {
            v = v.push_back(i as u32).unwrap();
        }
        assert_eq!(v.len(), n);
        for i in 0..n {
            assert_eq!(v.get(i).unwrap(), Some(i as u32));
        }
    }

    #[test]
    fn update_element() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..5u64 {
            v = v.push_back(i).unwrap();
        }
        let v2 = v.update(2, 99u64).unwrap();
        assert_eq!(v2.get(2).unwrap(), Some(99u64));
        // Original unchanged.
        assert_eq!(v.get(2).unwrap(), Some(2u64));
        // Other elements unchanged.
        assert_eq!(v2.get(0).unwrap(), Some(0u64));
        assert_eq!(v2.get(4).unwrap(), Some(4u64));
    }

    #[test]
    fn update_out_of_bounds_is_noop() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v2 = v.update(0, 42u64).unwrap();
        assert!(v2.is_empty());
    }

    #[test]
    fn pop_back_single_element() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v2 = v.push_back(42u64).unwrap();
        let (v3, elem) = v2.pop_back().unwrap();
        assert_eq!(elem, Some(42u64));
        assert!(v3.is_empty());
    }

    #[test]
    fn pop_back_multiple_elements() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..5u64 {
            v = v.push_back(i).unwrap();
        }
        let (v2, elem) = v.pop_back().unwrap();
        assert_eq!(elem, Some(4u64));
        assert_eq!(v2.len(), 4);
        // Original unchanged.
        assert_eq!(v.len(), 5);
    }

    #[test]
    fn pop_back_empty_vector() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let (v2, elem) = v.pop_back().unwrap();
        assert_eq!(elem, None);
        assert!(v2.is_empty());
    }

    #[test]
    fn pop_back_all_elements() {
        let n = 10u64;
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..n {
            v = v.push_back(i).unwrap();
        }
        for i in (0..n).rev() {
            let (new_v, elem) = v.pop_back().unwrap();
            assert_eq!(elem, Some(i));
            v = new_v;
        }
        assert!(v.is_empty());
    }

    #[test]
    fn push_front_creates_new_first_element() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v2 = v.push_back(2u64).unwrap();
        let v3 = v2.push_back(3u64).unwrap();
        let v4 = v3.push_front(1u64).unwrap();
        assert_eq!(v4.len(), 3);
        assert_eq!(v4.get(0).unwrap(), Some(1u64));
        assert_eq!(v4.get(1).unwrap(), Some(2u64));
        assert_eq!(v4.get(2).unwrap(), Some(3u64));
    }

    #[test]
    fn pop_front_removes_first_element() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v2 = v.push_back(1u64).unwrap();
        let v3 = v2.push_back(2u64).unwrap();
        let v4 = v3.push_back(3u64).unwrap();
        let (v5, elem) = v4.pop_front().unwrap();
        assert_eq!(elem, Some(1u64));
        assert_eq!(v5.len(), 2);
        assert_eq!(v5.get(0).unwrap(), Some(2u64));
    }

    #[test]
    fn concat_two_vectors() {
        let v1: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v1 = v1.push_back(1u64).unwrap();
        let v1 = v1.push_back(2u64).unwrap();

        let v2: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let v2 = v2.push_back(3u64).unwrap();
        let v2 = v2.push_back(4u64).unwrap();

        let merged = v1.concat(&v2).unwrap();
        assert_eq!(merged.len(), 4);
        assert_eq!(merged.get(0).unwrap(), Some(1u64));
        assert_eq!(merged.get(1).unwrap(), Some(2u64));
        assert_eq!(merged.get(2).unwrap(), Some(3u64));
        assert_eq!(merged.get(3).unwrap(), Some(4u64));
    }

    #[test]
    fn split_at_divides_vector() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..6u64 {
            v = v.push_back(i).unwrap();
        }
        let (left, right) = v.split_at(3).unwrap();
        assert_eq!(left.len(), 3);
        assert_eq!(right.len(), 3);
        for i in 0..3 {
            assert_eq!(left.get(i).unwrap(), Some(i as u64));
        }
        for i in 0..3 {
            assert_eq!(right.get(i).unwrap(), Some((i + 3) as u64));
        }
    }

    #[test]
    fn split_at_zero() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..4u64 {
            v = v.push_back(i).unwrap();
        }
        let (left, right) = v.split_at(0).unwrap();
        assert!(left.is_empty());
        assert_eq!(right.len(), 4);
    }

    #[test]
    fn split_at_len() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..4u64 {
            v = v.push_back(i).unwrap();
        }
        let (left, right) = v.split_at(4).unwrap();
        assert_eq!(left.len(), 4);
        assert!(right.is_empty());
    }

    #[test]
    fn iter_returns_all_elements_in_order() {
        let mut v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..8u64 {
            v = v.push_back(i).unwrap();
        }
        let elems: Vec<u64> = v.iter().collect();
        assert_eq!(elems, (0..8u64).collect::<Vec<_>>());
    }

    #[test]
    fn snapshot_isolation_push_back() {
        let v: FolioVector<u64, PodCodec, MemBackend> = FolioVector::new(make_store());
        let base = v.push_back(1u64).unwrap();
        let a = base.push_back(2u64).unwrap();
        let b = base.push_back(3u64).unwrap();

        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 2);
        assert_eq!(a.get(1).unwrap(), Some(2u64));
        assert_eq!(b.get(1).unwrap(), Some(3u64));
        // base unchanged.
        assert_eq!(base.len(), 1);
    }

    // --- PersistentVector trait ---

    // Helper to exercise PersistentVector via trait bound (avoids inherent method resolution).
    fn pv_push_get<V: pds::traits::PersistentVector<i32>>(empty: V) {
        let v = empty.push_back(10).push_back(20).push_back(30);
        assert_eq!(v.get(0), Some(10));
        assert_eq!(v.get(1), Some(20));
        assert_eq!(v.get(2), Some(30));
        assert_eq!(v.get(3), None);
        assert_eq!(v.len(), 3);
    }

    fn pv_pop_back<V: pds::traits::PersistentVector<i32>>(empty: V) {
        let v = empty.push_back(1).push_back(2).push_back(3);
        let (v2, elem) = v.pop_back();
        assert_eq!(elem, Some(3));
        assert_eq!(v2.len(), 2);
    }

    fn pv_split_concat<V: pds::traits::PersistentVector<i32>>(empty: V) {
        let v = empty.push_back(1).push_back(2).push_back(3).push_back(4);
        let (left, right) = v.split_at(2);
        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        let merged = left.concat(&right);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged.get(0), Some(1));
        assert_eq!(merged.get(3), Some(4));
    }

    #[test]
    fn persistent_vector_trait_push_get() {
        pv_push_get(FolioVector::<i32, PostcardCodec, MemBackend>::new(
            make_store(),
        ));
    }

    #[test]
    fn persistent_vector_trait_pop_back() {
        pv_pop_back(FolioVector::<i32, PostcardCodec, MemBackend>::new(
            make_store(),
        ));
    }

    #[test]
    fn persistent_vector_trait_split_concat() {
        pv_split_concat(FolioVector::<i32, PostcardCodec, MemBackend>::new(
            make_store(),
        ));
    }

    // --- Push across multiple tree levels ---

    #[test]
    fn push_back_large_n() {
        // Push enough elements to force multiple tree levels.
        let n = BRANCHING_FACTOR * BRANCHING_FACTOR + 10;
        let mut v: FolioVector<u32, PostcardCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..n {
            v = v.push_back(i as u32).unwrap();
        }
        assert_eq!(v.len(), n);
        for i in 0..n {
            assert_eq!(v.get(i).unwrap(), Some(i as u32), "mismatch at {i}");
        }
    }

    #[test]
    fn pop_back_large_n() {
        let n = BRANCHING_FACTOR + 5;
        let mut v: FolioVector<u32, PostcardCodec, MemBackend> = FolioVector::new(make_store());
        for i in 0..n {
            v = v.push_back(i as u32).unwrap();
        }
        // Pop all elements.
        for i in (0..n).rev() {
            let (new_v, elem) = v.pop_back().unwrap();
            assert_eq!(elem, Some(i as u32));
            v = new_v;
        }
        assert!(v.is_empty());
    }
}
