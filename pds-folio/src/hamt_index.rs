//! HAMT-backed page index for `merkle-spine`.
//!
//! [`HamtIndex<B>`] implements [`merkle_spine::index::PageIndexBackend`] using a
//! persistent HAMT to store the page index.  Each committed version materialises
//! a complete HAMT snapshot rather than a log-structured delta chain, giving
//! O(log N) point lookups from any historical root in a single snapshot.
//!
//! # Index-root hash
//!
//! The `index_root` used by `PageIndexBackend` is a 32-byte BLAKE3 Merkle hash
//! that commits to all `(region, page, PageEntry)` tuples in the HAMT.  It is
//! computed over the sorted serialisation of every entry after each
//! `commit_delta` call.  For identical entry sets the hash is always the same.
//!
//! # Design
//!
//! `HamtIndex` maintains an in-memory map from `index_root: [u8; 32]` to
//! `HamtMap` snapshots.  This keeps historical access O(1) — given a root hash,
//! the associated `HamtMap` snapshot is retrieved directly.
//!
//! The key type inside the HAMT is `IndexKey` — a Pod-encoded pair
//! `(region_id: u64, page_id: u64)` — giving naturally ordered lookups.
//! The value type is `IndexValue` — a fixed-size encoding of `PageEntry`.

use std::collections::HashMap;

use folio_core::{backend::Backend, error::BackendError, store::FolioStore};
use merkle_spine::{
    error::{Error as SpineError, RegionId},
    hash::{hash_hamt_node, Hash, ZERO_HASH},
    index::{PageEntry, PageIndexBackend},
};
use serde::{Deserialize, Serialize};

use crate::{
    codec::PodCodec,
    hamt::{HamtError, HamtMap},
};

// ---------------------------------------------------------------------------
// Key / Value types stored inside the HAMT
// ---------------------------------------------------------------------------

/// Composite key: `(region_id, page_id)`.
///
/// Uses big-endian byte order inside `PodCodec` so that the byte representation
/// matches the logical ordering — not strictly required for correctness here, but
/// keeps the on-disk format consistent with numerically ordered key comparisons.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    bytemuck::Pod,
    bytemuck::Zeroable,
)]
#[repr(C)]
pub(crate) struct IndexKey {
    /// Region identifier (little-endian in memory; serialised as-is by PodCodec).
    pub region_id: u64,
    /// Page identifier.
    pub page_id: u64,
}

/// Fixed-size serialisation of a [`PageEntry`] stored inside the HAMT.
///
/// Layout (all LE):
/// ```text
///  0  32  content_hash  [u8; 32]
/// 32   8  folio_page_id u64
/// 40   1  encoding_tag  u8
/// 41   1  chain_depth   u8
/// 42   6  _pad          zero
/// ```
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Serialize, Deserialize)]
#[repr(C)]
pub(crate) struct IndexValue {
    /// BLAKE3 content hash of the logical page.
    pub content_hash: [u8; 32],
    /// Folio page address.
    pub folio_page_id: u64,
    /// Encoding kind tag.
    pub encoding_tag: u8,
    /// Delta chain depth.
    pub chain_depth: u8,
    /// Reserved padding.
    pub _pad: [u8; 6],
}

impl From<&PageEntry> for IndexValue {
    fn from(e: &PageEntry) -> Self {
        Self {
            content_hash: e.content_hash,
            folio_page_id: e.folio_page_id,
            encoding_tag: e.encoding_tag,
            chain_depth: e.chain_depth,
            _pad: [0u8; 6],
        }
    }
}

impl From<IndexValue> for PageEntry {
    fn from(v: IndexValue) -> Self {
        Self {
            content_hash: v.content_hash,
            folio_page_id: v.folio_page_id,
            encoding_tag: v.encoding_tag,
            chain_depth: v.chain_depth,
        }
    }
}

// ---------------------------------------------------------------------------
// HamtIndex
// ---------------------------------------------------------------------------

/// HAMT-backed page index for [`merkle_spine::index::PageIndexBackend`].
///
/// Each committed version is a complete `HamtMap` snapshot stored in-memory and
/// addressed by a 32-byte Merkle root hash.  Lookups from any historical root
/// are O(log N) with no chain traversal.
///
/// All HAMT snapshots share a single underlying `NodeStore` (via `Arc<Mutex<…>>`)
/// created from the `FolioStore` passed to [`HamtIndex::new`].
pub struct HamtIndex<B = folio_core::backend::MemBackend>
where
    B: Backend<Error = BackendError>,
{
    /// In-memory map: `index_root` → HAMT snapshot.
    ///
    /// `ZERO_HASH` is the genesis root — always maps to an empty HAMT.
    snapshots: HashMap<Hash, HamtMap<IndexKey, IndexValue, PodCodec, B>>,
}

impl<B: Backend<Error = BackendError>> std::fmt::Debug for HamtIndex<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HamtIndex")
            .field("snapshot_count", &self.snapshots.len())
            .finish()
    }
}

impl<B: Backend<Error = BackendError>> HamtIndex<B> {
    /// Creates a new `HamtIndex` backed by `store`.
    ///
    /// The genesis root (`ZERO_HASH`) always maps to an empty HAMT.  Callers
    /// may pass `ZERO_HASH` as the initial `parent_index_root` for the first
    /// `commit_delta`.
    ///
    /// The `store` is consumed to create the shared `NodeStore` used by all
    /// HAMT snapshots.  All subsequent snapshots share the same underlying
    /// folio store via `Arc<Mutex<…>>` inside the `HamtMap`.
    #[must_use]
    pub fn new(store: FolioStore<B>) -> Self {
        let genesis_hamt: HamtMap<IndexKey, IndexValue, PodCodec, B> = HamtMap::new(store);

        let mut snapshots = HashMap::new();
        snapshots.insert(ZERO_HASH, genesis_hamt);

        Self { snapshots }
    }

    /// Looks up the HAMT snapshot for `index_root`.
    fn snapshot_for(
        &self,
        index_root: &Hash,
    ) -> Option<&HamtMap<IndexKey, IndexValue, PodCodec, B>> {
        self.snapshots.get(index_root)
    }

    /// Computes the Merkle root hash for a given HAMT snapshot.
    ///
    /// Iterates all entries in arbitrary order, serialises them into a sorted
    /// byte vector, and hashes the result with BLAKE3 using the `HAMT_NODE_KEY`
    /// domain.  Equal entry sets always produce equal hashes.
    fn compute_root_hash(
        hamt: &HamtMap<IndexKey, IndexValue, PodCodec, B>,
    ) -> Result<Hash, HamtIndexError> {
        // Collect all entries.
        let entries_iter = hamt.iter().map_err(HamtIndexError::Hamt)?;
        let mut entries: Vec<(IndexKey, IndexValue)> = entries_iter
            .map(|r| r.map_err(HamtIndexError::Hamt))
            .collect::<Result<Vec<_>, _>>()?;

        if entries.is_empty() {
            // Empty HAMT has a well-known hash: the domain-keyed BLAKE3 of an
            // empty byte slice.  Uses `hash_hamt_node` (already imported) so
            // we do not need to depend on `blake3` directly in this crate.
            return Ok(hash_hamt_node(b""));
        }

        // Sort by (region_id, page_id) for deterministic hashing.
        entries.sort_by_key(|(k, _)| (k.region_id, k.page_id));

        // Serialise all entries into a single buffer.
        let mut buf: Vec<u8> = Vec::with_capacity(entries.len() * 48);
        for (k, v) in &entries {
            buf.extend_from_slice(&k.region_id.to_le_bytes());
            buf.extend_from_slice(&k.page_id.to_le_bytes());
            buf.extend_from_slice(&v.content_hash);
            buf.extend_from_slice(&v.folio_page_id.to_le_bytes());
            buf.push(v.encoding_tag);
            buf.push(v.chain_depth);
        }

        Ok(hash_hamt_node(&buf))
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`HamtIndex`] operations.
#[derive(Debug, thiserror::Error)]
pub enum HamtIndexError {
    /// An error from the underlying HAMT.
    #[error("hamt error: {0}")]
    Hamt(#[from] HamtError),

    /// The requested `index_root` is not known.
    #[error("unknown index root")]
    UnknownRoot,
}

impl From<HamtIndexError> for SpineError {
    fn from(e: HamtIndexError) -> Self {
        // `InvalidConfig` is the closest variant in merkle-spine's Error enum
        // for index-layer failures that are not folio I/O errors.
        SpineError::InvalidConfig(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// PageIndexBackend impl
// ---------------------------------------------------------------------------

impl<B: Backend<Error = BackendError>> PageIndexBackend for HamtIndex<B> {
    fn lookup(
        &self,
        index_root: &Hash,
        region: RegionId,
        page: u64,
    ) -> Result<Option<PageEntry>, SpineError> {
        let key = IndexKey {
            region_id: region,
            page_id: page,
        };
        let hamt = self
            .snapshot_for(index_root)
            .ok_or(SpineError::InvalidConfig(
                "HamtIndex: unknown index_root".to_string(),
            ))?;
        let result = hamt
            .get(&key)
            .map_err(|e| SpineError::InvalidConfig(format!("HamtIndex: HAMT get failed: {e}")))?;
        Ok(result.map(PageEntry::from))
    }

    fn commit_delta(
        &mut self,
        parent_index_root: &Hash,
        entries: &[(RegionId, u64, PageEntry)],
    ) -> Result<Hash, SpineError> {
        // Load the parent snapshot.
        let parent = self
            .snapshots
            .get(parent_index_root)
            .ok_or_else(|| SpineError::InvalidConfig("HamtIndex: unknown parent root".to_string()))?
            .clone();

        // Apply all delta entries.
        let mut current = parent;
        for &(region, page, ref entry) in entries {
            let key = IndexKey {
                region_id: region,
                page_id: page,
            };
            let value = IndexValue::from(entry);
            current = current
                .insert(key, value)
                .map_err(|e| SpineError::InvalidConfig(format!("HamtIndex: insert failed: {e}")))?;
        }

        // Compute the new Merkle root hash.
        let new_root = Self::compute_root_hash(&current)
            .map_err(|e| SpineError::InvalidConfig(e.to_string()))?;

        // Store the new snapshot (idempotent — same hash = same content).
        self.snapshots.insert(new_root, current);

        Ok(new_root)
    }

    fn delete_index_page(&mut self, index_root: &Hash) -> Result<(), SpineError> {
        // Remove the snapshot from the in-memory map — the HAMT's Drop impl
        // will free the underlying folio pages.
        self.snapshots.remove(index_root);
        Ok(())
    }

    fn snapshot(&mut self, index_root: &Hash) -> Result<Hash, SpineError> {
        // The HAMT already IS a complete snapshot.  No additional work required.
        // Return `index_root` unchanged — it is still valid and complete.
        if self.snapshots.contains_key(index_root) {
            Ok(*index_root)
        } else {
            Err(SpineError::InvalidConfig(
                "HamtIndex: unknown index_root for snapshot".to_string(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
    use merkle_spine::{
        hash::ZERO_HASH,
        index::{PageEntry, PageIndexBackend},
    };

    fn make_store() -> FolioStore<MemBackend> {
        let backend = MemBackend::new(4096, 256);
        FolioStore::create(backend, 4096, 256, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    fn sample_entry(n: u8) -> PageEntry {
        PageEntry {
            content_hash: [n; 32],
            folio_page_id: n as u64 * 100,
            encoding_tag: 1,
            chain_depth: 0,
        }
    }

    #[test]
    fn new_index_is_empty_at_zero_hash() {
        let index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        // Lookup at ZERO_HASH should return None for any key.
        let result = index.lookup(&ZERO_HASH, 0, 1).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn commit_and_lookup_single_entry() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let entry = sample_entry(1);
        let entries = [(0u64, 1u64, entry.clone())];

        let new_root = index.commit_delta(&ZERO_HASH, &entries).unwrap();
        assert_ne!(new_root, ZERO_HASH);

        let found = index.lookup(&new_root, 0, 1).unwrap();
        assert_eq!(found, Some(entry));

        // Old root still works.
        let not_found = index.lookup(&ZERO_HASH, 0, 1).unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn commit_multiple_entries_all_accessible() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let e1 = sample_entry(1);
        let e2 = sample_entry(2);
        let entries = [(0u64, 1u64, e1.clone()), (0u64, 2u64, e2.clone())];

        let root = index.commit_delta(&ZERO_HASH, &entries).unwrap();
        assert_eq!(index.lookup(&root, 0, 1).unwrap(), Some(e1));
        assert_eq!(index.lookup(&root, 0, 2).unwrap(), Some(e2));
        assert_eq!(index.lookup(&root, 0, 3).unwrap(), None);
    }

    #[test]
    fn chained_commits_build_cumulative_index() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());

        let root1 = index
            .commit_delta(&ZERO_HASH, &[(0, 1, sample_entry(1))])
            .unwrap();
        let root2 = index
            .commit_delta(&root1, &[(0, 2, sample_entry(2))])
            .unwrap();

        // root1 has only entry 1.
        assert_eq!(index.lookup(&root1, 0, 1).unwrap(), Some(sample_entry(1)));
        assert_eq!(index.lookup(&root1, 0, 2).unwrap(), None);

        // root2 has both entries.
        assert_eq!(index.lookup(&root2, 0, 1).unwrap(), Some(sample_entry(1)));
        assert_eq!(index.lookup(&root2, 0, 2).unwrap(), Some(sample_entry(2)));
    }

    #[test]
    fn overwrite_entry_updates_value() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let e1 = sample_entry(1);
        let e2 = sample_entry(99); // Different content_hash.

        let root1 = index
            .commit_delta(&ZERO_HASH, &[(0, 1, e1.clone())])
            .unwrap();
        let root2 = index.commit_delta(&root1, &[(0, 1, e2.clone())]).unwrap();

        // root1 still sees original.
        assert_eq!(index.lookup(&root1, 0, 1).unwrap(), Some(e1));
        // root2 sees updated value.
        assert_eq!(index.lookup(&root2, 0, 1).unwrap(), Some(e2));
    }

    #[test]
    fn same_content_produces_same_root_hash() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let entries = [(0u64, 42u64, sample_entry(7))];

        let root_a = index.commit_delta(&ZERO_HASH, &entries).unwrap();
        let root_b = index.commit_delta(&ZERO_HASH, &entries).unwrap();
        assert_eq!(root_a, root_b, "same entries must produce same root hash");
    }

    #[test]
    fn snapshot_returns_same_root() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let root = index
            .commit_delta(&ZERO_HASH, &[(0, 1, sample_entry(1))])
            .unwrap();
        let snap = index.snapshot(&root).unwrap();
        assert_eq!(snap, root);
    }

    #[test]
    fn delete_removes_snapshot() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let root = index
            .commit_delta(&ZERO_HASH, &[(0, 1, sample_entry(1))])
            .unwrap();
        index.delete_index_page(&root).unwrap();

        // Lookup on deleted root returns an error (unknown root).
        let result = index.lookup(&root, 0, 1);
        assert!(result.is_err());
    }

    #[test]
    fn lookup_unknown_root_returns_error() {
        let index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let fake_root = [0xffu8; 32];
        let result = index.lookup(&fake_root, 0, 1);
        assert!(result.is_err());
    }

    #[test]
    fn multi_region_entries_are_independent() {
        let mut index: HamtIndex<MemBackend> = HamtIndex::new(make_store());
        let entries = [
            (0u64, 1u64, sample_entry(10)),
            (1u64, 1u64, sample_entry(20)),
        ];
        let root = index.commit_delta(&ZERO_HASH, &entries).unwrap();

        assert_eq!(index.lookup(&root, 0, 1).unwrap(), Some(sample_entry(10)));
        assert_eq!(index.lookup(&root, 1, 1).unwrap(), Some(sample_entry(20)));
        assert_eq!(index.lookup(&root, 2, 1).unwrap(), None);
    }
}
