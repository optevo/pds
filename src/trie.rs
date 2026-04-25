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
//! use pds::Trie;
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
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::{FromIterator, Sum};
use core::ops::Add;

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
    pub(crate) value: Option<V>,
    pub(crate) children: GenericHashMap<K, GenericTrie<K, V, S, P>, S, P>,
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
        let stack = alloc::vec![(alloc::vec::Vec::new(), self)];
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

impl<K, V, S, P> Hash for GenericTrie<K, V, S, P>
where
    K: Hash + Eq,
    V: Hash,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
        // children's Hash impl is order-independent (XOR-combines per-entry
        // hashes), consistent with PartialEq's structural comparison.
        self.children.hash(state);
    }
}

impl<K, V, S, P> Extend<(alloc::vec::Vec<K>, V)> for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn extend<I: IntoIterator<Item = (alloc::vec::Vec<K>, V)>>(&mut self, iter: I) {
        for (path, value) in iter {
            self.insert(&path, value);
        }
    }
}

impl<K, V, S, P> FromIterator<(alloc::vec::Vec<K>, V)> for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from_iter<I: IntoIterator<Item = (alloc::vec::Vec<K>, V)>>(iter: I) -> Self {
        let mut trie = GenericTrie {
            value: None,
            children: GenericHashMap::default(),
        };
        trie.extend(iter);
        trie
    }
}

impl<K, V, S, P> From<alloc::vec::Vec<(alloc::vec::Vec<K>, V)>> for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(v: alloc::vec::Vec<(alloc::vec::Vec<K>, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K, V, S, P, const N: usize> From<[(alloc::vec::Vec<K>, V); N]> for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(arr: [(alloc::vec::Vec<K>, V); N]) -> Self {
        arr.into_iter().collect()
    }
}

impl<'a, K, V, S, P> From<&'a [(alloc::vec::Vec<K>, V)]> for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn from(slice: &'a [(alloc::vec::Vec<K>, V)]) -> Self {
        slice.iter().map(|(p, v)| (p.clone(), v.clone())).collect()
    }
}

impl<K, V, S, P> Add for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    type Output = Self;

    /// Union two tries. When a path exists in both, the right operand's value wins.
    fn add(self, other: Self) -> Self {
        let mut result = self;
        for (path, value) in other {
            result.insert(&path, value);
        }
        result
    }
}

impl<K, V, S, P> Add for &GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    type Output = GenericTrie<K, V, S, P>;

    fn add(self, other: Self) -> Self::Output {
        self.clone() + other.clone()
    }
}

impl<K, V, S, P> Sum for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(
            GenericTrie { value: None, children: GenericHashMap::default() },
            |a, b| a + b,
        )
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

impl<'a, K, V, S, P> IntoIterator for &'a GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    type Item = (alloc::vec::Vec<&'a K>, &'a V);
    type IntoIter = TrieIter<'a, K, V, S, P>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Owning iterator over `(path, value)` pairs produced by consuming a trie.
///
/// Paths are `Vec<K>`. Produced by [`IntoIterator`] for [`GenericTrie`].
pub struct TrieConsumingIter<K, V> {
    // Delegate to Vec's IntoIter for O(1) amortised next().
    inner: alloc::vec::IntoIter<(alloc::vec::Vec<K>, V)>,
}

impl<K, V> Iterator for TrieConsumingIter<K, V> {
    type Item = (alloc::vec::Vec<K>, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V, S, P> IntoIterator for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone,
    V: Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    type Item = (alloc::vec::Vec<K>, V);
    type IntoIter = TrieConsumingIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        // Eagerly collect all (path, value) pairs. A zero-copy consuming iter
        // would require draining the recursive HashMap structure; this approach
        // is simpler and correct. K and V are cloned once per entry.
        let items: alloc::vec::Vec<_> = self
            .iter()
            .map(|(path, v)| (path.into_iter().cloned().collect(), v.clone()))
            .collect();
        TrieConsumingIter { inner: items.into_iter() }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(crate::Trie<i32, i32>: Send, Sync);

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

    #[test]
    fn hash_equal_tries_same_hash() {
        use std::collections::hash_map::DefaultHasher;
        use core::hash::{Hash, Hasher};
        let mut a: Trie<&str, i32> = Trie::new();
        a.insert(&["x", "y"], 1);
        let mut b: Trie<&str, i32> = Trie::new();
        b.insert(&["x", "y"], 1);
        assert_eq!(a, b);
        let hash = |t: &Trie<&str, i32>| {
            let mut h = DefaultHasher::new();
            t.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn from_iter_and_into_iter() {
        let entries = vec![
            (vec!["a", "b"], 1i32),
            (vec!["a", "c"], 2),
            (vec!["d"], 3),
        ];
        let trie: Trie<&str, i32> = entries.clone().into_iter().collect();
        assert_eq!(trie.get(&["a", "b"]), Some(&1));
        assert_eq!(trie.get(&["a", "c"]), Some(&2));
        assert_eq!(trie.get(&["d"]), Some(&3));

        let mut out: Vec<_> = trie.into_iter().collect();
        out.sort_by_key(|(_, v)| *v);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].1, 1);
        assert_eq!(out[1].1, 2);
        assert_eq!(out[2].1, 3);
    }

    #[test]
    fn ref_into_iter() {
        let mut trie: Trie<&str, i32> = Trie::new();
        trie.insert(&["a"], 1);
        trie.insert(&["b"], 2);
        let count = (&trie).into_iter().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn extend() {
        let mut trie: Trie<&str, i32> = Trie::new();
        trie.insert(&["a"], 1);
        trie.extend(vec![(vec!["b"], 2), (vec!["c"], 3)]);
        assert_eq!(trie.len(), 3);
        assert_eq!(trie.get(&["b"]), Some(&2));
    }

    #[test]
    fn from_vec() {
        let v = vec![(vec!["x"], 10i32), (vec!["y"], 20)];
        let trie: Trie<&str, i32> = Trie::from(v);
        assert_eq!(trie.get(&["x"]), Some(&10));
        assert_eq!(trie.get(&["y"]), Some(&20));
    }

    #[test]
    fn from_array() {
        let trie: Trie<&str, i32> = Trie::from([(vec!["a"], 1), (vec!["b"], 2)]);
        assert_eq!(trie.get(&["a"]), Some(&1));
        assert_eq!(trie.get(&["b"]), Some(&2));
    }

    #[test]
    fn from_slice() {
        let entries = [(vec!["p"], 7i32), (vec!["q"], 8)];
        let trie: Trie<&str, i32> = Trie::from(entries.as_slice());
        assert_eq!(trie.get(&["p"]), Some(&7));
        assert_eq!(trie.get(&["q"]), Some(&8));
    }

    #[test]
    fn add_union() {
        let mut a: Trie<&str, i32> = Trie::new();
        a.insert(&["x"], 1);
        let mut b: Trie<&str, i32> = Trie::new();
        b.insert(&["y"], 2);
        b.insert(&["x"], 99); // conflict — right wins
        let c = a + b;
        assert_eq!(c.get(&["x"]), Some(&99));
        assert_eq!(c.get(&["y"]), Some(&2));
    }

    #[test]
    fn add_ref() {
        let mut a: Trie<&str, i32> = Trie::new();
        a.insert(&["a"], 1);
        let mut b: Trie<&str, i32> = Trie::new();
        b.insert(&["b"], 2);
        let c = &a + &b;
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn sum() {
        let tries: Vec<Trie<&str, i32>> = vec![
            { let mut t = Trie::new(); t.insert(&["a"], 1); t },
            { let mut t = Trie::new(); t.insert(&["b"], 2); t },
            { let mut t = Trie::new(); t.insert(&["c"], 3); t },
        ];
        let total: Trie<&str, i32> = tries.into_iter().sum();
        assert_eq!(total.len(), 3);
        assert_eq!(total.get(&["b"]), Some(&2));
    }
}
