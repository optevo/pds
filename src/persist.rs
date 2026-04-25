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
//! use pds::intern::InternPool;
//!
//! let map1: HashMap<String, i32> = [("a".into(), 1), ("b".into(), 2)].into();
//! let mut map2 = map1.clone();
//! map2.insert("c".into(), 3);
//!
//! // Serialise both maps into a shared pool
//! let pool = HashMapPool::from_maps(&[&map1, &map2]);
//! let json = serde_json::to_string(&pool).unwrap();
//!
//! // Deserialise with interning
//! let mut intern = InternPool::new();
//! let loaded: HashMapPool<String, i32> = serde_json::from_str(&json).unwrap();
//! let maps: Vec<HashMap<String, i32>> = loaded.to_maps(&mut intern);
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
use crate::intern::InternPool;
use crate::nodes::hamt::{CollisionNode, Entry, HamtNode, LargeSimdNode, SmallSimdNode, HASH_WIDTH};

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

// ─── Reconstruction (deserialisation path) ───────────────────────────

/// Reconstructed node, indexed by pool ID.
enum ReconNode<A, P: SharedPointerKind, H: HashWidth> {
    Hamt(SharedPointer<HamtNode<A, P, H>, P>),
    SimdSmall(SharedPointer<SmallSimdNode<A, H>, P>),
    SimdLarge(SharedPointer<LargeSimdNode<A, H>, P>),
    Collision(SharedPointer<CollisionNode<A, H>, P>),
}

impl<K, V, H: HashWidth> HashMapPool<K, V, H>
where
    K: Clone + Hash + Eq + PartialEq,
    V: Clone + PartialEq,
{
    /// Reconstruct HashMaps from this pool with InternPool for
    /// cross-session deduplication.
    pub fn to_maps<S, P>(
        &self,
        intern: &mut InternPool<(K, V), P, H>,
    ) -> Vec<GenericHashMap<K, V, S, P, H>>
    where
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut recon: Vec<ReconNode<(K, V), P, H>> = Vec::with_capacity(self.nodes.len());

        for pool_node in &self.nodes {
            let node = match pool_node {
                PoolNode::SimdSmall(entries) => {
                    let node_entries: Vec<(usize, (K, V), H)> = entries
                        .iter()
                        .map(|(idx, (k, v), h)| (*idx as usize, (k.clone(), v.clone()), *h))
                        .collect();
                    let ptr = SharedPointer::new(SmallSimdNode::from_entries(&node_entries));
                    ReconNode::SimdSmall(intern.intern_small(ptr))
                }
                PoolNode::SimdLarge(entries) => {
                    let node_entries: Vec<(usize, (K, V), H)> = entries
                        .iter()
                        .map(|(idx, (k, v), h)| (*idx as usize, (k.clone(), v.clone()), *h))
                        .collect();
                    let ptr = SharedPointer::new(LargeSimdNode::from_entries(&node_entries));
                    ReconNode::SimdLarge(intern.intern_large(ptr))
                }
                PoolNode::Collision(hash, values) => {
                    let ptr = SharedPointer::new(CollisionNode {
                        hash: *hash,
                        data: values.clone(),
                    });
                    ReconNode::Collision(intern.intern_collision(ptr))
                }
                PoolNode::Hamt(entries) => {
                    let hamt_entries: Vec<(usize, Entry<(K, V), P, H>)> = entries
                        .iter()
                        .map(|(slot, pe)| {
                            let entry = match pe {
                                PoolEntry::Value(a, h) => Entry::Value(a.clone(), *h),
                                PoolEntry::Ref(id) => match &recon[*id as usize] {
                                    ReconNode::Hamt(p) => Entry::HamtNode(p.clone()),
                                    ReconNode::SimdSmall(p) => Entry::SmallSimdNode(p.clone()),
                                    ReconNode::SimdLarge(p) => Entry::LargeSimdNode(p.clone()),
                                    ReconNode::Collision(p) => Entry::Collision(p.clone()),
                                },
                            };
                            (*slot as usize, entry)
                        })
                        .collect();
                    let ptr = SharedPointer::new(HamtNode::from_entries(hamt_entries));
                    ReconNode::Hamt(intern.intern_hamt(ptr))
                }
            };
            recon.push(node);
        }

        self.containers
            .iter()
            .map(|c| {
                let root = c.root.map(|id| match &recon[id as usize] {
                    ReconNode::Hamt(ptr) => ptr.clone(),
                    _ => panic!("pool: root must be a HamtNode"),
                });
                GenericHashMap {
                    root,
                    size: c.size,
                    hasher: S::default(),
                    hasher_id: crate::hash::map::next_hasher_id(),
                    kv_merkle_hash: 0,
                    kv_merkle_valid: false,
                }
            })
            .collect()
    }

    /// Reconstruct a single HashMap (convenience for single-map pools).
    pub fn to_map<S, P>(
        &self,
        intern: &mut InternPool<(K, V), P, H>,
    ) -> GenericHashMap<K, V, S, P, H>
    where
        S: BuildHasher + Default,
        P: SharedPointerKind,
        BitsImpl<HASH_WIDTH>: Bits,
    {
        let mut maps = self.to_maps(intern);
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

        let mut intern = InternPool::new();
        let loaded: HashMapPool<String, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<String, i32> = loaded.to_map(&mut intern);

        assert_eq!(restored, map);
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn roundtrip_large_map() {
        let map: HashMap<i32, i32> = (0..1000).map(|i| (i, i * 2)).collect();

        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let mut intern = InternPool::new();
        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<i32, i32> = loaded.to_map(&mut intern);

        assert_eq!(restored, map);
    }

    #[test]
    fn shared_nodes_deduplicated_in_pool() {
        let base: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut modified = base.clone();
        modified.insert(999, 999);

        let pool = HashMapPool::from_maps(&[&base, &modified]);

        // Two maps sharing structure should produce fewer nodes than
        // two independently constructed maps
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

        let mut intern = InternPool::new();
        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let maps: Vec<HashMap<i32, i32>> = loaded.to_maps(&mut intern);

        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0], map1);
        assert_eq!(maps[1], map2);
    }

    #[test]
    fn intern_pool_deduplicates_on_deserialise() {
        // Serialise a map, deserialise it twice into the same InternPool.
        // The second deserialisation should hit the pool (cross-session dedup).
        let map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let mut intern = InternPool::new();

        // First deserialisation — populates the InternPool
        let loaded1: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let _map1: HashMap<i32, i32> = loaded1.to_map(&mut intern);
        let misses_after_first = intern.stats().misses;

        // Second deserialisation — should hit InternPool for all nodes
        let loaded2: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let map2: HashMap<i32, i32> = loaded2.to_map(&mut intern);
        let hits_after_second = intern.stats().hits;

        assert_eq!(map2, map);
        assert!(
            hits_after_second > 0,
            "expected InternPool hits on second deserialisation"
        );
        // All nodes from the second deserialisation should be hits
        assert!(
            hits_after_second >= misses_after_first,
            "expected at least as many hits ({hits_after_second}) \
             as first-pass misses ({misses_after_first})"
        );
    }

    #[test]
    fn empty_map_roundtrip() {
        let map: HashMap<i32, i32> = HashMap::new();
        let pool = HashMapPool::from_map(&map);
        let json = serde_json::to_string(&pool).unwrap();

        let mut intern = InternPool::new();
        let loaded: HashMapPool<i32, i32> = serde_json::from_str(&json).unwrap();
        let restored: HashMap<i32, i32> = loaded.to_map(&mut intern);

        assert_eq!(restored, map);
        assert!(restored.is_empty());
    }
}
