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
}
