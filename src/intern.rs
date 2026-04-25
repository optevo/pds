// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Hash consing / interning for HAMT nodes.
//!
//! When the `hash-intern` feature is enabled, this module provides
//! [`InternPool`] — an explicit deduplication table for HAMT tree nodes.
//! Two nodes with identical content (verified via Merkle hash + structural
//! equality) are collapsed to share a single allocation, reducing memory
//! and enabling O(1) equality via `ptr_eq`.
//!
//! # Design
//!
//! The pool is an explicit, user-created object — not a global or
//! thread-local table. This is necessary because Rust cannot have
//! generic statics (`HamtNode<A, P, H>` is parameterised over three
//! type variables).
//!
//! Interning is bottom-up and post-hoc (Appel's insight): users call
//! `map.intern(&mut pool)` after bulk construction, not during each
//! insert. This avoids interning ephemeral intermediates.
//!
//! The pool uses strong references with explicit `purge()` eviction
//! (entries where `strong_count == 1` — only the pool holds them).
//! This avoids the need for weak references, which `triomphe::Arc`
//! does not support.
//!
//! # Scope
//!
//! HAMT nodes only. B+ tree nodes (OrdMap/OrdSet) lack Merkle hashes.
//! RRB tree nodes (Vector) have lazy Merkle hashes but ephemeral
//! mutation patterns make interning ROI negative.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "hash-intern")]
//! # {
//! use pds::HashMap;
//! use pds::intern::InternPool;
//!
//! let mut pool = InternPool::new();
//!
//! let mut map1: HashMap<String, i32> = (0..1000).map(|i| (format!("key{i}"), i)).collect();
//! let mut map2 = map1.clone();
//! map2.insert("key999".to_string(), 42);
//!
//! // Before interning: map1 and map2 have independent node allocations
//! map1.intern(&mut pool);
//! map2.intern(&mut pool);
//!
//! // After interning: shared subtrees point to the same allocations
//! assert!(pool.len() > 0);
//!
//! // Evict nodes only held by the pool
//! pool.purge();
//! # }
//! ```

use std::collections::HashMap as StdHashMap;
use std::vec::Vec;

use archery::{SharedPointer, SharedPointerKind};

use crate::hash_width::HashWidth;
use crate::hashset::Value as SetValue;
use crate::nodes::hamt::{CollisionNode, HamtNode, LargeSimdNode, SmallSimdNode};
use crate::shared_ptr::DefaultSharedPtr;

/// Statistics about intern pool usage.
#[derive(Clone, Debug, Default)]
pub struct InternStats {
    /// Number of intern attempts that found an existing match.
    pub hits: u64,
    /// Number of intern attempts that stored a new entry.
    pub misses: u64,
    /// Number of entries evicted by `purge()`.
    pub evictions: u64,
}

/// A deduplication pool for HAMT tree nodes.
///
/// Stores interned nodes keyed by their Merkle hash. When a node with
/// matching Merkle hash and equal content is found, the existing
/// `SharedPointer` is returned (deduplicating the allocation).
///
/// The pool uses strong references — call [`purge()`](InternPool::purge)
/// periodically to evict entries that are only held by the pool itself
/// (`strong_count == 1`).
pub struct InternPool<A, P: SharedPointerKind = DefaultSharedPtr, H: HashWidth = u64> {
    hamt: StdHashMap<u64, Vec<SharedPointer<HamtNode<A, P, H>, P>>>,
    small: StdHashMap<u64, Vec<SharedPointer<SmallSimdNode<A, H>, P>>>,
    large: StdHashMap<u64, Vec<SharedPointer<LargeSimdNode<A, H>, P>>>,
    collision: StdHashMap<u64, Vec<SharedPointer<CollisionNode<A, H>, P>>>,
    stats: InternStats,
}

impl<A, P: SharedPointerKind, H: HashWidth> InternPool<A, P, H> {
    /// Create a new, empty intern pool.
    pub fn new() -> Self {
        InternPool {
            hamt: StdHashMap::new(),
            small: StdHashMap::new(),
            large: StdHashMap::new(),
            collision: StdHashMap::new(),
            stats: InternStats::default(),
        }
    }

    /// Total number of interned nodes across all node types.
    pub fn len(&self) -> usize {
        self.hamt.values().map(Vec::len).sum::<usize>()
            + self.small.values().map(Vec::len).sum::<usize>()
            + self.large.values().map(Vec::len).sum::<usize>()
            + self.collision.values().map(Vec::len).sum::<usize>()
    }

    /// Whether the pool contains no interned nodes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return a snapshot of pool statistics (hits, misses, evictions).
    pub fn stats(&self) -> &InternStats {
        &self.stats
    }

    /// Evict entries where `strong_count == 1` (only the pool holds
    /// them). Call this periodically to reclaim memory from nodes that
    /// are no longer referenced by any collection.
    ///
    /// Runs iteratively until stable: evicting a parent HAMT node
    /// decrements its children's refcount, which may make them eligible
    /// for eviction in the next pass.
    pub fn purge(&mut self) {
        let before = self.len();
        loop {
            let before_pass = self.len();
            purge_map(&mut self.hamt);
            purge_map(&mut self.small);
            purge_map(&mut self.large);
            purge_map(&mut self.collision);
            if self.len() == before_pass {
                break;
            }
        }
        let after = self.len();
        self.stats.evictions += (before - after) as u64;
    }
}

/// Intern pool for [`HashSet`](crate::HashSet) nodes.
///
/// This is a type alias that hides the internal `Value<A>` wrapper used
/// by the set's HAMT. Use this instead of constructing an `InternPool`
/// with the raw element type.
pub type HashSetInternPool<A, P = DefaultSharedPtr, H = u64> =
    InternPool<SetValue<A>, P, H>;

impl<A, P: SharedPointerKind, H: HashWidth> Default for InternPool<A, P, H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> core::fmt::Debug for InternPool<A, P, H> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InternPool")
            .field("len", &self.len())
            .field("stats", &self.stats)
            .finish()
    }
}

/// Intern a `HamtNode`. If a node with the same Merkle hash and equal
/// content already exists in the pool, return the existing pointer.
/// Otherwise, store the input and return it.
impl<A: Clone + PartialEq, P: SharedPointerKind, H: HashWidth> InternPool<A, P, H> {
    pub(crate) fn intern_hamt(
        &mut self,
        node: SharedPointer<HamtNode<A, P, H>, P>,
    ) -> SharedPointer<HamtNode<A, P, H>, P> {
        let hash = node.merkle_hash;
        let bucket = self.hamt.entry(hash).or_default();
        for existing in bucket.iter() {
            if hamt_nodes_equal(existing, &node) {
                self.stats.hits += 1;
                return existing.clone();
            }
        }
        self.stats.misses += 1;
        bucket.push(node.clone());
        node
    }

    pub(crate) fn intern_small(
        &mut self,
        node: SharedPointer<SmallSimdNode<A, H>, P>,
    ) -> SharedPointer<SmallSimdNode<A, H>, P> {
        let hash = node.merkle_hash;
        let bucket = self.small.entry(hash).or_default();
        for existing in bucket.iter() {
            if simd_nodes_equal(existing, &node) {
                self.stats.hits += 1;
                return existing.clone();
            }
        }
        self.stats.misses += 1;
        bucket.push(node.clone());
        node
    }

    pub(crate) fn intern_large(
        &mut self,
        node: SharedPointer<LargeSimdNode<A, H>, P>,
    ) -> SharedPointer<LargeSimdNode<A, H>, P> {
        let hash = node.merkle_hash;
        let bucket = self.large.entry(hash).or_default();
        for existing in bucket.iter() {
            if simd_nodes_equal(existing, &node) {
                self.stats.hits += 1;
                return existing.clone();
            }
        }
        self.stats.misses += 1;
        bucket.push(node.clone());
        node
    }

    pub(crate) fn intern_collision(
        &mut self,
        node: SharedPointer<CollisionNode<A, H>, P>,
    ) -> SharedPointer<CollisionNode<A, H>, P> {
        let hash = node.hash.to_u64();
        let bucket = self.collision.entry(hash).or_default();
        for existing in bucket.iter() {
            if collision_nodes_equal(existing, &node) {
                self.stats.hits += 1;
                return existing.clone();
            }
        }
        self.stats.misses += 1;
        bucket.push(node.clone());
        node
    }
}

/// Remove entries from a bucket map where the SharedPointer's strong
/// count is 1 (only the pool holds a reference).
fn purge_map<K, T, P: SharedPointerKind>(map: &mut StdHashMap<K, Vec<SharedPointer<T, P>>>)
where
    K: std::hash::Hash + Eq,
{
    map.retain(|_, bucket| {
        bucket.retain(|ptr| SharedPointer::strong_count(ptr) > 1);
        !bucket.is_empty()
    });
}

/// Compare two HamtNodes for structural equality. Uses `ptr_eq` on
/// child entries first (interned children are pointer-equal), falling
/// back to value comparison for leaf entries.
fn hamt_nodes_equal<A: PartialEq, P: SharedPointerKind, H: HashWidth>(
    a: &HamtNode<A, P, H>,
    b: &HamtNode<A, P, H>,
) -> bool {
    if a.merkle_hash != b.merkle_hash {
        return false;
    }
    if a.data.len() != b.data.len() {
        return false;
    }
    // Compare each occupied entry. SparseChunk iterates by slot index,
    // so two nodes with the same content in the same hash positions will
    // yield entries in the same order.
    a.data.iter().zip(b.data.iter()).all(|(ea, eb)| entries_equal(ea, eb))
}

/// Compare two SIMD nodes for equality by checking their data elements.
fn simd_nodes_equal<A: PartialEq, H: HashWidth, const W: usize, const G: usize>(
    a: &crate::nodes::hamt::GenericSimdNode<A, H, W, G>,
    b: &crate::nodes::hamt::GenericSimdNode<A, H, W, G>,
) -> bool
where
    bitmaps::BitsImpl<W>: bitmaps::Bits,
{
    if a.merkle_hash != b.merkle_hash {
        return false;
    }
    if a.data.len() != b.data.len() {
        return false;
    }
    // SIMD nodes store (A, H) pairs — compare values
    a.data.iter().zip(b.data.iter()).all(|((va, ha), (vb, hb))| va == vb && ha == hb)
}

/// Compare two collision nodes for equality.
fn collision_nodes_equal<A: PartialEq, H: HashWidth>(
    a: &CollisionNode<A, H>,
    b: &CollisionNode<A, H>,
) -> bool {
    a.hash == b.hash && a.data.len() == b.data.len() && a.data == b.data
}

/// Compare two HAMT entries for equality. Uses `ptr_eq` for node
/// variants (interned children are pointer-identical) and value
/// comparison for leaf entries.
fn entries_equal<A: PartialEq, P: SharedPointerKind, H: HashWidth>(
    a: &crate::nodes::hamt::Entry<A, P, H>,
    b: &crate::nodes::hamt::Entry<A, P, H>,
) -> bool {
    use crate::nodes::hamt::Entry;

    match (a, b) {
        // For interned nodes, ptr_eq is the fast path
        (Entry::HamtNode(na), Entry::HamtNode(nb)) => {
            SharedPointer::ptr_eq(na, nb) || hamt_nodes_equal(na, nb)
        }
        (Entry::SmallSimdNode(na), Entry::SmallSimdNode(nb)) => {
            SharedPointer::ptr_eq(na, nb) || simd_nodes_equal(na, nb)
        }
        (Entry::LargeSimdNode(na), Entry::LargeSimdNode(nb)) => {
            SharedPointer::ptr_eq(na, nb) || simd_nodes_equal(na, nb)
        }
        (Entry::Collision(na), Entry::Collision(nb)) => {
            SharedPointer::ptr_eq(na, nb) || collision_nodes_equal(na, nb)
        }
        (Entry::Value(va, ha), Entry::Value(vb, hb)) => ha == hb && va == vb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HashMap;

    #[test]
    fn intern_cloned_identical_maps() {
        let mut pool = InternPool::new();
        // Two clones of the same map share structure initially.
        // After modifying both identically (rebuild from scratch with
        // the same hasher), interning should deduplicate.
        let base: HashMap<i32, i32> = (0..100).map(|i| (i, i * 2)).collect();
        let mut map1 = base.clone();
        let mut map2 = base.clone();

        // Modify both identically — they diverge from base but should
        // have identical tree structures (same hasher, same content)
        map1.insert(999, 999);
        map2.insert(999, 999);

        map1.intern(&mut pool);
        map2.intern(&mut pool);

        // Shared subtrees (from the unmodified portions) should hit
        assert!(pool.len() > 0);
        assert!(pool.stats().hits > 0, "expected hits from shared subtrees");
    }

    #[test]
    fn intern_diverged_maps() {
        let mut pool = InternPool::new();
        let base: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut map1 = base.clone();
        let mut map2 = base.clone();

        // Diverge content
        map1.insert(999, 1);
        map2.insert(999, 2);

        map1.intern(&mut pool);
        map2.intern(&mut pool);

        // Shared unmodified subtrees should still be deduplicated
        assert!(pool.len() > 0);
        assert!(pool.stats().hits > 0, "expected hits from shared subtrees");
    }

    #[test]
    fn purge_evicts_unreferenced() {
        let mut pool = InternPool::new();
        {
            let mut map: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
            map.intern(&mut pool);
            assert!(pool.len() > 0);
        }
        // map is dropped, pool holds the only references
        pool.purge();
        assert_eq!(pool.len(), 0);
        assert!(pool.stats().evictions > 0);
    }

    #[test]
    fn purge_retains_referenced() {
        let mut pool = InternPool::new();
        let mut map: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        map.intern(&mut pool);
        let before = pool.len();
        pool.purge();
        // map still alive, so pool entries are retained
        assert_eq!(pool.len(), before);
        drop(map);
    }

    #[test]
    fn intern_cloned_maps_share_nodes() {
        let mut pool = InternPool::new();
        let map1: HashMap<i32, i32> = (0..200).map(|i| (i, i)).collect();
        let mut map2 = map1.clone();
        map2.insert(999, 999);

        let mut map1 = map1;
        map1.intern(&mut pool);
        map2.intern(&mut pool);

        // Cloned maps with one modification should share most subtrees
        assert!(pool.stats().hits > 0, "expected shared subtree hits");
    }

    #[test]
    fn mutation_after_intern_works() {
        let mut pool = InternPool::new();
        let mut map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        map.intern(&mut pool);

        // Mutation after interning uses standard COW (make_mut clones
        // since refcount > 1 from pool)
        map.insert(999, 999);
        assert_eq!(map.get(&999), Some(&999));
        assert_eq!(map.len(), 101);
    }

    #[test]
    fn empty_pool() {
        let pool: InternPool<(i32, i32)> = InternPool::new();
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn intern_empty_map() {
        let mut pool = InternPool::new();
        let mut map: HashMap<i32, i32> = HashMap::new();
        map.intern(&mut pool);
        assert_eq!(pool.len(), 0); // empty map has no root node
    }

    #[test]
    fn hashset_intern() {
        use crate::HashSet;

        let mut pool = HashSetInternPool::new();
        let base: HashSet<i32> = (0..100).collect();
        let mut set1 = base.clone();
        let mut set2 = base.clone();

        set1.insert(999);
        set2.insert(999);

        set1.intern(&mut pool);
        set2.intern(&mut pool);

        assert!(pool.len() > 0);
        assert!(pool.stats().hits > 0, "expected shared subtree hits");
    }

    #[test]
    fn default_pool() {
        let pool: InternPool<(i32, i32)> = InternPool::default();
        assert!(pool.is_empty());
    }

    #[test]
    fn debug_format() {
        let pool: InternPool<(i32, i32)> = InternPool::new();
        let debug = format!("{pool:?}");
        assert!(debug.contains("InternPool"));
        assert!(debug.contains("len"));
    }
}
