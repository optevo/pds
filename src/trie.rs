// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent trie (prefix tree).
//!
//! A `Trie<K, V>` maps sequences of keys (paths) to values. Each node
//! can hold a value and a set of children indexed by a key segment.
//! Backed by [`HashMap`][crate::HashMap], all operations benefit from
//! structural sharing — cloning is O(1) and modifications share
//! unchanged subtrees with the original.
//!
//! # Examples
//!
//! ```
//! use imbl::Trie;
//!
//! let mut trie = Trie::new();
//! trie.insert(&["usr", "bin", "rustc"], 1);
//! trie.insert(&["usr", "lib", "libc.so"], 2);
//! trie.insert(&["etc", "hosts"], 3);
//!
//! assert_eq!(trie.get(&["usr", "bin", "rustc"]), Some(&1));
//! assert_eq!(trie.get(&["etc", "hosts"]), Some(&3));
//! assert_eq!(trie.get(&["usr"]), None); // no value at interior node
//! ```

#[cfg(feature = "std")]
use std::collections::hash_map::RandomState;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{BuildHasher, Hash};

use archery::SharedPointerKind;

use crate::hashmap::GenericHashMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericTrie`] with default hasher and pointer type.
#[cfg(feature = "std")]
pub type Trie<K, V> = GenericTrie<K, V, RandomState, DefaultSharedPtr>;

/// Type alias for [`GenericTrie`] using [`foldhash::fast::RandomState`] — available
/// in `no_std` environments when the `foldhash` feature is enabled.
#[cfg(all(not(feature = "std"), feature = "foldhash"))]
pub type Trie<K, V> = GenericTrie<K, V, foldhash::fast::RandomState, DefaultSharedPtr>;

/// A persistent trie (prefix tree) backed by [`GenericHashMap`].
///
/// Keys are sequences of segments of type `K`. Values of type `V` can
/// be stored at any node (interior or leaf). Clone is O(1) via
/// structural sharing.
///
/// # Performance
///
/// For a path of length *d* (depth), operations are O(d × log<sub>32</sub> n)
/// where *n* is the fanout at each level. For typical use cases (shallow
/// tries with moderate fanout), this is effectively O(d).
pub struct GenericTrie<K, V, S, P: SharedPointerKind = DefaultSharedPtr> {
    value: Option<V>,
    children: GenericHashMap<K, GenericTrie<K, V, S, P>, S, P>,
}

// Manual Clone to avoid derive's spurious bounds.
impl<K: Clone, V: Clone, S: Clone, P: SharedPointerKind> Clone for GenericTrie<K, V, S, P> {
    fn clone(&self) -> Self {
        GenericTrie {
            value: self.value.clone(),
            children: self.children.clone(),
        }
    }
}

#[cfg(feature = "std")]
impl<K, V, P> GenericTrie<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty trie.
    #[must_use]
    pub fn new() -> Self {
        GenericTrie {
            value: None,
            children: GenericHashMap::new(),
        }
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> GenericTrie<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    /// Create an empty trie (no_std + foldhash).
    #[must_use]
    pub fn new() -> Self {
        GenericTrie {
            value: None,
            children: GenericHashMap::new(),
        }
    }
}

impl<K, V, S, P> GenericTrie<K, V, S, P>
where
    P: SharedPointerKind,
{
    /// Test whether this trie is empty (no values at any depth).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_none() && self.children.is_empty()
    }

    /// Get the value at this node (the root / empty path).
    #[must_use]
    pub fn value(&self) -> Option<&V> {
        self.value.as_ref()
    }

    /// Get a mutable reference to the value at this node.
    pub fn value_mut(&mut self) -> Option<&mut V> {
        self.value.as_mut()
    }

    /// Return the number of direct children.
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

impl<K, V, S, P> GenericTrie<K, V, S, P>
where
    K: Hash + Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    /// Get a reference to the subtrie at the given path.
    #[must_use]
    pub fn subtrie<Q>(&self, path: &[Q]) -> Option<&Self>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        let mut node = self;
        for segment in path {
            node = node.children.get(segment)?;
        }
        Some(node)
    }

    /// Get the value at the given path.
    #[must_use]
    pub fn get<Q>(&self, path: &[Q]) -> Option<&V>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        self.subtrie(path)?.value()
    }

    /// Test whether a value exists at the given path.
    #[must_use]
    pub fn contains_path<Q>(&self, path: &[Q]) -> bool
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        self.get(path).is_some()
    }
}

impl<K, V, S, P> GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    /// Insert a value at the given path, returning the previous value.
    pub fn insert(&mut self, path: &[K], value: V) -> Option<V> {
        if path.is_empty() {
            return self.value.replace(value);
        }
        let child = self
            .children
            .entry(path[0].clone())
            .or_insert_with(|| GenericTrie {
                value: None,
                children: GenericHashMap::default(),
            });
        child.insert(&path[1..], value)
    }

    /// Remove the value at the given path, returning it if present.
    ///
    /// Does not remove empty interior nodes — the trie structure is
    /// preserved. Use [`prune`][Self::prune] to clean up empty subtrees.
    pub fn remove<Q>(&mut self, path: &[Q]) -> Option<V>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        if path.is_empty() {
            return self.value.take();
        }
        let child = self.children.get_mut(&path[0])?;
        child.remove(&path[1..])
    }

    /// Remove a direct child by key, returning it if present.
    ///
    /// Uses the invalidating remove path to avoid requiring `V: Hash`
    /// on the trie value type.
    fn remove_child<Q>(&mut self, key: &Q) -> Option<GenericTrie<K, V, S, P>>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        self.children.remove_invalidate_kv(key).map(|(_, v)| v)
    }

    /// Remove the value at the given path and prune empty nodes.
    ///
    /// After removing the value, walks back up the path and removes
    /// any nodes that have no value and no children.
    pub fn remove_and_prune<Q>(&mut self, path: &[Q]) -> Option<V>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        self.remove_and_prune_inner(path)
    }

    fn remove_and_prune_inner<Q>(&mut self, path: &[Q]) -> Option<V>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        if path.is_empty() {
            return self.value.take();
        }
        let result = {
            let child = self.children.get_mut(&path[0])?;
            child.remove_and_prune_inner(&path[1..])
        };
        // Prune empty child
        if result.is_some() {
            if let Some(child) = self.children.get(&path[0]) {
                if child.is_empty() {
                    self.remove_child(&path[0]);
                }
            }
        }
        result
    }

    /// Remove all empty interior nodes (nodes with no value and no children).
    pub fn prune(&mut self) {
        // Recurse first so children are pruned bottom-up.
        for (_, child) in self.children.iter_mut() {
            child.prune();
        }
        let keys_to_remove: alloc::vec::Vec<K> = self
            .children
            .iter()
            .filter_map(|(k, v)| if v.is_empty() { Some(k.clone()) } else { None })
            .collect();
        for key in &keys_to_remove {
            self.remove_child(key);
        }
    }

    /// Iterate over all (path, value) pairs in the trie.
    ///
    /// Paths are returned as `Vec<&K>` segments. Iteration order follows
    /// the hash map's internal ordering at each level.
    pub fn iter(&self) -> TrieIter<'_, K, V, S, P> {
        let mut stack = alloc::vec::Vec::new();
        stack.push((alloc::vec::Vec::new(), self));
        TrieIter { stack }
    }

    /// Return the number of values stored in the trie (at all depths).
    #[must_use]
    pub fn len(&self) -> usize {
        let own = if self.value.is_some() { 1 } else { 0 };
        own + self
            .children
            .iter()
            .map(|(_, child)| child.len())
            .sum::<usize>()
    }

    /// Iterate over all paths that share the given prefix.
    ///
    /// Returns (remaining_path, value) pairs for all values under the prefix.
    pub fn iter_prefix<'a, Q>(
        &'a self,
        prefix: &[Q],
    ) -> Option<TrieIter<'a, K, V, S, P>>
    where
        Q: Hash + Eq,
        K: core::borrow::Borrow<Q>,
    {
        let subtrie = self.subtrie(prefix)?;
        Some(subtrie.iter())
    }
}

#[cfg(feature = "std")]
impl<K, V, P> Default for GenericTrie<K, V, RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(not(feature = "std"), feature = "foldhash"))]
impl<K, V, P> Default for GenericTrie<K, V, foldhash::fast::RandomState, P>
where
    P: SharedPointerKind,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S, P> PartialEq for GenericTrie<K, V, S, P>
where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.children == other.children
    }
}

impl<K, V, S, P> Eq for GenericTrie<K, V, S, P>
where
    K: Hash + Eq,
    V: Eq,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
}

impl<K, V, S, P> Debug for GenericTrie<K, V, S, P>
where
    K: Debug + Hash + Eq + Clone,
    V: Debug + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_struct("Trie");
        if let Some(v) = &self.value {
            d.field("value", v);
        }
        if !self.children.is_empty() {
            d.field("children", &self.children);
        }
        d.finish()
    }
}

/// Iterator over (path, value) pairs in a trie.
pub struct TrieIter<'a, K, V, S, P: SharedPointerKind> {
    stack: alloc::vec::Vec<(alloc::vec::Vec<&'a K>, &'a GenericTrie<K, V, S, P>)>,
}

impl<'a, K, V, S, P> Iterator for TrieIter<'a, K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    type Item = (alloc::vec::Vec<&'a K>, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (path, node) = self.stack.pop()?;
            // Push children onto the stack (in reverse so first child is popped first)
            let children: alloc::vec::Vec<_> = node.children.iter().collect();
            for (key, child) in children.into_iter().rev() {
                let mut child_path = path.clone();
                child_path.push(key);
                self.stack.push((child_path, child));
            }
            if let Some(value) = &node.value {
                return Some((path, value));
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn empty_trie() {
        let trie: Trie<&str, i32> = Trie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
        assert_eq!(trie.get::<&str>(&[]), None);
    }

    #[test]
    fn insert_and_get() {
        let mut trie = Trie::new();
        trie.insert(&["a", "b"], 1);
        trie.insert(&["a", "c"], 2);
        trie.insert(&["d"], 3);

        assert_eq!(trie.get(&["a", "b"]), Some(&1));
        assert_eq!(trie.get(&["a", "c"]), Some(&2));
        assert_eq!(trie.get(&["d"]), Some(&3));
        assert_eq!(trie.get(&["a"]), None);
        assert_eq!(trie.get(&["x"]), None);
    }

    #[test]
    fn insert_at_root() {
        let mut trie = Trie::new();
        trie.insert(&[] as &[&str], 42);
        assert_eq!(trie.get(&[] as &[&str]), Some(&42));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn insert_overwrites() {
        let mut trie = Trie::new();
        let old = trie.insert(&["a"], 1);
        assert_eq!(old, None);
        let old = trie.insert(&["a"], 2);
        assert_eq!(old, Some(1));
        assert_eq!(trie.get(&["a"]), Some(&2));
    }

    #[test]
    fn remove() {
        let mut trie = Trie::new();
        trie.insert(&["a", "b"], 1);
        trie.insert(&["a", "c"], 2);

        let removed = trie.remove(&["a", "b"]);
        assert_eq!(removed, Some(1));
        assert_eq!(trie.get(&["a", "b"]), None);
        assert_eq!(trie.get(&["a", "c"]), Some(&2));
    }

    #[test]
    fn remove_absent() {
        let mut trie: Trie<&str, i32> = Trie::new();
        assert_eq!(trie.remove(&["x"]), None);
    }

    #[test]
    fn remove_and_prune() {
        let mut trie = Trie::new();
        trie.insert(&["a", "b", "c"], 1);
        trie.remove_and_prune(&["a", "b", "c"]);
        assert!(trie.is_empty());
    }

    #[test]
    fn contains_path() {
        let mut trie = Trie::new();
        trie.insert(&["x", "y"], 1);
        assert!(trie.contains_path(&["x", "y"]));
        assert!(!trie.contains_path(&["x"]));
        assert!(!trie.contains_path(&["z"]));
    }

    #[test]
    fn subtrie() {
        let mut trie = Trie::new();
        trie.insert(&["a", "b"], 1);
        trie.insert(&["a", "c"], 2);

        let sub = trie.subtrie(&["a"]).unwrap();
        assert_eq!(sub.get(&["b"]), Some(&1));
        assert_eq!(sub.get(&["c"]), Some(&2));
    }

    #[test]
    fn len() {
        let mut trie = Trie::new();
        assert_eq!(trie.len(), 0);
        trie.insert(&["a"], 1);
        assert_eq!(trie.len(), 1);
        trie.insert(&["a", "b"], 2);
        assert_eq!(trie.len(), 2);
        trie.insert(&["c"], 3);
        assert_eq!(trie.len(), 3);
    }

    #[test]
    fn clone_shares_structure() {
        let mut trie = Trie::new();
        trie.insert(&["a", "b"], 1);
        let trie2 = trie.clone();
        assert_eq!(trie, trie2);

        // Modify original — clone unaffected
        trie.insert(&["a", "c"], 2);
        assert_ne!(trie, trie2);
        assert_eq!(trie2.get(&["a", "c"]), None);
    }

    #[test]
    fn iter_all_values() {
        let mut trie = Trie::new();
        trie.insert(&["a"], 1);
        trie.insert(&["b"], 2);
        trie.insert(&["a", "c"], 3);

        let mut items: alloc::vec::Vec<_> = trie.iter().collect();
        items.sort_by_key(|(_, v)| **v);

        assert_eq!(items.len(), 3);
        assert_eq!(*items[0].1, 1);
        assert_eq!(*items[1].1, 2);
        assert_eq!(*items[2].1, 3);
    }

    #[test]
    fn iter_prefix() {
        let mut trie = Trie::new();
        trie.insert(&["usr", "bin", "rustc"], 1);
        trie.insert(&["usr", "lib", "libc"], 2);
        trie.insert(&["etc", "hosts"], 3);

        let usr_items: alloc::vec::Vec<_> = trie.iter_prefix(&["usr"]).unwrap().collect();
        assert_eq!(usr_items.len(), 2);

        assert!(trie.iter_prefix(&["nonexistent"]).is_none());
    }

    #[test]
    fn interior_and_leaf_values() {
        let mut trie = Trie::new();
        trie.insert(&["a"], 1);
        trie.insert(&["a", "b"], 2);

        assert_eq!(trie.get(&["a"]), Some(&1));
        assert_eq!(trie.get(&["a", "b"]), Some(&2));
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn child_count() {
        let mut trie = Trie::new();
        assert_eq!(trie.child_count(), 0);
        trie.insert(&["a", "x"], 1);
        trie.insert(&["b", "y"], 2);
        assert_eq!(trie.child_count(), 2);
    }

    #[test]
    fn equality() {
        let mut a = Trie::new();
        a.insert(&["x", "y"], 1);
        a.insert(&["x", "z"], 2);

        let mut b = Trie::new();
        b.insert(&["x", "z"], 2);
        b.insert(&["x", "y"], 1);

        assert_eq!(a, b);
    }

    #[test]
    fn prune() {
        let mut trie = Trie::new();
        trie.insert(&["a", "b", "c"], 1);
        trie.remove(&["a", "b", "c"]);
        // Empty nodes remain after remove
        assert!(!trie.is_empty());
        assert_eq!(trie.child_count(), 1);

        trie.prune();
        assert!(trie.is_empty());
    }
}
