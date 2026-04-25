// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! An unordered map.
//!
//! An immutable hash map using [hash array mapped tries][1].
//!
//! Most operations on this map are O(log<sub>x</sub> n) for a
//! suitably high *x* that it should be nearly O(1) for most maps.
//! Because of this, it's a great choice for a generic map as long as
//! you don't mind that keys will need to implement
//! [`Hash`][std::hash::Hash] and [`Eq`][std::cmp::Eq].
//!
//! Map entries will have a predictable order based on the hasher
//! being used. Unless otherwise specified, this will be the standard
//! [`RandomState`][std::collections::hash_map::RandomState] hasher.
//!
//! [1]: https://en.wikipedia.org/wiki/Hash_array_mapped_trie
//! [std::cmp::Eq]: https://doc.rust-lang.org/std/cmp/trait.Eq.html
//! [std::hash::Hash]: https://doc.rust-lang.org/std/hash/trait.Hash.html
//! [std::collections::hash_map::RandomState]: https://doc.rust-lang.org/std/collections/hash_map/struct.RandomState.html

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::borrow::Borrow;
#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::{FromIterator, FusedIterator};
use core::mem;
use core::ops::{Index, IndexMut};
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use archery::{SharedPointer, SharedPointerKind};
use equivalent::Equivalent;

use crate::config::{MERKLE_HASH_BITS, MERKLE_POSITIVE_EQ_MIN_BITS};
use crate::hashset::GenericHashSet;
use crate::hash_width::HashWidth;
use crate::nodes::hamt::{
    fmix64, hash_key, Drain as NodeDrain, Entry as NodeEntry, HashValue,
    Iter as NodeIter, IterMut as NodeIterMut, Node, HASH_WIDTH,
};
#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::shared_ptr::DefaultSharedPtr;

/// Construct a hash map from a sequence of key/value pairs.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use pds::HashMap;
/// # fn main() {
/// assert_eq!(
///   hashmap!{
///     1 => 11,
///     2 => 22,
///     3 => 33
///   },
///   HashMap::from(vec![(1, 11), (2, 22), (3, 33)])
/// );
/// # }
/// ```
#[macro_export]
macro_rules! hashmap {
    () => { $crate::hashmap::HashMap::new() };

    ( $( $key:expr => $value:expr ),* ) => {{
        let mut map = $crate::hashmap::HashMap::new();
        $({
            map.insert($key, $value);
        })*;
        map
    }};

    ( $( $key:expr => $value:expr ,)* ) => {{
        let mut map = $crate::hashmap::HashMap::new();
        $({
            map.insert($key, $value);
        })*;
        map
    }};
}

/// Type alias for [`GenericHashMap`] that uses [`std::hash::RandomState`] as the default hasher and [`DefaultSharedPtr`] as the pointer type.
///
/// [GenericHashMap]: ./struct.GenericHashMap.html
/// [`std::hash::RandomState`]: https://doc.rust-lang.org/stable/std/collections/hash_map/struct.RandomState.html
/// [DefaultSharedPtr]: ../shared_ptr/type.DefaultSharedPtr.html
#[cfg(feature = "std")]
pub type HashMap<K, V> = GenericHashMap<K, V, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericHashMap`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type HashMap<K, V> =
    GenericHashMap<K, V, foldhash::fast::RandomState, DefaultSharedPtr>;

/// An unordered map backed by a [hash array mapped trie][1].
///
/// ## Complexity vs Standard Library
///
/// | Operation | `HashMap` | [`std::HashMap`] |
/// |---|---|---|
/// | `clone` | **O(1)** | O(n) |
/// | `eq` (Merkle, same lineage) | **O(1)**† | O(n) |
/// | `eq` (different lineage) | O(n) | O(n) |
/// | `get` / `contains_key` | O(log₃₂ n) ≈ O(1) | O(1) |
/// | `insert` | O(log₃₂ n) ≈ O(1) | O(1)\* |
/// | `remove` | O(log₃₂ n) ≈ O(1) | O(1) |
/// | `union` / `intersection` | O(n + m) | O(n + m) |
/// | `from_iter` | O(n log₃₂ n) ≈ O(n) | O(n) |
///
/// **Bold** = asymptotically better than the std alternative.
/// \* = amortised. † = requires both maps to share a hasher instance
/// (common ancestor via `clone`, which is the normal persistent-data
/// workflow). Invalidated by in-place mutations; call
/// [`recompute_kv_merkle`][Self::recompute_kv_merkle] to restore.
///
/// The O(log₃₂ n) operations are *effectively* O(1) for practical sizes:
/// log₃₂(1 billion) < 7, so the depth never exceeds single digits.
///
/// The key advantage over `std::HashMap` is `clone` in O(1) via
/// structural sharing. Two maps that share a common ancestor share
/// all unmodified subtries in memory — only the modified path is copied
/// on write. This also makes `union` faster in practice when most
/// entries are shared (Merkle hashes allow skipping identical subtries).
///
/// ## Merkle Hashing
///
/// Two levels of Merkle hash are maintained:
///
/// 1. **Key Merkle** (per HAMT node): commutative hash of all keys in
///    the subtrie. Maintained automatically. Used for O(1) inequality
///    detection — different key Merkle → maps are definitely not equal.
///
/// 2. **KV Merkle** (per map): commutative hash covering both keys and
///    values. Maintained incrementally by [`insert`][Self::insert] and
///    [`remove`][Self::remove] (requires `V: Hash`). In-place mutations
///    (`get_mut`, `index_mut`, entry API) invalidate it — call
///    [`recompute_kv_merkle`][Self::recompute_kv_merkle] to re-seal.
///
/// When both maps have valid KV Merkle and the same hasher instance:
/// matching hash + matching size = equal (O(1)). The false-positive
/// rate is ~2⁻⁶⁴, below DRAM bit-flip rates.
///
/// Keys must implement [`Hash`][std::hash::Hash] and [`Eq`][std::cmp::Eq].
/// Values must implement [`Hash`][std::hash::Hash] for mutation methods.
///
/// [`std::HashMap`]: https://doc.rust-lang.org/std/collections/struct.HashMap.html
/// [1]: https://en.wikipedia.org/wiki/Hash_array_mapped_trie
/// [std::cmp::Eq]: https://doc.rust-lang.org/std/cmp/trait.Eq.html
/// [std::hash::Hash]: https://doc.rust-lang.org/std/hash/trait.Hash.html
/// [std::collections::hash_map::RandomState]: https://doc.rust-lang.org/std/collections/hash_map/struct.RandomState.html
pub struct GenericHashMap<K, V, S, P: SharedPointerKind, H: HashWidth = u64> {
    pub(crate) size: usize,
    pub(crate) root: Option<SharedPointer<Node<(K, V), P, H>, P>>,
    pub(crate) hasher: S,
    /// Identifies the hasher lineage. Maps cloned from a common ancestor
    /// share the same `hasher_id`, enabling O(1) Merkle equality checks.
    /// Independently-constructed maps get unique IDs.
    pub(crate) hasher_id: u64,
    /// Merkle hash of keys AND values. When valid, enables O(1) positive
    /// equality: same hasher + same size + same kv_merkle → equal with
    /// probability 1 − 2⁻⁶⁴ (below hardware error rates).
    /// Contribution per entry: `fmix64(key_hash.to_u64().wrapping_add(value_hash))`.
    pub(crate) kv_merkle_hash: u64,
    /// Whether `kv_merkle_hash` is current. Invalidated by in-place
    /// mutations (`get_mut`, `index_mut`, `iter_mut`, entry API mutations)
    /// that bypass value hashing.
    pub(crate) kv_merkle_valid: bool,
}

/// Monotonic counter for hasher identity tracking. Maps cloned from the
/// same source share a hasher_id; independently-constructed maps get
/// distinct IDs. See DEC-024 in docs/decisions.md.
static NEXT_HASHER_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_hasher_id() -> u64 {
    NEXT_HASHER_ID.fetch_add(1, Relaxed)
}

impl<K, V> HashValue for (K, V)
where
    K: Eq,
{
    type Key = K;

    fn extract_key(&self) -> &Self::Key {
        &self.0
    }

    fn ptr_eq(&self, _other: &Self) -> bool {
        false
    }
}

#[cfg(feature = "std")]
impl<K, V, P, H: HashWidth> GenericHashMap<K, V, RandomState, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    P: SharedPointerKind,
{
    /// Construct a hash map with a single mapping.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::HashMap;
    /// let map = HashMap::unit(123, "onetwothree");
    /// assert_eq!(
    ///   map.get(&123),
    ///   Some(&"onetwothree")
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn unit(k: K, v: V) -> GenericHashMap<K, V, RandomState, P, H> {
        GenericHashMap::new().update(k, v)
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P, H: HashWidth> GenericHashMap<K, V, foldhash::fast::RandomState, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    P: SharedPointerKind,
{
    /// Construct a hash map with a single mapping (no_std + foldhash).
    #[inline]
    #[must_use]
    pub fn unit(k: K, v: V) -> GenericHashMap<K, V, foldhash::fast::RandomState, P, H> {
        GenericHashMap::new().update(k, v)
    }
}

impl<K, V, S, P: SharedPointerKind, H: HashWidth> GenericHashMap<K, V, S, P, H> {
    /// Construct an empty hash map.
    #[inline]
    #[must_use]
    pub fn new() -> Self
    where
        S: Default,
    {
        Self::default()
    }

    /// Test whether a hash map is empty.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// assert!(
    ///   !hashmap!{1 => 2}.is_empty()
    /// );
    /// assert!(
    ///   HashMap::<i32, i32>::new().is_empty()
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the size of a hash map.
    ///
    /// Time: O(1)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// assert_eq!(3, hashmap!{
    ///   1 => 11,
    ///   2 => 22,
    ///   3 => 33
    /// }.len());
    /// ```
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.size
    }

    /// Test whether two maps refer to the same content in memory.
    ///
    /// This is true if the two sides are references to the same map,
    /// or if the two maps refer to the same root node.
    ///
    /// This would return true if you're comparing a map to itself, or
    /// if you're comparing a map to a fresh clone of itself.
    ///
    /// Time: O(1)
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (Some(a), Some(b)) => SharedPointer::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// Construct an empty hash map using the provided hasher.
    #[inline]
    #[must_use]
    pub fn with_hasher(hasher: S) -> Self {
        GenericHashMap {
            size: 0,
            hasher,
            hasher_id: next_hasher_id(),
            root: None,
            kv_merkle_hash: 0,
            kv_merkle_valid: true,
        }
    }

    /// Get a reference to the map's [`BuildHasher`][BuildHasher].
    ///
    /// [BuildHasher]: https://doc.rust-lang.org/std/hash/trait.BuildHasher.html
    #[must_use]
    pub fn hasher(&self) -> &S {
        &self.hasher
    }

    /// Construct an empty hash map using the same hasher as the
    /// current hash map.
    #[inline]
    #[must_use]
    pub fn new_from<K1, V1>(&self) -> GenericHashMap<K1, V1, S, P, H>
    where
        K1: Hash + Eq + Clone,
        V1: Clone,
        S: Clone,
    {
        GenericHashMap {
            size: 0,
            root: None,
            hasher: self.hasher.clone(),
            hasher_id: self.hasher_id,
            kv_merkle_hash: 0,
            kv_merkle_valid: true,
        }
    }

    /// Get an iterator over the key/value pairs of a hash map.
    ///
    /// Please note that the order is consistent between maps using
    /// the same hasher, but no other ordering guarantee is offered.
    /// Items will not come out in insertion order or sort order.
    /// They will, however, come out in the same order every time for
    /// the same map.
    #[inline]
    #[must_use]
    pub fn iter(&self) -> Iter<'_, K, V, P, H> {
        Iter {
            it: NodeIter::new(self.root.as_deref(), self.size),
        }
    }

    /// Get an iterator over a hash map's keys.
    ///
    /// Please note that the order is consistent between maps using
    /// the same hasher, but no other ordering guarantee is offered.
    /// Items will not come out in insertion order or sort order.
    /// They will, however, come out in the same order every time for
    /// the same map.
    #[inline]
    #[must_use]
    pub fn keys(&self) -> Keys<'_, K, V, P, H> {
        Keys {
            it: NodeIter::new(self.root.as_deref(), self.size),
        }
    }

    /// Get an iterator over a hash map's values.
    ///
    /// Please note that the order is consistent between maps using
    /// the same hasher, but no other ordering guarantee is offered.
    /// Items will not come out in insertion order or sort order.
    /// They will, however, come out in the same order every time for
    /// the same map.
    #[inline]
    #[must_use]
    pub fn values(&self) -> Values<'_, K, V, P, H> {
        Values {
            it: NodeIter::new(self.root.as_deref(), self.size),
        }
    }

    /// Discard all elements from the map.
    ///
    /// This leaves you with an empty map, and all elements that
    /// were previously inside it are dropped.
    ///
    /// Time: O(n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::HashMap;
    /// let mut map = hashmap![1=>1, 2=>2, 3=>3];
    /// map.clear();
    /// assert!(map.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.root = None;
        self.size = 0;
        self.kv_merkle_hash = 0;
        self.kv_merkle_valid = true;
    }

    /// Recompute the key-value Merkle hash by traversing all entries.
    ///
    /// Call this after operations that invalidate the kv_merkle
    /// (`get_mut`, `index_mut`, entry API mutations, set operations)
    /// to re-enable O(1) positive equality checks.
    ///
    /// Time: O(n)
    pub fn recompute_kv_merkle(&mut self)
    where
        K: Hash + Eq,
        V: Hash,
        S: BuildHasher,
    {
        let mut kv_merkle: u64 = 0;
        if let Some(root) = &self.root {
            for ((_, v), key_hash) in NodeIter::new(Some(root), self.size) {
                let value_hash = self.hasher.hash_one(v);
                kv_merkle =
                    kv_merkle.wrapping_add(fmix64(key_hash.to_u64().wrapping_add(value_hash)));
            }
        }
        self.kv_merkle_hash = kv_merkle;
        self.kv_merkle_valid = true;
    }

    /// Whether the key-value Merkle hash is currently valid. When true,
    /// equality checks against maps sharing the same hasher are O(1).
    #[inline]
    #[must_use]
    pub fn kv_merkle_valid(&self) -> bool {
        self.kv_merkle_valid
    }

    /// Print a summary of the HashMap structure showing per-level statistics.
    /// This includes the number of nodes at each level and the distribution of child types.
    #[cfg(test)]
    pub fn print_structure_summary(&self) {
        use crate::nodes::hamt::Entry as NodeEntry;
        use alloc::collections::VecDeque;

        println!("HashMap Structure Summary:");

        #[derive(Default, Debug)]
        struct LevelStats {
            node_count: usize,
            value_count: usize,
            collision_count: usize,
            collision_entry_sum: usize,
            child_node_count: usize,
            small_simd_node_count: usize,
            large_simd_node_count: usize,
            small_simd_entry_sum: usize,
            large_simd_entry_sum: usize,
            total_entries: usize,
        }

        if self.root.is_none() {
            println!("  Empty HashMap (no root node)");
            println!("  Total entries: 0");
            return;
        }

        let mut level_stats: Vec<LevelStats> = Vec::new();
        let mut queue: VecDeque<(usize, SharedPointer<Node<(K, V), P, H>, P>)> = VecDeque::new();
        let mut max_depth = 0;

        // Start with root node at level 0
        if let Some(ref root) = self.root {
            queue.push_back((0, root.clone()));
        }

        // BFS traversal to collect statistics
        while let Some((level, node)) = queue.pop_front() {
            // Ensure we have stats for this level
            while level_stats.len() <= level {
                level_stats.push(LevelStats::default());
            }

            let stats = &mut level_stats[level];
            stats.node_count += 1;

            // Analyze this node's entries
            node.analyze_structure(|entry| {
                stats.total_entries += 1;
                match entry {
                    NodeEntry::Value(_, _) => {
                        stats.value_count += 1;
                        max_depth = max_depth.max(level);
                    }
                    NodeEntry::Collision(_coll) => {
                        stats.collision_count += 1;
                        // stats.collision_entry_sum += coll.len();
                        max_depth = max_depth.max(level);
                    }
                    NodeEntry::HamtNode(child_node) => {
                        stats.child_node_count += 1;
                        queue.push_back((level + 1, child_node.clone()));
                    }
                    NodeEntry::SmallSimdNode(small_node) => {
                        stats.small_simd_node_count += 1;
                        stats.small_simd_entry_sum += small_node.len();
                        max_depth = max_depth.max(level + 1);
                    }
                    NodeEntry::LargeSimdNode(large_node) => {
                        stats.large_simd_node_count += 1;
                        stats.large_simd_entry_sum += large_node.len();
                        max_depth = max_depth.max(level + 1);
                    }
                }
            })
        }

        // Print the summary
        println!(
            "  Hash level size (bits): {}",
            crate::config::HASH_LEVEL_SIZE
        );
        println!(
            "  Branching factor: {}",
            2_usize.pow(crate::config::HASH_LEVEL_SIZE as u32)
        );
        println!("  Total entries: {}", self.size);
        println!("  Tree depth: {} levels", max_depth + 1);
        println!();

        for (level, stats) in level_stats.iter().enumerate() {
            println!("  Level {}:", level);
            println!("    Nodes: {}", stats.node_count);

            if stats.total_entries > 0 {
                let avg_entries = stats.total_entries as f64 / stats.node_count as f64;
                println!("    Average entries per node: {:.2}", avg_entries);

                println!("    Entry types:");
                println!(
                    "      Values: {} ({:.1}%)",
                    stats.value_count,
                    (stats.value_count as f64 / stats.total_entries as f64) * 100.0
                );
                println!(
                    "      Collisions: {} (avg len: {:.1}) ({:.1}%)",
                    stats.collision_count,
                    if stats.collision_count > 0 {
                        stats.collision_entry_sum as f64 / stats.collision_count as f64
                    } else {
                        0.0
                    },
                    (stats.collision_count as f64 / stats.total_entries as f64) * 100.0
                );
                println!(
                    "      Child HAMT nodes: {} ({:.1}%)",
                    stats.child_node_count,
                    (stats.child_node_count as f64 / stats.total_entries as f64) * 100.0
                );
                if stats.small_simd_node_count > 0 {
                    println!(
                        "      Small SIMD leaf nodes: {} ({:.1}%) [total values: {}]",
                        stats.small_simd_node_count,
                        (stats.small_simd_node_count as f64 / stats.total_entries as f64) * 100.0,
                        stats.small_simd_entry_sum
                    );
                    println!(
                        "        → Avg values per small SIMD node: {:.1}",
                        stats.small_simd_entry_sum as f64 / stats.small_simd_node_count as f64
                    );
                }
                if stats.large_simd_node_count > 0 {
                    println!(
                        "      Large SIMD leaf nodes: {} ({:.1}%) [total values: {}]",
                        stats.large_simd_node_count,
                        (stats.large_simd_node_count as f64 / stats.total_entries as f64) * 100.0,
                        stats.large_simd_entry_sum
                    );
                    println!(
                        "        → Avg values per large SIMD node: {:.1}",
                        stats.large_simd_entry_sum as f64 / stats.large_simd_node_count as f64
                    );
                }
            }
            println!();
        }
    }
}

impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn test_eq<S2: BuildHasher, P2: SharedPointerKind>(
        &self,
        other: &GenericHashMap<K, V, S2, P2, H>,
    ) -> bool
    where
        V: PartialEq,
    {
        if self.len() != other.len() {
            return false;
        }
        // Fast path: if both roots point to the same allocation, the maps
        // are identical. Compares type-erased data pointers so it works
        // across different SharedPointerKind type parameters (which can
        // never share an allocation, so this correctly returns false).
        match (&self.root, &other.root) {
            (None, None) => return true,
            (Some(a), Some(b)) => {
                let a_ptr = &**a as *const _ as *const ();
                let b_ptr = &**b as *const _ as *const ();
                if a_ptr == b_ptr {
                    return true;
                }
                if self.hasher_id == other.hasher_id {
                    // Merkle negative check: different key Merkle → different key sets.
                    if a.merkle_hash != b.merkle_hash {
                        return false;
                    }
                    // KV Merkle positive check: same key+value Merkle → equal.
                    // Only safe when hash width ≥ 64 bits (DEC-023).
                    if MERKLE_HASH_BITS >= MERKLE_POSITIVE_EQ_MIN_BITS
                        && self.kv_merkle_valid
                        && other.kv_merkle_valid
                        && self.kv_merkle_hash == other.kv_merkle_hash
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
        for (key, value) in self.iter() {
            if Some(value) != other.get(key) {
                return false;
            }
        }
        true
    }

    /// Get the value for a key from a hash map.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{123 => "lol"};
    /// assert_eq!(
    ///   map.get(&123),
    ///   Some(&"lol")
    /// );
    /// ```
    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        if let Some(root) = &self.root {
            root.get(hash_key(&self.hasher, key), 0, key)
                .map(|(_, v)| v)
        } else {
            None
        }
    }

    /// Get the key/value pair for a key from a hash map.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{123 => "lol"};
    /// assert_eq!(
    ///   map.get_key_value(&123),
    ///   Some((&123, &"lol"))
    /// );
    /// ```
    #[must_use]
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        if let Some(root) = &self.root {
            root.get(hash_key(&self.hasher, key), 0, key)
                .map(|(k, v)| (k, v))
        } else {
            None
        }
    }

    /// Test for the presence of a key in a hash map.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{123 => "lol"};
    /// assert!(
    ///   map.contains_key(&123)
    /// );
    /// assert!(
    ///   !map.contains_key(&321)
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn contains_key<Q>(&self, k: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get(k).is_some()
    }

    /// Test whether a map is a submap of another map, meaning that
    /// all keys in our map must also be in the other map, with the
    /// same values.
    ///
    /// Use the provided function to decide whether values are equal.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn is_submap_by<B, RM, F, P2: SharedPointerKind>(&self, other: RM, mut cmp: F) -> bool
    where
        F: FnMut(&V, &B) -> bool,
        RM: Borrow<GenericHashMap<K, B, S, P2, H>>,
    {
        self.iter()
            .all(|(k, v)| other.borrow().get(k).map(|ov| cmp(v, ov)).unwrap_or(false))
    }

    /// Test whether a map is a proper submap of another map, meaning
    /// that all keys in our map must also be in the other map, with
    /// the same values. To be a proper submap, ours must also contain
    /// fewer keys than the other map.
    ///
    /// Use the provided function to decide whether values are equal.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn is_proper_submap_by<B, RM, F, P2: SharedPointerKind>(&self, other: RM, cmp: F) -> bool
    where
        F: FnMut(&V, &B) -> bool,
        RM: Borrow<GenericHashMap<K, B, S, P2, H>>,
    {
        self.len() != other.borrow().len() && self.is_submap_by(other, cmp)
    }

    /// Test whether a map is a submap of another map, meaning that
    /// all keys in our map must also be in the other map, with the
    /// same values.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 2 => 2};
    /// let map2 = hashmap!{1 => 1, 2 => 2, 3 => 3};
    /// assert!(map1.is_submap(map2));
    /// ```
    #[inline]
    #[must_use]
    pub fn is_submap<RM>(&self, other: RM) -> bool
    where
        V: PartialEq,
        RM: Borrow<Self>,
    {
        self.is_submap_by(other.borrow(), PartialEq::eq)
    }

    /// Test whether a map is a proper submap of another map, meaning
    /// that all keys in our map must also be in the other map, with
    /// the same values. To be a proper submap, ours must also contain
    /// fewer keys than the other map.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 2 => 2};
    /// let map2 = hashmap!{1 => 1, 2 => 2, 3 => 3};
    /// assert!(map1.is_proper_submap(map2));
    ///
    /// let map3 = hashmap!{1 => 1, 2 => 2};
    /// let map4 = hashmap!{1 => 1, 2 => 2};
    /// assert!(!map3.is_proper_submap(map4));
    /// ```
    #[inline]
    #[must_use]
    pub fn is_proper_submap<RM>(&self, other: RM) -> bool
    where
        V: PartialEq,
        RM: Borrow<Self>,
    {
        self.is_proper_submap_by(other.borrow(), PartialEq::eq)
    }

    /// Compute the diff between two hash maps.
    ///
    /// Returns an iterator of [`DiffItem`] values describing the
    /// differences between `self` (old) and `other` (new). Keys
    /// present only in `self` produce [`DiffItem::Remove`], keys
    /// present only in `other` produce [`DiffItem::Add`], and keys
    /// present in both with different values produce
    /// [`DiffItem::Update`].
    ///
    /// If the two maps share the same root (i.e.
    /// [`ptr_eq`][GenericHashMap::ptr_eq] returns true), the iterator
    /// is empty without traversing any elements.
    ///
    /// When the two maps share structure (one was derived from the other
    /// via insert/remove), shared subtrees are detected via pointer
    /// comparison and skipped in O(1), reducing complexity to
    /// O(changes × tree_depth). For independently-constructed maps with
    /// different hasher states, falls back to O(n + m).
    ///
    /// ## Performance tip
    ///
    /// For independently-constructed maps with high content overlap,
    /// call `intern` (requires the `hash-intern` feature) on both before
    /// diffing. After interning, content-equal subtrees share the same
    /// allocation and are skipped in O(1), reducing diff complexity from
    /// O(n + m) to O(changes × depth).
    #[must_use]
    pub fn diff<'a, 'b>(&'a self, other: &'b Self) -> DiffIter<'a, 'b, K, V, S, P, H>
    where
        V: PartialEq,
    {
        let mut diffs = Vec::new();
        if !self.ptr_eq(other) {
            // kv_merkle fast-path: same-lineage maps (same hasher_id) with
            // valid, matching kv_merkle hashes are almost certainly equal —
            // skip the tree walk entirely. Same probabilistic argument as
            // PartialEq (DEC-023): false positive rate ≈ 2^-64.
            if self.size == other.size
                && self.hasher_id == other.hasher_id
                && MERKLE_HASH_BITS >= MERKLE_POSITIVE_EQ_MIN_BITS
                && self.kv_merkle_valid
                && other.kv_merkle_valid
                && self.kv_merkle_hash == other.kv_merkle_hash
            {
                return DiffIter {
                    diffs,
                    index: 0,
                    _phantom: core::marker::PhantomData,
                };
            }
            match (&self.root, &other.root) {
                (Some(old_root), Some(new_root)) => {
                    if !SharedPointer::ptr_eq(old_root, new_root) {
                        // Tree walk requires compatible hashers (same hash
                        // seeds). Maps derived from a common ancestor share
                        // their hasher; independently-constructed maps may
                        // not. Probe with a sentinel to detect.
                        if hashers_compatible(&self.hasher, &other.hasher) {
                            diff_hamt_nodes(old_root, new_root, &mut diffs);
                        } else {
                            diff_iterate_and_lookup(self, other, &mut diffs);
                        }
                    }
                }
                (Some(_), None) => {
                    for (k, v) in self.iter() {
                        diffs.push(DiffItem::Remove(k, v));
                    }
                }
                (None, Some(_)) => {
                    for (k, v) in other.iter() {
                        diffs.push(DiffItem::Add(k, v));
                    }
                }
                (None, None) => {}
            }
        }
        DiffIter {
            diffs,
            index: 0,
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Apply a diff to produce a new map.
    ///
    /// Takes any iterator of [`DiffItem`] values (such as from
    /// [`diff`][GenericHashMap::diff]) and applies each change —
    /// `Add` and `Update` insert entries, `Remove` removes entries.
    ///
    /// Time: O(d log n) where d is the number of diff items
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let base = hashmap!{1 => "a", 2 => "b", 3 => "c"};
    /// let modified = hashmap!{1 => "a", 2 => "B", 4 => "d"};
    /// let diff: Vec<_> = base.diff(&modified).collect();
    /// let patched = base.apply_diff(diff);
    /// assert_eq!(patched, modified);
    /// ```
    #[must_use]
    pub fn apply_diff<'a, 'b, I>(&self, diff: I) -> Self
    where
        I: IntoIterator<Item = DiffItem<'a, 'b, K, V>>,
        K: 'a + 'b,
        V: 'a + 'b,
    {
        let mut out = self.clone();
        for item in diff {
            match item {
                DiffItem::Add(k, v) | DiffItem::Update { new: (k, v), .. } => {
                    out.insert_invalidate_kv(k.clone(), v.clone());
                }
                DiffItem::Remove(k, _) => {
                    out.remove_invalidate_kv(k);
                }
            }
        }
        out
    }

    /// Split a map into two maps, where the first contains entries
    /// that satisfy the predicate and the second contains entries
    /// that do not.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => "one", 2 => "two", 3 => "three", 4 => "four"};
    /// let (evens, odds) = map.partition(|k, _| k % 2 == 0);
    /// assert_eq!(evens, hashmap!{2 => "two", 4 => "four"});
    /// assert_eq!(odds, hashmap!{1 => "one", 3 => "three"});
    /// ```
    #[must_use]
    pub fn partition<F>(&self, mut f: F) -> (Self, Self)
    where
        S: Default,
        F: FnMut(&K, &V) -> bool,
    {
        let mut left = Self::new();
        let mut right = Self::new();
        for (k, v) in self.iter() {
            if f(k, v) {
                left.insert_invalidate_kv(k.clone(), v.clone());
            } else {
                right.insert_invalidate_kv(k.clone(), v.clone());
            }
        }
        (left, right)
    }

    /// Partition and transform a map into two maps with potentially
    /// different value types. The closure returns `Ok(v1)` to place
    /// the entry in the left map, or `Err(v2)` for the right map.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => 10, 2 => 20, 3 => 30};
    /// let (small, big): (HashMap<i32, String>, HashMap<i32, String>) =
    ///     map.partition_map(|_k, v| {
    ///         if *v <= 15 { Ok(format!("small:{v}")) }
    ///         else { Err(format!("big:{v}")) }
    ///     });
    /// assert_eq!(small.len(), 1);
    /// assert_eq!(small[&1], "small:10");
    /// assert_eq!(big.len(), 2);
    /// ```
    #[must_use]
    pub fn partition_map<V1, V2, F>(
        &self,
        mut f: F,
    ) -> (GenericHashMap<K, V1, S, P, H>, GenericHashMap<K, V2, S, P, H>)
    where
        V1: Clone,
        V2: Clone,
        S: Default,
        F: FnMut(&K, &V) -> Result<V1, V2>,
    {
        let mut left = GenericHashMap::new();
        let mut right = GenericHashMap::new();
        for (k, v) in self.iter() {
            match f(k, v) {
                Ok(v1) => {
                    left.insert_invalidate_kv(k.clone(), v1);
                }
                Err(v2) => {
                    right.insert_invalidate_kv(k.clone(), v2);
                }
            }
        }
        (left, right)
    }

    /// Asymmetric difference with a resolver function.
    ///
    /// For keys in both `self` and `other`, `f` decides whether to
    /// keep, modify, or discard the entry. Keys only in `self` are
    /// kept. Keys only in `other` are discarded.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let a = hashmap!{1 => 10, 2 => 20, 3 => 30};
    /// let b = hashmap!{2 => 5, 3 => 50, 4 => 40};
    /// let result = a.difference_with(&b, |_k, v_self, v_other| {
    ///     if v_self > v_other { Some(*v_self - *v_other) } else { None }
    /// });
    /// assert_eq!(result.len(), 2);
    /// assert_eq!(result[&1], 10);
    /// assert_eq!(result[&2], 15);
    /// ```
    #[must_use]
    pub fn difference_with<F>(&self, other: &Self, mut f: F) -> Self
    where
        S: Default,
        F: FnMut(&K, &V, &V) -> Option<V>,
    {
        let mut result = Self::new();
        for (k, v) in self.iter() {
            match other.get(k) {
                Some(v2) => {
                    if let Some(new_v) = f(k, v, v2) {
                        result.insert_invalidate_kv(k.clone(), new_v);
                    }
                }
                None => {
                    result.insert_invalidate_kv(k.clone(), v.clone());
                }
            }
        }
        result
    }

}

// Internal helpers that don't require V: Hash. Used by set operations and
// entry-based methods that go through the HAMT directly.
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Insert without maintaining kv_merkle (invalidates it).
    /// Used by internal code paths that don't have V: Hash.
    pub(crate) fn insert_invalidate_kv(&mut self, k: K, v: V) -> Option<V> {
        let hash = hash_key(&self.hasher, &k);
        let root = SharedPointer::make_mut(self.root.get_or_insert_with(SharedPointer::default));
        let result = root.insert(hash, 0, (k, v));
        if result.is_none() {
            self.size += 1;
        }
        self.kv_merkle_valid = false;
        result.map(|(_, v)| v)
    }

    /// Remove without maintaining kv_merkle (invalidates it).
    /// Used by internal code paths that don't have V: Hash.
    pub(crate) fn remove_invalidate_kv<Q>(&mut self, k: &Q) -> Option<(K, V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        let hash = hash_key(&self.hasher, k);
        let Some(root) = &mut self.root else {
            return None;
        };
        let result = SharedPointer::make_mut(root).remove(hash, 0, k);
        if result.is_some() {
            self.size -= 1;
            self.kv_merkle_valid = false;
        }
        result
    }
}

// Mutating methods that need K: Clone + V: Clone for copy-on-write but NOT S: Clone.
// These use SharedPointer::make_mut (which clones the node, needing K+V Clone)
// but never clone the hasher.
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Get a mutable iterator over the values of a hash map.
    ///
    /// Please note that the order is consistent between maps using
    /// the same hasher, but no other ordering guarantee is offered.
    /// Items will not come out in insertion order or sort order.
    /// They will, however, come out in the same order every time for
    /// the same map.
    #[inline]
    #[must_use]
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V, P, H> {
        self.kv_merkle_valid = false;
        let root = self.root.as_mut().map(|r| SharedPointer::make_mut(r));
        IterMut {
            it: NodeIterMut::new(root, self.size),
        }
    }

    /// Get a mutable reference to the value for a key from a hash
    /// map.
    ///
    /// Note: invalidates the key-value Merkle hash, causing subsequent
    /// equality checks to fall back to O(n). Use [`insert`][Self::insert]
    /// to replace values while preserving O(1) equality.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let mut map = hashmap!{123 => "lol"};
    /// if let Some(value) = map.get_mut(&123) {
    ///     *value = "omg";
    /// }
    /// assert_eq!(
    ///   map.get(&123),
    ///   Some(&"omg")
    /// );
    /// ```
    #[must_use]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_key_value_mut(key).map(|(_, v)| v)
    }

    /// Get the key/value pair for a key from a hash map, returning a mutable reference to the value.
    ///
    /// Note: invalidates the key-value Merkle hash. See [`get_mut`][Self::get_mut].
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let mut map = hashmap!{123 => "lol"};
    /// assert_eq!(
    ///   map.get_key_value_mut(&123),
    ///   Some((&123, &mut "lol"))
    /// );
    /// ```
    #[must_use]
    pub fn get_key_value_mut<Q>(&mut self, key: &Q) -> Option<(&K, &mut V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.kv_merkle_valid = false;
        let root = self.root.as_mut()?;
        match SharedPointer::make_mut(root).get_mut(hash_key(&self.hasher, key), 0, key) {
            None => None,
            Some((key, value)) => Some((key, value)),
        }
    }

    /// Insert a key/value mapping into a map, maintaining the
    /// key-value Merkle hash for O(1) equality checks.
    ///
    /// If the map already has a mapping for the given key, the
    /// previous value is overwritten.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let mut map = hashmap!{};
    /// map.insert(123, "123");
    /// map.insert(456, "456");
    /// assert_eq!(
    ///   map,
    ///   hashmap!{123 => "123", 456 => "456"}
    /// );
    /// ```
    #[inline]
    pub fn insert(&mut self, k: K, v: V) -> Option<V>
    where
        V: Hash,
    {
        let hash = hash_key(&self.hasher, &k);
        let value_hash = self.hasher.hash_one(&v);
        let root = SharedPointer::make_mut(self.root.get_or_insert_with(SharedPointer::default));
        let result = root.insert(hash, 0, (k, v));
        if let Some((_, ref old_v)) = result {
            if self.kv_merkle_valid {
                let old_value_hash = self.hasher.hash_one(old_v);
                self.kv_merkle_hash = self.kv_merkle_hash
                    .wrapping_sub(fmix64(hash.to_u64().wrapping_add(old_value_hash)))
                    .wrapping_add(fmix64(hash.to_u64().wrapping_add(value_hash)));
            }
        } else {
            self.size += 1;
            if self.kv_merkle_valid {
                self.kv_merkle_hash = self.kv_merkle_hash
                    .wrapping_add(fmix64(hash.to_u64().wrapping_add(value_hash)));
            }
        }
        result.map(|(_, v)| v)
    }

    /// Remove a key/value pair from a map, if it exists, and return
    /// the removed value. Maintains the key-value Merkle hash.
    ///
    /// This is a copy-on-write operation, so that the parts of the
    /// set's structure which are shared with other sets will be
    /// safely copied before mutating.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let mut map = hashmap!{123 => "123", 456 => "456"};
    /// assert_eq!(Some("123"), map.remove(&123));
    /// assert_eq!(Some("456"), map.remove(&456));
    /// assert_eq!(None, map.remove(&789));
    /// assert!(map.is_empty());
    /// ```
    pub fn remove<Q>(&mut self, k: &Q) -> Option<V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
        V: Hash,
    {
        self.remove_with_key(k).map(|(_, v)| v)
    }

    /// Remove a key/value pair from a map, if it exists, and return
    /// the removed key and value. Maintains the key-value Merkle hash.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let mut map = hashmap!{123 => "123", 456 => "456"};
    /// assert_eq!(Some((123, "123")), map.remove_with_key(&123));
    /// assert_eq!(Some((456, "456")), map.remove_with_key(&456));
    /// assert_eq!(None, map.remove_with_key(&789));
    /// assert!(map.is_empty());
    /// ```
    pub fn remove_with_key<Q>(&mut self, k: &Q) -> Option<(K, V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
        V: Hash,
    {
        let hash = hash_key(&self.hasher, k);
        let Some(root) = &mut self.root else {
            return None;
        };
        let result = SharedPointer::make_mut(root).remove(hash, 0, k);
        if let Some((_, ref v)) = result {
            self.size -= 1;
            if self.kv_merkle_valid {
                let value_hash = self.hasher.hash_one(v);
                self.kv_merkle_hash = self.kv_merkle_hash
                    .wrapping_sub(fmix64(hash.to_u64().wrapping_add(value_hash)));
            }
        }
        result
    }

    /// Filter out values from a map which don't satisfy a predicate.
    ///
    /// This is slightly more efficient than filtering using an
    /// iterator, in that it doesn't need to rehash the retained
    /// values, but it still needs to reconstruct the entire tree
    /// structure of the map.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::HashMap;
    /// let mut map = hashmap!{1 => 1, 2 => 2, 3 => 3};
    /// map.retain(|k, v| *k > 1);
    /// let expected = hashmap!{2 => 2, 3 => 3};
    /// assert_eq!(expected, map);
    /// ```
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&K, &V) -> bool,
    {
        let Some(root) = &mut self.root else {
            return;
        };
        self.kv_merkle_valid = false;
        let old_root = root.clone();
        let root = SharedPointer::make_mut(root);
        for ((key, value), hash) in NodeIter::new(Some(&old_root), self.size) {
            if !f(key, value) && root.remove(hash, 0, key).is_some() {
                self.size -= 1;
            }
        }
    }
}

// Methods that clone self or create new maps (persistent API).
// Previously required S: Clone; now S is behind SharedPointer so clone is free.
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Get the [`Entry`][Entry] for a key in the map for in-place manipulation.
    ///
    /// Time: O(log n)
    ///
    /// [Entry]: enum.Entry.html
    #[must_use]
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V, S, P, H> {
        let hash = hash_key(&self.hasher, &key);
        if self
            .root
            .as_ref()
            .and_then(|r| r.get(hash, 0, &key))
            .is_some()
        {
            Entry::Occupied(OccupiedEntry {
                map: self,
                hash,
                key,
            })
        } else {
            Entry::Vacant(VacantEntry {
                map: self,
                hash,
                key,
            })
        }
    }

    /// Construct a new hash map by inserting a key/value mapping into a map.
    ///
    /// If the map already has a mapping for the given key, the previous value
    /// is overwritten.
    ///
    /// Time: O(log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{};
    /// assert_eq!(
    ///   map.update(123, "123"),
    ///   hashmap!{123 => "123"}
    /// );
    /// ```
    #[inline]
    #[must_use]
    pub fn update(&self, k: K, v: V) -> Self
    where
        V: Hash,
    {
        let mut out = self.clone();
        out.insert(k, v);
        out
    }

    /// Construct a new hash map by inserting a key/value mapping into
    /// a map.
    ///
    /// If the map already has a mapping for the given key, we call
    /// the provided function with the old value and the new value,
    /// and insert the result as the new value.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn update_with<F>(&self, k: K, v: V, f: F) -> Self
    where
        F: FnOnce(V, V) -> V,
        V: Hash,
    {
        match self.extract_with_key(&k) {
            None => self.update(k, v),
            Some((_, v2, m)) => m.update(k, f(v2, v)),
        }
    }

    /// Construct a new map by inserting a key/value mapping into a
    /// map.
    ///
    /// If the map already has a mapping for the given key, we call
    /// the provided function with the key, the old value and the new
    /// value, and insert the result as the new value.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn update_with_key<F>(&self, k: K, v: V, f: F) -> Self
    where
        F: FnOnce(&K, V, V) -> V,
        V: Hash,
    {
        match self.extract_with_key(&k) {
            None => self.update(k, v),
            Some((_, v2, m)) => {
                let out_v = f(&k, v2, v);
                m.update(k, out_v)
            }
        }
    }

    /// Construct a new map by inserting a key/value mapping into a
    /// map, returning the old value for the key as well as the new
    /// map.
    ///
    /// If the map already has a mapping for the given key, we call
    /// the provided function with the key, the old value and the new
    /// value, and insert the result as the new value.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn update_lookup_with_key<F>(&self, k: K, v: V, f: F) -> (Option<V>, Self)
    where
        F: FnOnce(&K, &V, V) -> V,
        V: Hash,
    {
        match self.extract_with_key(&k) {
            None => (None, self.update(k, v)),
            Some((_, v2, m)) => {
                let out_v = f(&k, &v2, v);
                (Some(v2), m.update(k, out_v))
            }
        }
    }

    /// Update the value for a given key by calling a function with
    /// the current value and overwriting it with the function's
    /// return value.
    ///
    /// The function gets an [`Option<V>`][std::option::Option] and
    /// returns the same, so that it can decide to delete a mapping
    /// instead of updating the value, and decide what to do if the
    /// key isn't in the map.
    ///
    /// Time: O(log n)
    ///
    /// [std::option::Option]: https://doc.rust-lang.org/std/option/enum.Option.html
    #[must_use]
    pub fn alter<F>(&self, f: F, k: K) -> Self
    where
        F: FnOnce(Option<V>) -> Option<V>,
        V: Hash,
    {
        let pop = self.extract_with_key(&k);
        match (f(pop.as_ref().map(|(_, v, _)| v.clone())), pop) {
            (None, None) => self.clone(),
            (Some(v), None) => self.update(k, v),
            (None, Some((_, _, m))) => m,
            (Some(v), Some((_, _, m))) => m.update(k, v),
        }
    }

    /// Construct a new map without the given key.
    ///
    /// Construct a map that's a copy of the current map, absent the
    /// mapping for `key` if it's present.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn without<Q>(&self, k: &Q) -> Self
    where
        Q: Hash + Equivalent<K> + ?Sized,
        V: Hash,
    {
        match self.extract_with_key(k) {
            None => self.clone(),
            Some((_, _, map)) => map,
        }
    }

    /// Keep only entries whose keys are in the given set.
    ///
    /// Time: O(n log m) where n = self.len(), m = keys.len()
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// # use pds::hashset::HashSet;
    /// let map = hashmap!{1 => "a", 2 => "b", 3 => "c", 4 => "d"};
    /// let keys = hashset!{2, 4};
    /// let restricted = map.restrict_keys(&keys);
    /// assert_eq!(restricted, hashmap!{2 => "b", 4 => "d"});
    /// ```
    #[must_use]
    pub fn restrict_keys(&self, keys: &GenericHashSet<K, S, P>) -> Self {
        let mut out = self.clone();
        out.retain(|k, _| keys.contains(k));
        out
    }

    /// Remove all entries whose keys are in the given set.
    ///
    /// Time: O(m log n) where m = keys.len(), n = self.len()
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// # use pds::hashset::HashSet;
    /// let map = hashmap!{1 => "a", 2 => "b", 3 => "c", 4 => "d"};
    /// let keys = hashset!{2, 4};
    /// let reduced = map.without_keys(&keys);
    /// assert_eq!(reduced, hashmap!{1 => "a", 3 => "c"});
    /// ```
    #[must_use]
    pub fn without_keys(&self, keys: &GenericHashSet<K, S, P>) -> Self {
        let mut out = self.clone();
        for key in keys.iter() {
            out.remove_invalidate_kv(key);
        }
        out
    }

    /// Merge two maps with different value types using three closures:
    /// one for keys present only in `self`, one for keys in both maps,
    /// and one for keys present only in `other`.
    ///
    /// Each closure returns `Option<V3>` — returning `None` excludes
    /// the key from the result. This subsumes `union_with`,
    /// `intersection_with`, and `symmetric_difference_with` as special
    /// cases.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let left = hashmap!{1 => "a", 2 => "b", 3 => "c"};
    /// let right = hashmap!{2 => 10, 3 => 20, 4 => 30};
    /// let merged: HashMap<i32, String> = left.merge_with(
    ///     &right,
    ///     |_k, v| Some(v.to_string()),           // left only
    ///     |_k, l, r| Some(format!("{l}:{r}")),    // both
    ///     |_k, v| Some(v.to_string()),            // right only
    /// );
    /// assert_eq!(merged.len(), 4);
    /// assert_eq!(merged[&1], "a");
    /// assert_eq!(merged[&2], "b:10");
    /// assert_eq!(merged[&3], "c:20");
    /// assert_eq!(merged[&4], "30");
    /// ```
    #[must_use]
    pub fn merge_with<V2, V3, FL, FB, FR>(
        &self,
        other: &GenericHashMap<K, V2, S, P, H>,
        mut left_only: FL,
        mut both: FB,
        mut right_only: FR,
    ) -> GenericHashMap<K, V3, S, P, H>
    where
        V2: Clone,
        V3: Clone,
        S: Default,
        FL: FnMut(&K, &V) -> Option<V3>,
        FB: FnMut(&K, &V, &V2) -> Option<V3>,
        FR: FnMut(&K, &V2) -> Option<V3>,
    {
        let mut result = GenericHashMap::new();
        // Phase 1: iterate left, dispatch left-only and both
        for (k, v1) in self.iter() {
            match other.get(k) {
                Some(v2) => {
                    if let Some(v3) = both(k, v1, v2) {
                        result.insert_invalidate_kv(k.clone(), v3);
                    }
                }
                None => {
                    if let Some(v3) = left_only(k, v1) {
                        result.insert_invalidate_kv(k.clone(), v3);
                    }
                }
            }
        }
        // Phase 2: iterate right for keys not in left
        for (k, v2) in other.iter() {
            if !self.contains_key(k) {
                if let Some(v3) = right_only(k, v2) {
                    result.insert_invalidate_kv(k.clone(), v3);
                }
            }
        }
        result
    }

    /// Remove a key/value pair from a map, if it exists, and return
    /// the removed value as well as the updated map.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn extract<Q>(&self, k: &Q) -> Option<(V, Self)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
        V: Hash,
    {
        self.extract_with_key(k).map(|(_, v, m)| (v, m))
    }

    /// Remove a key/value pair from a map, if it exists, and return
    /// the removed key and value as well as the updated list.
    ///
    /// Time: O(log n)
    #[must_use]
    pub fn extract_with_key<Q>(&self, k: &Q) -> Option<(K, V, Self)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
        V: Hash,
    {
        let mut out = self.clone();
        out.remove_with_key(k).map(|(k, v)| (k, v, out))
    }

    /// Construct the union of two maps, keeping the values in the
    /// current map when keys exist in both maps.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 3 => 3};
    /// let map2 = hashmap!{2 => 2, 3 => 4};
    /// let expected = hashmap!{1 => 1, 2 => 2, 3 => 3};
    /// assert_eq!(expected, map1.union(map2));
    /// ```
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let (mut to_mutate, to_consume, use_to_consume) = if self.len() >= other.len() {
            (self, other, false)
        } else {
            (other, self, true)
        };
        for (k, v) in to_consume {
            match to_mutate.entry(k) {
                Entry::Occupied(mut e) if use_to_consume => {
                    e.insert(v);
                }
                Entry::Vacant(e) => {
                    e.insert(v);
                }
                _ => {}
            }
        }
        to_mutate
    }

    /// Construct the union of two maps, using a function to decide
    /// what to do with the value when a key is in both maps.
    ///
    /// The function is called when a value exists in both maps, and
    /// receives the value from the current map as its first argument,
    /// and the value from the other map as the second. It should
    /// return the value to be inserted in the resulting map.
    ///
    /// Time: O(n log n)
    #[inline]
    #[must_use]
    pub fn union_with<F>(self, other: Self, mut f: F) -> Self
    where
        F: FnMut(V, V) -> V,
    {
        self.union_with_key(other, |_, v1, v2| f(v1, v2))
    }

    /// Construct the union of two maps, using a function to decide
    /// what to do with the value when a key is in both maps.
    ///
    /// The function is called when a value exists in both maps, and
    /// receives a reference to the key as its first argument, the
    /// value from the current map as the second argument, and the
    /// value from the other map as the third argument. It should
    /// return the value to be inserted in the resulting map.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 3 => 4};
    /// let map2 = hashmap!{2 => 2, 3 => 5};
    /// let expected = hashmap!{1 => 1, 2 => 2, 3 => 9};
    /// assert_eq!(expected, map1.union_with_key(
    ///     map2,
    ///     |key, left, right| left + right
    /// ));
    /// ```
    #[must_use]
    pub fn union_with_key<F>(self, other: Self, mut f: F) -> Self
    where
        F: FnMut(&K, V, V) -> V,
    {
        if self.len() >= other.len() {
            self.union_with_key_inner(other, f)
        } else {
            other.union_with_key_inner(self, |key, other_value, self_value| {
                f(key, self_value, other_value)
            })
        }
    }

    fn union_with_key_inner<F>(mut self, other: Self, mut f: F) -> Self
    where
        F: FnMut(&K, V, V) -> V,
    {
        for (key, right_value) in other {
            match self.remove_invalidate_kv(&key) {
                None => {
                    self.insert_invalidate_kv(key, right_value);
                }
                Some((_, left_value)) => {
                    let final_value = f(&key, left_value, right_value);
                    self.insert_invalidate_kv(key, final_value);
                }
            }
        }
        self
    }

    /// Construct the union of a sequence of maps, selecting the value
    /// of the leftmost when a key appears in more than one map.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 3 => 3};
    /// let map2 = hashmap!{2 => 2};
    /// let expected = hashmap!{1 => 1, 2 => 2, 3 => 3};
    /// assert_eq!(expected, HashMap::unions(vec![map1, map2]));
    /// ```
    #[must_use]
    pub fn unions<I>(i: I) -> Self
    where
        S: Default,
        I: IntoIterator<Item = Self>,
    {
        i.into_iter().fold(Self::default(), Self::union)
    }

    /// Construct the union of a sequence of maps, using a function to
    /// decide what to do with the value when a key is in more than
    /// one map.
    ///
    /// The function is called when a value exists in multiple maps,
    /// and receives the value from the current map as its first
    /// argument, and the value from the next map as the second. It
    /// should return the value to be inserted in the resulting map.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn unions_with<I, F>(i: I, f: F) -> Self
    where
        S: Default,
        I: IntoIterator<Item = Self>,
        F: Fn(V, V) -> V,
    {
        i.into_iter()
            .fold(Self::default(), |a, b| a.union_with(b, &f))
    }

    /// Construct the union of a sequence of maps, using a function to
    /// decide what to do with the value when a key is in more than
    /// one map.
    ///
    /// The function is called when a value exists in multiple maps,
    /// and receives a reference to the key as its first argument, the
    /// value from the current map as the second argument, and the
    /// value from the next map as the third argument. It should
    /// return the value to be inserted in the resulting map.
    ///
    /// Time: O(n log n)
    #[must_use]
    pub fn unions_with_key<I, F>(i: I, f: F) -> Self
    where
        S: Default,
        I: IntoIterator<Item = Self>,
        F: Fn(&K, V, V) -> V,
    {
        i.into_iter()
            .fold(Self::default(), |a, b| a.union_with_key(b, &f))
    }

    /// Construct the symmetric difference between two maps by discarding keys
    /// which occur in both maps.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 3 => 4};
    /// let map2 = hashmap!{2 => 2, 3 => 5};
    /// let expected = hashmap!{1 => 1, 2 => 2};
    /// assert_eq!(expected, map1.symmetric_difference(map2));
    /// ```
    #[inline]
    #[must_use]
    pub fn symmetric_difference(self, other: Self) -> Self {
        self.symmetric_difference_with_key(other, |_, _, _| None)
    }

    /// Construct the symmetric difference between two maps by using a function
    /// to decide what to do if a key occurs in both.
    ///
    /// Time: O(n log n)
    #[inline]
    #[must_use]
    pub fn symmetric_difference_with<F>(self, other: Self, mut f: F) -> Self
    where
        F: FnMut(V, V) -> Option<V>,
    {
        self.symmetric_difference_with_key(other, |_, a, b| f(a, b))
    }

    /// Construct the symmetric difference between two maps by using a function
    /// to decide what to do if a key occurs in both. The function
    /// receives the key as well as both values.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 3 => 4};
    /// let map2 = hashmap!{2 => 2, 3 => 5};
    /// let expected = hashmap!{1 => 1, 2 => 2, 3 => 9};
    /// assert_eq!(expected, map1.symmetric_difference_with_key(
    ///     map2,
    ///     |key, left, right| Some(left + right)
    /// ));
    /// ```
    #[must_use]
    pub fn symmetric_difference_with_key<F>(mut self, other: Self, mut f: F) -> Self
    where
        F: FnMut(&K, V, V) -> Option<V>,
    {
        let mut out = self.new_from();
        for (key, right_value) in other {
            match self.remove_invalidate_kv(&key) {
                None => {
                    out.insert_invalidate_kv(key, right_value);
                }
                Some((_, left_value)) => {
                    if let Some(final_value) = f(&key, left_value, right_value) {
                        out.insert_invalidate_kv(key, final_value);
                    }
                }
            }
        }
        out.union(self)
    }

    /// Construct the relative complement between two maps by discarding keys
    /// which occur in `other`.
    ///
    /// Time: O(m log n) where m is the size of the other map
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 3 => 4};
    /// let map2 = hashmap!{2 => 2, 3 => 5};
    /// let expected = hashmap!{1 => 1};
    /// assert_eq!(expected, map1.difference(map2));
    /// ```
    #[inline]
    #[must_use]
    pub fn difference(mut self, other: Self) -> Self {
        for (key, _) in other {
            let _ = self.remove_invalidate_kv(&key);
        }
        self
    }

    /// Construct the intersection of two maps, keeping the values
    /// from the current map.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 2 => 2};
    /// let map2 = hashmap!{2 => 3, 3 => 4};
    /// let expected = hashmap!{2 => 2};
    /// assert_eq!(expected, map1.intersection(map2));
    /// ```
    #[inline]
    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        self.intersection_with_key(other, |_, v, _| v)
    }

    /// Construct the intersection of two maps, calling a function
    /// with both values for each key and using the result as the
    /// value for the key.
    ///
    /// Time: O(n log n)
    #[inline]
    #[must_use]
    pub fn intersection_with<B, C, F>(
        self,
        other: GenericHashMap<K, B, S, P, H>,
        mut f: F,
    ) -> GenericHashMap<K, C, S, P, H>
    where
        B: Clone,
        C: Clone,
        F: FnMut(V, B) -> C,
    {
        self.intersection_with_key(other, |_, v1, v2| f(v1, v2))
    }

    /// Construct the intersection of two maps, calling a function
    /// with the key and both values for each key and using the result
    /// as the value for the key.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map1 = hashmap!{1 => 1, 2 => 2};
    /// let map2 = hashmap!{2 => 3, 3 => 4};
    /// let expected = hashmap!{2 => 5};
    /// assert_eq!(expected, map1.intersection_with_key(
    ///     map2,
    ///     |key, left, right| left + right
    /// ));
    /// ```
    #[must_use]
    pub fn intersection_with_key<B, C, F>(
        mut self,
        other: GenericHashMap<K, B, S, P, H>,
        mut f: F,
    ) -> GenericHashMap<K, C, S, P, H>
    where
        B: Clone,
        C: Clone,
        F: FnMut(&K, V, B) -> C,
    {
        let mut out = self.new_from();
        for (key, right_value) in other {
            match self.remove_invalidate_kv(&key) {
                None => (),
                Some((_, left_value)) => {
                    let result = f(&key, left_value, right_value);
                    out.insert_invalidate_kv(key, result);
                }
            }
        }
        out
    }
}

// Methods that need K: Hash + Eq but not K: Clone, V: Clone, or S: Clone
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Check whether two maps share no keys.
    ///
    /// Time: O(n) — iterates the smaller map and checks each key
    /// against the larger map.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let a = hashmap!{1 => "a", 2 => "b"};
    /// let b = hashmap!{3 => "c", 4 => "d"};
    /// let c = hashmap!{2 => "x", 5 => "e"};
    /// assert!(a.disjoint(&b));
    /// assert!(!a.disjoint(&c));
    /// ```
    #[must_use]
    pub fn disjoint(&self, other: &Self) -> bool {
        let (smaller, larger) = if self.len() <= other.len() {
            (self, other)
        } else {
            (other, self)
        };
        smaller.keys().all(|k| !larger.contains_key(k))
    }
}

// Methods that need K: Clone but not V: Clone
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Construct a new map with the same keys but values transformed
    /// by the given function.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => 10, 2 => 20, 3 => 30};
    /// let doubled = map.map_values(|v| v * 2);
    /// assert_eq!(doubled, hashmap!{1 => 20, 2 => 40, 3 => 60});
    /// ```
    #[must_use]
    pub fn map_values<V2, F>(&self, mut f: F) -> GenericHashMap<K, V2, S, P, H>
    where
        V2: Clone,
        S: Default,
        F: FnMut(&V) -> V2,
    {
        let mut result = GenericHashMap::new();
        for (k, v) in self.iter() {
            result.insert_invalidate_kv(k.clone(), f(v));
        }
        result
    }

    /// Construct a new map with the same keys but values transformed
    /// by the given function, which also receives the key.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => 10, 2 => 20, 3 => 30};
    /// let sums = map.map_values_with_key(|k, v| k + v);
    /// assert_eq!(sums, hashmap!{1 => 11, 2 => 22, 3 => 33});
    /// ```
    #[must_use]
    pub fn map_values_with_key<V2, F>(&self, mut f: F) -> GenericHashMap<K, V2, S, P, H>
    where
        V2: Clone,
        S: Default,
        F: FnMut(&K, &V) -> V2,
    {
        let mut result = GenericHashMap::new();
        for (k, v) in self.iter() {
            result.insert_invalidate_kv(k.clone(), f(k, v));
        }
        result
    }

    /// Construct a new map with the same keys but values transformed
    /// by a fallible function. Returns the first error encountered.
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => "10", 2 => "20", 3 => "30"};
    /// let parsed: Result<HashMap<i32, i32>, _> =
    ///     map.try_map_values(|_, v| v.parse::<i32>());
    /// assert_eq!(parsed, Ok(hashmap!{1 => 10, 2 => 20, 3 => 30}));
    /// ```
    pub fn try_map_values<V2, E, F>(&self, mut f: F) -> Result<GenericHashMap<K, V2, S, P, H>, E>
    where
        V2: Clone,
        S: Default,
        F: FnMut(&K, &V) -> Result<V2, E>,
    {
        let mut out = GenericHashMap::default();
        for (k, v) in self.iter() {
            out.insert_invalidate_kv(k.clone(), f(k, v)?);
        }
        Ok(out)
    }

    /// Thread an accumulator through a traversal, producing a new
    /// map with transformed values.
    ///
    /// Note: HashMap iteration order is not guaranteed, so the
    /// accumulator sees entries in an arbitrary order.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => 10, 2 => 20};
    /// let (total, doubled): (i32, HashMap<i32, i32>) =
    ///     map.map_accum(0, |acc, _k, v| (acc + v, v * 2));
    /// assert_eq!(total, 30);
    /// assert_eq!(doubled[&1], 20);
    /// assert_eq!(doubled[&2], 40);
    /// ```
    #[must_use]
    pub fn map_accum<St, V2, F>(
        &self,
        init: St,
        mut f: F,
    ) -> (St, GenericHashMap<K, V2, S, P, H>)
    where
        V2: Clone,
        S: Default,
        F: FnMut(St, &K, &V) -> (St, V2),
    {
        let mut acc = init;
        let mut result = GenericHashMap::new();
        for (k, v) in self.iter() {
            let (new_acc, v2) = f(acc, k, v);
            acc = new_acc;
            result.insert_invalidate_kv(k.clone(), v2);
        }
        (acc, result)
    }
}

// Methods that need V: Clone but not K: Clone
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Construct a new map with keys transformed by the given
    /// function, keeping the values. If the function maps two
    /// different keys to the same new key, one entry is kept
    /// (unspecified which).
    ///
    /// Time: O(n log n)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate pds;
    /// # use pds::hashmap::HashMap;
    /// let map = hashmap!{1 => "a", 2 => "b", 3 => "c"};
    /// let negated = map.map_keys(|k| -k);
    /// assert_eq!(negated.len(), 3);
    /// assert_eq!(negated.get(&-1), Some(&"a"));
    /// ```
    #[must_use]
    pub fn map_keys<K2, F>(&self, mut f: F) -> GenericHashMap<K2, V, S, P, H>
    where
        K2: Hash + Eq + Clone,
        S: Default,
        F: FnMut(&K) -> K2,
    {
        let mut result = GenericHashMap::new();
        for (k, v) in self.iter() {
            result.insert_invalidate_kv(f(k), v.clone());
        }
        result
    }
}

// Entries

/// A handle for a key and its associated value.
///
/// ## Performance Note
///
/// When using an `Entry`, the key is only ever hashed once, when you
/// create the `Entry`. Operations on an `Entry` will never trigger a
/// rehash, where eg. a `contains_key(key)` followed by an
/// `insert(key, default_value)` (the equivalent of
/// `Entry::or_insert()`) would need to hash the key once for the
/// `contains_key` and again for the `insert`. The operations
/// generally perform similarly otherwise.
pub enum Entry<'a, K, V, S, P, H = u64>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
    H: HashWidth,
{
    /// An entry which exists in the map.
    Occupied(OccupiedEntry<'a, K, V, S, P, H>),
    /// An entry which doesn't exist in the map.
    Vacant(VacantEntry<'a, K, V, S, P, H>),
}

impl<'a, K, V, S, P, H: HashWidth> Entry<'a, K, V, S, P, H>
where
    K: 'a + Hash + Eq + Clone,
    V: 'a + Clone,
    S: 'a + BuildHasher,
    P: SharedPointerKind,
{
    /// Insert the default value provided if there was no value
    /// already, and return a mutable reference to the value.
    pub fn or_insert(self, default: V) -> &'a mut V {
        self.or_insert_with(|| default)
    }

    /// Insert the default value from the provided function if there
    /// was no value already, and return a mutable reference to the
    /// value.
    pub fn or_insert_with<F>(self, default: F) -> &'a mut V
    where
        F: FnOnce() -> V,
    {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default()),
        }
    }

    /// Insert a default value if there was no value already, and
    /// return a mutable reference to the value.
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        // `or_default()` is implemented via `or_insert_with(Default::default)`
        // rather than by calling `or_default()` recursively (which would stack-
        // overflow). Clippy's `unwrap_or_default` lint fires here because it
        // sees `or_insert_with(Default::default)` and suggests `.or_default()`.
        #[allow(clippy::unwrap_or_default)]
        self.or_insert_with(Default::default)
    }

    /// Get the key for this entry.
    #[must_use]
    pub fn key(&self) -> &K {
        match self {
            Entry::Occupied(entry) => entry.key(),
            Entry::Vacant(entry) => entry.key(),
        }
    }

    /// Call the provided function to modify the value if the value
    /// exists.
    pub fn and_modify<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut V),
    {
        match &mut self {
            Entry::Occupied(ref mut entry) => f(entry.get_mut()),
            Entry::Vacant(_) => (),
        }
        self
    }
}

/// An entry for a mapping that already exists in the map.
pub struct OccupiedEntry<'a, K, V, S, P, H = u64>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
    H: HashWidth,
{
    map: &'a mut GenericHashMap<K, V, S, P, H>,
    hash: H,
    key: K,
}

impl<'a, K, V, S, P, H: HashWidth> OccupiedEntry<'a, K, V, S, P, H>
where
    K: 'a + Hash + Eq + Clone,
    V: 'a + Clone,
    S: 'a + BuildHasher,
    P: SharedPointerKind,
{
    /// Get the key for this entry.
    #[must_use]
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Remove this entry from the map and return the removed mapping.
    ///
    /// Note: invalidates the key-value Merkle hash.
    pub fn remove_entry(self) -> (K, V) {
        self.map.kv_merkle_valid = false;
        // unwrap: occupied entries can only be created for non-empty maps
        let root = SharedPointer::make_mut(self.map.root.as_mut().unwrap());
        let result = root.remove(self.hash, 0, &self.key);
        self.map.size -= 1;
        result.unwrap()
    }

    /// Get the current value.
    #[must_use]
    pub fn get(&self) -> &V {
        // unwrap: occupied entries can only be created for non-empty maps
        &self
            .map
            .root
            .as_ref()
            .unwrap()
            .get(self.hash, 0, &self.key)
            .unwrap()
            .1
    }

    /// Get a mutable reference to the current value.
    ///
    /// Note: invalidates the key-value Merkle hash.
    #[must_use]
    pub fn get_mut(&mut self) -> &mut V {
        self.map.kv_merkle_valid = false;
        // unwrap: occupied entries can only be created for non-empty maps
        let root = SharedPointer::make_mut(self.map.root.as_mut().unwrap());
        &mut root.get_mut(self.hash, 0, &self.key).unwrap().1
    }

    /// Convert this entry into a mutable reference.
    ///
    /// Note: invalidates the key-value Merkle hash.
    #[must_use]
    pub fn into_mut(self) -> &'a mut V {
        self.map.kv_merkle_valid = false;
        // unwrap: occupied entries can only be created for non-empty maps
        let root = SharedPointer::make_mut(self.map.root.as_mut().unwrap());
        &mut root.get_mut(self.hash, 0, &self.key).unwrap().1
    }

    /// Overwrite the current value.
    ///
    /// Note: invalidates the key-value Merkle hash.
    pub fn insert(&mut self, value: V) -> V {
        mem::replace(self.get_mut(), value)
    }

    /// Remove this entry from the map and return the removed value.
    pub fn remove(self) -> V {
        self.remove_entry().1
    }
}

/// An entry for a mapping that does not already exist in the map.
pub struct VacantEntry<'a, K, V, S, P, H = u64>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
    H: HashWidth,
{
    map: &'a mut GenericHashMap<K, V, S, P, H>,
    hash: H,
    key: K,
}

impl<'a, K, V, S, P, H: HashWidth> VacantEntry<'a, K, V, S, P, H>
where
    K: 'a + Hash + Eq + Clone,
    V: 'a + Clone,
    S: 'a + BuildHasher,
    P: SharedPointerKind,
{
    /// Get the key for this entry.
    #[must_use]
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Convert this entry into its key.
    #[must_use]
    pub fn into_key(self) -> K {
        self.key
    }

    /// Insert a value into this entry.
    ///
    /// Note: invalidates the key-value Merkle hash.
    pub fn insert(self, value: V) -> &'a mut V {
        self.map.kv_merkle_valid = false;
        let root =
            SharedPointer::make_mut(self.map.root.get_or_insert_with(SharedPointer::default));
        if root
            .insert(self.hash, 0, (self.key.clone(), value))
            .is_none()
        {
            self.map.size += 1;
        }
        // TODO it's unfortunate that we need to look up the key again
        // here to get the mut ref.
        &mut root.get_mut(self.hash, 0, &self.key).unwrap().1
    }
}

// Core traits

impl<K, V, S, P, H: HashWidth> Clone for GenericHashMap<K, V, S, P, H>
where
    S: Clone,
    P: SharedPointerKind,
{
    /// Clone a map.
    ///
    /// Time: O(1), plus a cheap hasher clone.
    #[inline]
    fn clone(&self) -> Self {
        GenericHashMap {
            root: self.root.clone(),
            size: self.size,
            hasher: self.hasher.clone(),
            hasher_id: self.hasher_id,
            kv_merkle_hash: self.kv_merkle_hash,
            kv_merkle_valid: self.kv_merkle_valid,
        }
    }
}

#[cfg(feature = "hash-intern")]
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Clone + PartialEq,
    V: Clone + PartialEq,
    P: SharedPointerKind,
{
    /// Intern the internal HAMT nodes of this map into the given pool.
    ///
    /// Nodes with identical content (same Merkle hash + structural
    /// equality) are deduplicated: after interning, shared subtrees
    /// across different maps point to the same allocation. This reduces
    /// memory and enables O(1) `ptr_eq` checks during diff and equality.
    ///
    /// Interning is bottom-up: children are interned before parents, so
    /// parent equality checks can use `ptr_eq` on already-interned
    /// children.
    ///
    /// Mutation after interning works normally — `make_mut` clones the
    /// shared node (standard COW semantics).
    ///
    /// ## Performance tip — diff across independently-constructed maps
    ///
    /// After interning two maps built from the same data (even if
    /// constructed independently), content-equal subtrees share the same
    /// allocation. Subsequent `diff` calls reduce from O(n + m) to
    /// O(changes × depth) because shared subtrees are skipped in O(1)
    /// via pointer comparison.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "hash-intern")]
    /// # {
    /// use pds::HashMap;
    /// use pds::intern::InternPool;
    ///
    /// let mut pool = InternPool::new();
    /// let mut map: HashMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
    /// map.intern(&mut pool);
    /// # }
    /// ```
    pub fn intern(&mut self, pool: &mut crate::intern::InternPool<(K, V), P, H>) {
        if let Some(root) = &mut self.root {
            let node = SharedPointer::make_mut(root);
            for entry in node.data.iter_mut() {
                entry.intern(pool);
            }
            *root = pool.intern_hamt(root.clone());
        }
    }
}

#[cfg(feature = "hash-intern")]
impl<K, V, S, P, H: HashWidth> GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Hash + Clone + PartialEq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    /// Intern the HAMT nodes and seal the kv_merkle hash in one pass.
    ///
    /// Equivalent to calling [`intern`][Self::intern] followed by
    /// [`recompute_kv_merkle`][Self::recompute_kv_merkle]. After this
    /// call the map has both deduplicated nodes and a valid kv_merkle,
    /// enabling all three fast-paths in `diff` and `PartialEq`:
    ///
    /// 1. O(1) `ptr_eq` for maps sharing structure
    /// 2. O(1) kv_merkle equality check for same-lineage maps
    /// 3. O(1) subtree skipping via pointer comparison in the tree walk
    ///
    /// Use this instead of `intern()` when the map was constructed via
    /// bulk insertion (`from_iter`, repeated `insert_bulk`) that leaves
    /// `kv_merkle` invalid. Calling `intern()` alone after bulk
    /// construction deduplicates nodes but leaves the O(1) positive
    /// equality fast-path disabled.
    ///
    /// **Hasher-lineage requirement.** Fast-paths 1 and 2 require maps to
    /// share the same hasher lineage (same `hasher_id`). Maps cloned from a
    /// common ancestor share a lineage automatically. Two maps constructed
    /// independently via `new()` or `collect()` have different `hasher_id`
    /// values and will not benefit from those fast-paths even after sealing.
    ///
    /// Time: O(n)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "hash-intern")]
    /// # {
    /// use pds::HashMap;
    /// use pds::intern::InternPool;
    ///
    /// let mut pool = InternPool::new();
    ///
    /// // Both maps cloned from the same base — same hasher lineage.
    /// let base: HashMap<i32, i32> = (0..1000).map(|i| (i, i)).collect();
    /// let mut map1 = base.clone();
    /// let mut map2 = base.clone();
    ///
    /// // Apply the same bulk mutations independently (invalidates kv_merkle).
    /// for i in 1000..1100 { map1.insert(i, i); }
    /// for i in 1000..1100 { map2.insert(i, i); }
    ///
    /// map1.intern_and_seal(&mut pool);
    /// map2.intern_and_seal(&mut pool);
    ///
    /// // kv_merkle is valid and both maps share a lineage — O(1) fast-paths
    /// // fire for equality and diff.
    /// assert!(map1.kv_merkle_valid());
    /// assert_eq!(map1, map2);
    /// assert_eq!(map1.diff(&map2).count(), 0);
    /// # }
    /// ```
    pub fn intern_and_seal(&mut self, pool: &mut crate::intern::InternPool<(K, V), P, H>) {
        self.intern(pool);
        self.recompute_kv_merkle();
    }
}

impl<K, V, S1, S2, P1, P2, H: HashWidth> PartialEq<GenericHashMap<K, V, S2, P2, H>> for GenericHashMap<K, V, S1, P1, H>
where
    K: Hash + Eq,
    V: PartialEq,
    S1: BuildHasher,
    S2: BuildHasher,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn eq(&self, other: &GenericHashMap<K, V, S2, P2, H>) -> bool {
        self.test_eq(other)
    }
}

impl<K, V, S, P, H: HashWidth> Eq for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq,
    V: Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
}

impl<K, V, S, P, H: HashWidth> Hash for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq,
    V: Hash,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn hash<HR: Hasher>(&self, state: &mut HR) {
        self.len().hash(state);
        // Order-independent: wrapping_add of per-entry hashes.
        let mut combined: u64 = 0;
        for (k, v) in self.iter() {
            let mut h = crate::util::FnvHasher::new();
            k.hash(&mut h);
            v.hash(&mut h);
            combined = combined.wrapping_add(h.finish());
        }
        combined.hash(state);
    }
}

impl<K, V, S, P, H: HashWidth> Default for GenericHashMap<K, V, S, P, H>
where
    S: Default,
    P: SharedPointerKind,
{
    #[inline]
    fn default() -> Self {
        GenericHashMap {
            size: 0,
            root: None,
            hasher: S::default(),
            hasher_id: next_hasher_id(),
            kv_merkle_hash: 0,
            kv_merkle_valid: true,
        }
    }
}

impl<K, V, S, RK, RV, P, H: HashWidth> Extend<(RK, RV)> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone + From<RK>,
    V: Clone + Hash + From<RV>,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (RK, RV)>,
    {
        for (key, value) in iter {
            self.insert(From::from(key), From::from(value));
        }
    }
}

impl<Q, K, V, S, P, H: HashWidth> Index<&Q> for GenericHashMap<K, V, S, P, H>
where
    Q: Hash + Equivalent<K> + ?Sized,
    K: Hash + Eq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Output = V;

    fn index(&self, key: &Q) -> &Self::Output {
        match self.get(key) {
            None => panic!("HashMap::index: invalid key"),
            Some(value) => value,
        }
    }
}

impl<Q, K, V, S, P, H: HashWidth> IndexMut<&Q> for GenericHashMap<K, V, S, P, H>
where
    Q: Hash + Equivalent<K> + ?Sized,
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    fn index_mut(&mut self, key: &Q) -> &mut Self::Output {
        match self.get_mut(key) {
            None => panic!("HashMap::index_mut: invalid key"),
            Some(value) => value,
        }
    }
}

impl<K, V, S, P, H: HashWidth> Debug for GenericHashMap<K, V, S, P, H>
where
    K: Debug,
    V: Debug,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_map();
        for (k, v) in self {
            d.entry(k, v);
        }
        d.finish()
    }
}

// // Iterators

/// An iterator over the elements of a map.
pub struct Iter<'a, K, V, P: SharedPointerKind, H: HashWidth = u64> {
    it: NodeIter<'a, (K, V), P, H>,
}

// We impl Clone instead of deriving it, because we want Clone even if K and V aren't.
impl<'a, K, V, P: SharedPointerKind, H: HashWidth> Clone for Iter<'a, K, V, P, H> {
    fn clone(&self) -> Self {
        Iter {
            it: self.it.clone(),
        }
    }
}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> Iterator for Iter<'a, K, V, P, H> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|((k, v), _)| (k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for Iter<'a, K, V, P, H> {}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> FusedIterator for Iter<'a, K, V, P, H> {}

/// A mutable iterator over the elements of a map.
pub struct IterMut<'a, K, V, P, H = u64>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
    H: HashWidth,
{
    it: NodeIterMut<'a, (K, V), P, H>,
}

impl<'a, K, V, P, H: HashWidth> Iterator for IterMut<'a, K, V, P, H>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|((k, v), _)| (&*k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, K, V, P, H: HashWidth> ExactSizeIterator for IterMut<'a, K, V, P, H>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
}

impl<'a, K, V, P, H: HashWidth> FusedIterator for IterMut<'a, K, V, P, H>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
}

/// A consuming iterator over the elements of a map.
pub struct ConsumingIter<A: HashValue, P: SharedPointerKind, H: HashWidth = u64> {
    it: NodeDrain<A, P, H>,
}

impl<A, P: SharedPointerKind, H: HashWidth> Iterator for ConsumingIter<A, P, H>
where
    A: HashValue + Clone,
{
    type Item = A;

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|(p, _)| p)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<A, P, H: HashWidth> ExactSizeIterator for ConsumingIter<A, P, H>
where
    A: HashValue + Clone,
    P: SharedPointerKind,
{
}

impl<A, P, H: HashWidth> FusedIterator for ConsumingIter<A, P, H>
where
    A: HashValue + Clone,
    P: SharedPointerKind,
{
}

/// An iterator over the keys of a map.
pub struct Keys<'a, K, V, P: SharedPointerKind, H: HashWidth = u64> {
    it: NodeIter<'a, (K, V), P, H>,
}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> Iterator for Keys<'a, K, V, P, H> {
    type Item = &'a K;

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|((k, _), _)| k)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for Keys<'a, K, V, P, H> {}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> FusedIterator for Keys<'a, K, V, P, H> {}

/// An iterator over the values of a map.
pub struct Values<'a, K, V, P: SharedPointerKind, H: HashWidth = u64> {
    it: NodeIter<'a, (K, V), P, H>,
}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> Iterator for Values<'a, K, V, P, H> {
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().map(|((_, v), _)| v)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for Values<'a, K, V, P, H> {}

impl<'a, K, V, P: SharedPointerKind, H: HashWidth> FusedIterator for Values<'a, K, V, P, H> {}

impl<'a, K, V, S, P: SharedPointerKind, H: HashWidth> IntoIterator for &'a GenericHashMap<K, V, S, P, H> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V, P, H>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V, S, P, H: HashWidth> IntoIterator for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = (K, V);
    type IntoIter = ConsumingIter<(K, V), P, H>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        ConsumingIter {
            it: NodeDrain::new(self.root, self.size),
        }
    }
}

// Conversions

impl<K, V, S, P, H: HashWidth> FromIterator<(K, V)> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from_iter<T>(i: T) -> Self
    where
        T: IntoIterator<Item = (K, V)>,
    {
        let mut map = Self::default();
        for (k, v) in i {
            map.insert(k, v);
        }
        map
    }
}

impl<K, V, S, P: SharedPointerKind, H: HashWidth> AsRef<GenericHashMap<K, V, S, P, H>>
    for GenericHashMap<K, V, S, P, H>
{
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<K, V, OK, OV, SA, SB, P1, P2, H: HashWidth> From<&GenericHashMap<&K, &V, SA, P1, H>>
    for GenericHashMap<OK, OV, SB, P2, H>
where
    K: Hash + Equivalent<OK> + ToOwned<Owned = OK> + ?Sized,
    V: ToOwned<Owned = OV> + ?Sized,
    OK: Hash + Eq + Clone,
    OV: Borrow<V> + Clone + Hash,
    SA: BuildHasher + Clone,
    SB: BuildHasher + Default + Clone,
    P1: SharedPointerKind,
    P2: SharedPointerKind,
{
    fn from(m: &GenericHashMap<&K, &V, SA, P1, H>) -> Self {
        m.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a [(K, V)]> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(m: &'a [(K, V)]) -> Self {
        m.iter().cloned().collect()
    }
}

impl<K, V, S, P, H: HashWidth> From<Vec<(K, V)>> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(m: Vec<(K, V)>) -> Self {
        m.into_iter().collect()
    }
}

impl<K, V, S, const N: usize, P, H: HashWidth> From<[(K, V); N]> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(arr: [(K, V); N]) -> Self {
        IntoIterator::into_iter(arr).collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a Vec<(K, V)>> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(m: &'a Vec<(K, V)>) -> Self {
        m.iter().cloned().collect()
    }
}

#[cfg(feature = "std")]
impl<K, V, S1, S2, P, H: HashWidth> From<std::collections::HashMap<K, V, S2>> for GenericHashMap<K, V, S1, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S1: BuildHasher + Default + Clone,
    S2: BuildHasher,
    P: SharedPointerKind,
{
    fn from(m: std::collections::HashMap<K, V, S2>) -> Self {
        m.into_iter().collect()
    }
}

#[cfg(feature = "std")]
impl<'a, K, V, S1, S2, P, H: HashWidth> From<&'a std::collections::HashMap<K, V, S2>> for GenericHashMap<K, V, S1, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S1: BuildHasher + Default + Clone,
    S2: BuildHasher,
    P: SharedPointerKind,
{
    fn from(m: &'a std::collections::HashMap<K, V, S2>) -> Self {
        m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

impl<K, V, S, P, H: HashWidth> From<BTreeMap<K, V>> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(m: BTreeMap<K, V>) -> Self {
        m.into_iter().collect()
    }
}

impl<'a, K, V, S, P, H: HashWidth> From<&'a BTreeMap<K, V>> for GenericHashMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone,
    V: Clone + Hash,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn from(m: &'a BTreeMap<K, V>) -> Self {
        m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

// impl<K: Ord + Hash + Eq, V, S> From<OrdMap<K, V>> for HashMap<K, V, S>
// where
//     S: BuildHasher + Default,
// {
//     fn from(m: OrdMap<K, V>) -> Self {
//         m.into_iter().collect()
//     }
// }

// impl<'a, K: Ord + Hash + Eq, V, S> From<&'a OrdMap<K, V>> for HashMap<K, V, S>
// where
//     S: BuildHasher + Default,
// {
//     fn from(m: &'a OrdMap<K, V>) -> Self {
//         m.into_iter().collect()
//     }
// }

// Diff

/// An item in a diff between two hash maps.
///
/// Produced by [`GenericHashMap::diff`].
#[derive(Debug, PartialEq, Eq)]
pub enum DiffItem<'a, 'b, K, V> {
    /// This key-value pair was added (present in new map only).
    Add(&'b K, &'b V),
    /// This key's value changed between the two maps.
    Update {
        /// The old key-value pair.
        old: (&'a K, &'a V),
        /// The new key-value pair.
        new: (&'b K, &'b V),
    },
    /// This key-value pair was removed (present in old map only).
    Remove(&'a K, &'a V),
}

// Manual Clone/Copy — DiffItem contains only references, so it is always
// Copy regardless of whether K and V are Clone.
impl<K, V> Clone for DiffItem<'_, '_, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, V> Copy for DiffItem<'_, '_, K, V> {}

/// Check whether two BuildHasher instances produce the same hash output.
///
/// Maps derived from a common ancestor share their hasher state.
/// Independently-constructed maps (e.g. two `HashMap::new()` calls) have
/// different `RandomState` seeds. The tree-walk diff requires identical
/// hash function output — this probe detects incompatible hashers in O(1).
fn hashers_compatible<S: BuildHasher>(a: &S, b: &S) -> bool {
    use core::hash::Hasher;
    let mut ha = a.build_hasher();
    ha.write_u64(0x517c_c1b7_2722_0a95);
    let mut hb = b.build_hasher();
    hb.write_u64(0x517c_c1b7_2722_0a95);
    ha.finish() == hb.finish()
}

/// Fallback diff using iterate-and-lookup for maps with incompatible hashers.
///
/// O(n + m) — iterates both maps and probes the other map for each key.
fn diff_iterate_and_lookup<'a, 'b, K, V, S, P, H: HashWidth>(
    old_map: &'a GenericHashMap<K, V, S, P, H>,
    new_map: &'b GenericHashMap<K, V, S, P, H>,
    diffs: &mut Vec<DiffItem<'a, 'b, K, V>>,
) where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    // Phase 0: iterate old map, find Remove/Update items.
    for (k, v) in old_map.iter() {
        match new_map.get_key_value(k) {
            None => diffs.push(DiffItem::Remove(k, v)),
            Some((k2, v2)) => {
                if v != v2 {
                    diffs.push(DiffItem::Update {
                        old: (k, v),
                        new: (k2, v2),
                    });
                }
            }
        }
    }
    // Phase 1: iterate new map, find Add items.
    for (k, v) in new_map.iter() {
        if !old_map.contains_key(k) {
            diffs.push(DiffItem::Add(k, v));
        }
    }
}

/// An iterator over the differences between two hash maps.
///
/// Created by [`GenericHashMap::diff`].
///
/// Uses a simultaneous HAMT tree walk with pointer-based subtree
/// skipping. When two maps share structure (one was derived from the
/// other via insert/remove), shared subtrees are detected via
/// `SharedPointer::ptr_eq` and skipped in O(1) — reducing diff
/// complexity from O(n + m) to O(changes × tree_depth) for the
/// common case.
pub struct DiffIter<'a, 'b, K, V, S, P: SharedPointerKind, H: HashWidth = u64> {
    diffs: Vec<DiffItem<'a, 'b, K, V>>,
    index: usize,
    _phantom: core::marker::PhantomData<fn(&S, &P, &H)>,
}

/// Walk two HAMT nodes simultaneously, collecting diffs.
///
/// For each bitmap position, compares the entries in both nodes:
/// - Same pointer (ptr_eq) → skip the entire subtree
/// - Both HamtNode → recurse
/// - Both Value → compare keys/values directly
/// - Mixed or leaf node types → collect values from both, compare by key
/// - Present in one only → emit all values as Add or Remove
fn diff_hamt_nodes<'a, 'b, K, V, P, H: HashWidth>(
    old_node: &'a Node<(K, V), P, H>,
    new_node: &'b Node<(K, V), P, H>,
    diffs: &mut Vec<DiffItem<'a, 'b, K, V>>,
) where
    K: Eq,
    V: PartialEq,
    P: SharedPointerKind,
{
    for i in 0..HASH_WIDTH {
        match (old_node.data.get(i), new_node.data.get(i)) {
            (None, None) => {}
            (Some(old_entry), None) => {
                // Everything in old subtree was removed.
                let mut vals = Vec::new();
                old_entry.collect_values(&mut vals);
                for kv in vals {
                    diffs.push(DiffItem::Remove(&kv.0, &kv.1));
                }
            }
            (None, Some(new_entry)) => {
                // Everything in new subtree was added.
                let mut vals = Vec::new();
                new_entry.collect_values(&mut vals);
                for kv in vals {
                    diffs.push(DiffItem::Add(&kv.0, &kv.1));
                }
            }
            (Some(old_entry), Some(new_entry)) => {
                // Both have entries — check pointer equality first.
                if old_entry.ptr_eq(new_entry) {
                    continue;
                }
                diff_entries(old_entry, new_entry, diffs);
            }
        }
    }
}

/// Compare two HAMT entries that are not pointer-equal.
fn diff_entries<'a, 'b, K, V, P, H: HashWidth>(
    old_entry: &'a NodeEntry<(K, V), P, H>,
    new_entry: &'b NodeEntry<(K, V), P, H>,
    diffs: &mut Vec<DiffItem<'a, 'b, K, V>>,
) where
    K: Eq,
    V: PartialEq,
    P: SharedPointerKind,
{
    match (old_entry, new_entry) {
        // Both HamtNodes — recurse into the bitmap.
        (NodeEntry::HamtNode(old_node), NodeEntry::HamtNode(new_node)) => {
            diff_hamt_nodes(old_node, new_node, diffs);
        }
        // Both values — compare directly.
        (NodeEntry::Value(old_kv, _), NodeEntry::Value(new_kv, _)) => {
            if old_kv.0 == new_kv.0 {
                if old_kv.1 != new_kv.1 {
                    diffs.push(DiffItem::Update {
                        old: (&old_kv.0, &old_kv.1),
                        new: (&new_kv.0, &new_kv.1),
                    });
                }
            } else {
                diffs.push(DiffItem::Remove(&old_kv.0, &old_kv.1));
                diffs.push(DiffItem::Add(&new_kv.0, &new_kv.1));
            }
        }
        // Mixed types or non-HamtNode node pairs — fall back to value
        // comparison. This handles SmallSimdNode, LargeSimdNode, and
        // Collision nodes, as well as cross-type comparisons (e.g.
        // SmallSimdNode vs HamtNode when node promotion occurred).
        _ => {
            let mut old_vals: Vec<&'a (K, V)> = Vec::new();
            let mut new_vals: Vec<&'b (K, V)> = Vec::new();
            old_entry.collect_values(&mut old_vals);
            new_entry.collect_values(&mut new_vals);
            diff_value_lists(&old_vals, &new_vals, diffs);
        }
    }
}

/// Compare two flat lists of key-value pairs, producing diffs.
///
/// Used when two HAMT entries at the same position have different node
/// types (e.g. SmallSimdNode vs LargeSimdNode) and cannot be compared
/// structurally. O(n × m) but n and m are bounded by node capacity
/// (≤32 for non-recursive node types).
fn diff_value_lists<'a, 'b, K, V>(
    old_vals: &[&'a (K, V)],
    new_vals: &[&'b (K, V)],
    diffs: &mut Vec<DiffItem<'a, 'b, K, V>>,
) where
    K: Eq,
    V: PartialEq,
{
    for old in old_vals {
        match new_vals.iter().find(|new| old.0 == new.0) {
            None => diffs.push(DiffItem::Remove(&old.0, &old.1)),
            Some(new) => {
                if old.1 != new.1 {
                    diffs.push(DiffItem::Update {
                        old: (&old.0, &old.1),
                        new: (&new.0, &new.1),
                    });
                }
            }
        }
    }
    for new in new_vals {
        if !old_vals.iter().any(|old| old.0 == new.0) {
            diffs.push(DiffItem::Add(&new.0, &new.1));
        }
    }
}

impl<'a, 'b, K, V, S, P, H: HashWidth> Iterator for DiffIter<'a, 'b, K, V, S, P, H>
where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher,
    P: SharedPointerKind,
{
    type Item = DiffItem<'a, 'b, K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.diffs.len() {
            let item = self.diffs[self.index];
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.diffs.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl<K, V, S, P, H: HashWidth> ExactSizeIterator for DiffIter<'_, '_, K, V, S, P, H>
where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher,
    P: SharedPointerKind,
{
}

impl<K, V, S, P, H: HashWidth> FusedIterator for DiffIter<'_, '_, K, V, S, P, H>
where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher,
    P: SharedPointerKind,
{
}

// Proptest
#[cfg(any(test, feature = "proptest"))]
#[doc(hidden)]
pub mod proptest {
    #[deprecated(
        since = "14.3.0",
        note = "proptest strategies have moved to pds::proptest"
    )]
    pub use crate::proptest::hash_map;
}

// Tests

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::LolHasher;
    #[rustfmt::skip]
    use ::proptest::{collection, num::{i16, usize}, proptest};
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use core::hash::BuildHasherDefault;

    assert_impl_all!(HashMap<i32, i32>: Send, Sync);
    assert_not_impl_any!(HashMap<i32, *const i32>: Send, Sync);
    assert_not_impl_any!(HashMap<*const i32, i32>: Send, Sync);
    assert_covariant!(HashMap<T, i32> in T);
    assert_covariant!(HashMap<i32, T> in T);


    #[test]
    fn safe_mutation() {
        let v1: HashMap<usize, usize> = GenericHashMap::from_iter((0..131_072).map(|i| (i, i)));
        let mut v2 = v1.clone();
        v2.insert(131_000, 23);
        assert_eq!(Some(&23), v2.get(&131_000));
        assert_eq!(Some(&131_000), v1.get(&131_000));
    }

    #[test]
    fn index_operator() {
        let mut map: HashMap<usize, usize> = hashmap![1 => 2, 3 => 4, 5 => 6];
        assert_eq!(4, map[&3]);
        map[&3] = 8;
        let target_map: HashMap<usize, usize> = hashmap![1 => 2, 3 => 8, 5 => 6];
        assert_eq!(target_map, map);
    }

    #[test]
    fn proper_formatting() {
        let map: HashMap<usize, usize> = hashmap![1 => 2];
        assert_eq!("{1: 2}", format!("{:?}", map));

        assert_eq!("{}", format!("{:?}", HashMap::<(), ()>::new()));
    }

    #[test]
    fn remove_failing() {
        let pairs = [(1469, 0), (-67, 0)];
        let mut m: std::collections::HashMap<i16, i16, _> =
            std::collections::HashMap::with_hasher(BuildHasherDefault::<LolHasher>::default());
        for (k, v) in &pairs {
            m.insert(*k, *v);
        }
        let mut map: GenericHashMap<i16, i16, _, DefaultSharedPtr> =
            GenericHashMap::with_hasher(BuildHasherDefault::<LolHasher>::default());
        for (k, v) in &m {
            map = map.update(*k, *v);
        }
        for k in m.keys() {
            let l = map.len();
            assert_eq!(m.get(k).cloned(), map.get(k).cloned());
            map = map.without(k);
            assert_eq!(None, map.get(k));
            assert_eq!(l - 1, map.len());
        }
    }

    #[test]
    fn match_string_keys_with_string_slices() {
        let tmp_map: HashMap<&str, &i32> = hashmap! { "foo" => &1, "bar" => &2, "baz" => &3 };
        let mut map: HashMap<String, i32> = From::from(&tmp_map);
        assert_eq!(Some(&1), map.get("foo"));
        map = map.without("foo");
        assert_eq!(Some(3), map.remove("baz"));
        map["bar"] = 8;
        assert_eq!(8, map["bar"]);
    }

    #[test]
    fn macro_allows_trailing_comma() {
        let map1: HashMap<&str, i32> = hashmap! {"x" => 1, "y" => 2};
        let map2: HashMap<&str, i32> = hashmap! {
            "x" => 1,
            "y" => 2,
        };
        assert_eq!(map1, map2);
    }

    #[test]
    fn remove_top_level_collisions() {
        let pairs = vec![9, 2569, 27145];
        let mut map: GenericHashMap<i16, i16, BuildHasherDefault<LolHasher>, DefaultSharedPtr> =
            Default::default();
        for k in pairs.clone() {
            map.insert(k, k);
        }
        assert_eq!(pairs.len(), map.len());
        let keys: Vec<_> = map.keys().cloned().collect();
        for k in keys {
            let l = map.len();
            assert_eq!(Some(&k), map.get(&k));
            map.remove(&k);
            assert_eq!(None, map.get(&k));
            assert_eq!(l - 1, map.len());
        }
    }

    #[test]
    fn entry_api() {
        let mut map: HashMap<&str, i32> = hashmap! {"bar" => 5};
        map.entry("foo").and_modify(|v| *v += 5).or_insert(1);
        assert_eq!(1, map[&"foo"]);
        map.entry("foo").and_modify(|v| *v += 5).or_insert(1);
        assert_eq!(6, map[&"foo"]);
        map.entry("bar").and_modify(|v| *v += 5).or_insert(1);
        assert_eq!(10, map[&"bar"]);
        assert_eq!(
            10,
            match map.entry("bar") {
                Entry::Occupied(entry) => entry.remove(),
                _ => panic!(),
            }
        );
        assert!(!map.contains_key(&"bar"));
    }

    #[test]
    fn refpool_crash() {
        let _map = HashMap::<u128, usize>::new();
    }

    #[test]
    fn large_map() {
        let mut map = HashMap::<_, _>::new();
        let size = 32769;
        for i in 0..size {
            map.insert(i, i);
        }
        assert_eq!(size, map.len());
        for i in 0..size {
            assert_eq!(Some(&i), map.get(&i));
        }
    }

    #[derive(Hash)]
    struct PanicOnClone;

    impl Clone for PanicOnClone {
        fn clone(&self) -> Self {
            panic!("PanicOnClone::clone called")
        }
    }

    #[test]
    fn into_iter_no_clone() {
        let mut map = HashMap::new();
        for i in 0..10_000 {
            map.insert(i, PanicOnClone);
        }
        let _ = map.into_iter().collect::<Vec<_>>();
    }

    #[test]
    fn iter_mut_no_clone() {
        let mut map = HashMap::new();
        for i in 0..10_000 {
            map.insert(i, PanicOnClone);
        }
        let _ = map.iter_mut().collect::<Vec<_>>();
    }

    #[test]
    fn iter_no_clone() {
        let mut map = HashMap::new();
        for i in 0..10_000 {
            map.insert(i, PanicOnClone);
        }
        let _ = map.iter().collect::<Vec<_>>();
    }

    proptest! {
        #[test]
        fn update_and_length(ref m in collection::hash_map(i16::ANY, i16::ANY, 0..1000)) {
            let mut map: GenericHashMap<i16, i16, BuildHasherDefault<LolHasher>, DefaultSharedPtr> = Default::default();
            for (index, (k, v)) in m.iter().enumerate() {
                map = map.update(*k, *v);
                assert_eq!(Some(v), map.get(k));
                assert_eq!(index + 1, map.len());
            }
        }

        #[test]
        fn from_iterator(ref m in collection::hash_map(i16::ANY, i16::ANY, 0..1000)) {
            let map: HashMap<i16, i16> =
                FromIterator::from_iter(m.iter().map(|(k, v)| (*k, *v)));
            assert_eq!(m.len(), map.len());
        }

        #[test]
        fn iterate_over(ref m in collection::hash_map(i16::ANY, i16::ANY, 0..1000)) {
            let map: HashMap<i16, i16> = FromIterator::from_iter(m.iter().map(|(k, v)| (*k, *v)));
            assert_eq!(m.len(), map.iter().count());
        }

        #[test]
        fn equality(ref m in collection::hash_map(i16::ANY, i16::ANY, 0..1000)) {
            let map1: HashMap<i16, i16> = FromIterator::from_iter(m.iter().map(|(k, v)| (*k, *v)));
            let map2: HashMap<i16, i16> = FromIterator::from_iter(m.iter().map(|(k, v)| (*k, *v)));
            assert_eq!(map1, map2);
        }

        #[test]
        fn lookup(ref m in collection::hash_map(i16::ANY, i16::ANY, 0..1000)) {
            let map: HashMap<i16, i16> = FromIterator::from_iter(m.iter().map(|(k, v)| (*k, *v)));
            for (k, v) in m {
                assert_eq!(Some(*v), map.get(k).cloned(), "{k} not found in map {map:?}");
            }
        }

        #[test]
        fn without(ref pairs in collection::vec((i16::ANY, i16::ANY), 0..100)) {
            let mut m: std::collections::HashMap<i16, i16, _> =
                std::collections::HashMap::with_hasher(BuildHasherDefault::<LolHasher>::default());
            for (k, v) in pairs {
                m.insert(*k, *v);
            }
            let mut map: GenericHashMap<i16, i16, _, DefaultSharedPtr> = GenericHashMap::with_hasher(BuildHasherDefault::<LolHasher>::default());
            for (k, v) in &m {
                map = map.update(*k, *v);
            }
            for k in m.keys() {
                let l = map.len();
                assert_eq!(m.get(k).cloned(), map.get(k).cloned());
                map = map.without(k);
                assert_eq!(None, map.get(k));
                assert_eq!(l - 1, map.len());
            }
        }

        #[test]
        fn insert(ref m in collection::hash_map(i16::ANY, i16::ANY, 0..1000)) {
            let mut mut_map: GenericHashMap<i16, i16, BuildHasherDefault<LolHasher>, DefaultSharedPtr> = Default::default();
            let mut map: GenericHashMap<i16, i16, BuildHasherDefault<LolHasher>, DefaultSharedPtr> = Default::default();
            for (count, (k, v)) in m.iter().enumerate() {
                map = map.update(*k, *v);
                mut_map.insert(*k, *v);
                assert_eq!(count + 1, map.len());
                assert_eq!(count + 1, mut_map.len());
            }
            for (k, v) in m {
                assert_eq!(Some(v), map.get(k));
                assert_eq!(Some(v), mut_map.get(k));
            }
            assert_eq!(map, mut_map);
        }

        #[test]
        fn remove(ref pairs in collection::vec((i16::ANY, i16::ANY), 0..100)) {
            let mut m: std::collections::HashMap<i16, i16, _> =
                std::collections::HashMap::with_hasher(BuildHasherDefault::<LolHasher>::default());
            for (k, v) in pairs {
                m.insert(*k, *v);
            }
            let mut map: GenericHashMap<i16, i16, _, DefaultSharedPtr> = GenericHashMap::with_hasher(BuildHasherDefault::<LolHasher>::default());
            for (k, v) in &m {
                map.insert(*k, *v);
            }
            for k in m.keys() {
                let l = map.len();
                assert_eq!(m.get(k).cloned(), map.get(k).cloned());
                map.remove(k);
                assert_eq!(None, map.get(k));
                assert_eq!(l - 1, map.len());
            }
        }

        #[test]
        fn delete_and_reinsert(
            ref input in collection::hash_map(i16::ANY, i16::ANY, 1..1000),
            index_rand in usize::ANY
        ) {
            let index = *input.keys().nth(index_rand % input.len()).unwrap();
            let map1: HashMap<_, _> = HashMap::from_iter(input.clone());
            let (val, map2) = map1.extract(&index).unwrap();
            let map3 = map2.update(index, val);
            for key in map2.keys() {
                assert!(*key != index);
            }
            assert_eq!(map1.len(), map2.len() + 1);
            assert_eq!(map1, map3);
        }

        #[test]
        fn proptest_works(ref m in proptest::hash_map(0..9999, ".*", 10..100)) {
            assert!(m.len() < 100);
            assert!(m.len() >= 10);
        }

        #[test]
        fn exact_size_iterator(ref m in proptest::hash_map(i16::ANY, i16::ANY, 0..100)) {
            let mut should_be = m.len();
            let mut it = m.iter();
            loop {
                assert_eq!(should_be, it.len());
                match it.next() {
                    None => break,
                    Some(_) => should_be -= 1,
                }
            }
            assert_eq!(0, it.len());
        }

        #[test]
        fn union(ref m1 in collection::hash_map(i16::ANY, i16::ANY, 0..100),
                 ref m2 in collection::hash_map(i16::ANY, i16::ANY, 0..100)) {
            let map1: HashMap<i16, i16> = FromIterator::from_iter(m1.iter().map(|(k, v)| (*k, *v)));
            let map2: HashMap<i16, i16> = FromIterator::from_iter(m2.iter().map(|(k, v)| (*k, *v)));
            let union_map = map1.union(map2);

            for k in m1.keys() {
                assert!(union_map.contains_key(k));
            }

            for k in m2.keys() {
                assert!(union_map.contains_key(k));
            }

            for (k, v) in union_map.iter() {
                assert_eq!(v, m1.get(k).or_else(|| m2.get(k)).unwrap());
            }
        }
    }

    #[test]
    fn test_structure_summary() {
        // Test with different sizes of HashMaps
        let sizes = vec![10, 100, 1_000, 10_000, 100_000];

        for size in sizes {
            println!("\n=== Testing with {} entries ===", size);

            let mut map = HashMap::new();

            // Insert entries
            for i in 0..size {
                // dbg!(i);
                map.insert(i, i * 2);
            }

            // Print structure summary
            map.print_structure_summary();
        }
    }

    #[test]
    fn partial_eq_ptr_eq_fast_path() {
        // Cloned maps with shared structure are equal in O(1).
        let mut map = HashMap::new();
        for i in 0..100 {
            map.insert(i, i * 2);
        }
        let map2 = map.clone();
        assert_eq!(map, map2);

        // After mutation, ptr_eq is false but element-wise equality still works.
        let mut map3 = map.clone();
        map3.insert(50, 999);
        assert_ne!(map, map3);

        // Empty maps.
        let empty: HashMap<i32, i32> = HashMap::new();
        let empty2: HashMap<i32, i32> = HashMap::new();
        assert_eq!(empty, empty2);

        // Self-comparison.
        assert_eq!(map, map);
    }

    #[test]
    fn merkle_hash_basic() {
        // Two maps built from the same data with the same hasher should
        // have the same root merkle hash.
        let mut m1 = HashMap::new();
        for i in 0..100 {
            m1.insert(i, i * 2);
        }
        let m2 = m1.clone();

        // Same hasher → same merkle hash.
        let r1 = m1.root.as_ref().unwrap();
        let r2 = m2.root.as_ref().unwrap();
        assert_eq!(r1.merkle_hash, r2.merkle_hash);

        // After inserting a new key, merkle hash changes.
        let mut m3 = m1.clone();
        m3.insert(999, 0);
        let r3 = m3.root.as_ref().unwrap();
        assert_ne!(r1.merkle_hash, r3.merkle_hash);

        // After removing a key, merkle hash changes.
        let mut m4 = m1.clone();
        m4.remove(&50);
        let r4 = m4.root.as_ref().unwrap();
        assert_ne!(r1.merkle_hash, r4.merkle_hash);

        // Replacing a value for the same key does NOT change the merkle
        // hash (keys-only fingerprint by design — see DEC for rationale).
        let mut m5 = m1.clone();
        m5.insert(50, 9999);
        let r5 = m5.root.as_ref().unwrap();
        assert_eq!(r1.merkle_hash, r5.merkle_hash);
    }

    #[test]
    fn merkle_hash_insert_remove_roundtrip() {
        // Insert then remove should restore the original merkle hash.
        let mut m = HashMap::new();
        for i in 0..50 {
            m.insert(i, i);
        }
        let original_merkle = m.root.as_ref().unwrap().merkle_hash;

        // Insert a new key and then remove it.
        m.insert(9999, 0);
        assert_ne!(m.root.as_ref().unwrap().merkle_hash, original_merkle);
        m.remove(&9999);
        assert_eq!(m.root.as_ref().unwrap().merkle_hash, original_merkle);
    }

    #[test]
    fn merkle_hash_empty_map() {
        let m: HashMap<i32, i32> = HashMap::new();
        assert!(m.root.is_none()); // empty maps have no root
    }

    #[test]
    fn merkle_hash_equality_negative_check() {
        // Two maps derived from a common clone with different key sets
        // should have different merkle hashes and the equality check
        // should detect this without element-wise comparison.
        let mut m1 = HashMap::new();
        for i in 0..1000 {
            m1.insert(i, i);
        }
        let mut m2 = m1.clone();
        m2.remove(&500);
        m2.insert(99999, 0);

        // Maps have the same size but different key sets.
        assert_eq!(m1.len(), m2.len());
        // Merkle hashes differ because key sets differ.
        assert_ne!(
            m1.root.as_ref().unwrap().merkle_hash,
            m2.root.as_ref().unwrap().merkle_hash
        );
        // Equality check correctly returns false.
        assert_ne!(m1, m2);
    }

    #[test]
    fn diff_identical_maps() {
        let mut map = HashMap::new();
        for i in 0..50 {
            map.insert(i, i * 2);
        }
        let map2 = map.clone();
        assert_eq!(map.diff(&map2).count(), 0);
    }

    #[test]
    fn diff_ptr_eq_fast_path() {
        let mut map = HashMap::new();
        for i in 0..50 {
            map.insert(i, i * 2);
        }
        let map2 = map.clone();
        assert!(map.ptr_eq(&map2));
        assert_eq!(map.diff(&map2).count(), 0);
    }

    #[test]
    fn diff_additions() {
        let map1: HashMap<i32, i32> = HashMap::new();
        let mut map2 = HashMap::new();
        map2.insert(1, 10);
        map2.insert(2, 20);
        let diffs: Vec<_> = map1.diff(&map2).collect();
        assert_eq!(diffs.len(), 2);
        assert!(diffs.iter().all(|d| matches!(d, DiffItem::Add(_, _))));
    }

    #[test]
    fn diff_removals() {
        let mut map1 = HashMap::new();
        map1.insert(1, 10);
        map1.insert(2, 20);
        let map2: HashMap<i32, i32> = HashMap::new();
        let diffs: Vec<_> = map1.diff(&map2).collect();
        assert_eq!(diffs.len(), 2);
        assert!(diffs.iter().all(|d| matches!(d, DiffItem::Remove(_, _))));
    }

    #[test]
    fn diff_updates() {
        let mut map1 = HashMap::new();
        map1.insert(1, 10);
        map1.insert(2, 20);
        let mut map2 = map1.clone();
        map2.insert(1, 99);
        let diffs: Vec<_> = map1.diff(&map2).collect();
        assert_eq!(diffs.len(), 1);
        match &diffs[0] {
            DiffItem::Update { old, new } => {
                assert_eq!(*old.0, 1);
                assert_eq!(*old.1, 10);
                assert_eq!(*new.0, 1);
                assert_eq!(*new.1, 99);
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn diff_mixed() {
        let mut map1 = HashMap::new();
        for i in 0..5 {
            map1.insert(i, i * 10);
        }
        let mut map2 = map1.clone();
        map2.remove(&0); // Remove
        map2.insert(2, 999); // Update
        map2.insert(10, 100); // Add
        let diffs: Vec<_> = map1.diff(&map2).collect();
        assert_eq!(diffs.len(), 3);
        let mut adds = 0;
        let mut updates = 0;
        let mut removes = 0;
        for d in &diffs {
            match d {
                DiffItem::Add(_, _) => adds += 1,
                DiffItem::Update { .. } => updates += 1,
                DiffItem::Remove(_, _) => removes += 1,
            }
        }
        assert_eq!(adds, 1);
        assert_eq!(updates, 1);
        assert_eq!(removes, 1);
    }

    #[test]
    fn diff_empty_maps() {
        let map1: HashMap<i32, i32> = HashMap::new();
        let map2: HashMap<i32, i32> = HashMap::new();
        assert_eq!(map1.diff(&map2).count(), 0);
    }

    #[test]
    fn diff_is_fused() {
        let mut map1 = HashMap::new();
        map1.insert(1, 10);
        let map2: HashMap<i32, i32> = HashMap::new();
        let mut iter = map1.diff(&map2);
        assert!(iter.next().is_some());
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[test]
    fn apply_diff_roundtrip() {
        let base = hashmap! {1 => "a", 2 => "b", 3 => "c"};
        let modified = hashmap! {1 => "a", 2 => "B", 4 => "d"};
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_empty_diff() {
        let map = hashmap! {1 => "a", 2 => "b"};
        let patched = map.apply_diff(vec![]);
        assert_eq!(patched, map);
    }

    #[test]
    fn apply_diff_from_empty() {
        let base: HashMap<i32, &str> = HashMap::new();
        let modified = hashmap! {1 => "a", 2 => "b"};
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn apply_diff_to_empty() {
        let base = hashmap! {1 => "a", 2 => "b"};
        let modified: HashMap<i32, &str> = HashMap::new();
        let diff: Vec<_> = base.diff(&modified).collect();
        let patched = base.apply_diff(diff);
        assert_eq!(patched, modified);
    }

    #[test]
    fn diff_shared_structure_subtree_skipping() {
        // Build a large map to ensure deep HAMT structure, then derive
        // a second map with a small number of changes. The tree-walk diff
        // should only visit changed subtrees.
        let mut base = HashMap::new();
        for i in 0..10_000 {
            base.insert(i, i * 2);
        }
        let mut modified = base.clone();
        modified.insert(42, 999); // Update
        modified.remove(&100); // Remove
        modified.insert(99_999, 1); // Add

        let diffs: Vec<_> = base.diff(&modified).collect();
        assert_eq!(diffs.len(), 3);

        // Verify roundtrip.
        let patched = base.apply_diff(diffs);
        assert_eq!(patched, modified);
    }

    #[test]
    fn diff_shared_structure_exact_size() {
        let mut base = HashMap::new();
        for i in 0..100 {
            base.insert(i, i);
        }
        let mut modified = base.clone();
        modified.insert(50, 999);
        modified.remove(&0);

        let iter = base.diff(&modified);
        assert_eq!(iter.len(), 2);
    }

    #[test]
    fn diff_kv_merkle_fast_path_equal_maps() {
        // Maps built via public insert maintain kv_merkle. Diffing an equal
        // clone should return empty without a tree walk (kv_merkle fast-path).
        let mut base = HashMap::new();
        for i in 0..500 {
            base.insert(i, i * 2);
        }
        assert!(base.kv_merkle_valid());
        let other = base.clone();
        assert!(other.kv_merkle_valid());
        assert_eq!(base.diff(&other).count(), 0);
    }

    #[test]
    fn diff_kv_merkle_fast_path_after_roundtrip() {
        // Insert then remove the same key returns the map to its prior state.
        // The kv_merkle is maintained incrementally, so the resulting map has
        // the same kv_merkle as the original and diff should be empty.
        let mut map1 = HashMap::new();
        for i in 0..200 {
            map1.insert(i, i);
        }
        let map2 = {
            let mut m = map1.clone();
            m.insert(9999, 9999);
            m.remove(&9999);
            m
        };
        assert!(map1.kv_merkle_valid());
        assert!(map2.kv_merkle_valid());
        assert_eq!(map1.diff(&map2).count(), 0);
    }

    #[test]
    fn diff_kv_merkle_invalidated_still_correct() {
        // When kv_merkle is invalid (after get_mut), diff falls back to tree
        // walk and still produces correct results.
        let mut map1 = HashMap::new();
        for i in 0..100 {
            map1.insert(i, i);
        }
        let mut map2 = map1.clone();
        // get_mut invalidates kv_merkle
        if let Some(v) = map2.get_mut(&0) {
            *v = 999;
        }
        assert!(!map2.kv_merkle_valid());
        let diffs: Vec<_> = map1.diff(&map2).collect();
        assert_eq!(diffs.len(), 1);
    }

    #[test]
    fn apply_diff_preserves_original() {
        let base = hashmap! {1 => "a", 2 => "b"};
        let modified = hashmap! {1 => "a", 2 => "B", 3 => "c"};
        let diff: Vec<_> = base.diff(&modified).collect();
        let _patched = base.apply_diff(diff);
        assert_eq!(base, hashmap! {1 => "a", 2 => "b"});
    }

    #[test]
    fn map_values_basic() {
        let map = hashmap! {1 => 10, 2 => 20, 3 => 30};
        let doubled = map.map_values(|v| v * 2);
        assert_eq!(doubled, hashmap! {1 => 20, 2 => 40, 3 => 60});
    }

    #[test]
    fn map_values_type_change() {
        let map = hashmap! {1 => 10, 2 => 20};
        let strings: HashMap<i32, String> = map.map_values(|v| format!("{v}"));
        assert_eq!(strings.get(&1), Some(&"10".to_string()));
        assert_eq!(strings.get(&2), Some(&"20".to_string()));
    }

    #[test]
    fn map_values_empty() {
        let map: HashMap<i32, i32> = HashMap::new();
        let result = map.map_values(|v| v * 2);
        assert!(result.is_empty());
    }

    #[test]
    fn map_values_with_key_basic() {
        let map = hashmap! {1 => 10, 2 => 20, 3 => 30};
        let sums = map.map_values_with_key(|k, v| k + v);
        assert_eq!(sums, hashmap! {1 => 11, 2 => 22, 3 => 33});
    }

    #[test]
    fn try_map_values_ok() {
        let map = hashmap! {1 => "10", 2 => "20", 3 => "30"};
        let parsed: Result<HashMap<i32, i32>, _> = map.try_map_values(|_, v| v.parse::<i32>());
        assert_eq!(parsed, Ok(hashmap! {1 => 10, 2 => 20, 3 => 30}));
    }

    #[test]
    fn try_map_values_err() {
        let map = hashmap! {1 => "10", 2 => "bad", 3 => "30"};
        let result: Result<HashMap<i32, i32>, _> = map.try_map_values(|_, v| v.parse::<i32>());
        assert!(result.is_err());
    }

    #[test]
    fn partition_basic() {
        let map = hashmap! {1 => "one", 2 => "two", 3 => "three", 4 => "four"};
        let (evens, odds) = map.partition(|k, _| k % 2 == 0);
        assert_eq!(evens, hashmap! {2 => "two", 4 => "four"});
        assert_eq!(odds, hashmap! {1 => "one", 3 => "three"});
    }

    #[test]
    fn disjoint_basic() {
        let a = hashmap! {1 => "a", 2 => "b"};
        let b = hashmap! {3 => "c", 4 => "d"};
        let c = hashmap! {2 => "x", 5 => "e"};
        assert!(a.disjoint(&b));
        assert!(!a.disjoint(&c));
    }

    #[test]
    fn disjoint_empty() {
        let a = hashmap! {1 => "a"};
        let b: HashMap<i32, &str> = HashMap::new();
        assert!(a.disjoint(&b));
        assert!(b.disjoint(&a));
    }

    #[test]
    fn map_keys_basic() {
        let map = hashmap! {1 => "a", 2 => "b", 3 => "c"};
        let negated = map.map_keys(|k| -k);
        assert_eq!(negated.len(), 3);
        assert_eq!(negated.get(&-1), Some(&"a"));
        assert_eq!(negated.get(&-2), Some(&"b"));
        assert_eq!(negated.get(&-3), Some(&"c"));
    }

    #[test]
    fn restrict_keys_basic() {
        let map = hashmap! {1 => "a", 2 => "b", 3 => "c", 4 => "d"};
        let keys = crate::hashset::HashSet::from_iter(vec![2, 4]);
        let restricted = map.restrict_keys(&keys);
        assert_eq!(restricted, hashmap! {2 => "b", 4 => "d"});
    }

    #[test]
    fn without_keys_basic() {
        let map = hashmap! {1 => "a", 2 => "b", 3 => "c", 4 => "d"};
        let keys = crate::hashset::HashSet::from_iter(vec![2, 4]);
        let reduced = map.without_keys(&keys);
        assert_eq!(reduced, hashmap! {1 => "a", 3 => "c"});
    }

    #[test]
    fn merge_with_all_partitions() {
        let left = hashmap! {1 => "a", 2 => "b", 3 => "c"};
        let right = hashmap! {2 => 10, 3 => 20, 4 => 30};
        let merged: HashMap<i32, String> = left.merge_with(
            &right,
            |_k, v| Some(v.to_string()),
            |_k, l, r| Some(format!("{l}:{r}")),
            |_k, v| Some(v.to_string()),
        );
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[&1], "a");
        assert_eq!(merged[&2], "b:10");
        assert_eq!(merged[&3], "c:20");
        assert_eq!(merged[&4], "30");
    }

    #[test]
    fn merge_with_as_intersection() {
        let left = hashmap! {1 => 10, 2 => 20, 3 => 30};
        let right = hashmap! {2 => 200, 3 => 300, 4 => 400};
        let merged: HashMap<i32, i32> = left.merge_with(
            &right,
            |_, _| None,
            |_, l, r| Some(l + r),
            |_, _| None,
        );
        assert_eq!(merged, hashmap! {2 => 220, 3 => 330});
    }

    #[test]
    fn merge_with_as_difference() {
        let left = hashmap! {1 => "a", 2 => "b", 3 => "c"};
        let right = hashmap! {2 => 0, 3 => 0};
        let merged: HashMap<i32, String> = left.merge_with(
            &right,
            |_, v| Some(v.to_string()),
            |_, _, _| None,
            |_, _| None,
        );
        assert_eq!(merged, hashmap! {1 => "a".to_string()});
    }

    #[test]
    fn merge_with_empty_left() {
        let left: HashMap<i32, i32> = HashMap::new();
        let right = hashmap! {1 => 10, 2 => 20};
        let merged: HashMap<i32, i32> = left.merge_with(
            &right,
            |_, v| Some(*v),
            |_, l, r| Some(l + r),
            |_, v| Some(*v),
        );
        assert_eq!(merged, hashmap! {1 => 10, 2 => 20});
    }

    #[test]
    fn merge_with_empty_right() {
        let left = hashmap! {1 => 10, 2 => 20};
        let right: HashMap<i32, i32> = HashMap::new();
        let merged: HashMap<i32, i32> = left.merge_with(
            &right,
            |_, v| Some(*v),
            |_, l, r| Some(l + r),
            |_, v| Some(*v),
        );
        assert_eq!(merged, hashmap! {1 => 10, 2 => 20});
    }

    #[test]
    fn merge_with_both_empty() {
        let left: HashMap<i32, i32> = HashMap::new();
        let right: HashMap<i32, i32> = HashMap::new();
        let merged: HashMap<i32, i32> = left.merge_with(
            &right,
            |_, v| Some(*v),
            |_, l, r| Some(l + r),
            |_, v| Some(*v),
        );
        assert!(merged.is_empty());
    }

    #[test]
    fn partition_map_basic() {
        let map = hashmap! {1 => 10, 2 => 20, 3 => 30};
        let (small, big): (HashMap<i32, String>, HashMap<i32, String>) =
            map.partition_map(|_k, v| {
                if *v <= 15 {
                    Ok(format!("small:{v}"))
                } else {
                    Err(format!("big:{v}"))
                }
            });
        assert_eq!(small.len(), 1);
        assert_eq!(small[&1], "small:10");
        assert_eq!(big.len(), 2);
    }

    #[test]
    fn partition_map_all_left() {
        let map = hashmap! {1 => 1, 2 => 2};
        let (left, right): (HashMap<i32, i32>, HashMap<i32, i32>) =
            map.partition_map(|_, v| Ok(*v));
        assert_eq!(left, map);
        assert!(right.is_empty());
    }

    #[test]
    fn difference_with_basic() {
        let a = hashmap! {1 => 10, 2 => 20, 3 => 30};
        let b = hashmap! {2 => 5, 3 => 50, 4 => 40};
        let result = a.difference_with(&b, |_k, v_self, v_other| {
            if v_self > v_other {
                Some(*v_self - *v_other)
            } else {
                None
            }
        });
        assert_eq!(result.len(), 2);
        assert_eq!(result[&1], 10);
        assert_eq!(result[&2], 15);
    }

    #[test]
    fn difference_with_no_overlap() {
        let a = hashmap! {1 => 10, 2 => 20};
        let b = hashmap! {3 => 30, 4 => 40};
        let result = a.difference_with(&b, |_, _, _| None);
        assert_eq!(result, a);
    }

    #[test]
    fn map_accum_basic() {
        let map = hashmap! {1 => 10, 2 => 20};
        let (total, doubled): (i32, HashMap<i32, i32>) =
            map.map_accum(0, |acc, _k, v| (acc + v, v * 2));
        assert_eq!(total, 30);
        assert_eq!(doubled[&1], 20);
        assert_eq!(doubled[&2], 40);
    }

    #[test]
    fn map_accum_empty() {
        let map: HashMap<i32, i32> = HashMap::new();
        let (acc, result) = map.map_accum(42, |a, _, v| (a + v, *v));
        assert_eq!(acc, 42);
        assert!(result.is_empty());
    }

    // --- HAMT node demotion/upgrade edge case regression tests ---
    //
    // These target the node hierarchy transitions in nodes/hamt.rs:
    // - SmallSimdNode → LargeSimdNode (upgrade_to_large)
    // - LargeSimdNode → HamtNode (upgrade_to_hamt)
    // - HamtNode → Value demotion (remove, single Value child)
    // - SmallSimdNode → Value demotion (pop_value)
    // - LargeSimdNode → Value demotion (pop_value)
    // - CollisionNode → Value demotion (pop_value)
    //
    // Root cause context: demoting non-Value entries (SmallSimdNode,
    // LargeSimdNode, CollisionNode) from a child HamtNode to a parent
    // corrupts tree structure because those entries have shift-dependent
    // upgrade paths. Only Value entries can be safely demoted.

    type LolMap<K, V> =
        GenericHashMap<K, V, BuildHasherDefault<LolHasher>, DefaultSharedPtr>;

    // Bits per HAMT level — keys separated by (1 << SHIFT) land in the same level-0 slot.
    const SHIFT: usize = crate::config::HASH_LEVEL_SIZE;

    #[test]
    fn upgrade_small_to_large_simd() {
        // With LolHasher, i32 keys hash to their value. HAMT uses 5-bit
        // chunks (SHIFT=5 default, 3 for small-chunks).
        // Keys 0, 32, 64, ... all land in level-0 slot 0, forcing a single
        // SmallSimdNode to grow. When it exceeds SMALL_NODE_WIDTH (16 default,
        // 4 small-chunks), it upgrades to LargeSimdNode.
        let mut map = LolMap::default();
        // Insert enough same-slot keys to overflow a SmallSimdNode.
        // SMALL_NODE_WIDTH is HASH_WIDTH/2 (16 default, 4 small-chunks).
        // Use 20 keys to ensure overflow even with default config.
        let step = 1 << SHIFT; // keys with identical level-0 slot
        for i in 0..20 {
            map.insert(i * step, i);
        }
        assert_eq!(map.len(), 20);
        // Verify all keys retrievable
        for i in 0..20 {
            assert_eq!(map.get(&(i * step)), Some(&i));
        }
    }

    #[test]
    fn upgrade_large_to_hamt_node() {
        // Push beyond LargeSimdNode capacity (HASH_WIDTH = 32 default,
        // 8 small-chunks) to force upgrade_to_hamt.
        let mut map = LolMap::default();
        let step = 1 << SHIFT;
        for i in 0..40 {
            map.insert(i * step, i);
        }
        assert_eq!(map.len(), 40);
        for i in 0..40 {
            assert_eq!(map.get(&(i * step)), Some(&i));
        }
    }

    #[test]
    fn demote_small_simd_to_value() {
        // Build a SmallSimdNode with 2 entries, remove one → demotion to Value.
        let mut map = LolMap::default();
        let step = 1 << SHIFT;
        map.insert(0, 100);
        map.insert(step, 200); // same level-0 slot → SmallSimdNode
        assert_eq!(map.len(), 2);

        map.remove(&0);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&step), Some(&200));
        assert_eq!(map.get(&0), None);

        // Verify the map still works correctly after demotion
        map.insert(step * 2, 300);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&step), Some(&200));
        assert_eq!(map.get(&(step * 2)), Some(&300));
    }

    #[test]
    fn demote_large_simd_to_value() {
        // Build a LargeSimdNode (needs > SMALL_NODE_WIDTH entries in one slot),
        // then remove all but one → should demote through to Value.
        let mut map = LolMap::default();
        let step = 1 << SHIFT;
        let count = 20; // exceeds SMALL_NODE_WIDTH for both default and small-chunks
        for i in 0..count {
            map.insert(i * step, i);
        }
        assert_eq!(map.len(), count);

        // Remove all but the last
        for i in 0..(count - 1) {
            map.remove(&(i * step));
        }
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&((count - 1) * step)), Some(&(count - 1)));

        // Verify structural integrity by inserting more
        map.insert(0, 999);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&0), Some(&999));
    }

    #[test]
    fn demote_hamt_node_to_value() {
        // Build deep enough to create HamtNode children, then remove to
        // trigger HamtNode → Value demotion (the guarded path at line 678
        // that checks is_value() before demoting).
        let mut map = LolMap::default();
        let step = 1 << SHIFT;
        let count = 40; // forces upgrade_to_hamt
        for i in 0..count {
            map.insert(i * step, i);
        }

        // Remove down to 1 entry — must trigger demotion at each level
        for i in 1..count {
            map.remove(&(i * step));
        }
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&0), Some(&0));

        // Rebuild from the demoted state
        for i in 1..count {
            map.insert(i * step, i * 10);
        }
        assert_eq!(map.len(), count);
        for i in 0..count {
            let expected = if i == 0 { 0 } else { i * 10 };
            assert_eq!(map.get(&(i * step)), Some(&expected));
        }
    }

    #[test]
    fn demote_hamt_node_single_non_value_child() {
        // The critical edge case: after removal, a HamtNode has exactly 1
        // child that is itself a SmallSimdNode (not a Value). The demotion
        // guard (is_value() check) must prevent demoting it, because
        // SmallSimdNode entries are shift-dependent.
        let mut map = LolMap::default();
        let step = 1 << SHIFT;

        // Insert keys that will create a HamtNode at level 0 with multiple
        // children at different level-0 slots, AND entries that share a
        // level-1 slot within one of those children (creating a SmallSimdNode
        // at level 1).
        //
        // Keys 0 and step share level-0 slot 0 → SmallSimdNode at slot 0
        // Key 1 is in level-0 slot 1 → Value at slot 1
        map.insert(0, 100);
        map.insert(step, 200);
        map.insert(1, 300);
        assert_eq!(map.len(), 3);

        // Remove key 1 → HamtNode at level 0 has 1 child (slot 0) which is
        // a SmallSimdNode with 2 entries. The demotion guard must NOT demote
        // this SmallSimdNode to the parent level.
        map.remove(&1);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&0), Some(&100));
        assert_eq!(map.get(&step), Some(&200));
        assert_eq!(map.get(&1), None);

        // Verify tree integrity by reinserting and checking
        map.insert(1, 400);
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(&0), Some(&100));
        assert_eq!(map.get(&step), Some(&200));
        assert_eq!(map.get(&1), Some(&400));
    }

    #[test]
    fn collision_node_creation_and_demotion() {
        // Use LolHasher<5> which masks hashes to 5 bits — with SHIFT=5,
        // all hash bits are consumed at level 0. Keys with different values
        // but same 5-bit hash will create CollisionNode entries.
        type NarrowMap<K, V> =
            GenericHashMap<K, V, BuildHasherDefault<LolHasher<5>>, DefaultSharedPtr>;

        let mut map = NarrowMap::default();
        // Keys 0 and 32 both hash to 0 with LolHasher<5> (0 & 0x1F = 0,
        // 32 & 0x1F = 0). They have identical 5-bit hashes → collision.
        map.insert(0i32, 100);
        map.insert(32i32, 200);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&0), Some(&100));
        assert_eq!(map.get(&32), Some(&200));

        // Remove one → CollisionNode demotes to Value
        map.remove(&0);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&32), Some(&200));
        assert_eq!(map.get(&0), None);

        // Verify the demoted structure still works
        map.insert(64i32, 300); // also hashes to 0 with LolHasher<5>
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&32), Some(&200));
        assert_eq!(map.get(&64), Some(&300));
    }

    #[test]
    fn collision_node_multiple_entries() {
        // Build a collision node with several entries, remove incrementally
        type NarrowMap<K, V> =
            GenericHashMap<K, V, BuildHasherDefault<LolHasher<5>>, DefaultSharedPtr>;

        let mut map = NarrowMap::default();
        // All these keys hash to the same 5-bit value (0)
        let keys: Vec<i32> = (0..5).map(|i| i * 32).collect();
        for (i, &k) in keys.iter().enumerate() {
            map.insert(k, i as i32);
        }
        assert_eq!(map.len(), 5);

        // Remove one at a time, verifying at each step
        for (removed, &k) in keys.iter().enumerate() {
            map.remove(&k);
            assert_eq!(map.len(), 5 - removed - 1);
            assert_eq!(map.get(&k), None);
            // Remaining keys still accessible
            for &remaining in &keys[(removed + 1)..] {
                assert!(map.get(&remaining).is_some());
            }
        }
        assert!(map.is_empty());
    }

    #[test]
    fn delete_reinsert_all_node_types() {
        // Comprehensive: build a large map, delete every key, reinsert,
        // verify equality. Exercises all demotion paths during deletion
        // and all upgrade paths during reinsertion.
        let mut map = LolMap::default();
        let n = 200;
        for i in 0..n {
            map.insert(i, i * 3);
        }
        let original = map.clone();

        // Delete all
        for i in 0..n {
            map.remove(&i);
        }
        assert!(map.is_empty());

        // Reinsert all
        for i in 0..n {
            map.insert(i, i * 3);
        }
        assert_eq!(map, original);
    }

    #[test]
    fn delete_reinsert_reverse_order() {
        // Same as above but delete in reverse order — exercises different
        // demotion sequences since the tree structure is sensitive to
        // deletion order.
        let mut map = LolMap::default();
        let n = 200;
        for i in 0..n {
            map.insert(i, i * 3);
        }
        let original = map.clone();

        for i in (0..n).rev() {
            map.remove(&i);
        }
        assert!(map.is_empty());

        for i in 0..n {
            map.insert(i, i * 3);
        }
        assert_eq!(map, original);
    }

    #[test]
    fn persistent_remove_preserves_original() {
        // Persistent (non-mut) remove should not affect the original map.
        // Tests that demotion paths correctly clone before modifying.
        let step = 1 << SHIFT;
        let map: LolMap<i32, i32> = (0..20).map(|i| (i * step, i)).collect();
        let map2 = map.without(&0);
        assert_eq!(map.len(), 20);
        assert_eq!(map2.len(), 19);
        assert_eq!(map.get(&0), Some(&0));
        assert_eq!(map2.get(&0), None);
    }

    #[test]
    fn merkle_hash_consistency_across_demotions() {
        // Two maps built differently but containing the same entries
        // should be equal (Merkle hash check must agree).
        let mut map1 = LolMap::default();
        let step = 1 << SHIFT;
        // Build with extra keys, then remove them
        for i in 0..40 {
            map1.insert(i * step, i);
        }
        for i in 10..40 {
            map1.remove(&(i * step));
        }

        // Build directly with only the final keys
        let map2: LolMap<i32, i32> = (0..10).map(|i| (i * step, i)).collect();

        assert_eq!(map1, map2);
    }

    // ---- kv_merkle tests ----
    //
    // kv_merkle_hash depends on the hasher, so tests that compare hashes
    // across independently built maps must use a deterministic hasher
    // (LolMap). Tests that only check validity/invalidation can use HashMap.

    #[test]
    fn kv_merkle_empty_map() {
        let map: LolMap<i32, i32> = LolMap::default();
        assert!(map.kv_merkle_valid());
        assert_eq!(map.kv_merkle_hash, 0);
    }

    #[test]
    fn kv_merkle_identical_maps_match() {
        let mut a: LolMap<i32, i32> = LolMap::default();
        let mut b: LolMap<i32, i32> = LolMap::default();
        for i in 0..100 {
            a.insert(i, i * 10);
            b.insert(i, i * 10);
        }
        assert!(a.kv_merkle_valid());
        assert!(b.kv_merkle_valid());
        assert_eq!(a.kv_merkle_hash, b.kv_merkle_hash);
        assert_eq!(a, b);
    }

    #[test]
    fn kv_merkle_different_values_differ() {
        let mut a: LolMap<i32, i32> = LolMap::default();
        let mut b: LolMap<i32, i32> = LolMap::default();
        for i in 0..100 {
            a.insert(i, i * 10);
            b.insert(i, i * 10);
        }
        // Change one value
        b.insert(50, 999);
        assert!(a.kv_merkle_valid());
        assert!(b.kv_merkle_valid());
        assert_ne!(a.kv_merkle_hash, b.kv_merkle_hash);
        assert_ne!(a, b);
    }

    #[test]
    fn kv_merkle_order_independent() {
        let mut forward: LolMap<i32, i32> = LolMap::default();
        let mut reverse: LolMap<i32, i32> = LolMap::default();
        for i in 0..50 {
            forward.insert(i, i * 3);
        }
        for i in (0..50).rev() {
            reverse.insert(i, i * 3);
        }
        assert!(forward.kv_merkle_valid());
        assert!(reverse.kv_merkle_valid());
        assert_eq!(forward.kv_merkle_hash, reverse.kv_merkle_hash);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn kv_merkle_insert_remove_roundtrip() {
        let mut map: LolMap<i32, i32> = LolMap::default();
        for i in 0..20 {
            map.insert(i, i);
        }
        let original_hash = map.kv_merkle_hash;

        // Insert and remove an extra entry
        map.insert(999, 999);
        assert_ne!(map.kv_merkle_hash, original_hash);
        map.remove(&999);
        assert_eq!(map.kv_merkle_hash, original_hash);
    }

    #[test]
    fn kv_merkle_update_maintains() {
        let mut map: LolMap<i32, i32> = LolMap::default();
        for i in 0..20 {
            map.insert(i, i);
        }
        // Persistent update
        let map2 = map.update(5, 500);
        assert!(map.kv_merkle_valid());
        assert!(map2.kv_merkle_valid());
        assert_ne!(map.kv_merkle_hash, map2.kv_merkle_hash);

        // Update with same value — should restore original
        let map3 = map2.update(5, 5);
        assert_eq!(map.kv_merkle_hash, map3.kv_merkle_hash);
    }

    #[test]
    fn kv_merkle_clone_preserves() {
        let mut map: LolMap<i32, i32> = LolMap::default();
        for i in 0..50 {
            map.insert(i, i * 7);
        }
        let cloned = map.clone();
        assert!(cloned.kv_merkle_valid());
        assert_eq!(map.kv_merkle_hash, cloned.kv_merkle_hash);
    }

    #[test]
    fn kv_merkle_from_iterator() {
        // Build via from_iter and via manual insert; hashes should match
        // when using the same deterministic hasher.
        let map: LolMap<i32, i32> = (0..30).map(|i| (i, i * 2)).collect();
        assert!(map.kv_merkle_valid());

        let mut manual: LolMap<i32, i32> = LolMap::default();
        for i in 0..30 {
            manual.insert(i, i * 2);
        }
        assert_eq!(map.kv_merkle_hash, manual.kv_merkle_hash);
    }

    #[test]
    fn kv_merkle_get_mut_invalidates() {
        let mut map: LolMap<i32, i32> = LolMap::default();
        for i in 0..10 {
            map.insert(i, i);
        }
        assert!(map.kv_merkle_valid());

        // get_mut should invalidate
        if let Some(v) = map.get_mut(&5) {
            *v = 999;
        }
        assert!(!map.kv_merkle_valid());

        // recompute should restore validity
        map.recompute_kv_merkle();
        assert!(map.kv_merkle_valid());

        // The recomputed hash should match a freshly built map
        let mut fresh: LolMap<i32, i32> = LolMap::default();
        for i in 0..10 {
            if i == 5 {
                fresh.insert(i, 999);
            } else {
                fresh.insert(i, i);
            }
        }
        assert_eq!(map.kv_merkle_hash, fresh.kv_merkle_hash);
    }

    #[test]
    fn kv_merkle_iter_mut_invalidates() {
        let mut map = HashMap::new();
        for i in 0..10 {
            map.insert(i, i);
        }
        assert!(map.kv_merkle_valid());
        // iter_mut invalidates on call; drop the iterator before checking
        { let _ = map.iter_mut(); }
        assert!(!map.kv_merkle_valid());
    }

    #[test]
    fn kv_merkle_retain_invalidates() {
        let mut map = HashMap::new();
        for i in 0..10 {
            map.insert(i, i);
        }
        assert!(map.kv_merkle_valid());
        map.retain(|k, _| *k < 5);
        assert!(!map.kv_merkle_valid());
    }

    #[test]
    fn kv_merkle_entry_api_invalidates() {
        let mut map = HashMap::new();
        map.insert(1, 10);
        assert!(map.kv_merkle_valid());

        // or_insert on vacant entry invalidates
        map.entry(2).or_insert(20);
        assert!(!map.kv_merkle_valid());

        // Recompute and verify occupied entry get_mut invalidates
        map.recompute_kv_merkle();
        assert!(map.kv_merkle_valid());
        if let crate::hashmap::Entry::Occupied(mut occ) = map.entry(1) {
            let _v = occ.get_mut();
        }
        assert!(!map.kv_merkle_valid());
    }

    #[test]
    fn kv_merkle_clear_resets() {
        let mut map = HashMap::new();
        for i in 0..10 {
            map.insert(i, i);
        }
        assert_ne!(map.kv_merkle_hash, 0);
        map.clear();
        assert!(map.kv_merkle_valid());
        assert_eq!(map.kv_merkle_hash, 0);
    }

    #[test]
    fn kv_merkle_replace_value_incremental() {
        // Replacing a value should update kv_merkle incrementally.
        // The result should match a fresh build with the same hasher.
        let mut map: LolMap<i32, i32> = LolMap::default();
        for i in 0..20 {
            map.insert(i, i);
        }
        map.insert(10, 999); // replace value for key 10
        assert!(map.kv_merkle_valid());

        let mut fresh: LolMap<i32, i32> = LolMap::default();
        for i in 0..20 {
            if i == 10 {
                fresh.insert(i, 999);
            } else {
                fresh.insert(i, i);
            }
        }
        assert_eq!(map.kv_merkle_hash, fresh.kv_merkle_hash);
    }

    #[test]
    fn kv_merkle_set_operations_invalidate() {
        let a: HashMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        let b: HashMap<i32, i32> = (5..15).map(|i| (i, i * 2)).collect();

        // Union invalidates kv_merkle (uses internal helpers)
        let union = a.clone().union(b.clone());
        assert!(!union.kv_merkle_valid());

        // After recompute, should match manual construction
        let mut union_recomputed = union.clone();
        union_recomputed.recompute_kv_merkle();
        assert!(union_recomputed.kv_merkle_valid());
    }

    #[test]
    fn kv_merkle_without_maintains() {
        let mut map: LolMap<i32, i32> = LolMap::default();
        for i in 0..20 {
            map.insert(i, i * 5);
        }
        let without5 = map.without(&5);
        assert!(without5.kv_merkle_valid());

        let mut fresh: LolMap<i32, i32> = LolMap::default();
        for i in 0..20 {
            if i != 5 {
                fresh.insert(i, i * 5);
            }
        }
        assert_eq!(without5.kv_merkle_hash, fresh.kv_merkle_hash);
    }

    #[test]
    fn kv_merkle_extract_with_key_maintains() {
        let map: LolMap<i32, i32> = (0..10).map(|i| (i, i * 3)).collect();
        let result = map.extract_with_key(&5);
        let (k, v, remaining) = result.unwrap();
        assert_eq!(k, 5);
        assert_eq!(v, 15);
        assert!(remaining.kv_merkle_valid());

        let expected: LolMap<i32, i32> = (0..10)
            .filter(|&i| i != 5)
            .map(|i| (i, i * 3))
            .collect();
        assert_eq!(remaining.kv_merkle_hash, expected.kv_merkle_hash);
    }

    proptest! {
        #[test]
        fn kv_merkle_proptest_insert_order(
            pairs in collection::vec((0i32..1000, 0i32..1000), 0..200)
        ) {
            // Build two maps with the same deterministic hasher in different
            // insertion orders; kv_merkle should match if contents match.
            let mut forward: LolMap<i32, i32> = LolMap::default();
            for &(k, v) in &pairs {
                forward.insert(k, v);
            }
            let mut reverse: LolMap<i32, i32> = LolMap::default();
            for &(k, v) in pairs.iter().rev() {
                reverse.insert(k, v);
            }
            // Both maps keep the last value for duplicate keys but in
            // different order. Recompute to get canonical hashes.
            forward.recompute_kv_merkle();
            reverse.recompute_kv_merkle();
            if forward == reverse {
                assert_eq!(forward.kv_merkle_hash, reverse.kv_merkle_hash);
            }
        }

        #[test]
        fn kv_merkle_proptest_insert_remove_roundtrip(
            base in collection::vec((0i32..500, 0i32..500), 0..100),
            extra in collection::vec((500i32..1000, 0i32..500), 1..50),
        ) {
            let mut map: LolMap<i32, i32> = LolMap::default();
            for &(k, v) in &base {
                map.insert(k, v);
            }
            let original_hash = map.kv_merkle_hash;
            let original_valid = map.kv_merkle_valid();

            // Insert extras then remove them
            for &(k, v) in &extra {
                map.insert(k, v);
            }
            for &(k, _) in &extra {
                map.remove(&k);
            }

            // kv_merkle should match original (extras don't overlap base keys)
            if original_valid {
                assert!(map.kv_merkle_valid());
                assert_eq!(original_hash, map.kv_merkle_hash);
            }
        }
    }

    #[test]
    fn hash_order_independent() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        fn hash_of(m: &HashMap<i32, i32>) -> u64 {
            let mut h = DefaultHasher::new();
            m.hash(&mut h);
            h.finish()
        }
        let mut a = HashMap::new();
        a.insert(1, 10); a.insert(2, 20); a.insert(3, 30);
        let mut b = HashMap::new();
        b.insert(3, 30); b.insert(1, 10); b.insert(2, 20); // different insertion order
        assert_eq!(hash_of(&a), hash_of(&b));
    }
}
