//! Content-addressed Merkle identity over any [`PersistentMap`].
//!
//! [`MerkleWrapper<C, K, V>`] adds a BLAKE3 Merkle root hash to **any** type
//! that implements [`PersistentMap<K, V>`] — no folio dependency required.
//! It is the lightweight in-memory alternative to `pds-merkle-spine::VersionedHamt`.
//!
//! # Design
//!
//! The content hash **is** the version identity. Because pds collections are
//! functional (every mutation returns a new value), there is no need for a
//! separate history store. A [`VersionId`] is just the BLAKE3 Merkle root:
//!
//! ```text
//! VersionId = [u8; 32]   (the BLAKE3 Merkle root)
//! ```
//!
//! - `version()` → same value as `root_hash()`
//! - `get_at(vid, key)` → if `vid == self.version()` return current value, else `None`
//! - `checkout(vid)` → `Some(self.clone())` if `vid == self.version()`, else `None`
//! - `diff(from, to)` → iterates both versions, emits `DiffEntry` for changes
//!
//! # Merkle tree construction
//!
//! The root hash is a binary Merkle tree over sorted leaf hashes:
//!
//! 1. Serialise each `(key, value)` pair with `postcard`; hash with BLAKE3 → leaf hash.
//! 2. Sort leaves by their hash bytes.
//! 3. Build binary Merkle tree bottom-up, padding to the next power of 2 with zero hashes.
//! 4. Root = the final single node hash.
//!
//! # Two-tier Merkle capability
//!
//! | Capability | Type |
//! |------------|------|
//! | Content-addressed identity, in-memory only | `MerkleWrapper<C, K, V>` (this module) |
//! | Versioned history + disk durability + proofs | `pds-merkle-spine::VersionedHamt` |
//!
//! See the [`pds-folio` crate-level doc](pds_folio) and DEC-ARCH-MERKLE in
//! `docs/decisions.md` for guidance on which tier to use.

use std::marker::PhantomData;
use std::sync::OnceLock;

use serde_core::Serialize;

use crate::traits::{
    DiffEntry, MerklePersistentMap, PersistentCollection, PersistentMap, VersionedPersistentMap,
};

// --- MerkleWrapper struct ---

/// A [`PersistentMap`] wrapper that adds BLAKE3 Merkle identity.
///
/// `MerkleWrapper<C, K, V>` wraps any type `C` that implements
/// [`PersistentMap<K, V>`] and provides:
///
/// - [`root_hash`][MerkleWrapper::root_hash] — a deterministic BLAKE3 content hash of
///   all key-value pairs, cached in a [`OnceLock`].
/// - [`prove_inclusion`][MerkleWrapper::prove_inclusion] — a Merkle inclusion proof for
///   a given key.
/// - [`verify_proof`][MerkleWrapper::verify_proof] — pure-function proof verification.
/// - [`VersionedPersistentMap`] and [`MerklePersistentMap`] trait impls, where the
///   `VersionId` is the BLAKE3 root hash (`[u8; 32]`).
///
/// # Content addressing
///
/// The version identity is the content hash. Two `MerkleWrapper` values with the
/// same root hash have identical contents (up to BLAKE3's 2⁻²⁵⁶ collision probability).
/// There is no persistent history; `checkout` and `get_at` operate only on the current
/// version.
///
/// # Clone behaviour
///
/// Cloning `MerkleWrapper` clones the inner collection (O(1) via structural sharing)
/// and creates a **fresh** `OnceLock`. The root hash is cheap to recompute and is
/// deterministic, so this is correct.
///
/// # Type parameters
///
/// - `C` — the inner collection type (must implement `PersistentMap<K, V>`).
/// - `K` — key type; must implement `Clone + Eq + Hash + Serialize`.
/// - `V` — value type; must implement `Clone + Hash + Serialize`.
///
/// The `Serialize` bounds on `K` and `V` are placed on the impl blocks that need
/// them (hash computation), not on the struct itself — so you can hold a
/// `MerkleWrapper<C, K, V>` even if `K` or `V` are not serialisable, as long as you
/// do not call the hash/proof methods.
pub struct MerkleWrapper<C, K, V> {
    /// The wrapped inner collection.
    inner: C,
    /// Cached BLAKE3 Merkle root hash. Computed on first access; cleared on mutation
    /// (by creating a new wrapper). `OnceLock` is not `Clone`, so cloning creates a
    /// fresh lock — the hash is then recomputed on next access in the clone.
    root: OnceLock<[u8; 32]>,
    _k: PhantomData<K>,
    _v: PhantomData<V>,
}

impl<C, K, V> MerkleWrapper<C, K, V> {
    /// Wraps an existing inner collection in a `MerkleWrapper`.
    ///
    /// The root hash cache starts empty and is computed on first call to
    /// [`root_hash`][MerkleWrapper::root_hash].
    ///
    /// Time: O(1).
    pub fn new(inner: C) -> Self {
        MerkleWrapper {
            inner,
            root: OnceLock::new(),
            _k: PhantomData,
            _v: PhantomData,
        }
    }

    /// Returns a reference to the inner collection.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &C {
        &self.inner
    }

    /// Unwraps and returns the inner collection, discarding the cached hash.
    ///
    /// Time: O(1).
    pub fn into_inner(self) -> C {
        self.inner
    }
}

// Clone: clone the inner collection (O(1) structural share) but NOT the OnceLock.
// The hash is deterministic, so the fresh clone will compute the same value on demand.
impl<C: Clone, K, V> Clone for MerkleWrapper<C, K, V> {
    fn clone(&self) -> Self {
        MerkleWrapper {
            inner: self.inner.clone(),
            root: OnceLock::new(), // fresh lock; hash recomputed on first access
            _k: PhantomData,
            _v: PhantomData,
        }
    }
}

// --- Merkle hash computation ---

/// Computes the BLAKE3 binary Merkle root of a collection of `(key, value)` pairs.
///
/// Algorithm:
/// 1. Serialise each `(key, value)` with `postcard`; hash each with BLAKE3 → leaf hash.
/// 2. Sort leaf hashes lexicographically (gives a canonical, key-order-independent tree).
/// 3. Build a binary Merkle tree bottom-up, padding to the next power of 2 with `[0u8; 32]`.
/// 4. Return the root hash (the single remaining node after all pairings).
///
/// The empty collection produces `[0u8; 32]` (the zero hash, matching padding).
fn build_merkle_root<K, V>(pairs: impl Iterator<Item = (K, V)>) -> [u8; 32]
where
    K: Serialize,
    V: Serialize,
{
    // Step 1: hash each (key, value) pair.
    let mut leaves: Vec<[u8; 32]> = pairs
        .map(|(k, v)| {
            // Serialise the key and value separately, then hash them together.
            // Using postcard::to_allocvec for compact, no-std-compatible encoding.
            let key_bytes =
                postcard::to_allocvec(&k).expect("postcard serialisation of key must not fail");
            let value_bytes =
                postcard::to_allocvec(&v).expect("postcard serialisation of value must not fail");
            // Hash key_bytes || value_bytes in a single BLAKE3 hasher to produce a
            // single leaf hash that commits to both the key and the value.
            let mut h = blake3::Hasher::new();
            h.update(&key_bytes);
            h.update(&value_bytes);
            *h.finalize().as_bytes()
        })
        .collect();

    if leaves.is_empty() {
        return [0u8; 32];
    }

    // Step 2: sort leaves by hash bytes — canonical order regardless of insertion order.
    leaves.sort_unstable();

    // Step 3: build binary Merkle tree bottom-up.
    // Pad to next power of 2 with zero hashes so the tree is complete.
    let target_len = leaves.len().next_power_of_two();
    leaves.resize(target_len, [0u8; 32]);

    // Iteratively pair adjacent nodes until one root remains.
    while leaves.len() > 1 {
        leaves = leaves
            .chunks_exact(2)
            .map(|pair| {
                let mut h = blake3::Hasher::new();
                h.update(&pair[0]);
                h.update(&pair[1]);
                *h.finalize().as_bytes()
            })
            .collect();
    }

    // Exactly one element remains.
    leaves[0]
}

/// Computes the BLAKE3 leaf hash for a single `(key, value)` pair.
///
/// Consistent with [`build_merkle_root`] — uses the same postcard encoding and
/// hasher order.
fn leaf_hash<K: Serialize, V: Serialize>(key: &K, value: &V) -> [u8; 32] {
    let key_bytes =
        postcard::to_allocvec(key).expect("postcard serialisation of key must not fail");
    let value_bytes =
        postcard::to_allocvec(value).expect("postcard serialisation of value must not fail");
    let mut h = blake3::Hasher::new();
    h.update(&key_bytes);
    h.update(&value_bytes);
    *h.finalize().as_bytes()
}

// --- Trait impls ---

impl<C, K, V> PersistentCollection for MerkleWrapper<C, K, V>
where
    C: PersistentMap<K, V> + Clone,
    K: Clone + Eq + std::hash::Hash + Serialize,
    V: Clone + std::hash::Hash + Serialize,
{
}

impl<C, K, V> PersistentMap<K, V> for MerkleWrapper<C, K, V>
where
    C: PersistentMap<K, V> + Clone,
    K: Clone + Eq + std::hash::Hash + Serialize,
    V: Clone + std::hash::Hash + Serialize,
{
    fn get_cloned(&self, key: &K) -> Option<V> {
        self.inner.get_cloned(key)
    }

    /// Returns a new `MerkleWrapper` with `key` → `value` inserted.
    ///
    /// The root hash cache is cleared (a fresh `OnceLock` is created for the
    /// new value) because the mutation changes the content.
    ///
    /// Time: O(log N) for the inner insert; O(1) for the wrapper creation.
    fn insert(&self, key: K, value: V) -> Self {
        MerkleWrapper::new(self.inner.insert(key, value))
    }

    /// Returns a new `MerkleWrapper` with `key` removed, plus the evicted value.
    ///
    /// The root hash cache is cleared in the new wrapper.
    ///
    /// Time: O(log N) for the inner remove; O(1) for the wrapper creation.
    fn remove(&self, key: &K) -> (Self, Option<V>) {
        let (new_inner, old) = self.inner.remove(key);
        (MerkleWrapper::new(new_inner), old)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }
}

// MerkleWrapper requires iteration over K/V pairs to build the Merkle root.
// The `PersistentMap` trait does not expose iteration, so we require that the
// inner type also implements `IntoIterator<Item = (K, V)>` with `&C: IntoIterator`.
// This is satisfied by `GenericHashMap` (which implements `IntoIterator` for `&Self`
// yielding `(&K, &V)` — but we need owned values. We use a separate bounds
// extension in `VersionedPersistentMap` and `MerklePersistentMap` impl blocks
// that require `for<'a> &'a C: IntoIterator<Item = (&'a K, &'a V)>`.

impl<C, K, V> VersionedPersistentMap<K, V> for MerkleWrapper<C, K, V>
where
    C: PersistentMap<K, V> + Clone,
    K: Clone + Eq + std::hash::Hash + Serialize,
    V: Clone + std::hash::Hash + Serialize,
    for<'a> &'a C: IntoIterator<Item = (&'a K, &'a V)>,
{
    /// The version identifier is the BLAKE3 Merkle root hash.
    type VersionId = [u8; 32];

    fn version(&self) -> [u8; 32] {
        self.root_hash()
    }

    /// Returns a clone of the value for `key` at the given version.
    ///
    /// Because `MerkleWrapper` retains no history, this returns `Some` only if
    /// `version == self.version()` (i.e. the requested version is the current one).
    ///
    /// Time: O(log N) for the inner lookup; O(N) if the root hash has not yet
    /// been computed (amortised O(1) thereafter).
    fn get_at(&self, version: [u8; 32], key: &K) -> Option<V> {
        if version == self.version() {
            self.inner.get_cloned(key)
        } else {
            None
        }
    }

    /// Returns a clone of this wrapper if `version == self.version()`, else `None`.
    ///
    /// Because `MerkleWrapper` retains no history, only the current version is
    /// accessible via `checkout`.
    ///
    /// Time: O(1) clone + O(N) hash computation (if not yet cached).
    fn checkout(&self, version: [u8; 32]) -> Option<Self> {
        if version == self.version() {
            Some(self.clone())
        } else {
            None
        }
    }

    /// Returns an iterator over `DiffEntry` values between `from` and `to`.
    ///
    /// Because `MerkleWrapper` has no history, both `from` and `to` must equal
    /// `self.version()` for meaningful results. If they differ or one is not the
    /// current version, the diff treats the unknown version as empty.
    ///
    /// Implementation: iterates all pairs in the `to` version (if it equals
    /// `self.version()`), emitting `Inserted` for each. If `from == to`, emits
    /// nothing (empty diff). If both are `self.version()`, emits nothing (same
    /// content). If `from` is unknown but `to` is current, emits `Inserted` for
    /// everything.
    ///
    /// Time: O(N) to iterate all pairs when versions differ.
    fn diff(&self, from: [u8; 32], to: [u8; 32]) -> impl Iterator<Item = DiffEntry<K, V>> + '_ {
        let current = self.version();
        let from_is_current = from == current;
        let to_is_current = to == current;

        // Build the diff eagerly into a Vec to avoid lifetime issues with the
        // iterator referencing `self` across an `impl Trait` boundary.
        let mut entries: Vec<DiffEntry<K, V>> = Vec::new();

        if from == to {
            // Same version (or both unknown) — no diff.
        } else if from_is_current && !to_is_current {
            // `from` is current, `to` is unknown (empty) — everything is Removed.
            for (k, v) in &self.inner {
                entries.push(DiffEntry::Removed {
                    key: k.clone(),
                    old_value: v.clone(),
                });
            }
        } else if !from_is_current && to_is_current {
            // `from` is unknown (empty), `to` is current — everything is Inserted.
            for (k, v) in &self.inner {
                entries.push(DiffEntry::Inserted {
                    key: k.clone(),
                    value: v.clone(),
                });
            }
        }
        // If neither is the current version, treat both as empty — no diff.

        entries.into_iter()
    }
}

impl<C, K, V> MerklePersistentMap<K, V> for MerkleWrapper<C, K, V>
where
    C: PersistentMap<K, V> + Clone,
    K: Clone + Eq + std::hash::Hash + Serialize,
    V: Clone + std::hash::Hash + Serialize,
    for<'a> &'a C: IntoIterator<Item = (&'a K, &'a V)>,
{
    /// A Merkle inclusion proof.
    ///
    /// Each entry is `(sibling_hash, is_left_sibling)`:
    /// - `sibling_hash` — the hash of the sibling node at this level.
    /// - `is_left_sibling` — `true` if the sibling is the left child (i.e. the
    ///   current node is the right child at this level); `false` if the sibling
    ///   is the right child.
    ///
    /// Entries are ordered from the leaf level up to (but not including) the root.
    type Proof = Vec<([u8; 32], bool)>;

    /// Returns the BLAKE3 Merkle root hash of the current collection contents.
    ///
    /// The hash is computed once and cached in a [`OnceLock`]. Subsequent calls
    /// are O(1). The cache is NOT propagated through `Clone` — a fresh clone
    /// recomputes the hash on first access (O(N) that one time).
    ///
    /// The root hash is deterministic given the same key-value pairs: it depends
    /// only on the content, not insertion order.
    ///
    /// Time: O(N) on first call; O(1) thereafter.
    fn root_hash(&self) -> [u8; 32] {
        *self.root.get_or_init(|| {
            build_merkle_root(
                (&self.inner)
                    .into_iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            )
        })
    }

    /// Returns the root hash at a historical version.
    ///
    /// Because `MerkleWrapper` retains no history, this returns `Some` only when
    /// `version == self.version()`.
    ///
    /// Time: O(N) if hash not yet cached; O(1) thereafter.
    fn root_hash_at(&self, version: [u8; 32]) -> Option<[u8; 32]> {
        if version == self.root_hash() {
            Some(version)
        } else {
            None
        }
    }

    /// Generates a Merkle inclusion proof for `key` at the current version.
    ///
    /// Returns `None` if `key` is not present.
    ///
    /// The proof is a `Vec<([u8; 32], bool)>` of `(sibling_hash, is_left_sibling)`
    /// entries from the leaf level up to (but not including) the root.
    /// `is_left_sibling` is `true` when the sibling is the left child (current node
    /// is right), and `false` when the sibling is the right child (current node is
    /// left). This encodes the tree path needed for verification.
    ///
    /// Time: O(N log N) — builds all leaf hashes, sorts, constructs the tree,
    /// then extracts siblings for the target leaf.
    fn prove_inclusion(&self, key: &K) -> Option<Self::Proof> {
        // Only produce a proof if the key is present.
        let value = self.inner.get_cloned(key)?;

        // Build the full list of leaf hashes in the same order used by
        // build_merkle_root, so we can find the target leaf's position.
        let mut leaves: Vec<[u8; 32]> = (&self.inner)
            .into_iter()
            .map(|(k, v)| leaf_hash(k, v))
            .collect();

        if leaves.is_empty() {
            return None;
        }

        // The target leaf hash.
        let target = leaf_hash(key, &value);

        // Sort — must match build_merkle_root's sort order.
        leaves.sort_unstable();

        // Find the position of the target leaf.
        let mut pos = leaves.binary_search(&target).ok()?;

        // Pad to next power of 2.
        let target_len = leaves.len().next_power_of_two();
        leaves.resize(target_len, [0u8; 32]);

        // Collect (sibling_hash, is_left_sibling) entries as we walk up the tree.
        // `is_left_sibling` is true when the sibling is on the left (i.e. current
        // node is the right child, pos is odd).
        let mut proof = Vec::with_capacity(leaves.len().trailing_zeros() as usize);
        let mut current_level = leaves;

        while current_level.len() > 1 {
            // Even pos → current is left child, sibling is at pos+1 (right).
            // Odd pos  → current is right child, sibling is at pos-1 (left).
            let is_left_sibling = pos % 2 == 1;
            let sibling_pos = if pos % 2 == 0 { pos + 1 } else { pos - 1 };
            proof.push((current_level[sibling_pos], is_left_sibling));

            // Build the next level.
            current_level = current_level
                .chunks_exact(2)
                .map(|pair| {
                    let mut h = blake3::Hasher::new();
                    h.update(&pair[0]);
                    h.update(&pair[1]);
                    *h.finalize().as_bytes()
                })
                .collect();

            // Move up to the parent level — parent index is pos / 2.
            pos /= 2;
        }

        Some(proof)
    }

    /// Generates a Merkle inclusion proof for `key` at a historical version.
    ///
    /// Returns `None` if `key` is absent at that version or the version is not
    /// the current version (since `MerkleWrapper` has no history).
    ///
    /// Time: O(N log N).
    fn prove_inclusion_at(&self, version: [u8; 32], key: &K) -> Option<Self::Proof> {
        if version != self.root_hash() {
            return None;
        }
        self.prove_inclusion(key)
    }

    /// Verifies a Merkle inclusion proof against a trusted root hash.
    ///
    /// Returns `true` if `proof` demonstrates that `key` maps to `value` in a
    /// collection whose Merkle root is `root_hash`. This is a pure function —
    /// no collection access is required.
    ///
    /// Time: O(log N) where N is the collection size at the time the proof was generated.
    fn verify_proof(root_hash: &[u8; 32], key: &K, value: &V, proof: &Self::Proof) -> bool {
        let mut current = leaf_hash(key, value);

        for (sibling, is_left_sibling) in proof {
            // `is_left_sibling` encodes the position:
            // - true  → sibling is left child, current is right child
            // - false → sibling is right child, current is left child
            let mut h = blake3::Hasher::new();
            if *is_left_sibling {
                h.update(sibling);
                h.update(&current);
            } else {
                h.update(&current);
                h.update(sibling);
            }
            current = *h.finalize().as_bytes();
        }

        &current == root_hash
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashmap::HashMap;

    type TestMap = MerkleWrapper<HashMap<String, u64>, String, u64>;

    fn empty() -> TestMap {
        MerkleWrapper::new(HashMap::new())
    }

    // --- Root hash ---

    #[test]
    fn root_hash_empty_is_zero() {
        let m = empty();
        assert_eq!(m.root_hash(), [0u8; 32]);
    }

    #[test]
    fn root_hash_round_trip_same_content() {
        let m = empty()
            .insert("a".to_string(), 1u64)
            .insert("b".to_string(), 2u64);
        let m2 = empty()
            .insert("b".to_string(), 2u64)
            .insert("a".to_string(), 1u64);
        // Same content regardless of insertion order → same root.
        assert_eq!(m.root_hash(), m2.root_hash());
    }

    #[test]
    fn root_hash_mutation_changes_hash() {
        let m1 = empty().insert("x".to_string(), 42u64);
        let m2 = m1.insert("x".to_string(), 99u64);
        assert_ne!(m1.root_hash(), m2.root_hash());
    }

    #[test]
    fn root_hash_insert_changes_hash() {
        let m1 = empty();
        let m2 = m1.insert("k".to_string(), 1u64);
        assert_ne!(m1.root_hash(), m2.root_hash());
    }

    #[test]
    fn root_hash_cached_after_first_call() {
        let m = empty().insert("z".to_string(), 7u64);
        let h1 = m.root_hash();
        let h2 = m.root_hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn clone_has_same_root() {
        let m = empty().insert("c".to_string(), 3u64);
        let h = m.root_hash();
        let clone = m.clone();
        assert_eq!(clone.root_hash(), h);
    }

    // --- version / get_at / checkout ---

    #[test]
    fn version_equals_root_hash() {
        let m = empty().insert("v".to_string(), 5u64);
        assert_eq!(m.version(), m.root_hash());
    }

    #[test]
    fn get_at_current_version_returns_value() {
        let m = empty().insert("k".to_string(), 100u64);
        let vid = m.version();
        assert_eq!(m.get_at(vid, &"k".to_string()), Some(100u64));
    }

    #[test]
    fn get_at_wrong_version_returns_none() {
        let m = empty().insert("k".to_string(), 100u64);
        let wrong_vid = [0u8; 32];
        // Only fails if the root is actually non-zero.
        if m.version() != wrong_vid {
            assert_eq!(m.get_at(wrong_vid, &"k".to_string()), None);
        }
    }

    #[test]
    fn checkout_current_version_returns_clone() {
        let m = empty().insert("r".to_string(), 8u64);
        let vid = m.version();
        let checked = m.checkout(vid);
        assert!(checked.is_some());
        let c = checked.unwrap();
        assert_eq!(c.root_hash(), vid);
    }

    #[test]
    fn checkout_wrong_version_returns_none() {
        let m = empty().insert("r".to_string(), 8u64);
        let wrong_vid = [1u8; 32];
        if m.version() != wrong_vid {
            assert!(m.checkout(wrong_vid).is_none());
        }
    }

    // --- prove_inclusion / verify_proof ---

    #[test]
    fn prove_inclusion_absent_key_returns_none() {
        let m = empty().insert("a".to_string(), 1u64);
        assert!(m.prove_inclusion(&"z".to_string()).is_none());
    }

    #[test]
    fn proof_round_trip_single_entry() {
        let m = empty().insert("only".to_string(), 42u64);
        let root = m.root_hash();
        let proof = m
            .prove_inclusion(&"only".to_string())
            .expect("key is present");
        assert!(TestMap::verify_proof(
            &root,
            &"only".to_string(),
            &42u64,
            &proof
        ));
    }

    #[test]
    fn proof_round_trip_multiple_entries() {
        let m = empty()
            .insert("alpha".to_string(), 1u64)
            .insert("beta".to_string(), 2u64)
            .insert("gamma".to_string(), 3u64);
        let root = m.root_hash();

        for (k, v) in [
            ("alpha".to_string(), 1u64),
            ("beta".to_string(), 2u64),
            ("gamma".to_string(), 3u64),
        ] {
            let proof = m.prove_inclusion(&k).expect("key is present");
            assert!(
                TestMap::verify_proof(&root, &k, &v, &proof),
                "verify_proof failed for key {k}"
            );
        }
    }

    #[test]
    fn verify_proof_fails_for_tampered_value() {
        let m = empty().insert("key".to_string(), 1u64);
        let root = m.root_hash();
        let proof = m
            .prove_inclusion(&"key".to_string())
            .expect("key is present");
        // Use a different value — proof should fail.
        assert!(!TestMap::verify_proof(
            &root,
            &"key".to_string(),
            &999u64,
            &proof
        ));
    }

    #[test]
    fn verify_proof_fails_for_wrong_key() {
        let m = empty().insert("key".to_string(), 1u64);
        let root = m.root_hash();
        let proof = m
            .prove_inclusion(&"key".to_string())
            .expect("key is present");
        // Use a different key — proof should fail.
        assert!(!TestMap::verify_proof(
            &root,
            &"other_key".to_string(),
            &1u64,
            &proof
        ));
    }

    // --- PersistentMap delegation ---

    #[test]
    fn persistent_map_get_cloned() {
        let m = empty().insert("a".to_string(), 10u64);
        assert_eq!(m.get_cloned(&"a".to_string()), Some(10u64));
        assert_eq!(m.get_cloned(&"b".to_string()), None);
    }

    #[test]
    fn persistent_map_remove() {
        let m = empty()
            .insert("a".to_string(), 1u64)
            .insert("b".to_string(), 2u64);
        let (m2, removed) = m.remove(&"a".to_string());
        assert_eq!(removed, Some(1u64));
        assert!(!m2.contains_key(&"a".to_string()));
        // Original unchanged.
        assert!(m.contains_key(&"a".to_string()));
        // Root hash changes after remove.
        assert_ne!(m.root_hash(), m2.root_hash());
    }

    #[test]
    fn persistent_map_is_empty_len() {
        let m = empty();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        let m2 = m.insert("x".to_string(), 0u64);
        assert!(!m2.is_empty());
        assert_eq!(m2.len(), 1);
    }

    // --- diff ---

    #[test]
    fn diff_same_version_is_empty() {
        let m = empty().insert("a".to_string(), 1u64);
        let vid = m.version();
        let entries: Vec<_> = m.diff(vid, vid).collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn diff_unknown_to_current_yields_inserted() {
        let m = empty().insert("a".to_string(), 1u64);
        let current = m.version();
        let unknown = [0u8; 32];
        if current != unknown {
            let entries: Vec<_> = m.diff(unknown, current).collect();
            assert_eq!(entries.len(), 1);
            assert!(matches!(
                &entries[0],
                DiffEntry::Inserted { key, .. } if key == "a"
            ));
        }
    }

    #[test]
    fn diff_current_to_unknown_yields_removed() {
        let m = empty().insert("a".to_string(), 1u64);
        let current = m.version();
        let unknown = [0u8; 32];
        if current != unknown {
            let entries: Vec<_> = m.diff(current, unknown).collect();
            assert_eq!(entries.len(), 1);
            assert!(matches!(
                &entries[0],
                DiffEntry::Removed { key, .. } if key == "a"
            ));
        }
    }
}
