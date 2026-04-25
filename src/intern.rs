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
    fn independently_built_identical_maps_deduplicate() {
        // The critical test: two maps built from scratch (not cloned)
        // with the same deterministic hasher and same content must
        // produce identical HAMT trees. Interning should deduplicate
        // every node, giving ptr_eq on the roots.
        use crate::hash::map::GenericHashMap;
        use crate::shared_ptr::DefaultSharedPtr;
        use crate::test::LolHasher;
        use core::hash::BuildHasherDefault;

        type DetMap<K, V> =
            GenericHashMap<K, V, BuildHasherDefault<LolHasher>, DefaultSharedPtr>;

        let mut pool = InternPool::new();

        // Build two maps independently — no shared pointers between them
        let mut map1: DetMap<i32, i32> = (0..200).map(|i| (i, i * 3)).collect();
        let mut map2: DetMap<i32, i32> = (0..200).map(|i| (i, i * 3)).collect();

        // Sanity: they are equal but do NOT share root pointers
        assert_eq!(map1, map2);
        assert!(!map1.ptr_eq(&map2));

        map1.intern(&mut pool);
        map2.intern(&mut pool);

        // After interning, roots should be pointer-equal
        assert!(
            map1.ptr_eq(&map2),
            "independently built identical maps should share root after interning"
        );
        assert!(pool.stats().hits > 0);
    }

    #[test]
    fn cow_correctness_after_interning() {
        // Mutating an interned map must not corrupt the sibling
        // that shares nodes via the pool.
        let mut pool = InternPool::new();
        let base: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut map1 = base.clone();
        let mut map2 = base.clone();

        map1.intern(&mut pool);
        map2.intern(&mut pool);

        // Mutate map1 — COW should clone the affected path
        map1.insert(50, 9999);
        map1.remove(&0);

        // map2 must be unaffected
        assert_eq!(map2.get(&50), Some(&50));
        assert_eq!(map2.get(&0), Some(&0));
        assert_eq!(map2.len(), 100);

        // map1 reflects the mutations
        assert_eq!(map1.get(&50), Some(&9999));
        assert_eq!(map1.get(&0), None);
        assert_eq!(map1.len(), 99);
    }

    #[test]
    fn re_intern_after_mutation() {
        // Intern, mutate, re-intern. Unchanged subtrees should hit;
        // the mutated path should be new misses.
        let mut pool = InternPool::new();
        let mut map: HashMap<i32, i32> = (0..200).map(|i| (i, i)).collect();
        map.intern(&mut pool);

        let misses_before = pool.stats().misses;
        let hits_before = pool.stats().hits;

        // Mutate one key — only the path to that key changes
        map.insert(42, 9999);
        map.intern(&mut pool);

        // Unchanged subtrees produce more hits
        assert!(
            pool.stats().hits > hits_before,
            "unchanged subtrees should hit on re-intern"
        );
        // The mutated path produces new misses
        assert!(
            pool.stats().misses > misses_before,
            "mutated path should produce new misses"
        );
    }

    #[test]
    fn purge_cascading_eviction() {
        // A HAMT parent references HAMT children in the pool. Dropping
        // the map leaves all at strong_count==1, but single-pass purge
        // would visit children before parents (HashMap iteration order
        // is arbitrary) — seeing refcount > 1 because the parent still
        // holds a reference. The multi-pass fix handles this.
        let mut pool = InternPool::new();

        // Build a large enough map to have multi-level HAMT nodes
        {
            let mut map: HashMap<i32, i32> = (0..1000).map(|i| (i, i)).collect();
            map.intern(&mut pool);
        }
        // map is dropped — only pool holds references
        let before = pool.len();
        assert!(before > 0);

        pool.purge();

        // Every node should be evicted — nothing else references them
        assert_eq!(
            pool.len(),
            0,
            "cascading purge should evict all {} nodes",
            before
        );
        assert_eq!(pool.stats().evictions as usize, before);
    }

    #[test]
    fn collision_node_interning() {
        // Force HAMT hash collisions using LolHasher<5> (5-bit hashes).
        // Keys with the same 5-bit hash land in CollisionNode entries.
        // These should be internable too.
        use crate::hash::map::GenericHashMap;
        use crate::shared_ptr::DefaultSharedPtr;
        use crate::test::LolHasher;
        use core::hash::BuildHasherDefault;

        type NarrowMap<K, V> =
            GenericHashMap<K, V, BuildHasherDefault<LolHasher<5>>, DefaultSharedPtr>;

        let mut pool = InternPool::new();

        // Keys 0, 32, 64 all hash to 0 with LolHasher<5>
        let mut map1: NarrowMap<i32, i32> = NarrowMap::default();
        map1.insert(0, 100);
        map1.insert(32, 200);
        map1.insert(64, 300);

        let mut map2 = map1.clone();

        // Modify map2 at a non-colliding key so the collision subtree stays
        map2.insert(1, 999);

        map1.intern(&mut pool);
        map2.intern(&mut pool);

        // The collision node (keys 0, 32, 64) should be shared
        assert!(pool.len() > 0);
        assert!(
            pool.stats().hits > 0,
            "collision node should be deduplicated"
        );
    }

    #[test]
    fn stats_accuracy() {
        let mut pool = InternPool::new();
        assert_eq!(pool.stats().hits, 0);
        assert_eq!(pool.stats().misses, 0);
        assert_eq!(pool.stats().evictions, 0);

        // First map — every node is a miss
        let mut map1: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        map1.intern(&mut pool);

        let first_misses = pool.stats().misses;
        assert!(first_misses > 0);
        assert_eq!(pool.stats().hits, 0, "first intern should have zero hits");
        assert_eq!(pool.len() as u64, first_misses);

        // Clone + intern — every node should hit
        let mut map2 = map1.clone();
        map2.intern(&mut pool);

        assert_eq!(
            pool.stats().hits,
            first_misses,
            "cloned map should hit every node exactly once"
        );
        // Pool size unchanged — no new entries
        assert_eq!(pool.len() as u64, first_misses);

        // Drop both maps, purge
        drop(map1);
        drop(map2);
        pool.purge();
        assert_eq!(pool.stats().evictions, first_misses);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn intern_idempotent() {
        // Interning the same map twice should hit everything the second time
        let mut pool = InternPool::new();
        let mut map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();

        map.intern(&mut pool);
        let after_first = pool.stats().misses;

        map.intern(&mut pool);
        // Second intern: every node already in pool → all hits
        assert_eq!(
            pool.stats().hits, after_first,
            "re-interning same map should hit every node"
        );
        // No new misses
        assert_eq!(pool.stats().misses, after_first);
    }

    #[test]
    fn many_overlapping_maps() {
        // Intern many maps that share a common prefix. Pool size should
        // grow sub-linearly compared to total node count.
        let mut pool = InternPool::new();
        let base: HashMap<i32, i32> = (0..500).map(|i| (i, i)).collect();

        let mut maps: Vec<HashMap<i32, i32>> = Vec::new();
        for i in 0..20 {
            let mut m = base.clone();
            m.insert(1000 + i, i);
            maps.push(m);
        }

        for m in &mut maps {
            m.intern(&mut pool);
        }

        // Most nodes are shared — hits should vastly outnumber misses
        assert!(
            pool.stats().hits > pool.stats().misses * 5,
            "20 overlapping maps: hits={} should vastly exceed misses={}",
            pool.stats().hits,
            pool.stats().misses
        );
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
