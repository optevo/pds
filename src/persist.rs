// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Structural-sharing-preserving (SSP) serialisation.
//!
//! Standard serde impls flatten collections into sequences/maps, losing
//! internal tree structure. This module provides **pool-based**
//! serialisation that writes each node once and references shared nodes
//! by integer ID. On deserialisation, nodes are hash-consed on the fly
//! via [`InternPool`] for cross-session deduplication.
//!
//! See DEC-027 in `docs/decisions.md` for design rationale.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "persist")]
//! # {
//! use pds::HashMap;
//! use pds::persist::HashMapPool;
//!
//! let map1: HashMap<String, i32> = [("a".into(), 1), ("b".into(), 2)].into();
//! let mut map2 = map1.clone();
//! map2.insert("c".into(), 3);
//!
//! // Serialise both maps into a shared pool
//! let pool = HashMapPool::from_maps(&[&map1, &map2]);
//! let json = serde_json::to_string(&pool).unwrap();
//!
//! // Deserialise
//! let loaded: HashMapPool<String, i32> = serde_json::from_str(&json).unwrap();
//! let maps: Vec<HashMap<String, i32>> = loaded.to_maps();
//! assert_eq!(maps[0], map1);
//! assert_eq!(maps[1], map2);
//! # }
//! ```

use std::collections::HashMap as StdHashMap;
use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::vec::Vec;

use archery::{SharedPointer, SharedPointerKind};
use bitmaps::{Bits, BitsImpl};
use serde_core::de::{self, Deserialize, Deserializer, MapAccess, Visitor};
use serde_core::ser::{Serialize, SerializeMap, SerializeStruct, Serializer};

use crate::hash_width::HashWidth;
use crate::hashmap::GenericHashMap;
use crate::nodes::hamt::{CollisionNode, Entry, HamtNode, LargeSimdNode, SmallSimdNode, HASH_WIDTH};

// Note: SharedPointer, Entry, HamtNode, SmallSimdNode, LargeSimdNode, CollisionNode
// are used by PoolCollector (serialisation path). The reconstruction path uses
// FromIterator (re-inserts leaves into fresh maps).

// ─── Serialised pool types ───────────────────────────────────────────

/// A serialised entry within a HamtNode.
#[derive(Clone, Debug)]
enum PoolEntry<A, H> {
    /// Leaf value with its hash.
    Value(A, H),
    /// Reference to another node by pool index.
    Ref(u32),
}

/// A serialised node in the pool.
#[derive(Clone, Debug)]
enum PoolNode<A, H> {
    /// HamtNode: (slot_index, entry) pairs where entry is value or node ref.
    Hamt(Vec<(u8, PoolEntry<A, H>)>),
    /// SmallSimdNode: (slot_index, value, hash) triples.
    SimdSmall(Vec<(u8, A, H)>),
    /// LargeSimdNode: (slot_index, value, hash) triples.
    SimdLarge(Vec<(u8, A, H)>),
    /// CollisionNode: hash + values.
    Collision(H, Vec<A>),
}

/// Container metadata referencing nodes in the pool.
#[derive(Clone, Debug)]
struct PoolContainer {
    root: Option<u32>,
    size: usize,
}

// ─── HashMapPool ─────────────────────────────────────────────────────

/// A pool of serialised HashMap nodes with container metadata.
///
/// Multiple HashMaps can share nodes within the same pool. Serialise
/// with any serde-compatible format (JSON, bincode, etc.).
#[derive(Clone, Debug)]
pub struct HashMapPool<K, V, H: HashWidth = u64> {
    nodes: Vec<PoolNode<(K, V), H>>,
    containers: Vec<PoolContainer>,
}

// ─── Building pools (serialisation path) ─────────────────────────────

struct PoolCollector<A, H> {
    seen: StdHashMap<usize, u32>,
    nodes: Vec<PoolNode<A, H>>,
}

impl<A: Clone, H: HashWidth> PoolCollector<A, H> {
    fn new() -> Self {
        PoolCollector {
            seen: StdHashMap::new(),
            nodes: Vec::new(),
        }
    }

    fn push(&mut self, node: PoolNode<A, H>, addr: usize) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        self.seen.insert(addr, id);
        id
    }

    fn visit_hamt<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<HamtNode<A, P, H>, P>,
    ) -> u32
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        // Visit children first (post-order)
        let mut entries = Vec::with_capacity(node.data.len());
        for (slot, entry) in node.data.entries() {
            let pool_entry = match entry {
                Entry::Value(a, h) => PoolEntry::Value(a.clone(), *h),
                Entry::HamtNode(child) => PoolEntry::Ref(self.visit_hamt(child)),
                Entry::SmallSimdNode(child) => PoolEntry::Ref(self.visit_small_simd(child)),
                Entry::LargeSimdNode(child) => PoolEntry::Ref(self.visit_large_simd(child)),
                Entry::Collision(child) => PoolEntry::Ref(self.visit_collision(child)),
            };
            entries.push((slot as u8, pool_entry));
        }

        self.push(PoolNode::Hamt(entries), addr)
    }

    fn visit_small_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<SmallSimdNode<A, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.clone(), *hash))
            .collect();
        self.push(PoolNode::SimdSmall(entries), addr)
    }

    fn visit_large_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<LargeSimdNode<A, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.clone(), *hash))
            .collect();
        self.push(PoolNode::SimdLarge(entries), addr)
    }

    fn visit_collision<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<CollisionNode<A, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        self.push(PoolNode::Collision(node.hash, node.data.clone()), addr)
    }
}

// ─── Deduplication helpers ────────────────────────────────────────────
//
// These compare a freshly-built PoolEntry/PoolNode against an already-
// stored PoolNode. Used by DedupPoolCollector and SetDedupPoolCollector.
// The post-order traversal in the dedup collectors ensures that child
// Ref IDs are already normalised before any parent comparison, so these
// checks are always O(node_size) — never recursive.

fn pool_entry_eq<A: PartialEq, H: PartialEq>(a: &PoolEntry<A, H>, b: &PoolEntry<A, H>) -> bool {
    match (a, b) {
        (PoolEntry::Ref(ia), PoolEntry::Ref(ib)) => ia == ib,
        (PoolEntry::Value(va, ha), PoolEntry::Value(vb, hb)) => va == vb && ha == hb,
        _ => false,
    }
}

fn hamt_node_matches<A: PartialEq, H: PartialEq>(
    stored: &PoolNode<A, H>,
    entries: &[(u8, PoolEntry<A, H>)],
) -> bool {
    match stored {
        PoolNode::Hamt(se) => {
            se.len() == entries.len()
                && se
                    .iter()
                    .zip(entries)
                    .all(|(s, n)| s.0 == n.0 && pool_entry_eq(&s.1, &n.1))
        }
        _ => false,
    }
}

fn simd_small_node_matches<A: PartialEq, H: PartialEq>(
    stored: &PoolNode<A, H>,
    entries: &[(u8, A, H)],
) -> bool {
    match stored {
        PoolNode::SimdSmall(se) => {
            se.len() == entries.len()
                && se
                    .iter()
                    .zip(entries)
                    .all(|(s, n)| s.0 == n.0 && s.1 == n.1 && s.2 == n.2)
        }
        _ => false,
    }
}

fn simd_large_node_matches<A: PartialEq, H: PartialEq>(
    stored: &PoolNode<A, H>,
    entries: &[(u8, A, H)],
) -> bool {
    match stored {
        PoolNode::SimdLarge(se) => {
            se.len() == entries.len()
                && se
                    .iter()
                    .zip(entries)
                    .all(|(s, n)| s.0 == n.0 && s.1 == n.1 && s.2 == n.2)
        }
        _ => false,
    }
}

fn collision_node_matches<A: PartialEq, H: PartialEq>(
    stored: &PoolNode<A, H>,
    hash: H,
    values: &[A],
) -> bool {
    match stored {
        PoolNode::Collision(h, v) => *h == hash && v == values,
        _ => false,
    }
}

// ─── DedupPoolCollector ───────────────────────────────────────────────
//
// Extends PoolCollector with a Merkle-keyed secondary index so that
// content-equal nodes without shared pointers (e.g. independently-built
// maps, round-tripped maps) are still deduplicated.
//
// On a `seen` miss, reads the live node's `merkle_hash`, scans
// `merkle_index[hash]` for a content-equal stored entry, and reuses
// it if found. Correctness follows from post-order traversal: children
// are processed before parents, so by the time any parent is compared,
// all child Ref IDs are already normalised — making equality O(node_size).
//
// Collision nodes have no dedicated `merkle_hash`; their stored key hash
// (`node.hash.to_u64()`) serves as the bucket key.

struct DedupPoolCollector<A, H> {
    seen: StdHashMap<usize, u32>,
    merkle_index: StdHashMap<u64, Vec<u32>>,
    nodes: Vec<PoolNode<A, H>>,
}

impl<A: Clone + PartialEq, H: HashWidth> DedupPoolCollector<A, H> {
    fn new() -> Self {
        DedupPoolCollector {
            seen: StdHashMap::new(),
            merkle_index: StdHashMap::new(),
            nodes: Vec::new(),
        }
    }

    fn push(&mut self, node: PoolNode<A, H>, addr: usize, key: u64) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        self.seen.insert(addr, id);
        self.merkle_index.entry(key).or_default().push(id);
        id
    }

    fn visit_hamt<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<HamtNode<A, P, H>, P>,
    ) -> u32
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.merkle_hash;

        let mut entries = Vec::with_capacity(node.data.len());
        for (slot, entry) in node.data.entries() {
            let pool_entry = match entry {
                Entry::Value(a, h) => PoolEntry::Value(a.clone(), *h),
                Entry::HamtNode(child) => PoolEntry::Ref(self.visit_hamt(child)),
                Entry::SmallSimdNode(child) => PoolEntry::Ref(self.visit_small_simd(child)),
                Entry::LargeSimdNode(child) => PoolEntry::Ref(self.visit_large_simd(child)),
                Entry::Collision(child) => PoolEntry::Ref(self.visit_collision(child)),
            };
            entries.push((slot as u8, pool_entry));
        }

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if hamt_node_matches(&self.nodes[candidate_id as usize], &entries) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::Hamt(entries), addr, key)
    }

    fn visit_small_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<SmallSimdNode<A, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.merkle_hash;

        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.clone(), *hash))
            .collect();

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if simd_small_node_matches(&self.nodes[candidate_id as usize], &entries) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::SimdSmall(entries), addr, key)
    }

    fn visit_large_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<LargeSimdNode<A, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.merkle_hash;

        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.clone(), *hash))
            .collect();

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if simd_large_node_matches(&self.nodes[candidate_id as usize], &entries) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::SimdLarge(entries), addr, key)
    }

    fn visit_collision<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<CollisionNode<A, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        // Collision nodes have no merkle_hash; use the stored key hash
        // (which uniquely identifies the collision bucket).
        let key = node.hash.to_u64();
        let values = node.data.clone();

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if collision_node_matches(&self.nodes[candidate_id as usize], node.hash, &values) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::Collision(node.hash, values), addr, key)
    }
}

// ─── HashMapPool public API ──────────────────────────────────────────

impl<K: Clone, V: Clone, H: HashWidth> HashMapPool<K, V, H> {
    /// Build a pool from one or more HashMaps. Maps that share structure
    /// (e.g., clones with modifications) will deduplicate shared nodes.
    pub fn from_maps<S, P: SharedPointerKind>(maps: &[&GenericHashMap<K, V, S, P, H>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut collector: PoolCollector<(K, V), H> = PoolCollector::new();
        let mut containers = Vec::with_capacity(maps.len());

        for map in maps {
            let root = map.root.as_ref().map(|r| collector.visit_hamt(r));
            containers.push(PoolContainer {
                root,
                size: map.size,
            });
        }

        HashMapPool {
            nodes: collector.nodes,
            containers,
        }
    }

    /// Build a pool from a single HashMap.
    pub fn from_map<S, P: SharedPointerKind>(map: &GenericHashMap<K, V, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_maps(&[map])
    }
}

impl<K: Clone + PartialEq, V: Clone + PartialEq, H: HashWidth> HashMapPool<K, V, H> {
    /// Build a pool from one or more HashMaps, deduplicating content-equal
    /// nodes even when they do not share the same allocation.
    ///
    /// Unlike [`from_maps`][Self::from_maps], which deduplicates by pointer
    /// identity alone, this variant uses the HAMT node Merkle hash as a
    /// secondary index and performs a structural equality check on matches.
    ///
    /// **Hasher-lineage requirement.** Deduplication beyond pointer identity
    /// only fires when maps share the same hasher seed. Two maps cloned from
    /// a common ancestor always share a seed (cloning preserves the hasher).
    /// Maps constructed independently via `HashMap::new()` or `collect()`
    /// each get a fresh `RandomState` seed — their HAMT structures differ
    /// even for identical content, so no Merkle-keyed match is possible.
    ///
    /// This variant is useful for:
    ///
    /// - Maps cloned from a common source that were subsequently mutated
    ///   identically but independently, losing pointer sharing in the
    ///   modified subtrees.
    /// - Any scenario where the same series of inserts was applied to
    ///   multiple clones of the same base map.
    ///
    /// For cross-session or cross-process deduplication, use a deterministic
    /// hasher (e.g. `foldhash::fast::FixedState`) so all maps share the same
    /// hash function regardless of construction order.
    ///
    /// Requires `K: PartialEq, V: PartialEq`. If the extra bound is not
    /// available, use [`from_maps`][Self::from_maps] instead.
    ///
    /// Time: O(n) amortised (same as `from_maps`; Merkle hash collisions
    /// are negligible at 2^-64).
    pub fn from_maps_dedup<S, P: SharedPointerKind>(
        maps: &[&GenericHashMap<K, V, S, P, H>],
    ) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut collector = DedupPoolCollector::<(K, V), H>::new();
        let mut containers = Vec::with_capacity(maps.len());

        for map in maps {
            let root = map.root.as_ref().map(|r| collector.visit_hamt(r));
            containers.push(PoolContainer {
                root,
                size: map.size,
            });
        }

        HashMapPool {
            nodes: collector.nodes,
            containers,
        }
    }

    /// Build a deduplicating pool from a single HashMap.
    ///
    /// See [`from_maps_dedup`][Self::from_maps_dedup] for details.
    pub fn from_map_dedup<S, P: SharedPointerKind>(map: &GenericHashMap<K, V, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_maps_dedup(&[map])
    }
}

// ─── Reconstruction (deserialisation path) ───────────────────────────
//
// The pool preserves the original HAMT tree structure, but that
// structure depends on the hasher used during construction.
// Reconstructing the exact tree with a different hasher would produce
// maps where `get()` follows the wrong path.
//
// Instead, we extract all leaf (K, V) pairs and rebuild each map from
// scratch via `FromIterator`, which inserts elements with the new
// hasher. This guarantees correct lookups with any hasher.
//
// Structural sharing between deserialized maps is possible when the
// user supplies the same hasher to both maps and then calls
// `map.intern(&mut pool)` after deserialization.

impl<K, V, H: HashWidth> HashMapPool<K, V, H>
where
    K: Clone,
    V: Clone,
{
    /// Collect all leaf (K, V) pairs for a given container root.
    fn collect_leaves(&self, root_id: usize, out: &mut Vec<(K, V)>) {
        match &self.nodes[root_id] {
            PoolNode::Hamt(entries) => {
                for (_slot, entry) in entries {
                    match entry {
                        PoolEntry::Value((k, v), _h) => out.push((k.clone(), v.clone())),
                        PoolEntry::Ref(id) => self.collect_leaves(*id as usize, out),
                    }
                }
            }
            PoolNode::SimdSmall(entries) | PoolNode::SimdLarge(entries) => {
                for (_slot, (k, v), _h) in entries {
                    out.push((k.clone(), v.clone()));
                }
            }
            PoolNode::Collision(_hash, values) => {
                for (k, v) in values {
                    out.push((k.clone(), v.clone()));
                }
            }
        }
    }

    /// Reconstruct HashMaps from this pool.
    ///
    /// Each map is rebuilt by inserting all leaf elements into a fresh
    /// map using `S::default()` as the hasher. This ensures lookups
    /// work correctly regardless of the original hasher.
    ///
    /// For cross-session node deduplication, call
    /// [`GenericHashMap::intern`](crate::hashmap::GenericHashMap::intern)
    /// on each returned map with a shared [`InternPool`].
    pub fn to_maps<S, P>(&self) -> Vec<GenericHashMap<K, V, S, P, H>>
    where
        K: Hash + Eq,
        V: Hash,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.containers
            .iter()
            .map(|c| {
                if let Some(root_id) = c.root {
                    let mut pairs = Vec::with_capacity(c.size);
                    self.collect_leaves(root_id as usize, &mut pairs);
                    pairs.into_iter().collect()
                } else {
                    GenericHashMap::default()
                }
            })
            .collect()
    }

    /// Reconstruct a single HashMap (convenience for single-map pools).
    pub fn to_map<S, P>(&self) -> GenericHashMap<K, V, S, P, H>
    where
        K: Hash + Eq,
        V: Hash,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut maps = self.to_maps();
        assert!(!maps.is_empty(), "pool contains no containers");
        maps.swap_remove(0)
    }
}

// ─── Serde Serialize ─────────────────────────────────────────────────
//
// Format:
//   {
//     "nodes": [
//       {"h": [[[slot, [k,v], hash], ...], [[slot, ref_id], ...]]},
//       {"s": [[slot, [k,v], hash], ...]},
//       {"l": [[slot, [k,v], hash], ...]},
//       {"c": [hash, [[k,v], ...]]},
//       ...
//     ],
//     "containers": [[root_id_or_null, size], ...]
//   }
//
// Each node is a single-key map whose key is the type tag:
//   "h" = HamtNode, "s" = SmallSimdNode, "l" = LargeSimdNode, "c" = CollisionNode
//
// HAMT node value is a pair: (value_entries, ref_entries) — split to
// avoid heterogeneous tuples, making deserialization straightforward.

impl<A: Serialize, H: Serialize> Serialize for PoolNode<A, H> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            PoolNode::Hamt(entries) => {
                let values: Vec<_> = entries
                    .iter()
                    .filter_map(|(s, e)| match e {
                        PoolEntry::Value(a, h) => Some((s, a, h)),
                        _ => None,
                    })
                    .collect();
                let refs: Vec<_> = entries
                    .iter()
                    .filter_map(|(s, e)| match e {
                        PoolEntry::Ref(id) => Some((s, id)),
                        _ => None,
                    })
                    .collect();
                map.serialize_entry("h", &(&values, &refs))?;
            }
            PoolNode::SimdSmall(entries) => {
                map.serialize_entry("s", entries)?;
            }
            PoolNode::SimdLarge(entries) => {
                map.serialize_entry("l", entries)?;
            }
            PoolNode::Collision(hash, values) => {
                map.serialize_entry("c", &(hash, values))?;
            }
        }
        map.end()
    }
}

impl Serialize for PoolContainer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.root, &self.size).serialize(serializer)
    }
}

impl<K, V, H: HashWidth> Serialize for HashMapPool<K, V, H>
where
    K: Serialize,
    V: Serialize,
    H: Serialize,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HashMapPool", 2)?;
        state.serialize_field("nodes", &self.nodes)?;
        state.serialize_field("containers", &self.containers)?;
        state.end()
    }
}

// ─── Serde Deserialize ───────────────────────────────────────────────
//
// Manual implementations — serde derive macros require the `derive`
// feature on the serde crate, which this library does not enable
// (the dependency is named `serde_core`).

impl<'de, K, V, H: HashWidth> Deserialize<'de> for HashMapPool<K, V, H>
where
    K: Deserialize<'de> + Clone,
    V: Deserialize<'de> + Clone,
    H: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PoolVisitor<K, V, H>(PhantomData<(K, V, H)>);

        impl<'de, K, V, H: HashWidth> Visitor<'de> for PoolVisitor<K, V, H>
        where
            K: Deserialize<'de> + Clone,
            V: Deserialize<'de> + Clone,
            H: Deserialize<'de>,
        {
            type Value = HashMapPool<K, V, H>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a HashMapPool struct")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
                let mut nodes: Option<Vec<PoolNode<(K, V), H>>> = None;
                let mut containers: Option<Vec<PoolContainer>> = None;

                while let Some(key) = access.next_key::<&str>()? {
                    match key {
                        "nodes" => {
                            let raw: Vec<TaggedNode<K, V, H>> = access.next_value()?;
                            nodes = Some(raw.into_iter().map(|t| t.0).collect());
                        }
                        "containers" => {
                            let raw: Vec<(Option<u32>, usize)> = access.next_value()?;
                            containers = Some(
                                raw.into_iter()
                                    .map(|(root, size)| PoolContainer { root, size })
                                    .collect(),
                            );
                        }
                        _ => {
                            let _ = access.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(HashMapPool {
                    nodes: nodes.ok_or_else(|| de::Error::missing_field("nodes"))?,
                    containers: containers
                        .ok_or_else(|| de::Error::missing_field("containers"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "HashMapPool",
            &["nodes", "containers"],
            PoolVisitor(PhantomData),
        )
    }
}

/// Wrapper for deserializing a single tagged `PoolNode` from the
/// `{"h": ...}` / `{"s": ...}` / `{"l": ...}` / `{"c": ...}` format.
struct TaggedNode<K, V, H>(PoolNode<(K, V), H>);

impl<'de, K, V, H> Deserialize<'de> for TaggedNode<K, V, H>
where
    K: Deserialize<'de>,
    V: Deserialize<'de>,
    H: Deserialize<'de> + HashWidth,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor<K, V, H>(PhantomData<(K, V, H)>);

        impl<'de, K, V, H> Visitor<'de> for NodeVisitor<K, V, H>
        where
            K: Deserialize<'de>,
            V: Deserialize<'de>,
            H: Deserialize<'de> + HashWidth,
        {
            type Value = TaggedNode<K, V, H>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a tagged pool node object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let tag: String =
                    map.next_key()?.ok_or_else(|| de::Error::custom("empty node object"))?;

                let node = match tag.as_str() {
                    "h" => {
                        // HAMT: pair of (value_entries, ref_entries)
                        let (values, refs): (Vec<(u8, (K, V), H)>, Vec<(u8, u32)>) =
                            map.next_value()?;
                        let mut entries = Vec::with_capacity(values.len() + refs.len());
                        for (slot, val, hash) in values {
                            entries.push((slot, PoolEntry::Value(val, hash)));
                        }
                        for (slot, id) in refs {
                            entries.push((slot, PoolEntry::Ref(id)));
                        }
                        PoolNode::Hamt(entries)
                    }
                    "s" => {
                        let entries: Vec<(u8, (K, V), H)> = map.next_value()?;
                        PoolNode::SimdSmall(entries)
                    }
                    "l" => {
                        let entries: Vec<(u8, (K, V), H)> = map.next_value()?;
                        PoolNode::SimdLarge(entries)
                    }
                    "c" => {
                        let (hash, values): (H, Vec<(K, V)>) = map.next_value()?;
                        PoolNode::Collision(hash, values)
                    }
                    other => {
                        return Err(de::Error::custom(format!("unknown node tag: {other}")));
                    }
                };

                Ok(TaggedNode(node))
            }
        }

        deserializer.deserialize_map(NodeVisitor(PhantomData))
    }
}

// ─── OrdMap/OrdSet pool types ────────────────────────────────────────

use crate::nodes::btree::{Branch, Children, Leaf, Node as BTreeNode};
use crate::ord::map::GenericOrdMap;
use crate::ord::set::GenericOrdSet;

/// A serialised node in the B+ tree pool.
#[derive(Clone, Debug)]
enum OrdPoolNode<K, V> {
    /// Branch with leaf children: separator keys + leaf pool IDs.
    BranchLeaves { keys: Vec<K>, children: Vec<u32> },
    /// Branch with branch children: separator keys + branch pool IDs + level.
    BranchBranches {
        keys: Vec<K>,
        children: Vec<u32>,
        level: usize,
    },
    /// Leaf: key-value pairs in sorted order.
    Leaf(Vec<(K, V)>),
}

/// A pool of serialised B+ tree nodes with container metadata.
///
/// Multiple OrdMaps can share nodes within the same pool. Serialise
/// with any serde-compatible format (JSON, bincode, etc.).
///
/// # Example
///
/// ```
/// # #[cfg(feature = "persist")]
/// # {
/// use pds::OrdMap;
/// use pds::persist::OrdMapPool;
///
/// let map1: OrdMap<String, i32> = [("a".into(), 1), ("b".into(), 2)].into();
/// let mut map2 = map1.clone();
/// map2.insert("c".into(), 3);
///
/// let pool = OrdMapPool::from_maps(&[&map1, &map2]);
/// let json = serde_json::to_string(&pool).unwrap();
///
/// let loaded: OrdMapPool<String, i32> = serde_json::from_str(&json).unwrap();
/// let maps: Vec<OrdMap<String, i32>> = loaded.to_maps();
/// assert_eq!(maps[0], map1);
/// assert_eq!(maps[1], map2);
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct OrdMapPool<K, V> {
    nodes: Vec<OrdPoolNode<K, V>>,
    containers: Vec<PoolContainer>,
}

/// Type alias for [`OrdMapPool`] specialised for sets (`V = ()`).
///
/// Use [`from_sets`][OrdMapPool::from_sets] and
/// [`to_sets`][OrdMapPool::to_sets] for ergonomic set operations.
pub type OrdSetPool<A> = OrdMapPool<A, ()>;

// ─── OrdMap pool collector ──────────────────────────────────────────

struct OrdPoolCollector<K, V> {
    seen: StdHashMap<usize, u32>,
    nodes: Vec<OrdPoolNode<K, V>>,
}

impl<K: Clone, V: Clone> OrdPoolCollector<K, V> {
    fn new() -> Self {
        OrdPoolCollector {
            seen: StdHashMap::new(),
            nodes: Vec::new(),
        }
    }

    fn push(&mut self, node: OrdPoolNode<K, V>, addr: usize) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        self.seen.insert(addr, id);
        id
    }

    fn visit_node<P: SharedPointerKind>(&mut self, node: &BTreeNode<K, V, P>) -> u32 {
        match node {
            BTreeNode::Branch(branch) => self.visit_branch(branch),
            BTreeNode::Leaf(leaf) => self.visit_leaf(leaf),
        }
    }

    fn visit_branch<P: SharedPointerKind>(
        &mut self,
        branch: &SharedPointer<Branch<K, V, P>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(branch) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        let keys: Vec<K> = branch.keys.iter().cloned().collect();

        match &branch.children {
            Children::Leaves { leaves } => {
                let child_ids: Vec<u32> = leaves.iter().map(|l| self.visit_leaf(l)).collect();
                self.push(
                    OrdPoolNode::BranchLeaves {
                        keys,
                        children: child_ids,
                    },
                    addr,
                )
            }
            Children::Branches { branches, level } => {
                let child_ids: Vec<u32> =
                    branches.iter().map(|b| self.visit_branch(b)).collect();
                self.push(
                    OrdPoolNode::BranchBranches {
                        keys,
                        children: child_ids,
                        level: level.get(),
                    },
                    addr,
                )
            }
        }
    }

    fn visit_leaf<P: SharedPointerKind>(
        &mut self,
        leaf: &SharedPointer<Leaf<K, V>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(leaf) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        let pairs: Vec<(K, V)> = leaf.keys.iter().cloned().collect();
        self.push(OrdPoolNode::Leaf(pairs), addr)
    }
}

// ─── OrdMapPool public API ──────────────────────────────────────────

impl<K: Clone, V: Clone> OrdMapPool<K, V> {
    /// Build a pool from one or more OrdMaps. Maps that share structure
    /// (e.g., clones with modifications) will deduplicate shared nodes.
    pub fn from_maps<P: SharedPointerKind>(maps: &[&GenericOrdMap<K, V, P>]) -> Self {
        let mut collector = OrdPoolCollector::new();
        let mut containers = Vec::with_capacity(maps.len());

        for map in maps {
            let root = map.root.as_ref().map(|r| collector.visit_node(r));
            containers.push(PoolContainer {
                root,
                size: map.size,
            });
        }

        OrdMapPool {
            nodes: collector.nodes,
            containers,
        }
    }

    /// Build a pool from a single OrdMap.
    pub fn from_map<P: SharedPointerKind>(map: &GenericOrdMap<K, V, P>) -> Self {
        Self::from_maps(&[map])
    }
}

impl<K: Clone, V: Clone> OrdMapPool<K, V> {
    /// Collect all leaf (K, V) pairs for a given container root.
    fn collect_leaves(&self, root_id: usize, out: &mut Vec<(K, V)>) {
        match &self.nodes[root_id] {
            OrdPoolNode::BranchLeaves { children, .. }
            | OrdPoolNode::BranchBranches { children, .. } => {
                for &child_id in children {
                    self.collect_leaves(child_id as usize, out);
                }
            }
            OrdPoolNode::Leaf(pairs) => {
                out.extend(pairs.iter().cloned());
            }
        }
    }

    /// Reconstruct OrdMaps from this pool.
    ///
    /// Each map is rebuilt by inserting all leaf elements into a fresh
    /// map via `FromIterator`.
    pub fn to_maps<P>(&self) -> Vec<GenericOrdMap<K, V, P>>
    where
        K: Ord,
        P: SharedPointerKind,
    {
        self.containers
            .iter()
            .map(|c| {
                if let Some(root_id) = c.root {
                    let mut pairs = Vec::with_capacity(c.size);
                    self.collect_leaves(root_id as usize, &mut pairs);
                    pairs.into_iter().collect()
                } else {
                    GenericOrdMap::default()
                }
            })
            .collect()
    }

    /// Reconstruct a single OrdMap (convenience for single-map pools).
    pub fn to_map<P>(&self) -> GenericOrdMap<K, V, P>
    where
        K: Ord,
        P: SharedPointerKind,
    {
        let mut maps = self.to_maps();
        assert!(!maps.is_empty(), "pool contains no containers");
        maps.swap_remove(0)
    }
}

// ─── OrdSetPool convenience methods ─────────────────────────────────

impl<A: Clone> OrdMapPool<A, ()> {
    /// Build a pool from one or more OrdSets.
    pub fn from_sets<P: SharedPointerKind>(sets: &[&GenericOrdSet<A, P>]) -> Self {
        let maps: Vec<&GenericOrdMap<A, (), P>> = sets.iter().map(|s| &s.map).collect();
        Self::from_maps(&maps)
    }

    /// Build a pool from a single OrdSet.
    pub fn from_set<P: SharedPointerKind>(set: &GenericOrdSet<A, P>) -> Self {
        Self::from_sets(&[set])
    }

    /// Reconstruct OrdSets from this pool.
    pub fn to_sets<P>(&self) -> Vec<GenericOrdSet<A, P>>
    where
        A: Ord,
        P: SharedPointerKind,
    {
        self.to_maps()
            .into_iter()
            .map(|map| GenericOrdSet { map })
            .collect()
    }

    /// Reconstruct a single OrdSet (convenience for single-set pools).
    pub fn to_set<P>(&self) -> GenericOrdSet<A, P>
    where
        A: Ord,
        P: SharedPointerKind,
    {
        let mut sets = self.to_sets();
        assert!(!sets.is_empty(), "pool contains no containers");
        sets.swap_remove(0)
    }
}

// ─── OrdMapPool Serde Serialize ─────────────────────────────────────
//
// Format:
//   {
//     "nodes": [
//       {"bl": [[k, ...], [child_id, ...]]},
//       {"bb": [[k, ...], [child_id, ...], level]},
//       {"lf": [[k, v], ...]},
//       ...
//     ],
//     "containers": [[root_id_or_null, size], ...]
//   }

impl<K: Serialize, V: Serialize> Serialize for OrdPoolNode<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            OrdPoolNode::BranchLeaves { keys, children } => {
                map.serialize_entry("bl", &(keys, children))?;
            }
            OrdPoolNode::BranchBranches {
                keys,
                children,
                level,
            } => {
                map.serialize_entry("bb", &(keys, children, level))?;
            }
            OrdPoolNode::Leaf(pairs) => {
                map.serialize_entry("lf", pairs)?;
            }
        }
        map.end()
    }
}

impl<K: Serialize, V: Serialize> Serialize for OrdMapPool<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("OrdMapPool", 2)?;
        state.serialize_field("nodes", &self.nodes)?;
        state.serialize_field("containers", &self.containers)?;
        state.end()
    }
}

// ─── OrdMapPool Serde Deserialize ───────────────────────────────────

/// Wrapper for deserializing a single tagged OrdPoolNode.
struct TaggedOrdNode<K, V>(OrdPoolNode<K, V>);

impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de> for TaggedOrdNode<K, V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor<K, V>(PhantomData<(K, V)>);

        impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Visitor<'de> for NodeVisitor<K, V> {
            type Value = TaggedOrdNode<K, V>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a tagged B+ tree pool node")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let tag: String =
                    map.next_key()?.ok_or_else(|| de::Error::custom("empty node object"))?;
                let node = match tag.as_str() {
                    "bl" => {
                        let (keys, children): (Vec<K>, Vec<u32>) = map.next_value()?;
                        OrdPoolNode::BranchLeaves { keys, children }
                    }
                    "bb" => {
                        let (keys, children, level): (Vec<K>, Vec<u32>, usize) =
                            map.next_value()?;
                        OrdPoolNode::BranchBranches {
                            keys,
                            children,
                            level,
                        }
                    }
                    "lf" => {
                        let pairs: Vec<(K, V)> = map.next_value()?;
                        OrdPoolNode::Leaf(pairs)
                    }
                    other => {
                        return Err(de::Error::custom(format!("unknown node tag: {other}")));
                    }
                };
                Ok(TaggedOrdNode(node))
            }
        }

        deserializer.deserialize_map(NodeVisitor(PhantomData))
    }
}

impl<'de, K, V> Deserialize<'de> for OrdMapPool<K, V>
where
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PoolVisitor<K, V>(PhantomData<(K, V)>);

        impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Visitor<'de> for PoolVisitor<K, V> {
            type Value = OrdMapPool<K, V>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an OrdMapPool struct")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut access: M,
            ) -> Result<Self::Value, M::Error> {
                let mut nodes: Option<Vec<OrdPoolNode<K, V>>> = None;
                let mut containers: Option<Vec<PoolContainer>> = None;

                while let Some(key) = access.next_key::<&str>()? {
                    match key {
                        "nodes" => {
                            let raw: Vec<TaggedOrdNode<K, V>> = access.next_value()?;
                            nodes = Some(raw.into_iter().map(|t| t.0).collect());
                        }
                        "containers" => {
                            let raw: Vec<(Option<u32>, usize)> = access.next_value()?;
                            containers = Some(
                                raw.into_iter()
                                    .map(|(root, size)| PoolContainer { root, size })
                                    .collect(),
                            );
                        }
                        _ => {
                            let _ = access.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(OrdMapPool {
                    nodes: nodes.ok_or_else(|| de::Error::missing_field("nodes"))?,
                    containers: containers
                        .ok_or_else(|| de::Error::missing_field("containers"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "OrdMapPool",
            &["nodes", "containers"],
            PoolVisitor(PhantomData),
        )
    }
}

// ─── VectorPool ─────────────────────────────────────────────────────

use crate::vector::GenericVector;

/// A pool of serialised Vector elements.
///
/// Stores each vector as a flat element list. Multiple vectors
/// in the same pool share the container-level format for consistent
/// serialisation.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "persist")]
/// # {
/// use pds::Vector;
/// use pds::persist::VectorPool;
///
/// let v1: Vector<i32> = (0..100).collect();
/// let mut v2 = v1.clone();
/// v2.push_back(999);
///
/// let pool = VectorPool::from_vectors(&[&v1, &v2]);
/// let json = serde_json::to_string(&pool).unwrap();
///
/// let loaded: VectorPool<i32> = serde_json::from_str(&json).unwrap();
/// let vecs: Vec<Vector<i32>> = loaded.to_vectors();
/// assert_eq!(vecs[0], v1);
/// assert_eq!(vecs[1], v2);
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct VectorPool<A> {
    containers: Vec<Vec<A>>,
}

impl<A: Clone> VectorPool<A> {
    /// Build a pool from one or more Vectors.
    pub fn from_vectors<P: SharedPointerKind>(vectors: &[&GenericVector<A, P>]) -> Self {
        VectorPool {
            containers: vectors
                .iter()
                .map(|v| v.iter().cloned().collect())
                .collect(),
        }
    }

    /// Build a pool from a single Vector.
    pub fn from_vector<P: SharedPointerKind>(vector: &GenericVector<A, P>) -> Self {
        Self::from_vectors(&[vector])
    }

    /// Reconstruct Vectors from this pool.
    pub fn to_vectors<P>(&self) -> Vec<GenericVector<A, P>>
    where
        P: SharedPointerKind,
    {
        self.containers
            .iter()
            .map(|elems| elems.iter().cloned().collect())
            .collect()
    }

    /// Reconstruct a single Vector (convenience for single-vector pools).
    pub fn to_vector<P>(&self) -> GenericVector<A, P>
    where
        P: SharedPointerKind,
    {
        let mut vecs = self.to_vectors();
        assert!(!vecs.is_empty(), "pool contains no containers");
        vecs.swap_remove(0)
    }
}

impl<A: Serialize> Serialize for VectorPool<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.containers.serialize(serializer)
    }
}

impl<'de, A: Deserialize<'de>> Deserialize<'de> for VectorPool<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(VectorPool {
            containers: Vec::deserialize(deserializer)?,
        })
    }
}

// ─── HashSetPool ─────────────────────────────────────────────────────
//
// HashSet is backed by HamtNode<Value<A>> rather than HamtNode<(K, V)>.
// We use a dedicated pool collector that unwraps Value<A> to A when
// extracting leaf elements. The pool format is identical to HashMapPool.

use crate::hashset::GenericHashSet;
use crate::hashset::Value as SetValue;
use crate::nodes::hamt::Node as HamtRootNode;

/// A pool of serialised HashSet nodes with container metadata.
///
/// Multiple HashSets can share nodes within the same pool. Serialise
/// with any serde-compatible format (JSON, bincode, etc.).
///
/// # Example
///
/// ```
/// # #[cfg(feature = "persist")]
/// # {
/// use pds::HashSet;
/// use pds::persist::HashSetPool;
///
/// let set1: HashSet<i32> = (0..100).collect();
/// let mut set2 = set1.clone();
/// set2.insert(999);
///
/// let pool = HashSetPool::from_sets(&[&set1, &set2]);
/// let json = serde_json::to_string(&pool).unwrap();
///
/// let loaded: HashSetPool<i32> = serde_json::from_str(&json).unwrap();
/// let sets: Vec<HashSet<i32>> = loaded.to_sets();
/// assert_eq!(sets[0], set1);
/// assert_eq!(sets[1], set2);
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct HashSetPool<A, H: HashWidth = u64> {
    nodes: Vec<PoolNode<A, H>>,
    containers: Vec<PoolContainer>,
}

/// Pool collector for HashSet — visits HamtNode<Value<A>> and extracts A.
struct SetPoolCollector<A, H> {
    seen: StdHashMap<usize, u32>,
    nodes: Vec<PoolNode<A, H>>,
}

impl<A: Clone, H: HashWidth> SetPoolCollector<A, H> {
    fn new() -> Self {
        SetPoolCollector {
            seen: StdHashMap::new(),
            nodes: Vec::new(),
        }
    }

    fn push(&mut self, node: PoolNode<A, H>, addr: usize) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        self.seen.insert(addr, id);
        id
    }

    fn visit_hamt<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<HamtRootNode<SetValue<A>, P, H>, P>,
    ) -> u32
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }

        let mut entries = Vec::with_capacity(node.data.len());
        for (slot, entry) in node.data.entries() {
            let pool_entry = match entry {
                Entry::Value(val, h) => PoolEntry::Value(val.0.clone(), *h),
                Entry::HamtNode(child) => PoolEntry::Ref(self.visit_hamt(child)),
                Entry::SmallSimdNode(child) => PoolEntry::Ref(self.visit_set_small_simd(child)),
                Entry::LargeSimdNode(child) => PoolEntry::Ref(self.visit_set_large_simd(child)),
                Entry::Collision(child) => PoolEntry::Ref(self.visit_set_collision(child)),
            };
            entries.push((slot as u8, pool_entry));
        }
        self.push(PoolNode::Hamt(entries), addr)
    }

    fn visit_set_small_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<SmallSimdNode<SetValue<A>, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.0.clone(), *hash))
            .collect();
        self.push(PoolNode::SimdSmall(entries), addr)
    }

    fn visit_set_large_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<LargeSimdNode<SetValue<A>, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.0.clone(), *hash))
            .collect();
        self.push(PoolNode::SimdLarge(entries), addr)
    }

    fn visit_set_collision<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<CollisionNode<SetValue<A>, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let values: Vec<A> = node.data.iter().map(|v| v.0.clone()).collect();
        self.push(PoolNode::Collision(node.hash, values), addr)
    }
}

// ─── SetDedupPoolCollector ────────────────────────────────────────────
//
// Mirrors DedupPoolCollector but unwraps Value<A> when visiting set nodes,
// storing raw A in the pool. Identical dedup strategy: Merkle-keyed secondary
// index with O(node_size) structural equality on match.

struct SetDedupPoolCollector<A, H> {
    seen: StdHashMap<usize, u32>,
    merkle_index: StdHashMap<u64, Vec<u32>>,
    nodes: Vec<PoolNode<A, H>>,
}

impl<A: Clone + PartialEq, H: HashWidth> SetDedupPoolCollector<A, H> {
    fn new() -> Self {
        SetDedupPoolCollector {
            seen: StdHashMap::new(),
            merkle_index: StdHashMap::new(),
            nodes: Vec::new(),
        }
    }

    fn push(&mut self, node: PoolNode<A, H>, addr: usize, key: u64) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        self.seen.insert(addr, id);
        self.merkle_index.entry(key).or_default().push(id);
        id
    }

    fn visit_hamt<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<HamtRootNode<SetValue<A>, P, H>, P>,
    ) -> u32
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.merkle_hash;

        let mut entries = Vec::with_capacity(node.data.len());
        for (slot, entry) in node.data.entries() {
            let pool_entry = match entry {
                Entry::Value(val, h) => PoolEntry::Value(val.0.clone(), *h),
                Entry::HamtNode(child) => PoolEntry::Ref(self.visit_hamt(child)),
                Entry::SmallSimdNode(child) => PoolEntry::Ref(self.visit_set_small_simd(child)),
                Entry::LargeSimdNode(child) => PoolEntry::Ref(self.visit_set_large_simd(child)),
                Entry::Collision(child) => PoolEntry::Ref(self.visit_set_collision(child)),
            };
            entries.push((slot as u8, pool_entry));
        }

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if hamt_node_matches(&self.nodes[candidate_id as usize], &entries) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::Hamt(entries), addr, key)
    }

    fn visit_set_small_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<SmallSimdNode<SetValue<A>, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.merkle_hash;

        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.0.clone(), *hash))
            .collect();

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if simd_small_node_matches(&self.nodes[candidate_id as usize], &entries) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::SimdSmall(entries), addr, key)
    }

    fn visit_set_large_simd<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<LargeSimdNode<SetValue<A>, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.merkle_hash;

        let entries: Vec<(u8, A, H)> = node
            .data
            .entries()
            .map(|(idx, (val, hash))| (idx as u8, val.0.clone(), *hash))
            .collect();

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if simd_large_node_matches(&self.nodes[candidate_id as usize], &entries) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::SimdLarge(entries), addr, key)
    }

    fn visit_set_collision<P: SharedPointerKind>(
        &mut self,
        node: &SharedPointer<CollisionNode<SetValue<A>, H>, P>,
    ) -> u32 {
        let addr = SharedPointer::as_ptr(node) as usize;
        if let Some(&id) = self.seen.get(&addr) {
            return id;
        }
        let key = node.hash.to_u64();
        let values: Vec<A> = node.data.iter().map(|v| v.0.clone()).collect();

        let candidates = self.merkle_index.get(&key).cloned().unwrap_or_default();
        for candidate_id in candidates {
            if collision_node_matches(&self.nodes[candidate_id as usize], node.hash, &values) {
                self.seen.insert(addr, candidate_id);
                return candidate_id;
            }
        }

        self.push(PoolNode::Collision(node.hash, values), addr, key)
    }
}

// ─── HashSetPool public API ──────────────────────────────────────────

impl<A: Clone, H: HashWidth> HashSetPool<A, H> {
    /// Build a pool from one or more HashSets.
    pub fn from_sets<S, P: SharedPointerKind>(sets: &[&GenericHashSet<A, S, P, H>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut collector: SetPoolCollector<A, H> = SetPoolCollector::new();
        let mut containers = Vec::with_capacity(sets.len());

        for set in sets {
            let root = set.root.as_ref().map(|r| collector.visit_hamt(r));
            containers.push(PoolContainer {
                root,
                size: set.size,
            });
        }

        HashSetPool {
            nodes: collector.nodes,
            containers,
        }
    }

    /// Build a pool from a single HashSet.
    pub fn from_set<S, P: SharedPointerKind>(set: &GenericHashSet<A, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_sets(&[set])
    }
}

impl<A: Clone + PartialEq, H: HashWidth> HashSetPool<A, H> {
    /// Build a pool from one or more HashSets, deduplicating content-equal
    /// nodes even without shared pointers.
    ///
    /// See [`HashMapPool::from_maps_dedup`] for the full explanation.
    /// Requires `A: PartialEq`.
    pub fn from_sets_dedup<S, P: SharedPointerKind>(
        sets: &[&GenericHashSet<A, S, P, H>],
    ) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut collector = SetDedupPoolCollector::<A, H>::new();
        let mut containers = Vec::with_capacity(sets.len());

        for set in sets {
            let root = set.root.as_ref().map(|r| collector.visit_hamt(r));
            containers.push(PoolContainer {
                root,
                size: set.size,
            });
        }

        HashSetPool {
            nodes: collector.nodes,
            containers,
        }
    }

    /// Build a deduplicating pool from a single HashSet.
    ///
    /// See [`from_sets_dedup`][Self::from_sets_dedup] for details.
    pub fn from_set_dedup<S, P: SharedPointerKind>(set: &GenericHashSet<A, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_sets_dedup(&[set])
    }
}

impl<A: Clone, H: HashWidth> HashSetPool<A, H> {
    /// Collect all leaf elements for a given container root.
    fn collect_leaves(&self, root_id: usize, out: &mut Vec<A>) {
        match &self.nodes[root_id] {
            PoolNode::Hamt(entries) => {
                for (_slot, entry) in entries {
                    match entry {
                        PoolEntry::Value(a, _h) => out.push(a.clone()),
                        PoolEntry::Ref(id) => self.collect_leaves(*id as usize, out),
                    }
                }
            }
            PoolNode::SimdSmall(entries) | PoolNode::SimdLarge(entries) => {
                for (_slot, a, _h) in entries {
                    out.push(a.clone());
                }
            }
            PoolNode::Collision(_hash, values) => {
                out.extend(values.iter().cloned());
            }
        }
    }

    /// Reconstruct HashSets from this pool.
    pub fn to_sets<S, P>(&self) -> Vec<GenericHashSet<A, S, P, H>>
    where
        A: Hash + Eq,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.containers
            .iter()
            .map(|c| {
                if let Some(root_id) = c.root {
                    let mut elems = Vec::with_capacity(c.size);
                    self.collect_leaves(root_id as usize, &mut elems);
                    elems.into_iter().collect()
                } else {
                    GenericHashSet::default()
                }
            })
            .collect()
    }

    /// Reconstruct a single HashSet (convenience for single-set pools).
    pub fn to_set<S, P>(&self) -> GenericHashSet<A, S, P, H>
    where
        A: Hash + Eq,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut sets = self.to_sets();
        assert!(!sets.is_empty(), "pool contains no containers");
        sets.swap_remove(0)
    }
}

// ─── HashSetPool Serde ───────────────────────────────────────────────
//
// Format is identical to HashMapPool (same PoolNode structure), just
// with element type A instead of (K, V).

impl<A: Serialize, H: HashWidth + Serialize> Serialize for HashSetPool<A, H> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HashSetPool", 2)?;
        state.serialize_field("nodes", &self.nodes)?;
        state.serialize_field("containers", &self.containers)?;
        state.end()
    }
}

impl<'de, A, H: HashWidth> Deserialize<'de> for HashSetPool<A, H>
where
    A: Deserialize<'de> + Clone,
    H: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PoolVisitor<A, H>(PhantomData<(A, H)>);

        impl<'de, A, H: HashWidth> Visitor<'de> for PoolVisitor<A, H>
        where
            A: Deserialize<'de> + Clone,
            H: Deserialize<'de>,
        {
            type Value = HashSetPool<A, H>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a HashSetPool struct")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
                let mut nodes: Option<Vec<PoolNode<A, H>>> = None;
                let mut containers: Option<Vec<PoolContainer>> = None;

                while let Some(key) = access.next_key::<&str>()? {
                    match key {
                        "nodes" => {
                            // Reuse the TaggedNode deserialiser — element type A, no tuple wrapper.
                            let raw: Vec<TaggedSetNode<A, H>> = access.next_value()?;
                            nodes = Some(raw.into_iter().map(|t| t.0).collect());
                        }
                        "containers" => {
                            let raw: Vec<(Option<u32>, usize)> = access.next_value()?;
                            containers = Some(
                                raw.into_iter()
                                    .map(|(root, size)| PoolContainer { root, size })
                                    .collect(),
                            );
                        }
                        _ => {
                            let _ = access.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(HashSetPool {
                    nodes: nodes.ok_or_else(|| de::Error::missing_field("nodes"))?,
                    containers: containers
                        .ok_or_else(|| de::Error::missing_field("containers"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "HashSetPool",
            &["nodes", "containers"],
            PoolVisitor(PhantomData),
        )
    }
}

/// Wrapper for deserializing a single tagged `PoolNode<A, H>` for HashSetPool.
/// The element type is `A` directly (not `(K, V)`).
struct TaggedSetNode<A, H>(PoolNode<A, H>);

impl<'de, A, H> Deserialize<'de> for TaggedSetNode<A, H>
where
    A: Deserialize<'de>,
    H: Deserialize<'de> + HashWidth,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor<A, H>(PhantomData<(A, H)>);

        impl<'de, A, H> Visitor<'de> for NodeVisitor<A, H>
        where
            A: Deserialize<'de>,
            H: Deserialize<'de> + HashWidth,
        {
            type Value = TaggedSetNode<A, H>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a tagged pool node object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let tag: String =
                    map.next_key()?.ok_or_else(|| de::Error::custom("empty node object"))?;

                let node = match tag.as_str() {
                    "h" => {
                        let (values, refs): (Vec<(u8, A, H)>, Vec<(u8, u32)>) =
                            map.next_value()?;
                        let mut entries = Vec::with_capacity(values.len() + refs.len());
                        for (slot, val, hash) in values {
                            entries.push((slot, PoolEntry::Value(val, hash)));
                        }
                        for (slot, id) in refs {
                            entries.push((slot, PoolEntry::Ref(id)));
                        }
                        PoolNode::Hamt(entries)
                    }
                    "s" => {
                        let entries: Vec<(u8, A, H)> = map.next_value()?;
                        PoolNode::SimdSmall(entries)
                    }
                    "l" => {
                        let entries: Vec<(u8, A, H)> = map.next_value()?;
                        PoolNode::SimdLarge(entries)
                    }
                    "c" => {
                        let (hash, values): (H, Vec<A>) = map.next_value()?;
                        PoolNode::Collision(hash, values)
                    }
                    other => {
                        return Err(de::Error::custom(format!("unknown node tag: {other}")));
                    }
                };

                Ok(TaggedSetNode(node))
            }
        }

        deserializer.deserialize_map(NodeVisitor(PhantomData))
    }
}

// ─── BagPool ─────────────────────────────────────────────────────────
//
// Bag is backed by GenericHashMap<A, usize>. BagPool delegates directly
// to HashMapPool<A, usize>, preserving HAMT structural sharing between
// bags that were cloned from a common ancestor.

use crate::bag::GenericBag;

/// A pool of serialised Bag nodes with container metadata.
///
/// Delegates to [`HashMapPool`] over the internal `HashMap<A, usize>` (element
/// → count). Multiple Bags that share structure via clone share HAMT nodes in
/// the pool.
///
/// `BagPool` always uses the default `u64` hash width, matching `GenericBag`
/// which does not expose a hash width parameter.
#[derive(Clone, Debug)]
pub struct BagPool<A>(HashMapPool<A, usize>);

impl<A: Clone> BagPool<A> {
    /// Build a pool from one or more Bags.
    pub fn from_bags<S, P: SharedPointerKind>(bags: &[&GenericBag<A, S, P>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let maps: Vec<&GenericHashMap<A, usize, S, P>> =
            bags.iter().map(|b| &b.map).collect();
        BagPool(HashMapPool::from_maps(&maps))
    }

    /// Build a pool from a single Bag.
    pub fn from_bag<S, P: SharedPointerKind>(bag: &GenericBag<A, S, P>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_bags(&[bag])
    }
}

impl<A: Clone + PartialEq> BagPool<A> {
    /// Build a pool from one or more Bags, deduplicating content-equal nodes.
    ///
    /// See [`HashMapPool::from_maps_dedup`] for the full explanation.
    pub fn from_bags_dedup<S, P: SharedPointerKind>(bags: &[&GenericBag<A, S, P>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let maps: Vec<&GenericHashMap<A, usize, S, P>> =
            bags.iter().map(|b| &b.map).collect();
        BagPool(HashMapPool::from_maps_dedup(&maps))
    }

    /// Build a deduplicating pool from a single Bag.
    pub fn from_bag_dedup<S, P: SharedPointerKind>(bag: &GenericBag<A, S, P>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_bags_dedup(&[bag])
    }
}

impl<A: Clone> BagPool<A> {
    /// Reconstruct Bags from this pool.
    pub fn to_bags<S, P>(&self) -> Vec<GenericBag<A, S, P>>
    where
        A: Hash + Eq,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.0
            .to_maps::<S, P>()
            .into_iter()
            .map(|map| {
                let total: usize = map.values().sum();
                GenericBag { map, total }
            })
            .collect()
    }

    /// Reconstruct a single Bag (convenience for single-bag pools).
    pub fn to_bag<S, P>(&self) -> GenericBag<A, S, P>
    where
        A: Hash + Eq,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut bags = self.to_bags();
        assert!(!bags.is_empty(), "pool contains no containers");
        bags.swap_remove(0)
    }
}

impl<A: Serialize> Serialize for BagPool<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, A> Deserialize<'de> for BagPool<A>
where
    A: Deserialize<'de> + Clone,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(BagPool(HashMapPool::deserialize(deserializer)?))
    }
}

// ─── BiMapPool ───────────────────────────────────────────────────────
//
// BiMap stores a forward GenericHashMap<K, V> and a backward
// GenericHashMap<V, K>. The backward map is redundant — it can be
// reconstructed by inverting the forward pairs. BiMapPool pools only
// the forward direction, halving storage and preserving SSP for BiMaps
// that share forward-map structure.

use crate::bimap::GenericBiMap;

/// A pool of serialised BiMap nodes with container metadata.
///
/// Only the forward direction `K → V` is pooled; the backward `V → K` map
/// is rebuilt during deserialisation. Multiple BiMaps that share forward
/// structure via clone share HAMT nodes in the pool.
#[derive(Clone, Debug)]
pub struct BiMapPool<K, V, H: HashWidth = u64>(HashMapPool<K, V, H>);

impl<K: Clone, V: Clone, H: HashWidth> BiMapPool<K, V, H> {
    /// Build a pool from one or more BiMaps.
    pub fn from_bimaps<S, P: SharedPointerKind>(maps: &[&GenericBiMap<K, V, S, P, H>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let fwd: Vec<&GenericHashMap<K, V, S, P, H>> =
            maps.iter().map(|b| &b.forward).collect();
        BiMapPool(HashMapPool::from_maps(&fwd))
    }

    /// Build a pool from a single BiMap.
    pub fn from_bimap<S, P: SharedPointerKind>(map: &GenericBiMap<K, V, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_bimaps(&[map])
    }
}

impl<K: Clone + PartialEq, V: Clone + PartialEq, H: HashWidth> BiMapPool<K, V, H> {
    /// Build a pool from one or more BiMaps, deduplicating content-equal nodes.
    ///
    /// See [`HashMapPool::from_maps_dedup`] for the full explanation.
    pub fn from_bimaps_dedup<S, P: SharedPointerKind>(
        maps: &[&GenericBiMap<K, V, S, P, H>],
    ) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let fwd: Vec<&GenericHashMap<K, V, S, P, H>> =
            maps.iter().map(|b| &b.forward).collect();
        BiMapPool(HashMapPool::from_maps_dedup(&fwd))
    }

    /// Build a deduplicating pool from a single BiMap.
    pub fn from_bimap_dedup<S, P: SharedPointerKind>(map: &GenericBiMap<K, V, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_bimaps_dedup(&[map])
    }
}

impl<K: Clone, V: Clone, H: HashWidth> BiMapPool<K, V, H> {
    /// Reconstruct BiMaps from this pool.
    pub fn to_bimaps<S, P>(&self) -> Vec<GenericBiMap<K, V, S, P, H>>
    where
        K: Hash + Eq + Clone,
        V: Hash + Eq + Clone,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.0
            .to_maps::<S, P>()
            .into_iter()
            .map(|fwd_map| {
                let mut bwd_map: GenericHashMap<V, K, S, P, H> =
                    GenericHashMap::with_hasher(S::default());
                for (k, v) in fwd_map.iter() {
                    bwd_map.insert(v.clone(), k.clone());
                }
                GenericBiMap {
                    forward: fwd_map,
                    backward: bwd_map,
                }
            })
            .collect()
    }

    /// Reconstruct a single BiMap (convenience for single-map pools).
    pub fn to_bimap<S, P>(&self) -> GenericBiMap<K, V, S, P, H>
    where
        K: Hash + Eq + Clone,
        V: Hash + Eq + Clone,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut maps = self.to_bimaps();
        assert!(!maps.is_empty(), "pool contains no containers");
        maps.swap_remove(0)
    }
}

impl<K: Serialize, V: Serialize, H: HashWidth + Serialize> Serialize for BiMapPool<K, V, H> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, K, V, H: HashWidth> Deserialize<'de> for BiMapPool<K, V, H>
where
    K: Deserialize<'de> + Clone,
    V: Deserialize<'de> + Clone,
    H: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(BiMapPool(HashMapPool::deserialize(deserializer)?))
    }
}

// ─── SymMapPool ──────────────────────────────────────────────────────
//
// SymMap stores forward GenericHashMap<A, A> and backward GenericHashMap<A, A>.
// Same strategy as BiMapPool: pool only forward, rebuild backward on load.

use crate::symmap::GenericSymMap;

/// A pool of serialised SymMap nodes with container metadata.
///
/// Only the forward direction is pooled; the backward map is rebuilt during
/// deserialisation. Multiple SymMaps that share forward structure share HAMT
/// nodes in the pool.
#[derive(Clone, Debug)]
pub struct SymMapPool<A, H: HashWidth = u64>(HashMapPool<A, A, H>);

impl<A: Clone, H: HashWidth> SymMapPool<A, H> {
    /// Build a pool from one or more SymMaps.
    pub fn from_symmaps<S, P: SharedPointerKind>(maps: &[&GenericSymMap<A, S, P, H>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let fwd: Vec<&GenericHashMap<A, A, S, P, H>> =
            maps.iter().map(|s| &s.forward).collect();
        SymMapPool(HashMapPool::from_maps(&fwd))
    }

    /// Build a pool from a single SymMap.
    pub fn from_symmap<S, P: SharedPointerKind>(map: &GenericSymMap<A, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_symmaps(&[map])
    }

    /// Reconstruct SymMaps from this pool.
    pub fn to_symmaps<S, P>(&self) -> Vec<GenericSymMap<A, S, P, H>>
    where
        A: Hash + Eq + Clone,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.0
            .to_maps::<S, P>()
            .into_iter()
            .map(|fwd_map| {
                let mut bwd_map: GenericHashMap<A, A, S, P, H> =
                    GenericHashMap::with_hasher(S::default());
                for (k, v) in fwd_map.iter() {
                    bwd_map.insert(v.clone(), k.clone());
                }
                GenericSymMap {
                    forward: fwd_map,
                    backward: bwd_map,
                }
            })
            .collect()
    }

    /// Reconstruct a single SymMap (convenience for single-map pools).
    pub fn to_symmap<S, P>(&self) -> GenericSymMap<A, S, P, H>
    where
        A: Hash + Eq + Clone,
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut maps = self.to_symmaps();
        assert!(!maps.is_empty(), "pool contains no containers");
        maps.swap_remove(0)
    }
}

impl<A: Clone + PartialEq, H: HashWidth> SymMapPool<A, H> {
    /// Build a pool from one or more SymMaps using Merkle-keyed deduplication.
    ///
    /// Content-equal forward-map nodes that are not pointer-equal — because they
    /// were independently built or round-tripped — are mapped to the same pool
    /// entry, reducing pool size and restoring structural sharing.
    ///
    /// See [`HashMapPool::from_maps_dedup`] for the full explanation.
    pub fn from_symmaps_dedup<S, P: SharedPointerKind>(maps: &[&GenericSymMap<A, S, P, H>]) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let fwd: Vec<&GenericHashMap<A, A, S, P, H>> =
            maps.iter().map(|s| &s.forward).collect();
        SymMapPool(HashMapPool::from_maps_dedup(&fwd))
    }

    /// Build a dedup pool from a single SymMap.
    pub fn from_symmap_dedup<S, P: SharedPointerKind>(map: &GenericSymMap<A, S, P, H>) -> Self
    where
        BitsImpl<HASH_WIDTH>: Bits,
    {
        Self::from_symmaps_dedup(&[map])
    }
}

impl<A: Serialize, H: HashWidth + Serialize> Serialize for SymMapPool<A, H> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, A, H: HashWidth> Deserialize<'de> for SymMapPool<A, H>
where
    A: Deserialize<'de> + Clone,
    H: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(SymMapPool(HashMapPool::deserialize(deserializer)?))
    }
}

// ─── HashMultiMapPool ────────────────────────────────────────────────
//
// HashMultiMap is backed by HashMap<K, HashSet<V>>. Since the value type is
// itself a collection, deep HAMT pooling would require a two-level pool. We
// use a flat approach instead: extract all (K, V) pairs and rebuild via
// FromIterator. This is equivalent to VectorPool's flat element serialisation.

use crate::hash_multimap::GenericHashMultiMap;

/// A pool of serialised HashMultiMap entries.
///
/// Stores each multimap as a flat list of `(K, V)` pairs. Rebuilds via
/// `FromIterator`. Deep HAMT node deduplication is not preserved; correctness
/// and compact representation are.
#[derive(Clone, Debug)]
pub struct HashMultiMapPool<K, V> {
    containers: Vec<Vec<(K, V)>>,
}

impl<K: Clone, V: Clone> HashMultiMapPool<K, V> {
    /// Build a pool from one or more HashMultiMaps.
    pub fn from_maps<S, P: SharedPointerKind, H: HashWidth>(
        maps: &[&GenericHashMultiMap<K, V, S, P, H>],
    ) -> Self
    where
        K: Hash + Eq,
        V: Hash + Eq,
        S: BuildHasher + Clone + Default,
    {
        HashMultiMapPool {
            containers: maps
                .iter()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .collect(),
        }
    }

    /// Build a pool from a single HashMultiMap.
    pub fn from_map<S, P: SharedPointerKind, H: HashWidth>(
        map: &GenericHashMultiMap<K, V, S, P, H>,
    ) -> Self
    where
        K: Hash + Eq,
        V: Hash + Eq,
        S: BuildHasher + Clone + Default,
    {
        Self::from_maps(&[map])
    }

    /// Reconstruct HashMultiMaps from this pool.
    pub fn to_maps<S, P, H>(&self) -> Vec<GenericHashMultiMap<K, V, S, P, H>>
    where
        K: Hash + Eq,
        V: Hash + Eq,
        S: BuildHasher + Clone + Default,
        P: SharedPointerKind,
        H: HashWidth,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.containers
            .iter()
            .map(|pairs| pairs.iter().cloned().collect())
            .collect()
    }

    /// Reconstruct a single HashMultiMap (convenience for single-map pools).
    pub fn to_map<S, P, H>(&self) -> GenericHashMultiMap<K, V, S, P, H>
    where
        K: Hash + Eq,
        V: Hash + Eq,
        S: BuildHasher + Clone + Default,
        P: SharedPointerKind,
        H: HashWidth,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut maps = self.to_maps();
        assert!(!maps.is_empty(), "pool contains no containers");
        maps.swap_remove(0)
    }
}

impl<K: Serialize, V: Serialize> Serialize for HashMultiMapPool<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.containers.serialize(serializer)
    }
}

impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de>
    for HashMultiMapPool<K, V>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(HashMultiMapPool {
            containers: Vec::deserialize(deserializer)?,
        })
    }
}

// ─── InsertionOrderMapPool ───────────────────────────────────────────
//
// InsertionOrderMap is backed by HashMap<K, usize> + OrdMap<usize, (K, V)>.
// The internal indices are an implementation detail. We serialise the
// ordered (K, V) pairs directly; rebuilding via FromIterator assigns
// fresh monotonic indices starting from 0, preserving insertion order.

use crate::insertion_order_map::GenericInsertionOrderMap;

/// A pool of serialised InsertionOrderMap entries.
///
/// Stores each map as an ordered list of `(K, V)` pairs (insertion order
/// preserved). Rebuilds via `FromIterator`. Internal indices are compacted
/// on deserialisation (starting from 0) but insertion order is identical.
#[derive(Clone, Debug)]
pub struct InsertionOrderMapPool<K, V> {
    containers: Vec<Vec<(K, V)>>,
}

impl<K: Clone, V: Clone> InsertionOrderMapPool<K, V> {
    /// Build a pool from one or more InsertionOrderMaps.
    pub fn from_maps<S, P: SharedPointerKind, H: HashWidth>(
        maps: &[&GenericInsertionOrderMap<K, V, S, P, H>],
    ) -> Self
    where
        K: Hash + Eq,
        S: BuildHasher + Clone,
    {
        InsertionOrderMapPool {
            containers: maps
                .iter()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .collect(),
        }
    }

    /// Build a pool from a single InsertionOrderMap.
    pub fn from_map<S, P: SharedPointerKind, H: HashWidth>(
        map: &GenericInsertionOrderMap<K, V, S, P, H>,
    ) -> Self
    where
        K: Hash + Eq,
        S: BuildHasher + Clone,
    {
        Self::from_maps(&[map])
    }

    /// Reconstruct InsertionOrderMaps from this pool.
    pub fn to_maps<S, P, H>(&self) -> Vec<GenericInsertionOrderMap<K, V, S, P, H>>
    where
        K: Hash + Eq + Ord + Clone,
        V: Clone,
        S: BuildHasher + Clone + Default,
        P: SharedPointerKind,
        H: HashWidth,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        self.containers
            .iter()
            .map(|pairs| pairs.iter().cloned().collect())
            .collect()
    }

    /// Reconstruct a single InsertionOrderMap (convenience for single-map pools).
    pub fn to_map<S, P, H>(&self) -> GenericInsertionOrderMap<K, V, S, P, H>
    where
        K: Hash + Eq + Ord + Clone,
        V: Clone,
        S: BuildHasher + Clone + Default,
        P: SharedPointerKind,
        H: HashWidth,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut maps = self.to_maps();
        assert!(!maps.is_empty(), "pool contains no containers");
        maps.swap_remove(0)
    }
}

impl<K: Serialize, V: Serialize> Serialize for InsertionOrderMapPool<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.containers.serialize(serializer)
    }
}

impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de>
    for InsertionOrderMapPool<K, V>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(InsertionOrderMapPool {
            containers: Vec::deserialize(deserializer)?,
        })
    }
}

// ─── TriePool ────────────────────────────────────────────────────────
//
// Trie is a recursive HashMap<K, Trie<K, V>>. We flatten it to a list of
// (path, value) pairs (all nodes that carry a value), then rebuild by
// calling trie.insert for each path.

use crate::trie::GenericTrie;

/// A pool of serialised Trie entries.
///
/// Stores each trie as a flat list of `(path, value)` pairs where `path`
/// is a `Vec<K>` of key segments. Rebuilds via `insert`. The recursive
/// HAMT structure is not pooled; correctness and compact representation
/// are preserved.
#[derive(Clone, Debug)]
pub struct TriePool<K, V> {
    containers: Vec<Vec<(Vec<K>, V)>>,
}

impl<K: Clone, V: Clone> TriePool<K, V> {
    /// Build a pool from one or more Tries.
    pub fn from_tries<S, P: SharedPointerKind>(tries: &[&GenericTrie<K, V, S, P>]) -> Self
    where
        K: Hash + Eq,
        S: BuildHasher + Clone + Default,
    {
        TriePool {
            containers: tries
                .iter()
                .map(|t| {
                    t.iter()
                        .map(|(path_refs, v)| {
                            let path: Vec<K> = path_refs.iter().map(|k| (*k).clone()).collect();
                            (path, v.clone())
                        })
                        .collect()
                })
                .collect(),
        }
    }

    /// Build a pool from a single Trie.
    pub fn from_trie<S, P: SharedPointerKind>(trie: &GenericTrie<K, V, S, P>) -> Self
    where
        K: Hash + Eq,
        S: BuildHasher + Clone + Default,
    {
        Self::from_tries(&[trie])
    }

    /// Reconstruct Tries from this pool.
    pub fn to_tries<S, P>(&self) -> Vec<GenericTrie<K, V, S, P>>
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: BuildHasher + Clone + Default,
        P: SharedPointerKind,
    {
        self.containers
            .iter()
            .map(|pairs| {
                let mut trie = GenericTrie {
                    value: None,
                    children: GenericHashMap::with_hasher(S::default()),
                };
                for (path, value) in pairs {
                    trie.insert(path, value.clone());
                }
                trie
            })
            .collect()
    }

    /// Reconstruct a single Trie (convenience for single-trie pools).
    pub fn to_trie<S, P>(&self) -> GenericTrie<K, V, S, P>
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: BuildHasher + Clone + Default,
        P: SharedPointerKind,
    {
        let mut tries = self.to_tries();
        assert!(!tries.is_empty(), "pool contains no containers");
        tries.swap_remove(0)
    }
}

impl<K: Serialize, V: Serialize> Serialize for TriePool<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.containers.serialize(serializer)
    }
}

impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de> for TriePool<K, V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(TriePool {
            containers: Vec::deserialize(deserializer)?,
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HashMap;

    #[test]
    fn roundtrip_single_map() {
        let map: HashMap<String, i32> =
            [("a".into(), 1), ("b".into(), 2), ("c".into(), 3)].into();

        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMapPool<String, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<String, i32> = loaded.to_map();

        assert_eq!(restored, map);
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn roundtrip_large_map() {
        let map: HashMap<i32, i32> = (0..1000).map(|i| (i, i * 2)).collect();

        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<i32, i32> = loaded.to_map();

        assert_eq!(restored, map);
    }

    #[test]
    fn roundtrip_get_works() {
        // Verify deserialized maps support get() — not just equality.
        let map: HashMap<i32, i32> = (0..500).map(|i| (i, i * 2)).collect();
        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<i32, i32> = loaded.to_map();

        for i in 0..500 {
            assert_eq!(
                restored.get(&i),
                Some(&(i * 2)),
                "get({i}) failed on deserialized map"
            );
        }
    }

    #[test]
    fn shared_nodes_deduplicated_in_pool() {
        // Two maps sharing structure (via clone) should produce fewer
        // serialised nodes than two independently constructed maps.
        let base: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut modified = base.clone();
        modified.insert(999, 999);

        let pool = HashMapPool::from_maps(&[&base, &modified]);

        let independent: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let pool_independent = HashMapPool::from_maps(&[&base, &independent]);

        assert!(
            pool.nodes.len() < pool_independent.nodes.len(),
            "shared pool ({}) should have fewer nodes than independent ({})",
            pool.nodes.len(),
            pool_independent.nodes.len()
        );
    }

    #[test]
    fn roundtrip_preserves_both_maps() {
        let map1: HashMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let mut map2 = map1.clone();
        map2.insert(999, 42);

        let pool = HashMapPool::from_maps(&[&map1, &map2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<HashMap<i32, i32>> = loaded.to_maps();

        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0], map1);
        assert_eq!(maps[1], map2);
        // Verify get() works on both maps
        for i in 0..50 {
            assert_eq!(maps[0].get(&i), Some(&i));
            assert_eq!(maps[1].get(&i), Some(&i));
        }
        assert_eq!(maps[1].get(&999), Some(&42));
    }

    #[test]
    fn intern_after_deserialise_deduplicates() {
        // Deserialise the same data twice, intern both results.
        // With different RandomState hashers, tree structures differ,
        // so we only verify that interning works and maps remain correct.
        use crate::intern::InternPool;

        let map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded1: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let mut map1: HashMap<i32, i32> = loaded1.to_map();

        let loaded2: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let mut map2: HashMap<i32, i32> = loaded2.to_map();

        // Intern both into the same pool
        let mut intern = InternPool::new();
        map1.intern(&mut intern);
        map2.intern(&mut intern);

        assert!(intern.len() > 0);

        // Both maps must still work correctly after interning
        for i in 0..100 {
            assert_eq!(map1.get(&i), Some(&i));
            assert_eq!(map2.get(&i), Some(&i));
        }
    }

    #[test]
    fn empty_map_roundtrip() {
        let map: HashMap<i32, i32> = HashMap::new();
        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<i32, i32> = loaded.to_map();

        assert_eq!(restored, map);
        assert!(restored.is_empty());
    }

    // --- OrdMapPool tests ---

    #[test]
    fn ordmap_roundtrip_single() {
        use crate::OrdMap;

        let map: OrdMap<String, i32> =
            [("a".into(), 1), ("b".into(), 2), ("c".into(), 3)].into();

        let pool = OrdMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: OrdMapPool<String, i32> = serde_json::from_str(&json).unwrap();
        let restored: OrdMap<String, i32> = loaded.to_map();

        assert_eq!(restored, map);
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn ordmap_roundtrip_large() {
        use crate::OrdMap;

        let map: OrdMap<i32, i32> = (0..1000).map(|i| (i, i * 2)).collect();

        let pool = OrdMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: OrdMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: OrdMap<i32, i32> = loaded.to_map();

        assert_eq!(restored, map);
        for i in 0..1000 {
            assert_eq!(restored.get(&i), Some(&(i * 2)));
        }
    }

    #[test]
    fn ordmap_shared_nodes_deduplicated() {
        use crate::OrdMap;

        let base: OrdMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut modified = base.clone();
        modified.insert(999, 999);

        let pool = OrdMapPool::from_maps(&[&base, &modified]);

        let independent: OrdMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let pool_independent = OrdMapPool::from_maps(&[&base, &independent]);

        assert!(
            pool.nodes.len() < pool_independent.nodes.len(),
            "shared pool ({}) should have fewer nodes than independent ({})",
            pool.nodes.len(),
            pool_independent.nodes.len()
        );
    }

    #[test]
    fn ordmap_roundtrip_preserves_both() {
        use crate::OrdMap;

        let map1: OrdMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let mut map2 = map1.clone();
        map2.insert(999, 42);

        let pool = OrdMapPool::from_maps(&[&map1, &map2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: OrdMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<OrdMap<i32, i32>> = loaded.to_maps();

        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0], map1);
        assert_eq!(maps[1], map2);
    }

    #[test]
    fn ordmap_empty_roundtrip() {
        use crate::OrdMap;

        let map: OrdMap<i32, i32> = OrdMap::new();
        let pool = OrdMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: OrdMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: OrdMap<i32, i32> = loaded.to_map();

        assert_eq!(restored, map);
        assert!(restored.is_empty());
    }

    // --- OrdSetPool tests ---

    #[test]
    fn ordset_roundtrip_single() {
        use crate::OrdSet;

        let set: OrdSet<i32> = (0..100).collect();

        let pool = OrdSetPool::from_set(&set);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: OrdSetPool<i32> = serde_json::from_str(&json).unwrap();
        let restored: OrdSet<i32> = loaded.to_set();

        assert_eq!(restored, set);
    }

    #[test]
    fn ordset_roundtrip_preserves_both() {
        use crate::OrdSet;

        let set1: OrdSet<i32> = (0..50).collect();
        let mut set2 = set1.clone();
        set2.insert(999);

        let pool = OrdSetPool::from_sets(&[&set1, &set2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: OrdSetPool<i32> = serde_json::from_str(&json).unwrap();
        let sets: Vec<OrdSet<i32>> = loaded.to_sets();

        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0], set1);
        assert_eq!(sets[1], set2);
    }

    // --- VectorPool tests ---

    #[test]
    fn vector_roundtrip_single() {
        use crate::Vector;

        let v: Vector<i32> = (0..100).collect();

        let pool = VectorPool::from_vector(&v);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: VectorPool<i32> = serde_json::from_str(&json).unwrap();
        let restored: Vector<i32> = loaded.to_vector();

        assert_eq!(restored, v);
    }

    #[test]
    fn vector_roundtrip_preserves_both() {
        use crate::Vector;

        let v1: Vector<i32> = (0..100).collect();
        let mut v2 = v1.clone();
        v2.push_back(999);

        let pool = VectorPool::from_vectors(&[&v1, &v2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: VectorPool<i32> = serde_json::from_str(&json).unwrap();
        let vecs: Vec<Vector<i32>> = loaded.to_vectors();

        assert_eq!(vecs.len(), 2);
        assert_eq!(vecs[0], v1);
        assert_eq!(vecs[1], v2);
    }

    #[test]
    fn vector_empty_roundtrip() {
        use crate::Vector;

        let v: Vector<i32> = Vector::new();
        let pool = VectorPool::from_vector(&v);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: VectorPool<i32> = serde_json::from_str(&json).unwrap();
        let restored: Vector<i32> = loaded.to_vector();

        assert_eq!(restored, v);
        assert!(restored.is_empty());
    }

    #[test]
    fn vector_roundtrip_large() {
        use crate::Vector;

        let v: Vector<i32> = (0..10_000).collect();

        let pool = VectorPool::from_vector(&v);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: VectorPool<i32> = serde_json::from_str(&json).unwrap();
        let restored: Vector<i32> = loaded.to_vector();

        assert_eq!(restored, v);
        assert_eq!(restored.len(), 10_000);
    }

    // --- HashSetPool tests ---

    #[test]
    fn hashset_roundtrip_single() {
        use crate::HashSet;
        let set: HashSet<i32> = (0..100).collect();

        let pool = HashSetPool::from_set(&set);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashSetPool<i32> = serde_json::from_str(&json).unwrap();
        let restored: HashSet<i32> = loaded.to_set();

        assert_eq!(restored, set);
        assert_eq!(restored.len(), 100);
    }

    #[test]
    fn hashset_roundtrip_preserves_both() {
        use crate::HashSet;
        let set1: HashSet<i32> = (0..50).collect();
        let mut set2 = set1.clone();
        set2.insert(999);

        let pool = HashSetPool::from_sets(&[&set1, &set2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashSetPool<i32> = serde_json::from_str(&json).unwrap();
        let sets: Vec<HashSet<i32>> = loaded.to_sets();

        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0], set1);
        assert_eq!(sets[1], set2);
    }

    #[test]
    fn hashset_shared_nodes_deduplicated() {
        use crate::HashSet;
        let base: HashSet<i32> = (0..100).collect();
        let mut modified = base.clone();
        modified.insert(999);

        let pool = HashSetPool::from_sets(&[&base, &modified]);

        let independent: HashSet<i32> = (0..100).collect();
        let pool_independent = HashSetPool::from_sets(&[&base, &independent]);

        assert!(
            pool.nodes.len() < pool_independent.nodes.len(),
            "shared pool ({}) should have fewer nodes than independent ({})",
            pool.nodes.len(),
            pool_independent.nodes.len()
        );
    }

    #[test]
    fn hashset_empty_roundtrip() {
        use crate::HashSet;
        let set: HashSet<i32> = HashSet::new();

        let pool = HashSetPool::from_set(&set);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashSetPool<i32> = serde_json::from_str(&json).unwrap();
        let restored: HashSet<i32> = loaded.to_set();

        assert_eq!(restored, set);
        assert!(restored.is_empty());
    }

    // --- BagPool tests ---

    #[test]
    fn bag_roundtrip_single() {
        use crate::Bag;
        let mut bag: Bag<&str> = Bag::new();
        bag.insert("apple");
        bag.insert("apple");
        bag.insert("banana");

        let pool = BagPool::from_bag(&bag);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: BagPool<&str> = serde_json::from_str(&json).unwrap();
        let restored: Bag<&str> = loaded.to_bag();

        assert_eq!(restored.count(&"apple"), 2);
        assert_eq!(restored.count(&"banana"), 1);
        assert_eq!(restored.total_count(), 3);
    }

    #[test]
    fn bag_roundtrip_preserves_both() {
        use crate::Bag;
        let mut bag1: Bag<i32> = Bag::new();
        for i in 0..50 {
            bag1.insert(i);
        }
        let mut bag2 = bag1.clone();
        bag2.insert(999);

        let pool = BagPool::from_bags(&[&bag1, &bag2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: BagPool<i32> = serde_json::from_str(&json).unwrap();
        let bags: Vec<Bag<i32>> = loaded.to_bags();

        assert_eq!(bags.len(), 2);
        assert_eq!(bags[0].total_count(), bag1.total_count());
        assert_eq!(bags[1].total_count(), bag2.total_count());
        for i in 0..50 {
            assert_eq!(bags[0].count(&i), 1);
            assert_eq!(bags[1].count(&i), 1);
        }
        assert_eq!(bags[1].count(&999), 1);
    }

    // --- BiMapPool tests ---

    #[test]
    fn bimap_roundtrip_single() {
        use crate::BiMap;
        let mut bm: BiMap<&str, i32> = BiMap::new();
        bm.insert("alice", 1);
        bm.insert("bob", 2);
        bm.insert("carol", 3);

        let pool = BiMapPool::from_bimap(&bm);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: BiMapPool<&str, i32> = serde_json::from_str(&json).unwrap();
        let restored: BiMap<&str, i32> = loaded.to_bimap();

        assert_eq!(restored.get_by_key(&"alice"), Some(&1));
        assert_eq!(restored.get_by_value(&2), Some(&"bob"));
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn bimap_roundtrip_both_directions_work() {
        use crate::BiMap;
        let mut bm: BiMap<String, i32> = BiMap::new();
        for i in 0..50 {
            bm.insert(format!("key-{i}"), i);
        }

        let pool = BiMapPool::from_bimap(&bm);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: BiMapPool<String, i32> = serde_json::from_str(&json).unwrap();
        let restored: BiMap<String, i32> = loaded.to_bimap();

        for i in 0..50 {
            assert_eq!(restored.get_by_key(&format!("key-{i}")), Some(&i));
            assert_eq!(restored.get_by_value(&i), Some(&format!("key-{i}")));
        }
    }

    // --- SymMapPool tests ---

    #[test]
    fn symmap_roundtrip_single() {
        use crate::{Direction, SymMap};
        let mut sm: SymMap<&str> = SymMap::new();
        sm.insert("hello", "hola");
        sm.insert("goodbye", "adiós");

        let pool = SymMapPool::from_symmap(&sm);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: SymMapPool<&str> = serde_json::from_str(&json).unwrap();
        let restored: SymMap<&str> = loaded.to_symmap();

        assert_eq!(
            restored.get(Direction::Forward, &"hello"),
            Some(&"hola")
        );
        assert_eq!(
            restored.get(Direction::Backward, &"hola"),
            Some(&"hello")
        );
    }

    #[test]
    fn symmap_roundtrip_preserves_both() {
        use crate::{Direction, SymMap};
        let mut sm1: SymMap<i32> = SymMap::new();
        for i in 0..50i32 {
            sm1.insert(i, i + 1000);
        }
        let mut sm2 = sm1.clone();
        sm2.insert(999, 9999);

        let pool = SymMapPool::from_symmaps(&[&sm1, &sm2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: SymMapPool<i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<SymMap<i32>> = loaded.to_symmaps();

        for i in 0..50i32 {
            assert_eq!(maps[0].get(Direction::Forward, &i), Some(&(i + 1000)));
            assert_eq!(maps[0].get(Direction::Backward, &(i + 1000)), Some(&i));
        }
        assert_eq!(maps[1].get(Direction::Forward, &999), Some(&9999));
    }

    // --- 6.10 dedup tests ---

    #[test]
    fn hashmap_dedup_same_lineage_share_nodes() {
        // Two maps cloned from the same base, then independently mutated
        // identically, end up with content-equal but non-pointer-equal nodes
        // for the new entries. The dedup collector merges them via Merkle hash;
        // the plain collector cannot (different pointers, no shared allocation).
        use crate::HashMap;
        let base: HashMap<i32, i32> = (0..100).map(|i| (i, i * 2)).collect();
        let mut map1 = base.clone();
        let mut map2 = base.clone();

        // Insert the same 50 entries into each map independently.
        // These nodes are built from the same hasher seed (inherited from base),
        // so they have the same Merkle hashes — but live at different addresses.
        for i in 200..250i32 {
            map1.insert(i, i * 2);
        }
        for i in 200..250i32 {
            map2.insert(i, i * 2);
        }

        let pool_dedup = HashMapPool::from_maps_dedup(&[&map1, &map2]);
        let pool_plain = HashMapPool::from_maps(&[&map1, &map2]);

        // Dedup pool should have fewer nodes: the new nodes added to map1 and
        // map2 independently are content-equal and have matching Merkle hashes,
        // so they are merged into a single pool entry.
        assert!(
            pool_dedup.nodes.len() < pool_plain.nodes.len(),
            "dedup pool ({}) should be smaller than plain pool ({})",
            pool_dedup.nodes.len(),
            pool_plain.nodes.len()
        );
    }

    #[test]
    fn hashmap_dedup_roundtrip_correctness() {
        // Dedup serialisation must round-trip to equal maps.
        use crate::HashMap;
        let map1: HashMap<String, i32> = (0..50).map(|i| (format!("k{i}"), i)).collect();
        let map2: HashMap<String, i32> = (0..50).map(|i| (format!("k{i}"), i + 1)).collect();

        let pool = HashMapPool::from_maps_dedup(&[&map1, &map2]);
        let json = serde_json::to_string(&pool).unwrap();
        let loaded: HashMapPool<String, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<HashMap<String, i32>> = loaded.to_maps();

        assert_eq!(maps[0], map1);
        assert_eq!(maps[1], map2);
    }

    #[test]
    fn hashmap_dedup_pointer_shared_still_deduplicated() {
        // Pointer-shared maps should still dedup — dedup path must not produce
        // a larger pool than the pointer-based path.
        use crate::HashMap;
        let map1: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut map2 = map1.clone();
        map2.insert(9999, 9999);

        let pool_dedup = HashMapPool::from_maps_dedup(&[&map1, &map2]);
        let pool_plain = HashMapPool::from_maps(&[&map1, &map2]);

        // Dedup must never inflate the pool compared to the pointer-only path.
        assert!(
            pool_dedup.nodes.len() <= pool_plain.nodes.len(),
            "dedup ({}) must not exceed plain ({})",
            pool_dedup.nodes.len(),
            pool_plain.nodes.len()
        );
    }

    #[test]
    fn hashmap_dedup_diverged_clones_correctness() {
        // Two clones of the same map that were independently mutated (same
        // insertions): dedup pool is correct and smaller than plain.
        use crate::HashMap;
        let base: HashMap<String, i32> = (0..50).map(|i| (format!("k{i}"), i)).collect();
        let mut map1 = base.clone();
        let mut map2 = base.clone();

        // Apply same mutations independently — new nodes are content-equal
        // but not pointer-equal.
        for i in 100..130 {
            map1.insert(format!("x{i}"), i);
        }
        for i in 100..130 {
            map2.insert(format!("x{i}"), i);
        }

        let pool = HashMapPool::from_maps_dedup(&[&map1, &map2]);
        let json = serde_json::to_string(&pool).unwrap();
        let loaded: HashMapPool<String, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<HashMap<String, i32>> = loaded.to_maps();

        assert_eq!(maps[0], map1);
        assert_eq!(maps[1], map2);
    }

    #[test]
    fn hashset_dedup_same_lineage_share_nodes() {
        // Two sets cloned from the same base, independently extended with the
        // same elements, produce content-equal but non-pointer-equal new nodes.
        // Dedup merges them; plain does not.
        use crate::HashSet;
        let base: HashSet<i32> = (0..100).collect();
        let mut set1 = base.clone();
        let mut set2 = base.clone();

        for i in 200..250i32 {
            set1.insert(i);
        }
        for i in 200..250i32 {
            set2.insert(i);
        }

        let pool_dedup = HashSetPool::from_sets_dedup(&[&set1, &set2]);
        let pool_plain = HashSetPool::from_sets(&[&set1, &set2]);

        assert!(
            pool_dedup.nodes.len() < pool_plain.nodes.len(),
            "dedup set pool ({}) should be smaller than plain ({})",
            pool_dedup.nodes.len(),
            pool_plain.nodes.len()
        );
    }

    #[test]
    fn hashset_dedup_roundtrip_correctness() {
        use crate::HashSet;
        let set1: HashSet<i32> = (0..50).collect();
        let set2: HashSet<i32> = (50..100).collect();

        let pool = HashSetPool::from_sets_dedup(&[&set1, &set2]);
        let json = serde_json::to_string(&pool).unwrap();
        let loaded: HashSetPool<i32> = serde_json::from_str(&json).unwrap();
        let sets: Vec<HashSet<i32>> = loaded.to_sets();

        assert_eq!(sets[0], set1);
        assert_eq!(sets[1], set2);
    }

    #[test]
    fn bag_dedup_roundtrip_correctness() {
        use crate::Bag;
        let mut bag1: Bag<i32> = Bag::new();
        for i in 0..50 {
            bag1.insert(i);
        }
        let bag2 = bag1.clone();

        let pool = BagPool::from_bags_dedup(&[&bag1, &bag2]);
        let json = serde_json::to_string(&pool).unwrap();
        let loaded: BagPool<i32> = serde_json::from_str(&json).unwrap();
        let bags: Vec<Bag<i32>> = loaded.to_bags();

        assert_eq!(bags[0].total_count(), 50);
        assert_eq!(bags[1].total_count(), 50);
        for i in 0..50 {
            assert_eq!(bags[0].count(&i), 1);
            assert_eq!(bags[1].count(&i), 1);
        }
    }

    #[test]
    fn bimap_dedup_roundtrip_correctness() {
        use crate::BiMap;
        let mut bm1: BiMap<i32, i32> = BiMap::new();
        for i in 0..50 {
            bm1.insert(i, i + 1000);
        }
        let bm2 = bm1.clone();

        let pool = BiMapPool::from_bimaps_dedup(&[&bm1, &bm2]);
        let json = serde_json::to_string(&pool).unwrap();
        let loaded: BiMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<BiMap<i32, i32>> = loaded.to_bimaps();

        for i in 0..50 {
            assert_eq!(maps[0].get_by_key(&i), Some(&(i + 1000)));
            assert_eq!(maps[1].get_by_key(&i), Some(&(i + 1000)));
        }
    }

    #[test]
    fn symmap_dedup_roundtrip_correctness() {
        use crate::{Direction, SymMap};
        let mut sm1: SymMap<i32> = SymMap::new();
        for i in 0..50i32 {
            sm1.insert(i, i + 1000);
        }
        let sm2 = sm1.clone();

        let pool = SymMapPool::from_symmaps_dedup(&[&sm1, &sm2]);
        let json = serde_json::to_string(&pool).unwrap();
        let loaded: SymMapPool<i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<SymMap<i32>> = loaded.to_symmaps();

        for i in 0..50i32 {
            assert_eq!(maps[0].get(Direction::Forward, &i), Some(&(i + 1000)));
            assert_eq!(maps[1].get(Direction::Forward, &i), Some(&(i + 1000)));
        }
    }

    // --- HashMultiMapPool tests ---

    #[test]
    fn hashmultimap_roundtrip_single() {
        use crate::HashMultiMap;
        let mut mm: HashMultiMap<&str, i32> = HashMultiMap::new();
        mm.insert("fruit", 1);
        mm.insert("fruit", 2);
        mm.insert("veggie", 3);

        let pool = HashMultiMapPool::from_map(&mm);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMultiMapPool<&str, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMultiMap<&str, i32> = loaded.to_map();

        assert_eq!(restored.get("fruit").len(), 2);
        assert!(restored.contains("fruit", &1));
        assert!(restored.contains("fruit", &2));
        assert!(restored.contains("veggie", &3));
    }

    #[test]
    fn hashmultimap_empty_roundtrip() {
        use crate::HashMultiMap;
        let mm: HashMultiMap<i32, i32> = HashMultiMap::new();

        let pool = HashMultiMapPool::from_map(&mm);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: HashMultiMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMultiMap<i32, i32> = loaded.to_map();

        assert!(restored.is_empty());
    }

    // --- InsertionOrderMapPool tests ---

    #[test]
    fn insertion_order_map_roundtrip_single() {
        use crate::InsertionOrderMap;
        let mut map: InsertionOrderMap<&str, i32> = InsertionOrderMap::new();
        map.insert("c", 3);
        map.insert("a", 1);
        map.insert("b", 2);

        let pool = InsertionOrderMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: InsertionOrderMapPool<&str, i32> = serde_json::from_str(&json).unwrap();
        let restored: InsertionOrderMap<&str, i32> = loaded.to_map();

        // Verify insertion order preserved
        let keys: Vec<&&str> = restored.keys().collect();
        assert_eq!(keys, vec![&"c", &"a", &"b"]);
        assert_eq!(restored.get(&"a"), Some(&1));
    }

    #[test]
    fn insertion_order_map_roundtrip_preserves_both() {
        use crate::InsertionOrderMap;
        let mut map1: InsertionOrderMap<i32, i32> = InsertionOrderMap::new();
        for i in 0..50 {
            map1.insert(i, i * 2);
        }
        let mut map2 = map1.clone();
        map2.insert(999, 1998);

        let pool = InsertionOrderMapPool::from_maps(&[&map1, &map2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: InsertionOrderMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<InsertionOrderMap<i32, i32>> = loaded.to_maps();

        assert_eq!(maps[0].len(), 50);
        assert_eq!(maps[1].len(), 51);
        assert_eq!(maps[1].get(&999), Some(&1998));
        // Verify original insertion order preserved
        let keys1: Vec<&i32> = maps[0].keys().collect();
        assert_eq!(keys1, (0..50).collect::<Vec<_>>().iter().collect::<Vec<_>>());
    }

    // --- TriePool tests ---

    #[test]
    fn trie_roundtrip_single() {
        use crate::Trie;
        let mut trie: Trie<&str, i32> = Trie::new();
        trie.insert(&["usr", "bin", "rustc"], 1);
        trie.insert(&["usr", "lib", "libc.so"], 2);
        trie.insert(&["etc", "hosts"], 3);

        let pool = TriePool::from_trie(&trie);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: TriePool<&str, i32> = serde_json::from_str(&json).unwrap();
        let restored: Trie<&str, i32> = loaded.to_trie();

        assert_eq!(restored.get(&["usr", "bin", "rustc"]), Some(&1));
        assert_eq!(restored.get(&["usr", "lib", "libc.so"]), Some(&2));
        assert_eq!(restored.get(&["etc", "hosts"]), Some(&3));
    }

    #[test]
    fn trie_roundtrip_preserves_both() {
        use crate::Trie;
        let mut trie1: Trie<&str, i32> = Trie::new();
        trie1.insert(&["a", "b"], 1);
        trie1.insert(&["a", "c"], 2);
        let mut trie2 = trie1.clone();
        trie2.insert(&["d"], 3);

        let pool = TriePool::from_tries(&[&trie1, &trie2]);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: TriePool<&str, i32> = serde_json::from_str(&json).unwrap();
        let tries: Vec<Trie<&str, i32>> = loaded.to_tries();

        assert_eq!(tries[0].get(&["a", "b"]), Some(&1));
        assert_eq!(tries[0].get(&["a", "c"]), Some(&2));
        assert_eq!(tries[0].get(&["d"]), None);
        assert_eq!(tries[1].get(&["d"]), Some(&3));
    }

    #[test]
    fn trie_empty_roundtrip() {
        use crate::Trie;
        let trie: Trie<&str, i32> = Trie::new();

        let pool = TriePool::from_trie(&trie);
        let json = serde_json::to_string(&pool).unwrap();

        let loaded: TriePool<&str, i32> = serde_json::from_str(&json).unwrap();
        let restored: Trie<&str, i32> = loaded.to_trie();

        assert_eq!(restored.get(&["a"]), None);
    }
}
