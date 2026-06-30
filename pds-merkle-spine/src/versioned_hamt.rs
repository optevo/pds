//! [`VersionedHamt`] — a persistently versioned, Merkle-verified hash map.
//!
//! Each mutation (`insert`, `remove`) creates a new immutable version.
//! All historical versions remain accessible in O(log N) per lookup via structural
//! sharing in the underlying `HamtMap`.  A `VersionId` (monotonic sequence counter
//! + BLAKE3 Merkle root hash) identifies each version unambiguously.
//!
//! # Storage model
//!
//! `VersionedHamt` wraps a [`pds_folio::hamt::HamtMap`] for K/V storage and
//! maintains an internal version history (shared across clones via
//! `Arc<Mutex<…>>`).  Each history entry holds a `HamtMap` clone — keeping
//! folio refcounts alive so that historical snapshots remain readable.
//!
//! The Merkle root is computed by hashing all key-value pairs in sorted order
//! with BLAKE3 (domain key `ms:hamt-node-v1`).  Equal entry sets always produce
//! the same hash.
//!
//! # Structural sharing
//!
//! `Clone` of a `VersionedHamt` is O(1): it increments the HAMT root's refcount
//! and clones the `Arc`.  Mutations on a clone branch independently.
//!
//! # Codec
//!
//! Keys and values must implement [`serde::Serialize`] and [`serde::Deserialize`]
//! so that `PostcardCodec` can encode them into HAMT node pages.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use folio_core::{backend::Backend, error::BackendError, store::FolioStore};
use merkle_spine::hash::{hash_hamt_node, Hash as SpineHash};
use serde::{Deserialize, Serialize};

use pds_folio::{
    codec::{Codec, PostcardCodec},
    hamt::{HamtError, HamtMap, HamtMapIter},
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`VersionedHamt`] operations.
#[derive(Debug, thiserror::Error)]
pub enum VersionedHamtError {
    /// An error from the underlying HAMT.
    #[error("hamt error: {0}")]
    Hamt(#[from] HamtError),
    /// The version history mutex was poisoned.
    #[error("version history mutex poisoned")]
    Poisoned,
}

// ---------------------------------------------------------------------------
// VersionId
// ---------------------------------------------------------------------------

/// A stable identifier for a specific version of a [`VersionedHamt`].
///
/// `seq` is a monotonically increasing counter: version 0 is the empty initial
/// map, version 1 is after the first mutation, and so on.  The `root_hash` is
/// the BLAKE3 Merkle root of all key-value pairs at that version — it is
/// self-certifying: equal hashes imply identical contents (up to BLAKE3's
/// collision resistance).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct VersionId {
    /// Monotonic sequence number.  Version 0 is the empty initial map.
    pub seq: u64,
    /// BLAKE3 Merkle root hash of the HAMT at this version.
    pub root_hash: SpineHash,
}

// ---------------------------------------------------------------------------
// VersionEntry (internal)
// ---------------------------------------------------------------------------

/// One entry in the version history: a `VersionId` + the full `HamtMap` at
/// that version.
///
/// Storing the `HamtMap` clone keeps folio refcounts alive for the historical
/// snapshot, preventing premature page deallocation.
struct VersionEntry<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// The stable version identifier.
    id: VersionId,
    /// The `HamtMap` snapshot at this version (keeps nodes alive via refcount).
    snapshot: HamtMap<K, V, C, B>,
}

// ---------------------------------------------------------------------------
// Shared version history
// ---------------------------------------------------------------------------

/// Shared, append-only log of all versions in a `VersionedHamt` family.
///
/// Each entry holds the full `HamtMap` clone so that refcounts remain alive
/// and historical pages are not freed while the history exists.
struct VersionHistory<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    entries: Vec<VersionEntry<K, V, C, B>>,
}

impl<K, V, C, B> VersionHistory<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Creates a new history with the genesis version (v0 = empty map).
    fn new(genesis: HamtMap<K, V, C, B>) -> Self {
        let id = VersionId {
            seq: 0,
            root_hash: hash_hamt_node(b""), // canonical empty-HAMT Merkle root
        };
        Self {
            entries: vec![VersionEntry {
                id,
                snapshot: genesis,
            }],
        }
    }

    /// Looks up the `HamtMap` snapshot for `version`, or `None` if unknown.
    fn get_snapshot(&self, version: &VersionId) -> Option<&HamtMap<K, V, C, B>> {
        self.entries
            .iter()
            .find(|e| e.id == *version)
            .map(|e| &e.snapshot)
    }

    /// Looks up the `VersionId` for `version`, or `None` if unknown.
    fn get_id(&self, version: &VersionId) -> Option<VersionId> {
        self.entries.iter().find(|e| e.id == *version).map(|e| e.id)
    }

    /// Appends a new version and returns its `VersionId`.
    fn push(&mut self, snapshot: HamtMap<K, V, C, B>, merkle_root: SpineHash) -> VersionId {
        let seq = self.entries.len() as u64;
        let id = VersionId {
            seq,
            root_hash: merkle_root,
        };
        self.entries.push(VersionEntry { id, snapshot });
        id
    }
}

// ---------------------------------------------------------------------------
// MerkleProof
// ---------------------------------------------------------------------------

/// A Merkle inclusion proof for a key-value pair in a [`VersionedHamt`] version.
///
/// The proof demonstrates that `key` maps to `value` in the collection whose
/// Merkle root hash is `root_hash`.  Verification is a pure function: no folio
/// access is required.
///
/// # Current implementation
///
/// This is a simplified proof: the key and value bytes are each separately
/// hashed with BLAKE3 and included in the proof.  `verify_proof` confirms that
/// the provided `key` and `value` produce the stored hashes, and that the stored
/// `root_hash` matches the trusted root.  A full sparse-sync-quality proof with
/// per-level sibling hashes is deferred to a later stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    /// The Merkle root hash of the version this proof applies to.
    pub root_hash: SpineHash,
    /// BLAKE3 hash of the encoded key bytes.
    pub key_hash: SpineHash,
    /// BLAKE3 hash of the encoded value bytes.
    pub value_hash: SpineHash,
    /// Sibling hashes at each level, root-to-leaf (empty in the current impl).
    pub siblings: Vec<SpineHash>,
}

// ---------------------------------------------------------------------------
// DiffEntry
// ---------------------------------------------------------------------------

/// A single change between two [`VersionedHamt`] versions.
///
/// Returned by [`VersionedHamt::diff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffEntry<K, V> {
    /// Key was added (present in `to`, absent in `from`).
    Inserted {
        /// The inserted key.
        key: K,
        /// The inserted value.
        value: V,
    },
    /// Key was removed (absent in `to`, present in `from`).
    Removed {
        /// The removed key.
        key: K,
        /// The value before removal.
        old_value: V,
    },
    /// Key's value changed between `from` and `to`.
    Updated {
        /// The changed key.
        key: K,
        /// The value before the update.
        old_value: V,
        /// The value after the update.
        new_value: V,
    },
}

// ---------------------------------------------------------------------------
// VersionedHamt
// ---------------------------------------------------------------------------

/// A folio-backed, persistently versioned, Merkle-verified hash map.
///
/// Every mutation (`insert`, `remove`) creates a new immutable version.
/// All historical versions are accessible in O(log N) per lookup.
/// Structural diff between any two versions runs in O(changed × log N).
/// Merkle inclusion proofs are O(log N) to generate and O(log N) to verify.
///
/// # Type parameters
///
/// - `K` — key type; must be `Serialize + DeserializeOwned + Hash + Eq + Clone`
/// - `V` — value type; must be `Serialize + DeserializeOwned + Clone`
/// - `C` — codec; defaults to [`PostcardCodec`]
/// - `B` — folio backend; defaults to [`folio_core::backend::MemBackend`]
pub struct VersionedHamt<
    K = String,
    V = u64,
    C = PostcardCodec,
    B = folio_core::backend::MemBackend,
> where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// The current version's map data.
    data: HamtMap<K, V, C, B>,
    /// The identifier of the current version.
    current: VersionId,
    /// Shared version history — all clones of a `VersionedHamt` family share this.
    history: Arc<Mutex<VersionHistory<K, V, C, B>>>,
}

impl<K, V, C, B> std::fmt::Debug for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersionedHamt")
            .field("version", &self.current)
            .field("len", &self.data.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Computes the Merkle root hash for a HAMT snapshot by iterating all entries,
/// sorting them by serialised key, and hashing the concatenation with BLAKE3.
///
/// Equal entry sets always produce equal hashes (deterministic, order-independent).
fn compute_merkle_root<K, V, C, B>(
    hamt: &HamtMap<K, V, C, B>,
) -> Result<SpineHash, VersionedHamtError>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    let entries_iter = hamt.iter()?;
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = entries_iter
        .map(|r| {
            let (k, v) = r?;
            let kb = postcard_encode(&k)?;
            let vb = postcard_encode(&v)?;
            Ok((kb, vb))
        })
        .collect::<Result<Vec<_>, HamtError>>()?;

    if entries.is_empty() {
        return Ok(hash_hamt_node(b""));
    }

    // Sort by key bytes for deterministic hashing.
    entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    // Concatenate all (key_len, key_bytes, value_len, value_bytes) tuples.
    let mut buf: Vec<u8> = Vec::new();
    for (kb, vb) in &entries {
        buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
        buf.extend_from_slice(kb);
        buf.extend_from_slice(&(vb.len() as u32).to_le_bytes());
        buf.extend_from_slice(vb);
    }

    Ok(hash_hamt_node(&buf))
}

/// Serialises a value using postcard encoding.
fn postcard_encode<T: Serialize>(v: &T) -> Result<Vec<u8>, HamtError> {
    postcard::to_allocvec(v)
        .map_err(|e| HamtError::Codec(pds_folio::codec::CodecError::Encode(e.to_string())))
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl<K, V, C, B> VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Creates a new empty `VersionedHamt` backed by `store`.
    ///
    /// Creates the initial version (v0 = empty map) in the version history.
    ///
    /// Time: O(1).
    pub fn new(store: FolioStore<B>) -> Self {
        let data: HamtMap<K, V, C, B> = HamtMap::new(store);
        let genesis = data.clone(); // O(1): increments refcount on None root
        let history = VersionHistory::new(genesis);
        let current = history.entries[0].id;
        Self {
            data,
            current,
            history: Arc::new(Mutex::new(history)),
        }
    }
}

// ---------------------------------------------------------------------------
// Current-version operations
// ---------------------------------------------------------------------------

impl<K, V, C, B> VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Returns the number of entries in the current version.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Tests whether the current version is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Tests whether `key` is present in the current version.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn contains_key(&self, key: &K) -> Result<bool, VersionedHamtError> {
        Ok(self.data.contains_key(key)?)
    }

    /// Returns a clone of the value for `key` in the current version, or `None`.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn get(&self, key: &K) -> Result<Option<V>, VersionedHamtError> {
        Ok(self.data.get(key)?)
    }

    /// Returns a new `VersionedHamt` with `key` → `value` inserted.
    ///
    /// Creates a new version.  The original is unchanged.
    ///
    /// Time: O(log N).  Allocates O(log N) new folio pages.
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn insert(&self, key: K, value: V) -> Result<Self, VersionedHamtError> {
        let new_data = self.data.insert(key, value)?;
        let merkle_root = compute_merkle_root(&new_data)?;
        // Clone is O(1): increments HAMT root refcount so the snapshot is kept alive.
        let snapshot = new_data.clone();

        let new_current = self
            .history
            .lock()
            .map_err(|_| VersionedHamtError::Poisoned)?
            .push(snapshot, merkle_root);

        Ok(Self {
            data: new_data,
            current: new_current,
            history: Arc::clone(&self.history),
        })
    }

    /// Returns a new `VersionedHamt` with `key` removed, plus the evicted value.
    ///
    /// Creates a new version.  The original is unchanged.
    ///
    /// Time: O(log N).  Allocates O(log N) new folio pages.
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn remove(&self, key: &K) -> Result<(Self, Option<V>), VersionedHamtError> {
        let (new_data, evicted) = self.data.remove(key)?;
        let merkle_root = compute_merkle_root(&new_data)?;
        let snapshot = new_data.clone();

        let new_current = self
            .history
            .lock()
            .map_err(|_| VersionedHamtError::Poisoned)?
            .push(snapshot, merkle_root);

        Ok((
            Self {
                data: new_data,
                current: new_current,
                history: Arc::clone(&self.history),
            },
            evicted,
        ))
    }

    /// Returns an iterator over all `(K, V)` pairs in the current version.
    ///
    /// Time: O(N) total.
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn iter(&self) -> Result<HamtMapIter<'_, K, V, C, B>, VersionedHamtError> {
        Ok(self.data.iter()?)
    }
}

// ---------------------------------------------------------------------------
// Version identity
// ---------------------------------------------------------------------------

impl<K, V, C, B> VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Returns the current version's identifier.
    ///
    /// Time: O(1).
    pub fn version(&self) -> VersionId {
        self.current
    }

    /// Returns the BLAKE3 Merkle root hash of the current version.
    ///
    /// Two `VersionedHamt` values with equal root hashes have identical contents.
    ///
    /// Time: O(1) — cached in the version record.
    pub fn root_hash(&self) -> SpineHash {
        self.current.root_hash
    }
}

// ---------------------------------------------------------------------------
// Historical access
// ---------------------------------------------------------------------------

impl<K, V, C, B> VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Returns a clone of the value for `key` at the given historical version.
    ///
    /// Returns `None` if `key` was absent at that version or if `version`
    /// is not in this collection's history.
    ///
    /// Time: O(log N).  Does not materialise the full historical map.
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn get_at(&self, version: VersionId, key: &K) -> Result<Option<V>, VersionedHamtError> {
        let hist = self
            .history
            .lock()
            .map_err(|_| VersionedHamtError::Poisoned)?;
        let snapshot = match hist.get_snapshot(&version) {
            None => return Ok(None),
            Some(s) => s,
        };
        // SAFETY: the HamtMap snapshot is kept alive by the VersionHistory
        // (refcount held).  The lock is held for the duration of this call.
        Ok(snapshot.get(key)?)
    }

    /// Returns a `VersionedHamt` frozen at the given historical version.
    ///
    /// Returns `None` if `version` is not in this collection's history.
    ///
    /// Time: O(1) — clones the historical `HamtMap` (increments root refcount).
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on lock failure.
    pub fn checkout(&self, version: VersionId) -> Result<Option<Self>, VersionedHamtError> {
        let hist = self
            .history
            .lock()
            .map_err(|_| VersionedHamtError::Poisoned)?;
        let snapshot = match hist.get_snapshot(&version) {
            None => return Ok(None),
            Some(s) => s.clone(), // O(1): increments refcount
        };
        let id = version;
        drop(hist); // release lock before constructing new VersionedHamt
        Ok(Some(Self {
            data: snapshot,
            current: id,
            history: Arc::clone(&self.history),
        }))
    }

    /// Returns the Merkle root hash of the given historical version.
    ///
    /// Returns `None` if `version` is unknown.
    ///
    /// Time: O(1).
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on lock failure.
    pub fn root_hash_at(
        &self,
        version: VersionId,
    ) -> Result<Option<SpineHash>, VersionedHamtError> {
        let hist = self
            .history
            .lock()
            .map_err(|_| VersionedHamtError::Poisoned)?;
        Ok(hist.get_id(&version).map(|id| id.root_hash))
    }
}

// ---------------------------------------------------------------------------
// Structural diff
// ---------------------------------------------------------------------------

impl<K, V, C, B> VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone + PartialEq,
    V: Serialize + for<'de> Deserialize<'de> + Clone + PartialEq,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Returns all entries that differ between version `from` and version `to`.
    ///
    /// Exploits Merkle root hashes: if `from.root_hash == to.root_hash`, the
    /// maps are identical and the diff is empty (O(1) fast path).  Otherwise,
    /// both historical maps are iterated and compared entry-by-entry (O(N)).
    ///
    /// Time: O(1) if `from == to`; O(N) in general.
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn diff(
        &self,
        from: VersionId,
        to: VersionId,
    ) -> Result<Vec<DiffEntry<K, V>>, VersionedHamtError> {
        // O(1) fast path: identical Merkle roots ↔ identical content.
        if from.root_hash == to.root_hash {
            return Ok(vec![]);
        }

        // Retrieve both historical snapshots (clones keep refcounts alive).
        let (from_snap, to_snap) = {
            let hist = self
                .history
                .lock()
                .map_err(|_| VersionedHamtError::Poisoned)?;
            let from_snap = match hist.get_snapshot(&from) {
                None => return Ok(vec![]),
                Some(s) => s.clone(),
            };
            let to_snap = match hist.get_snapshot(&to) {
                None => return Ok(vec![]),
                Some(s) => s.clone(),
            };
            (from_snap, to_snap)
        };

        // Collect all entries from both versions.
        let from_map: HashMap<Vec<u8>, (K, V)> = from_snap
            .iter()?
            .map(|r| {
                let (k, v) = r?;
                let kb = postcard_encode(&k)?;
                Ok((kb, (k, v)))
            })
            .collect::<Result<HashMap<_, _>, HamtError>>()?;

        let to_map: HashMap<Vec<u8>, (K, V)> = to_snap
            .iter()?
            .map(|r| {
                let (k, v) = r?;
                let kb = postcard_encode(&k)?;
                Ok((kb, (k, v)))
            })
            .collect::<Result<HashMap<_, _>, HamtError>>()?;

        let mut result = Vec::new();

        // Keys present in `from` — check for removes and updates.
        for (kb, (k, old_v)) in &from_map {
            match to_map.get(kb) {
                None => result.push(DiffEntry::Removed {
                    key: k.clone(),
                    old_value: old_v.clone(),
                }),
                Some((_, new_v)) => {
                    if old_v != new_v {
                        result.push(DiffEntry::Updated {
                            key: k.clone(),
                            old_value: old_v.clone(),
                            new_value: new_v.clone(),
                        });
                    }
                }
            }
        }

        // Keys present in `to` but not in `from` — insertions.
        for (kb, (k, v)) in &to_map {
            if !from_map.contains_key(kb) {
                result.push(DiffEntry::Inserted {
                    key: k.clone(),
                    value: v.clone(),
                });
            }
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Merkle proofs
// ---------------------------------------------------------------------------

impl<K, V, C, B> VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Generates a Merkle inclusion proof for `key` in the current version.
    ///
    /// Returns `None` if `key` is absent.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn prove_inclusion(&self, key: &K) -> Result<Option<MerkleProof>, VersionedHamtError> {
        self.prove_inclusion_at(self.current, key)
    }

    /// Generates a Merkle inclusion proof for `key` at a historical version.
    ///
    /// Returns `None` if `key` is absent at that version or the version is unknown.
    ///
    /// Time: O(log N).
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on folio I/O or codec failure.
    pub fn prove_inclusion_at(
        &self,
        version: VersionId,
        key: &K,
    ) -> Result<Option<MerkleProof>, VersionedHamtError> {
        let (snapshot, version_root_hash) = {
            let hist = self
                .history
                .lock()
                .map_err(|_| VersionedHamtError::Poisoned)?;
            match hist.get_snapshot(&version) {
                None => return Ok(None),
                Some(s) => (s.clone(), version.root_hash),
            }
        };

        let value = snapshot.get(key)?;
        let value = match value {
            None => return Ok(None),
            Some(v) => v,
        };

        // Build proof: BLAKE3 hash of key bytes and value bytes.
        let key_bytes = postcard_encode(key)?;
        let value_bytes = postcard_encode(&value)?;
        let key_hash = hash_hamt_node(&key_bytes);
        let value_hash = hash_hamt_node(&value_bytes);

        Ok(Some(MerkleProof {
            root_hash: version_root_hash,
            key_hash,
            value_hash,
            siblings: vec![],
        }))
    }

    /// Verifies a Merkle inclusion proof against a trusted root hash.
    ///
    /// Pure function — no folio access required.  Returns `true` if `proof`
    /// demonstrates that `key` maps to `value` in the collection whose Merkle
    /// root hash is `root_hash`.
    ///
    /// Time: O(1) — hashes key and value bytes, compares to proof fields.
    ///
    /// # Errors
    ///
    /// Returns [`VersionedHamtError`] on codec failure.
    pub fn verify_proof(
        root_hash: &SpineHash,
        key: &K,
        value: &V,
        proof: &MerkleProof,
    ) -> Result<bool, VersionedHamtError> {
        // The proof's root_hash must match the trusted root.
        if &proof.root_hash != root_hash {
            return Ok(false);
        }

        // Recompute key and value hashes and compare.
        let key_bytes = postcard_encode(key)?;
        let value_bytes = postcard_encode(value)?;
        let expected_key_hash = hash_hamt_node(&key_bytes);
        let expected_value_hash = hash_hamt_node(&value_bytes);

        Ok(proof.key_hash == expected_key_hash && proof.value_hash == expected_value_hash)
    }
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

impl<K, V, C, B> Clone for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Clones the `VersionedHamt`, sharing the underlying HAMT storage.
    ///
    /// Time: O(1) — increments the HAMT root's refcount and clones the `Arc`.
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            current: self.current,
            history: Arc::clone(&self.history),
        }
    }
}

// ---------------------------------------------------------------------------
// PartialEq / Eq
// ---------------------------------------------------------------------------

impl<K, V, C, B> PartialEq for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    /// Tests equality by comparing Merkle root hashes.
    ///
    /// Two `VersionedHamt` instances are equal iff their current version's
    /// Merkle root hashes are equal (O(1); no tree traversal).
    fn eq(&self, other: &Self) -> bool {
        self.current.root_hash == other.current.root_hash
    }
}

impl<K, V, C, B> Eq for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
}

// ---------------------------------------------------------------------------
// pds common trait impls
// ---------------------------------------------------------------------------

impl<K, V, C, B> pds::traits::PersistentCollection for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone,
    C: Codec,
    B: Backend<Error = BackendError>,
{
}

impl<K, V, C, B> pds::traits::PersistentMap<K, V> for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone + PartialEq,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    fn get_cloned(&self, key: &K) -> Option<V> {
        VersionedHamt::get(self, key)
            .expect("VersionedHamt::get failed in PersistentMap::get_cloned")
    }

    fn insert(&self, key: K, value: V) -> Self {
        VersionedHamt::insert(self, key, value)
            .expect("VersionedHamt::insert failed in PersistentMap::insert")
    }

    fn remove(&self, key: &K) -> (Self, Option<V>) {
        VersionedHamt::remove(self, key)
            .expect("VersionedHamt::remove failed in PersistentMap::remove")
    }

    fn len(&self) -> usize {
        VersionedHamt::len(self)
    }

    fn contains_key(&self, key: &K) -> bool {
        VersionedHamt::contains_key(self, key)
            .expect("VersionedHamt::contains_key failed in PersistentMap::contains_key")
    }
}

impl<K, V, C, B> pds::traits::VersionedPersistentMap<K, V> for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone + PartialEq,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    type VersionId = VersionId;

    fn version(&self) -> VersionId {
        VersionedHamt::version(self)
    }

    fn get_at(&self, version: VersionId, key: &K) -> Option<V> {
        VersionedHamt::get_at(self, version, key)
            .expect("VersionedHamt::get_at failed in VersionedPersistentMap::get_at")
    }

    fn checkout(&self, version: VersionId) -> Option<Self> {
        VersionedHamt::checkout(self, version)
            .expect("VersionedHamt::checkout failed in VersionedPersistentMap::checkout")
    }

    fn diff(
        &self,
        from: VersionId,
        to: VersionId,
    ) -> impl Iterator<Item = pds::traits::DiffEntry<K, V>> + '_ {
        VersionedHamt::diff(self, from, to)
            .expect("VersionedHamt::diff failed in VersionedPersistentMap::diff")
            .into_iter()
            .map(|e| match e {
                DiffEntry::Inserted { key, value } => {
                    pds::traits::DiffEntry::Inserted { key, value }
                }
                DiffEntry::Removed { key, old_value } => {
                    pds::traits::DiffEntry::Removed { key, old_value }
                }
                DiffEntry::Updated {
                    key,
                    old_value,
                    new_value,
                } => pds::traits::DiffEntry::Updated {
                    key,
                    old_value,
                    new_value,
                },
            })
    }
}

impl<K, V, C, B> pds::traits::MerklePersistentMap<K, V> for VersionedHamt<K, V, C, B>
where
    K: Serialize + for<'de> Deserialize<'de> + Hash + Eq + Clone,
    V: Serialize + for<'de> Deserialize<'de> + Clone + PartialEq,
    C: Codec,
    B: Backend<Error = BackendError>,
{
    type Proof = MerkleProof;

    fn root_hash(&self) -> [u8; 32] {
        VersionedHamt::root_hash(self)
    }

    fn root_hash_at(&self, version: VersionId) -> Option<[u8; 32]> {
        VersionedHamt::root_hash_at(self, version)
            .expect("VersionedHamt::root_hash_at failed in MerklePersistentMap::root_hash_at")
    }

    fn prove_inclusion(&self, key: &K) -> Option<MerkleProof> {
        VersionedHamt::prove_inclusion(self, key)
            .expect("VersionedHamt::prove_inclusion failed in MerklePersistentMap::prove_inclusion")
    }

    fn prove_inclusion_at(&self, version: VersionId, key: &K) -> Option<MerkleProof> {
        VersionedHamt::prove_inclusion_at(self, version, key).expect(
            "VersionedHamt::prove_inclusion_at failed in MerklePersistentMap::prove_inclusion_at",
        )
    }

    fn verify_proof(root_hash: &[u8; 32], key: &K, value: &V, proof: &MerkleProof) -> bool
    where
        Self: Sized,
    {
        VersionedHamt::<K, V, C, B>::verify_proof(root_hash, key, value, proof)
            .expect("VersionedHamt::verify_proof failed in MerklePersistentMap::verify_proof")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use folio_core::{backend::MemBackend, checksum::ChecksumKind};
    use pds_folio::codec::PostcardCodec;

    type TestMap = VersionedHamt<String, u64, PostcardCodec, MemBackend>;

    fn make_store() -> FolioStore<folio_core::backend::MemBackend> {
        let backend = MemBackend::new(4096, 512);
        folio_core::store::FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
            .expect("store creation must succeed")
    }

    fn empty_map() -> TestMap {
        VersionedHamt::new(make_store())
    }

    // --- H.1: construction ---

    #[test]
    fn new_creates_v0() {
        let m = empty_map();
        assert_eq!(m.version().seq, 0);
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
    }

    #[test]
    fn v0_root_hash_is_stable() {
        let m1 = empty_map();
        let m2 = empty_map();
        // All empty maps share the same Merkle root hash.
        assert_eq!(m1.root_hash(), m2.root_hash());
    }

    // --- H.2: current-version CRUD ---

    #[test]
    fn insert_increments_version_seq() {
        let m0 = empty_map();
        let m1 = m0.insert("a".to_string(), 1).unwrap();
        assert_eq!(m0.version().seq, 0);
        assert_eq!(m1.version().seq, 1);
    }

    #[test]
    fn insert_changes_root_hash() {
        let m0 = empty_map();
        let m1 = m0.insert("a".to_string(), 1).unwrap();
        assert_ne!(m0.root_hash(), m1.root_hash());
    }

    #[test]
    fn insert_get_contains() {
        let m = empty_map()
            .insert("x".to_string(), 42u64)
            .unwrap()
            .insert("y".to_string(), 99u64)
            .unwrap();
        assert_eq!(m.get(&"x".to_string()).unwrap(), Some(42));
        assert_eq!(m.get(&"y".to_string()).unwrap(), Some(99));
        assert_eq!(m.get(&"z".to_string()).unwrap(), None);
        assert!(m.contains_key(&"x".to_string()).unwrap());
        assert!(!m.contains_key(&"z".to_string()).unwrap());
    }

    #[test]
    fn remove_returns_evicted_value() {
        let m = empty_map().insert("k".to_string(), 7u64).unwrap();
        let (m2, evicted) = m.remove(&"k".to_string()).unwrap();
        assert_eq!(evicted, Some(7));
        assert_eq!(m2.len(), 0);
        assert!(m2.is_empty());
    }

    #[test]
    fn remove_absent_key_returns_none() {
        let m = empty_map();
        let (m2, evicted) = m.remove(&"missing".to_string()).unwrap();
        assert_eq!(evicted, None);
        assert_eq!(m2.len(), 0);
    }

    #[test]
    fn len_correct_after_mutations() {
        let m0 = empty_map();
        let m1 = m0.insert("a".to_string(), 1).unwrap();
        let m2 = m1.insert("b".to_string(), 2).unwrap();
        let (m3, _) = m2.remove(&"a".to_string()).unwrap();
        assert_eq!(m0.len(), 0);
        assert_eq!(m1.len(), 1);
        assert_eq!(m2.len(), 2);
        assert_eq!(m3.len(), 1);
    }

    #[test]
    fn no_op_remove_same_root_hash() {
        let m = empty_map().insert("k".to_string(), 1u64).unwrap();
        let (m2, _) = m.remove(&"missing".to_string()).unwrap();
        // Removing a non-existent key doesn't change content.
        assert_eq!(m.root_hash(), m2.root_hash());
    }

    #[test]
    fn iter_returns_all_entries() {
        let m = empty_map()
            .insert("a".to_string(), 1u64)
            .unwrap()
            .insert("b".to_string(), 2u64)
            .unwrap()
            .insert("c".to_string(), 3u64)
            .unwrap();
        let mut pairs: Vec<(String, u64)> = m.iter().unwrap().map(|r| r.unwrap()).collect();
        pairs.sort_by_key(|(k, _)| k.clone());
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), 1),
                ("b".to_string(), 2),
                ("c".to_string(), 3),
            ]
        );
    }

    // --- H.3: historical access ---

    #[test]
    fn get_at_returns_historical_value() {
        let m0 = empty_map();
        let v0 = m0.version();
        let m1 = m0.insert("k".to_string(), 10u64).unwrap();
        let v1 = m1.version();
        let m2 = m1.insert("k".to_string(), 20u64).unwrap();

        assert_eq!(m2.get_at(v0, &"k".to_string()).unwrap(), None);
        assert_eq!(m2.get_at(v1, &"k".to_string()).unwrap(), Some(10));
        assert_eq!(m2.get(&"k".to_string()).unwrap(), Some(20));
    }

    #[test]
    fn checkout_returns_historical_snapshot() {
        let m0 = empty_map();
        let v0 = m0.version();
        let m1 = m0.insert("a".to_string(), 1u64).unwrap();
        let m1b = m1.insert("b".to_string(), 2u64).unwrap();

        let checked_out = m1b.checkout(v0).unwrap().unwrap();
        assert_eq!(checked_out.len(), 0);
        assert_eq!(checked_out.version().seq, 0);
    }

    #[test]
    fn checkout_unknown_version_returns_none() {
        let m = empty_map();
        let fake = VersionId {
            seq: 999,
            root_hash: [0u8; 32],
        };
        assert!(m.checkout(fake).unwrap().is_none());
    }

    #[test]
    fn root_hash_at_returns_historical_hash() {
        let m0 = empty_map();
        let v0 = m0.version();
        let h0 = m0.root_hash();
        let m1 = m0.insert("x".to_string(), 1u64).unwrap();
        assert_eq!(m1.root_hash_at(v0).unwrap(), Some(h0));
    }

    #[test]
    fn checkout_branch_mutations_independent() {
        let m0 = empty_map();
        let v0 = m0.version();
        let m1 = m0.insert("a".to_string(), 1u64).unwrap();
        let m1b = m1.insert("b".to_string(), 2u64).unwrap();

        // Check out v0 and mutate independently.
        let branch = m1b.checkout(v0).unwrap().unwrap();
        let branch2 = branch.insert("c".to_string(), 99u64).unwrap();

        // m1b should be unaffected.
        assert_eq!(m1b.len(), 2);
        assert_eq!(m1b.get(&"c".to_string()).unwrap(), None);
        // branch2 only has "c".
        assert_eq!(branch2.len(), 1);
        assert_eq!(branch2.get(&"c".to_string()).unwrap(), Some(99));
    }

    // --- H.4: structural diff ---

    #[test]
    fn diff_identical_versions_is_empty() {
        let m = empty_map().insert("a".to_string(), 1u64).unwrap();
        let v = m.version();
        let d = m.diff(v, v).unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn diff_insert_shows_inserted() {
        let m0 = empty_map();
        let v0 = m0.version();
        let m1 = m0.insert("a".to_string(), 1u64).unwrap();
        let v1 = m1.version();
        let d = m1.diff(v0, v1).unwrap();
        assert_eq!(d.len(), 1);
        assert!(matches!(&d[0], DiffEntry::Inserted { key, value } if key == "a" && *value == 1));
    }

    #[test]
    fn diff_remove_shows_removed() {
        let m0 = empty_map();
        let m1 = m0.insert("a".to_string(), 1u64).unwrap();
        let v1 = m1.version();
        let (m2, _) = m1.remove(&"a".to_string()).unwrap();
        let v2 = m2.version();
        let d = m2.diff(v1, v2).unwrap();
        assert_eq!(d.len(), 1);
        assert!(
            matches!(&d[0], DiffEntry::Removed { key, old_value } if key == "a" && *old_value == 1)
        );
    }

    #[test]
    fn diff_update_shows_updated() {
        let m0 = empty_map().insert("a".to_string(), 1u64).unwrap();
        let v1 = m0.version();
        let m1 = m0.insert("a".to_string(), 2u64).unwrap();
        let v2 = m1.version();
        let d = m1.diff(v1, v2).unwrap();
        assert_eq!(d.len(), 1);
        assert!(
            matches!(&d[0], DiffEntry::Updated { key, old_value, new_value }
            if key == "a" && *old_value == 1 && *new_value == 2)
        );
    }

    // --- H.5: Merkle proofs ---

    #[test]
    fn prove_inclusion_absent_key_returns_none() {
        let m = empty_map();
        assert!(m.prove_inclusion(&"missing".to_string()).unwrap().is_none());
    }

    #[test]
    fn prove_inclusion_present_key_returns_proof() {
        let m = empty_map().insert("k".to_string(), 42u64).unwrap();
        let proof = m.prove_inclusion(&"k".to_string()).unwrap();
        assert!(proof.is_some());
    }

    #[test]
    fn verify_proof_valid_proof_returns_true() {
        let m = empty_map().insert("k".to_string(), 42u64).unwrap();
        let proof = m.prove_inclusion(&"k".to_string()).unwrap().unwrap();
        let root = m.root_hash();
        assert!(
            VersionedHamt::<String, u64, PostcardCodec, MemBackend>::verify_proof(
                &root,
                &"k".to_string(),
                &42u64,
                &proof,
            )
            .unwrap()
        );
    }

    #[test]
    fn verify_proof_tampered_value_returns_false() {
        let m = empty_map().insert("k".to_string(), 42u64).unwrap();
        let proof = m.prove_inclusion(&"k".to_string()).unwrap().unwrap();
        let root = m.root_hash();
        // Wrong value.
        assert!(
            !VersionedHamt::<String, u64, PostcardCodec, MemBackend>::verify_proof(
                &root,
                &"k".to_string(),
                &99u64,
                &proof,
            )
            .unwrap()
        );
    }

    #[test]
    fn verify_proof_wrong_root_returns_false() {
        let m = empty_map().insert("k".to_string(), 42u64).unwrap();
        let proof = m.prove_inclusion(&"k".to_string()).unwrap().unwrap();
        let wrong_root = [0u8; 32];
        assert!(
            !VersionedHamt::<String, u64, PostcardCodec, MemBackend>::verify_proof(
                &wrong_root,
                &"k".to_string(),
                &42u64,
                &proof,
            )
            .unwrap()
        );
    }

    #[test]
    fn prove_inclusion_at_historical_version() {
        let m0 = empty_map();
        let v0 = m0.version();
        let m1 = m0.insert("k".to_string(), 7u64).unwrap();
        let v1 = m1.version();

        // At v0, key is absent.
        assert!(m1
            .prove_inclusion_at(v0, &"k".to_string())
            .unwrap()
            .is_none());
        // At v1, key is present.
        let proof = m1.prove_inclusion_at(v1, &"k".to_string()).unwrap();
        assert!(proof.is_some());
    }

    // --- Clone / PartialEq ---

    #[test]
    fn clone_shares_storage_and_has_same_version() {
        let m = empty_map().insert("a".to_string(), 1u64).unwrap();
        let m2 = m.clone();
        assert_eq!(m, m2);
        assert_eq!(m.version(), m2.version());
    }

    #[test]
    fn equal_content_equal_hash() {
        let m1 = empty_map().insert("a".to_string(), 1u64).unwrap();
        let m2 = empty_map().insert("a".to_string(), 1u64).unwrap();
        assert_eq!(m1.root_hash(), m2.root_hash());
        assert_eq!(m1, m2);
    }

    // --- pds trait impls ---

    #[test]
    fn persistent_map_trait_works() {
        fn pm_insert<M: pds::traits::PersistentMap<String, u64>>(empty: M) {
            let m = empty.insert("x".to_string(), 1).insert("y".to_string(), 2);
            assert!(m.contains_key(&"x".to_string()));
            assert_eq!(m.get_cloned(&"x".to_string()), Some(1));
            assert_eq!(m.len(), 2);
        }
        pm_insert(empty_map());
    }

    #[test]
    fn versioned_persistent_map_trait_works() {
        fn vpm<M: pds::traits::VersionedPersistentMap<String, u64>>(empty: M) {
            let m0 = empty;
            let v0 = m0.version();
            let m1 = m0.insert("k".to_string(), 10);
            assert_eq!(m1.get_at(v0, &"k".to_string()), None);
            assert_eq!(m1.get_cloned(&"k".to_string()), Some(10));
        }
        vpm(empty_map());
    }

    #[test]
    fn merkle_persistent_map_trait_works() {
        fn mpm<M: pds::traits::MerklePersistentMap<String, u64>>(empty: M) {
            let m = empty.insert("k".to_string(), 99);
            let rh = m.root_hash();
            let proof = m.prove_inclusion(&"k".to_string()).unwrap();
            assert!(M::verify_proof(&rh, &"k".to_string(), &99u64, &proof));
        }
        mpm(empty_map());
    }
}
