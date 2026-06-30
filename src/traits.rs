//! Cross-variant trait layer for pds.
//!
//! Defines a portable trait hierarchy that works across the three pds
//! collection backends:
//!
//! - **pds** (in-memory, heap-backed) — implemented here for `HashMap`,
//!   `HashSet`, `Vector`, `OrdMap`, and `OrdSet`.
//! - **pds-folio** (folio page-backed) — `HamtMap`, `HamtSet`, etc. (planned).
//! - **pds-merkle-spine** (versioned, cryptographic) — `VersionedHamt` (planned).
//!
//! All traits are behind the `traits` Cargo feature.
//!
//! See `docs/cross-variant-traits.md` for the full design specification.

use std::fmt::Debug;
use std::hash::Hash;
use std::ops::RangeBounds;

use crate::hash_width::HashWidth;
use crate::shared_ptr::SharedPointerKind;

// --- Marker trait ---

/// Marker trait for persistent (immutable with structural sharing) collections.
///
/// Collections implementing this trait provide O(1) `Clone` via reference-count
/// increment — not a deep copy of all elements.
///
/// Implementations in this crate: [`crate::HashMap`], [`crate::HashSet`],
/// [`crate::Vector`], [`crate::OrdMap`], [`crate::OrdSet`].
pub trait PersistentCollection: Clone {}

// --- PersistentMap ---

/// A persistent (functional) map with O(log N) point operations and O(1) clone.
///
/// Implemented by all hash-map variants across the pds ecosystem.
///
/// # Value return convention
///
/// [`get_cloned`][PersistentMap::get_cloned] returns an owned `V`. This is
/// required for portability: folio-backed variants store values in mmap'd pages
/// whose lifetime is not tied to `&self`. In-memory pds implements this via
/// `HashMap::get(key).cloned()`.
///
/// For in-memory-only code that needs a reference (`Option<&V>`), use the
/// concrete [`crate::HashMap`] type directly rather than this trait.
pub trait PersistentMap<K, V>: PersistentCollection
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// Returns a clone of the value associated with `key`, or `None` if absent.
    ///
    /// Time: O(log N).
    fn get_cloned(&self, key: &K) -> Option<V>;

    /// Returns a new collection with `key` mapped to `value`.
    ///
    /// If `key` is already present, the old value is replaced. The original
    /// collection is unchanged.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new collection with `key` removed, plus the evicted value.
    ///
    /// If `key` is absent, returns `(self.clone(), None)`.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn remove(&self, key: &K) -> (Self, Option<V>)
    where
        Self: Sized;

    /// Returns the number of key-value pairs.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the collection is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tests whether `key` is present.
    ///
    /// Time: O(log N).
    fn contains_key(&self, key: &K) -> bool;
}

// --- PersistentSet ---

/// A persistent (functional) set with O(log N) point operations and O(1) clone.
///
/// Implemented by all hash-set variants across the pds ecosystem.
pub trait PersistentSet<A>: PersistentCollection
where
    A: Clone + Eq + Hash,
{
    /// Tests whether `value` is a member of the set.
    ///
    /// Time: O(log N).
    fn contains(&self, value: &A) -> bool;

    /// Returns a new set with `value` inserted.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn insert(&self, value: A) -> Self;

    /// Returns a new set with `value` removed.
    ///
    /// Time: O(log N). Allocates O(log N) new nodes via path-copy.
    fn remove(&self, value: &A) -> Self;

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// --- PersistentVector ---

/// A persistent (functional) vector with O(log N) point operations and O(1) clone.
///
/// Based on an RRB-tree. Index operations are O(log_B N) where B is the branching
/// factor (typically 32 — effectively O(1) for practical sizes).
pub trait PersistentVector<A>: PersistentCollection
where
    A: Clone,
{
    /// Returns the element at `index`, or `None` if out of bounds.
    ///
    /// Time: O(log N).
    fn get(&self, index: usize) -> Option<A>;

    /// Returns a new vector with `value` appended.
    ///
    /// Time: O(log N) amortised.
    fn push_back(&self, value: A) -> Self;

    /// Returns a new vector with `value` prepended.
    ///
    /// Time: O(log N) amortised.
    fn push_front(&self, value: A) -> Self;

    /// Returns a new vector with the element at `index` replaced by `value`.
    ///
    /// Time: O(log N).
    fn update(&self, index: usize, value: A) -> Self;

    /// Returns a new vector with the last element removed, plus that element.
    ///
    /// Time: O(log N).
    fn pop_back(&self) -> (Self, Option<A>)
    where
        Self: Sized;

    /// Returns a new vector with the first element removed, plus that element.
    ///
    /// Time: O(log N).
    fn pop_front(&self) -> (Self, Option<A>)
    where
        Self: Sized;

    /// Returns a new vector that is the concatenation of `self` and `other`.
    ///
    /// Time: O(log N).
    fn concat(&self, other: &Self) -> Self;

    /// Splits at `index`, returning `(left, right)`.
    ///
    /// Time: O(log N).
    fn split_at(&self, index: usize) -> (Self, Self)
    where
        Self: Sized;

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the vector is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// --- PersistentOrdMap ---

/// A persistent ordered map with O(log N) point operations and O(log N + k) range
/// queries, and O(1) clone.
///
/// `K: Ord` — keys are maintained in sorted order. Parallel to [`PersistentMap`]
/// (hash-based); they are not related by inheritance.
pub trait PersistentOrdMap<K, V>: PersistentCollection
where
    K: Clone + Ord,
    V: Clone,
{
    /// Returns a clone of the value for `key`, or `None`.
    ///
    /// Time: O(log N).
    fn get_cloned(&self, key: &K) -> Option<V>;

    /// Returns a new map with `key` → `value` inserted.
    ///
    /// Time: O(log N).
    fn insert(&self, key: K, value: V) -> Self;

    /// Returns a new map with `key` removed, plus the evicted value.
    ///
    /// Time: O(log N).
    fn remove(&self, key: &K) -> (Self, Option<V>)
    where
        Self: Sized;

    /// Returns the number of key-value pairs.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the map is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tests whether `key` is present.
    ///
    /// Time: O(log N).
    fn contains_key(&self, key: &K) -> bool;

    /// Returns the smallest key-value pair, or `None`.
    ///
    /// Time: O(log N).
    fn first(&self) -> Option<(K, V)>;

    /// Returns the largest key-value pair, or `None`.
    ///
    /// Time: O(log N).
    fn last(&self) -> Option<(K, V)>;

    /// Returns an iterator over pairs with keys in `bounds`, in ascending order.
    ///
    /// Time: O(log N) to seek; O(k) to iterate k results.
    fn range<R: RangeBounds<K>>(&self, bounds: R) -> impl Iterator<Item = (K, V)> + '_;
}

// --- PersistentOrdSet ---

/// A persistent ordered set.
///
/// Parallel to [`PersistentSet`] (hash-based); they are not related by inheritance.
pub trait PersistentOrdSet<A>: PersistentCollection
where
    A: Clone + Ord,
{
    /// Tests whether `value` is a member.
    ///
    /// Time: O(log N).
    fn contains(&self, value: &A) -> bool;

    /// Returns a new set with `value` inserted.
    ///
    /// Time: O(log N).
    fn insert(&self, value: A) -> Self;

    /// Returns a new set with `value` removed.
    ///
    /// Time: O(log N).
    fn remove(&self, value: &A) -> Self;

    /// Returns the number of elements.
    ///
    /// Time: O(1).
    fn len(&self) -> usize;

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the smallest element, or `None`.
    ///
    /// Time: O(log N).
    fn first(&self) -> Option<A>;

    /// Returns the largest element, or `None`.
    ///
    /// Time: O(log N).
    fn last(&self) -> Option<A>;

    /// Returns an iterator over elements in `bounds`, in ascending order.
    ///
    /// Time: O(log N) to seek; O(k) to iterate k results.
    fn range<R: RangeBounds<A>>(&self, bounds: R) -> impl Iterator<Item = A> + '_;
}

// --- VersionedPersistentMap ---

/// A single entry from a structural diff between two versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffEntry<K, V> {
    /// Key was inserted between `from` and `to`.
    Inserted {
        /// The inserted key.
        key: K,
        /// The inserted value.
        value: V,
    },
    /// Key was removed between `from` and `to`.
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

/// A persistent map that retains its full mutation history as navigable versions.
///
/// Every mutation (insert, remove) creates a new version. Past versions remain
/// readable indefinitely at O(log N) cost, with structural sharing between
/// adjacent versions.
///
/// # Version identity
///
/// [`VersionId`][VersionedPersistentMap::VersionId] is a stable, O(1)-comparable
/// handle to a specific point in the collection's history.
pub trait VersionedPersistentMap<K, V>: PersistentMap<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// The stable identifier for a snapshot of this collection.
    type VersionId: Copy + Eq + Hash + Debug;

    /// Returns the identifier of the current version.
    ///
    /// Time: O(1).
    fn version(&self) -> Self::VersionId;

    /// Returns a clone of the value of `key` at a specific historical version.
    ///
    /// Time: O(log N).
    fn get_at(&self, version: Self::VersionId, key: &K) -> Option<V>;

    /// Returns a read-only view frozen at `version`.
    ///
    /// Returns `None` if `version` is not in this collection's history.
    ///
    /// Time: O(1).
    fn checkout(&self, version: Self::VersionId) -> Option<Self>;

    /// Returns an iterator over diff entries between `from` and `to`.
    ///
    /// Time: O(changed_entries × log N). O(1) if `from == to`.
    fn diff(
        &self,
        from: Self::VersionId,
        to: Self::VersionId,
    ) -> impl Iterator<Item = DiffEntry<K, V>> + '_;
}

// --- MerklePersistentMap ---

/// A versioned persistent map with cryptographic Merkle identity.
///
/// The root hash of each version fully determines its contents. Inclusion proofs
/// let external parties verify that a key-value pair exists in a specific version
/// without access to the full collection.
pub trait MerklePersistentMap<K, V>: VersionedPersistentMap<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// An inclusion proof that a key has a specific value in a specific version.
    type Proof;

    /// Returns the BLAKE3 Merkle root hash of the current version.
    ///
    /// Two collections with equal root hashes have identical contents at that
    /// version (up to BLAKE3's 2⁻²⁵⁶ collision probability).
    ///
    /// Time: O(1) — cached in the version record.
    fn root_hash(&self) -> [u8; 32];

    /// Returns the Merkle root hash of the given historical version.
    ///
    /// Time: O(1) — retrieved from the version DAG.
    fn root_hash_at(&self, version: Self::VersionId) -> Option<[u8; 32]>;

    /// Generates a Merkle inclusion proof for `key` at the current version.
    ///
    /// Returns `None` if `key` is absent.
    ///
    /// Time: O(log N).
    fn prove_inclusion(&self, key: &K) -> Option<Self::Proof>;

    /// Generates a Merkle inclusion proof for `key` at a historical version.
    ///
    /// Returns `None` if `key` is absent at that version or the version is unknown.
    ///
    /// Time: O(log N).
    fn prove_inclusion_at(&self, version: Self::VersionId, key: &K) -> Option<Self::Proof>;

    /// Verifies a Merkle inclusion proof against a trusted root hash.
    ///
    /// Returns `true` if `proof` demonstrates that `key` maps to `value` in
    /// a collection whose root hash is `root_hash`. Pure function — no
    /// collection access required.
    ///
    /// Time: O(log N).
    fn verify_proof(root_hash: &[u8; 32], key: &K, value: &V, proof: &Self::Proof) -> bool
    where
        Self: Sized;
}

// --- Impls for in-memory pds types ---

// PersistentCollection blanket for all five in-memory types.
impl<K, V, S, P, H> PersistentCollection for crate::hashmap::GenericHashMap<K, V, S, P, H>
where
    K: Clone + Eq + Hash,
    V: Clone,
    S: Clone,
    P: SharedPointerKind,
    H: HashWidth,
{
}

impl<A, S, P, H> PersistentCollection for crate::hashset::GenericHashSet<A, S, P, H>
where
    A: Clone + Eq + Hash,
    S: Clone,
    P: SharedPointerKind,
    H: HashWidth,
{
}

impl<A: Clone, P: SharedPointerKind> PersistentCollection for crate::vector::GenericVector<A, P> {}

impl<K: Clone + Ord, V: Clone, P: SharedPointerKind> PersistentCollection
    for crate::ordmap::GenericOrdMap<K, V, P>
{
}

impl<A: Clone + Ord, P: SharedPointerKind> PersistentCollection
    for crate::ordset::GenericOrdSet<A, P>
{
}

// PersistentMap for GenericHashMap.
//
// `V: Hash` is required because the HAMT implementation hashes values to
// maintain per-node Merkle hashes (see `GenericHashMap::update`). This is
// more restrictive than the base `PersistentMap<K, V>` trait bound (`V: Clone`),
// but it is an inherent property of the HAMT structure.
impl<K, V, S, P, H> PersistentMap<K, V> for crate::hashmap::GenericHashMap<K, V, S, P, H>
where
    K: Clone + Eq + Hash,
    V: Clone + Hash,
    S: Clone + std::hash::BuildHasher + Default,
    P: SharedPointerKind,
    H: HashWidth,
{
    fn get_cloned(&self, key: &K) -> Option<V> {
        self.get(key).cloned()
    }

    fn insert(&self, key: K, value: V) -> Self {
        self.update(key, value)
    }

    fn remove(&self, key: &K) -> (Self, Option<V>) {
        let old = self.get(key).cloned();
        let new_map = self.without(key);
        (new_map, old)
    }

    fn len(&self) -> usize {
        crate::hashmap::GenericHashMap::len(self)
    }

    fn contains_key(&self, key: &K) -> bool {
        crate::hashmap::GenericHashMap::contains_key(self, key)
    }
}

// PersistentSet for GenericHashSet.
impl<A, S, P, H> PersistentSet<A> for crate::hashset::GenericHashSet<A, S, P, H>
where
    A: Clone + Eq + Hash,
    S: Clone + std::hash::BuildHasher + Default,
    P: SharedPointerKind,
    H: HashWidth,
{
    fn contains(&self, value: &A) -> bool {
        crate::hashset::GenericHashSet::contains(self, value)
    }

    fn insert(&self, value: A) -> Self {
        crate::hashset::GenericHashSet::update(self, value)
    }

    fn remove(&self, value: &A) -> Self {
        crate::hashset::GenericHashSet::without(self, value)
    }

    fn len(&self) -> usize {
        crate::hashset::GenericHashSet::len(self)
    }
}

// PersistentVector for GenericVector.
impl<A: Clone, P: SharedPointerKind> PersistentVector<A> for crate::vector::GenericVector<A, P> {
    fn get(&self, index: usize) -> Option<A> {
        crate::vector::GenericVector::get(self, index).cloned()
    }

    fn push_back(&self, value: A) -> Self {
        let mut v = self.clone();
        // Calls the concrete GenericVector::push_back, not the trait method.
        crate::vector::GenericVector::push_back(&mut v, value);
        v
    }

    fn push_front(&self, value: A) -> Self {
        let mut v = self.clone();
        // Calls the concrete GenericVector::push_front, not the trait method.
        crate::vector::GenericVector::push_front(&mut v, value);
        v
    }

    fn update(&self, index: usize, value: A) -> Self {
        let mut v = self.clone();
        v.set(index, value);
        v
    }

    fn pop_back(&self) -> (Self, Option<A>) {
        let mut v = self.clone();
        // Calls the concrete GenericVector::pop_back (returns Option<A>),
        // not this trait method (which returns (Self, Option<A>)).
        let elem = crate::vector::GenericVector::pop_back(&mut v);
        (v, elem)
    }

    fn pop_front(&self) -> (Self, Option<A>) {
        let mut v = self.clone();
        // Calls the concrete GenericVector::pop_front (returns Option<A>).
        let elem = crate::vector::GenericVector::pop_front(&mut v);
        (v, elem)
    }

    fn concat(&self, other: &Self) -> Self {
        let mut v = self.clone();
        v.append(other.clone());
        v
    }

    fn split_at(&self, index: usize) -> (Self, Self) {
        let mut left = self.clone();
        let right = left.split_off(index);
        (left, right)
    }

    fn len(&self) -> usize {
        crate::vector::GenericVector::len(self)
    }
}

// PersistentOrdMap for GenericOrdMap.
impl<K, V, P> PersistentOrdMap<K, V> for crate::ordmap::GenericOrdMap<K, V, P>
where
    K: Clone + Ord,
    V: Clone,
    P: SharedPointerKind,
{
    fn get_cloned(&self, key: &K) -> Option<V> {
        self.get(key).cloned()
    }

    fn insert(&self, key: K, value: V) -> Self {
        self.update(key, value)
    }

    fn remove(&self, key: &K) -> (Self, Option<V>) {
        let old = self.get(key).cloned();
        let new_map = self.without(key);
        (new_map, old)
    }

    fn len(&self) -> usize {
        crate::ordmap::GenericOrdMap::len(self)
    }

    fn contains_key(&self, key: &K) -> bool {
        crate::ordmap::GenericOrdMap::contains_key(self, key)
    }

    fn first(&self) -> Option<(K, V)> {
        self.get_min().map(|(k, v)| (k.clone(), v.clone()))
    }

    fn last(&self) -> Option<(K, V)> {
        self.get_max().map(|(k, v)| (k.clone(), v.clone()))
    }

    fn range<R: RangeBounds<K>>(&self, bounds: R) -> impl Iterator<Item = (K, V)> + '_ {
        // Calls GenericOrdMap::range (concrete), not this trait method.
        crate::ordmap::GenericOrdMap::range(self, bounds).map(|(k, v)| (k.clone(), v.clone()))
    }
}

// PersistentOrdSet for GenericOrdSet.
impl<A, P> PersistentOrdSet<A> for crate::ordset::GenericOrdSet<A, P>
where
    A: Clone + Ord,
    P: SharedPointerKind,
{
    fn contains(&self, value: &A) -> bool {
        crate::ordset::GenericOrdSet::contains(self, value)
    }

    fn insert(&self, value: A) -> Self {
        self.update(value)
    }

    fn remove(&self, value: &A) -> Self {
        self.without(value)
    }

    fn len(&self) -> usize {
        crate::ordset::GenericOrdSet::len(self)
    }

    fn first(&self) -> Option<A> {
        self.get_min().cloned()
    }

    fn last(&self) -> Option<A> {
        self.get_max().cloned()
    }

    fn range<R: RangeBounds<A>>(&self, bounds: R) -> impl Iterator<Item = A> + '_ {
        // Calls GenericOrdSet::range (concrete), not this trait method.
        crate::ordset::GenericOrdSet::range(self, bounds).cloned()
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashmap::HashMap, hashset::HashSet, ordmap::OrdMap, ordset::OrdSet, vector::Vector,
    };

    // --- PersistentMap ---

    fn pm_get_insert_contains<M: PersistentMap<i32, &'static str>>(mut_start: M) {
        let m = mut_start.insert(1, "one");
        assert_eq!(m.get_cloned(&1), Some("one"));
        assert_eq!(m.get_cloned(&2), None);
        assert!(m.contains_key(&1));
        assert!(!m.contains_key(&99));
        assert_eq!(m.len(), 1);
        assert!(!m.is_empty());
    }

    fn pm_remove<M: PersistentMap<i32, &'static str>>(mut_start: M) {
        let m = mut_start.insert(1, "one").insert(2, "two");
        let (m2, removed) = m.remove(&1);
        assert_eq!(removed, Some("one"));
        assert!(!m2.contains_key(&1));
        assert!(m2.contains_key(&2));
        // m unchanged
        assert!(m.contains_key(&1));
    }

    fn pm_is_empty<M: PersistentMap<i32, i32>>(empty: M) {
        assert!(empty.is_empty());
        let m = empty.insert(0, 0);
        assert!(!m.is_empty());
    }

    #[test]
    fn hashmap_persistent_map() {
        pm_get_insert_contains(HashMap::<i32, &str>::new());
        pm_remove(HashMap::<i32, &str>::new());
        pm_is_empty(HashMap::<i32, i32>::new());
    }

    // --- PersistentSet ---

    fn ps_insert_contains<S: PersistentSet<i32>>(empty: S) {
        let s = empty.insert(1).insert(2);
        assert!(s.contains(&1));
        assert!(s.contains(&2));
        assert!(!s.contains(&3));
        assert_eq!(s.len(), 2);
    }

    fn ps_remove<S: PersistentSet<i32>>(empty: S) {
        let s = empty.insert(1).insert(2);
        let s2 = s.remove(&1);
        assert!(!s2.contains(&1));
        assert!(s2.contains(&2));
        assert!(s.contains(&1)); // original unchanged
    }

    fn ps_is_empty<S: PersistentSet<i32>>(empty: S) {
        assert!(empty.is_empty());
        assert!(!empty.insert(1).is_empty());
    }

    #[test]
    fn hashset_persistent_set() {
        ps_insert_contains(HashSet::<i32>::new());
        ps_remove(HashSet::<i32>::new());
        ps_is_empty(HashSet::<i32>::new());
    }

    // --- PersistentVector ---

    fn pv_push_get<V: PersistentVector<i32>>(empty: V) {
        let v = empty.push_back(1).push_back(2).push_back(3);
        assert_eq!(v.get(0), Some(1));
        assert_eq!(v.get(1), Some(2));
        assert_eq!(v.get(2), Some(3));
        assert_eq!(v.get(3), None);
        assert_eq!(v.len(), 3);
        assert!(!v.is_empty());
    }

    fn pv_update<V: PersistentVector<i32>>(empty: V) {
        let v = empty.push_back(10).push_back(20);
        let v2 = v.update(0, 99);
        assert_eq!(v2.get(0), Some(99));
        assert_eq!(v2.get(1), Some(20));
        assert_eq!(v.get(0), Some(10)); // original unchanged
    }

    fn pv_pop<V: PersistentVector<i32>>(empty: V) {
        let v = empty.push_back(1).push_back(2);
        let (v2, e) = v.pop_back();
        assert_eq!(e, Some(2));
        assert_eq!(v2.len(), 1);

        let v3 = empty.push_back(3).push_back(4);
        let (v4, f) = v3.pop_front();
        assert_eq!(f, Some(3));
        assert_eq!(v4.len(), 1);
    }

    fn pv_is_empty<V: PersistentVector<i32>>(empty: V) {
        assert!(empty.is_empty());
        assert!(!empty.push_back(1).is_empty());
    }

    fn pv_split_concat<V: PersistentVector<i32>>(empty: V) {
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
    fn vector_persistent_vector() {
        pv_push_get(Vector::<i32>::new());
        pv_update(Vector::<i32>::new());
        pv_pop(Vector::<i32>::new());
        pv_is_empty(Vector::<i32>::new());
        pv_split_concat(Vector::<i32>::new());
    }

    // --- PersistentOrdMap ---

    fn pom_get_insert<M: PersistentOrdMap<i32, &'static str>>(empty: M) {
        let m = empty.insert(3, "three").insert(1, "one").insert(2, "two");
        assert_eq!(m.get_cloned(&1), Some("one"));
        assert_eq!(m.get_cloned(&4), None);
        assert!(m.contains_key(&2));
        assert!(!m.contains_key(&5));
        assert_eq!(m.len(), 3);
    }

    fn pom_first_last<M: PersistentOrdMap<i32, i32>>(empty: M) {
        let m = empty.insert(5, 50).insert(1, 10).insert(3, 30);
        assert_eq!(m.first(), Some((1, 10)));
        assert_eq!(m.last(), Some((5, 50)));
    }

    fn pom_remove<M: PersistentOrdMap<i32, i32>>(empty: M) {
        let m = empty.insert(1, 10).insert(2, 20);
        let (m2, v) = m.remove(&1);
        assert_eq!(v, Some(10));
        assert!(!m2.contains_key(&1));
        assert!(m.contains_key(&1)); // original unchanged
    }

    fn pom_range<M: PersistentOrdMap<i32, i32>>(empty: M) {
        let m = empty
            .insert(1, 10)
            .insert(3, 30)
            .insert(5, 50)
            .insert(7, 70);
        let pairs: Vec<_> = m.range(2..=6).collect();
        assert_eq!(pairs, vec![(3, 30), (5, 50)]);
    }

    #[test]
    fn ordmap_persistent_ord_map() {
        pom_get_insert(OrdMap::<i32, &str>::new());
        pom_first_last(OrdMap::<i32, i32>::new());
        pom_remove(OrdMap::<i32, i32>::new());
        pom_range(OrdMap::<i32, i32>::new());
    }

    // --- PersistentOrdSet ---

    fn pos_insert_contains<S: PersistentOrdSet<i32>>(empty: S) {
        let s = empty.insert(3).insert(1).insert(2);
        assert!(s.contains(&1));
        assert!(!s.contains(&4));
        assert_eq!(s.len(), 3);
    }

    fn pos_first_last<S: PersistentOrdSet<i32>>(empty: S) {
        let s = empty.insert(5).insert(1).insert(3);
        assert_eq!(s.first(), Some(1));
        assert_eq!(s.last(), Some(5));
    }

    fn pos_remove<S: PersistentOrdSet<i32>>(empty: S) {
        let s = empty.insert(1).insert(2);
        let s2 = s.remove(&1);
        assert!(!s2.contains(&1));
        assert!(s2.contains(&2));
        assert!(s.contains(&1)); // original unchanged
    }

    fn pos_range<S: PersistentOrdSet<i32>>(empty: S) {
        let s = empty.insert(1).insert(3).insert(5).insert(7);
        let elems: Vec<_> = s.range(2..=6).collect();
        assert_eq!(elems, vec![3, 5]);
    }

    #[test]
    fn ordset_persistent_ord_set() {
        pos_insert_contains(OrdSet::<i32>::new());
        pos_first_last(OrdSet::<i32>::new());
        pos_remove(OrdSet::<i32>::new());
        pos_range(OrdSet::<i32>::new());
    }

    // --- DiffEntry type ---

    #[test]
    fn diff_entry_types_usable() {
        let _inserted: DiffEntry<i32, &str> = DiffEntry::Inserted { key: 1, value: "a" };
        let _removed: DiffEntry<i32, &str> = DiffEntry::Removed {
            key: 2,
            old_value: "b",
        };
        let _updated: DiffEntry<i32, &str> = DiffEntry::Updated {
            key: 3,
            old_value: "c",
            new_value: "d",
        };
    }

    // --- PersistentCollection marker ---

    fn accepts_persistent_collection<C: PersistentCollection>(_c: &C) {}

    #[test]
    fn persistent_collection_marker_compiles() {
        let hm = HashMap::<i32, i32>::new();
        accepts_persistent_collection(&hm);
        let hs = HashSet::<i32>::new();
        accepts_persistent_collection(&hs);
        let v = Vector::<i32>::new();
        accepts_persistent_collection(&v);
        let om = OrdMap::<i32, i32>::new();
        accepts_persistent_collection(&om);
        let os = OrdSet::<i32>::new();
        accepts_persistent_collection(&os);
    }
}
