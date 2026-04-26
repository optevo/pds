// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A persistent sorted trie (prefix tree).
//!
//! An [`OrdTrie`] maps sequences of keys (paths) to values. Each node can hold a
//! value and a set of children indexed by a key segment. Backed by
//! [`OrdMap<K, OrdTrie<K, V>>`][crate::OrdMap], children at every level are stored
//! in sorted key order. Iteration visits paths in sorted lexicographic order.
//! Clone is O(1) and modifications share unchanged subtrees.
//!
//! Prefer [`OrdTrie`] over [`Trie`][crate::Trie] when:
//! - Key segments implement `Ord` but not `Hash + Eq`.
//! - You need sorted iteration over stored paths without a post-sort step.
//! - You want `PartialOrd` / `Ord` on the trie itself.
//! - You need `no_std` without the `foldhash` feature.
//!
//! # Examples
//!
//! ```
//! use pds::OrdTrie;
//!
//! let mut trie = OrdTrie::new();
//! trie.insert(&["usr", "bin", "rustc"], 1);
//! trie.insert(&["usr", "lib", "libc.so"], 2);
//! trie.insert(&["etc", "hosts"], 3);
//!
//! assert_eq!(trie.get(&["usr", "bin", "rustc"]), Some(&1));
//!
//! // Iteration is in sorted lexicographic path order.
//! let paths: Vec<_> = trie.iter().map(|(p, _)| p.into_iter().copied().collect::<Vec<_>>()).collect();
//! assert_eq!(paths, vec![
//!     vec!["etc", "hosts"],
//!     vec!["usr", "bin", "rustc"],
//!     vec!["usr", "lib", "libc.so"],
//! ]);
//! ```

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::{Debug, Error, Formatter};
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;
use core::ops::{Index, IndexMut};

use archery::SharedPointerKind;
use equivalent::Comparable;

use crate::ordmap::GenericOrdMap;
use crate::shared_ptr::DefaultSharedPtr;

/// Type alias for [`GenericOrdTrie`] with the default pointer type.
pub type OrdTrie<K, V> = GenericOrdTrie<K, V, DefaultSharedPtr>;

/// A persistent sorted trie (prefix tree) backed by [`GenericOrdMap`].
///
/// Keys are sequences of segments of type `K`. Values of type `V` can be stored
/// at any node (interior or leaf). Clone is O(1) via structural sharing.
///
/// Children at every level are kept in sorted key order, so iteration visits
/// paths in sorted lexicographic order. Unlike [`Trie`][crate::Trie], this type
/// requires only `K: Ord + Clone` — no `Hash + Eq` constraint.
///
/// # Performance
///
/// For a path of length *d*, operations are O(d × log n) where *n* is the
/// fanout at each level.
pub struct GenericOrdTrie<K, V, P: SharedPointerKind = DefaultSharedPtr> {
    pub(crate) value: Option<V>,
    pub(crate) children: GenericOrdMap<K, GenericOrdTrie<K, V, P>, P>,
}

// Manual Clone — avoid spurious `P: Clone` bound from derive.
impl<K: Clone, V: Clone, P: SharedPointerKind> Clone for GenericOrdTrie<K, V, P> {
    fn clone(&self) -> Self {
        GenericOrdTrie {
            value: self.value.clone(),
            children: self.children.clone(),
        }
    }
}

impl<K, V, P: SharedPointerKind> GenericOrdTrie<K, V, P> {
    /// Create an empty trie.
    #[must_use]
    pub fn new() -> Self {
        GenericOrdTrie {
            value: None,
            children: GenericOrdMap::new(),
        }
    }

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

impl<K: Ord, V, P: SharedPointerKind> GenericOrdTrie<K, V, P> {
    /// Get a reference to the subtrie at the given path.
    #[must_use]
    pub fn subtrie<Q>(&self, path: &[Q]) -> Option<&Self>
    where
        Q: Comparable<K>,
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
        Q: Comparable<K>,
    {
        self.subtrie(path)?.value()
    }

    /// Test whether a value exists at the given path.
    #[must_use]
    pub fn contains_path<Q>(&self, path: &[Q]) -> bool
    where
        Q: Comparable<K>,
    {
        self.get(path).is_some()
    }

    /// Iterate over all (path, value) pairs in sorted lexicographic path order.
    ///
    /// Paths are returned as `Vec<&K>` segments. Because children at every level
    /// are stored in a sorted [`OrdMap`][crate::OrdMap], the traversal order is
    /// deterministic and lexicographic.
    pub fn iter(&self) -> OrdTrieIter<'_, K, V, P> {
        let stack = alloc::vec![(alloc::vec::Vec::new(), self)];
        OrdTrieIter { stack }
    }

    /// Return the number of values stored in the trie (at all depths).
    #[must_use]
    pub fn len(&self) -> usize {
        let own = usize::from(self.value.is_some());
        own + self
            .children
            .iter()
            .map(|(_, child)| child.len())
            .sum::<usize>()
    }

    /// Iterate over all paths that share the given prefix, returning
    /// (remaining_path, value) pairs in sorted order.
    pub fn iter_prefix<'a, Q>(&'a self, prefix: &[Q]) -> Option<OrdTrieIter<'a, K, V, P>>
    where
        Q: Comparable<K>,
    {
        let subtrie = self.subtrie(prefix)?;
        Some(subtrie.iter())
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> GenericOrdTrie<K, V, P> {
    /// Get a mutable reference to the value at the given path.
    #[must_use]
    pub fn get_mut(&mut self, path: &[K]) -> Option<&mut V> {
        if path.is_empty() {
            return self.value.as_mut();
        }
        self.children
            .get_mut(&path[0])
            .and_then(|child| child.get_mut(&path[1..]))
    }

    /// Insert a value at the given path, returning the previous value.
    pub fn insert(&mut self, path: &[K], value: V) -> Option<V> {
        if path.is_empty() {
            return self.value.replace(value);
        }
        let child = self
            .children
            .entry(path[0].clone())
            .or_insert_with(|| GenericOrdTrie {
                value: None,
                children: GenericOrdMap::new(),
            });
        child.insert(&path[1..], value)
    }

    /// Remove the value at the given path, returning it if present.
    ///
    /// Does not remove empty interior nodes — the trie structure is preserved.
    /// Use [`remove_and_prune`][Self::remove_and_prune] to clean up empty subtrees.
    pub fn remove<Q>(&mut self, path: &[Q]) -> Option<V>
    where
        Q: Comparable<K>,
    {
        if path.is_empty() {
            return self.value.take();
        }
        let child = self.children.get_mut(&path[0])?;
        child.remove(&path[1..])
    }

    /// Remove a direct child by key, returning it if present.
    fn remove_child<Q>(&mut self, key: &Q) -> Option<GenericOrdTrie<K, V, P>>
    where
        Q: Comparable<K>,
    {
        self.children.remove(key)
    }

    /// Remove the value at the given path and prune empty nodes.
    ///
    /// After removing the value, walks back up the path and removes any nodes
    /// that have no value and no children.
    pub fn remove_and_prune<Q>(&mut self, path: &[Q]) -> Option<V>
    where
        Q: Comparable<K>,
    {
        self.remove_and_prune_inner(path)
    }

    fn remove_and_prune_inner<Q>(&mut self, path: &[Q]) -> Option<V>
    where
        Q: Comparable<K>,
    {
        if path.is_empty() {
            return self.value.take();
        }
        let result = {
            let child = self.children.get_mut(&path[0])?;
            child.remove_and_prune_inner(&path[1..])
        };
        // Prune empty child.
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
        let keys_to_remove: Vec<K> = self
            .children
            .iter()
            .filter_map(|(k, v)| if v.is_empty() { Some(k.clone()) } else { None })
            .collect();
        for key in &keys_to_remove {
            self.remove_child(key);
        }
    }

    /// Return the union of two tries; when a path exists in both, `other`'s value wins.
    #[must_use]
    pub fn union(mut self, other: Self) -> Self {
        for (path, value) in other {
            self.insert(&path, value);
        }
        self
    }

    /// Return entries whose paths are in `self` but not in `other`.
    #[must_use]
    pub fn difference(self, other: &Self) -> Self {
        self.into_iter()
            .filter(|(path, _)| !other.contains_path(path.as_slice()))
            .collect()
    }

    /// Return entries whose paths are in both `self` and `other`; `self`'s values are kept.
    #[must_use]
    pub fn intersection(self, other: &Self) -> Self {
        self.into_iter()
            .filter(|(path, _)| other.contains_path(path.as_slice()))
            .collect()
    }

    /// Return entries whose paths are in exactly one of `self` or `other`.
    #[must_use]
    pub fn symmetric_difference(self, other: &Self) -> Self {
        // Clone self — O(1) via structural sharing — to check membership after consuming.
        let self_clone = self.clone();
        let self_diff: Self = self
            .into_iter()
            .filter(|(path, _)| !other.contains_path(path.as_slice()))
            .collect();
        let other_diff: Self = other
            .clone()
            .into_iter()
            .filter(|(path, _)| !self_clone.contains_path(path.as_slice()))
            .collect();
        self_diff.union(other_diff)
    }
}

impl<K: Ord, V, P: SharedPointerKind> Default for GenericOrdTrie<K, V, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord, V: PartialEq, P: SharedPointerKind> PartialEq for GenericOrdTrie<K, V, P> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.children == other.children
    }
}

impl<K: Ord, V: Eq, P: SharedPointerKind> Eq for GenericOrdTrie<K, V, P> {}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> PartialOrd for GenericOrdTrie<K, V, P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord + Clone, V: Ord + Clone, P: SharedPointerKind> Ord for GenericOrdTrie<K, V, P> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare by sorted lexicographic (path, value) iteration — deterministic
        // because children at every level are stored in sorted OrdMap order.
        self.iter().cmp(other.iter())
    }
}

impl<K: Ord + Hash, V: Hash, P: SharedPointerKind> Hash for GenericOrdTrie<K, V, P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
        // OrdMap iterates in sorted key order, so children.hash() is canonical
        // and two equal tries always produce the same hash.
        self.children.hash(state);
    }
}

impl<K: Ord + Clone + Debug, V: Debug + Clone, P: SharedPointerKind> Debug
    for GenericOrdTrie<K, V, P>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut d = f.debug_struct("OrdTrie");
        if let Some(v) = &self.value {
            d.field("value", v);
        }
        if !self.children.is_empty() {
            d.field("children", &self.children);
        }
        d.finish()
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Extend<(Vec<K>, V)>
    for GenericOrdTrie<K, V, P>
{
    fn extend<I: IntoIterator<Item = (Vec<K>, V)>>(&mut self, iter: I) {
        for (path, value) in iter {
            self.insert(&path, value);
        }
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> FromIterator<(Vec<K>, V)>
    for GenericOrdTrie<K, V, P>
{
    fn from_iter<I: IntoIterator<Item = (Vec<K>, V)>>(iter: I) -> Self {
        let mut trie = Self::new();
        trie.extend(iter);
        trie
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> From<Vec<(Vec<K>, V)>>
    for GenericOrdTrie<K, V, P>
{
    fn from(v: Vec<(Vec<K>, V)>) -> Self {
        v.into_iter().collect()
    }
}

impl<K: Ord + Clone, V: Clone, const N: usize, P: SharedPointerKind> From<[(Vec<K>, V); N]>
    for GenericOrdTrie<K, V, P>
{
    fn from(arr: [(Vec<K>, V); N]) -> Self {
        arr.into_iter().collect()
    }
}

impl<'a, K: Ord + Clone, V: Clone, P: SharedPointerKind> From<&'a [(Vec<K>, V)]>
    for GenericOrdTrie<K, V, P>
{
    fn from(slice: &'a [(Vec<K>, V)]) -> Self {
        slice.iter().map(|(p, v)| (p.clone(), v.clone())).collect()
    }
}

impl<'a, K: Ord + Clone, V: Clone, P: SharedPointerKind> From<&'a Vec<(Vec<K>, V)>>
    for GenericOrdTrie<K, V, P>
{
    fn from(v: &'a Vec<(Vec<K>, V)>) -> Self {
        v.iter().map(|(p, val)| (p.clone(), val.clone())).collect()
    }
}

/// Index by path, panicking if the path has no associated value.
impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Index<&[K]> for GenericOrdTrie<K, V, P> {
    type Output = V;

    fn index(&self, path: &[K]) -> &Self::Output {
        match self.get(path) {
            Some(v) => v,
            None => panic!("OrdTrie::index: path not found"),
        }
    }
}

/// Index mutably by path, panicking if the path has no associated value.
impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> IndexMut<&[K]> for GenericOrdTrie<K, V, P> {
    fn index_mut(&mut self, path: &[K]) -> &mut Self::Output {
        match self.get_mut(path) {
            Some(v) => v,
            None => panic!("OrdTrie::index_mut: path not found"),
        }
    }
}

/// Iterator over (path, value) pairs in a [`GenericOrdTrie`] in sorted lexicographic order.
pub struct OrdTrieIter<'a, K, V, P: SharedPointerKind> {
    stack: Vec<(Vec<&'a K>, &'a GenericOrdTrie<K, V, P>)>,
}

impl<'a, K: Ord, V, P: SharedPointerKind> Iterator for OrdTrieIter<'a, K, V, P> {
    type Item = (Vec<&'a K>, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (path, node) = self.stack.pop()?;
            // Push children in reverse sorted order so the first (smallest) child is
            // popped first, producing sorted lexicographic DFS traversal.
            let children: Vec<_> = node.children.iter().collect();
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

impl<'a, K: Ord, V, P: SharedPointerKind> IntoIterator for &'a GenericOrdTrie<K, V, P> {
    type Item = (Vec<&'a K>, &'a V);
    type IntoIter = OrdTrieIter<'a, K, V, P>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Owning iterator over (path, value) pairs produced by consuming an [`OrdTrie`].
///
/// Paths are `Vec<K>` in sorted lexicographic order.
pub struct OrdTrieConsumingIter<K, V> {
    inner: alloc::vec::IntoIter<(Vec<K>, V)>,
}

impl<K, V> Iterator for OrdTrieConsumingIter<K, V> {
    type Item = (Vec<K>, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> IntoIterator for GenericOrdTrie<K, V, P> {
    type Item = (Vec<K>, V);
    type IntoIter = OrdTrieConsumingIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        // Eagerly collect all (path, value) pairs in sorted order.
        // A zero-copy consuming iter would require draining the recursive OrdMap
        // structure; this approach is correct and K/V are cloned once per entry.
        let items: Vec<_> = self
            .iter()
            .map(|(path, v)| (path.into_iter().cloned().collect(), v.clone()))
            .collect();
        OrdTrieConsumingIter {
            inner: items.into_iter(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(OrdTrie<i32, i32>: Send, Sync);

    #[test]
    fn empty_trie() {
        let trie: OrdTrie<&str, i32> = OrdTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
        assert_eq!(trie.get::<&str>(&[]), None);
    }

    #[test]
    fn insert_and_get() {
        let mut trie = OrdTrie::new();
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
        let mut trie = OrdTrie::new();
        trie.insert(&[] as &[&str], 42);
        assert_eq!(trie.get(&[] as &[&str]), Some(&42));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn insert_overwrites() {
        let mut trie = OrdTrie::new();
        let old = trie.insert(&["a"], 1);
        assert_eq!(old, None);
        let old = trie.insert(&["a"], 2);
        assert_eq!(old, Some(1));
        assert_eq!(trie.get(&["a"]), Some(&2));
    }

    #[test]
    fn remove() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a", "b"], 1);
        trie.insert(&["a", "c"], 2);

        let removed = trie.remove(&["a", "b"]);
        assert_eq!(removed, Some(1));
        assert_eq!(trie.get(&["a", "b"]), None);
        assert_eq!(trie.get(&["a", "c"]), Some(&2));
    }

    #[test]
    fn remove_absent() {
        let mut trie: OrdTrie<&str, i32> = OrdTrie::new();
        assert_eq!(trie.remove(&["x"]), None);
    }

    #[test]
    fn remove_and_prune() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a", "b", "c"], 1);
        trie.remove_and_prune(&["a", "b", "c"]);
        assert!(trie.is_empty());
    }

    #[test]
    fn contains_path() {
        let mut trie = OrdTrie::new();
        trie.insert(&["x", "y"], 1);
        assert!(trie.contains_path(&["x", "y"]));
        assert!(!trie.contains_path(&["x"]));
        assert!(!trie.contains_path(&["z"]));
    }

    #[test]
    fn subtrie() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a", "b"], 1);
        trie.insert(&["a", "c"], 2);

        let sub = trie.subtrie(&["a"]).unwrap();
        assert_eq!(sub.get(&["b"]), Some(&1));
        assert_eq!(sub.get(&["c"]), Some(&2));
    }

    #[test]
    fn len() {
        let mut trie = OrdTrie::new();
        assert_eq!(trie.len(), 0);
        trie.insert(&["a"], 1);
        assert_eq!(trie.len(), 1);
        trie.insert(&["a", "b"], 2);
        assert_eq!(trie.len(), 2);
        trie.insert(&["c"], 3);
        assert_eq!(trie.len(), 3);
    }

    #[test]
    fn iter_is_sorted_lexicographic() {
        let mut trie = OrdTrie::new();
        trie.insert(&["usr", "lib"], 2);
        trie.insert(&["usr", "bin"], 1);
        trie.insert(&["etc", "hosts"], 3);

        // K = &str, so iter() produces Vec<&&str>; copied() dereferences to Vec<&str>.
        let pairs: Vec<(Vec<&str>, i32)> = trie
            .iter()
            .map(|(path, v)| (path.into_iter().copied().collect(), *v))
            .collect();
        assert_eq!(pairs, vec![
            (vec!["etc", "hosts"], 3),
            (vec!["usr", "bin"], 1),
            (vec!["usr", "lib"], 2),
        ]);
    }

    #[test]
    fn iter_prefix() {
        let mut trie = OrdTrie::new();
        trie.insert(&["usr", "bin", "rustc"], 1);
        trie.insert(&["usr", "lib", "libc"], 2);
        trie.insert(&["etc", "hosts"], 3);

        let result: Vec<(Vec<&str>, i32)> = trie.iter_prefix(&["usr"]).unwrap()
            .map(|(path, v)| (path.into_iter().copied().collect(), *v))
            .collect();
        assert_eq!(result, vec![
            (vec!["bin", "rustc"], 1),
            (vec!["lib", "libc"], 2),
        ]);
    }

    #[test]
    fn clone_shares_structure() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a", "b"], 1);
        let trie2 = trie.clone();
        assert_eq!(trie, trie2);
    }

    #[test]
    fn default_is_empty() {
        let trie: OrdTrie<&str, i32> = OrdTrie::default();
        assert!(trie.is_empty());
    }

    #[test]
    fn equality() {
        let mut t1 = OrdTrie::new();
        t1.insert(&["a"], 1);
        let mut t2 = OrdTrie::new();
        t2.insert(&["a"], 1);
        assert_eq!(t1, t2);
    }

    #[test]
    fn inequality() {
        let mut t1 = OrdTrie::new();
        t1.insert(&["a"], 1);
        let mut t2 = OrdTrie::new();
        t2.insert(&["b"], 2);
        assert_ne!(t1, t2);
    }

    #[test]
    fn ord_comparison() {
        let mut t1: OrdTrie<&str, i32> = OrdTrie::new();
        t1.insert(&["a"], 1);
        let mut t2: OrdTrie<&str, i32> = OrdTrie::new();
        t2.insert(&["b"], 2);
        assert!(t1 < t2);
    }

    #[test]
    fn hash_stable() {
        use std::hash::DefaultHasher;

        let mut t1 = OrdTrie::new();
        t1.insert(&["a", "b"], 1i32);
        t1.insert(&["a", "c"], 2);

        let mut t2 = OrdTrie::new();
        t2.insert(&["a", "c"], 2i32);
        t2.insert(&["a", "b"], 1);

        assert_eq!(t1, t2);
        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        t1.hash(&mut h1);
        t2.hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn from_vec() {
        let trie: OrdTrie<&str, i32> = OrdTrie::from(vec![
            (vec!["a"], 1),
            (vec!["b"], 2),
        ]);
        assert_eq!(trie.get(&["a"]), Some(&1));
        assert_eq!(trie.get(&["b"]), Some(&2));
    }

    #[test]
    fn from_array() {
        let trie: OrdTrie<&str, i32> = OrdTrie::from([
            (vec!["a"], 1),
            (vec!["b"], 2),
        ]);
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn from_slice() {
        let v = vec![(vec!["a"], 1i32), (vec!["b"], 2)];
        let trie: OrdTrie<&str, i32> = OrdTrie::from(v.as_slice());
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn from_vec_ref() {
        let v = vec![(vec!["a"], 1i32), (vec!["b"], 2)];
        let trie: OrdTrie<&str, i32> = OrdTrie::from(&v);
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn from_iterator() {
        let trie: OrdTrie<&str, i32> = vec![
            (vec!["a"], 1),
            (vec!["b"], 2),
        ]
        .into_iter()
        .collect();
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn extend() {
        let mut trie: OrdTrie<&str, i32> = OrdTrie::new();
        trie.extend(vec![(vec!["a"], 1), (vec!["b"], 2)]);
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn into_iter_consuming() {
        let mut trie = OrdTrie::new();
        trie.insert(&["b"], 2i32);
        trie.insert(&["a"], 1);

        let pairs: Vec<_> = trie.into_iter().collect();
        assert_eq!(pairs, vec![
            (vec!["a"], 1),
            (vec!["b"], 2),
        ]);
    }

    #[test]
    fn into_iter_ref() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a"], 1i32);

        let pairs: Vec<(Vec<&str>, i32)> = (&trie)
            .into_iter()
            .map(|(p, v)| (p.into_iter().copied().collect(), *v))
            .collect();
        assert_eq!(pairs, vec![(vec!["a"], 1)]);
    }

    #[test]
    fn index() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a", "b"], 42i32);
        assert_eq!(trie[&["a", "b"][..]], 42);
    }

    #[test]
    fn index_mut() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a"], 1i32);
        trie[&["a"][..]] = 99;
        assert_eq!(trie.get(&["a"]), Some(&99));
    }

    #[test]
    #[should_panic]
    fn index_panics_on_missing() {
        let trie: OrdTrie<&str, i32> = OrdTrie::new();
        let _ = trie[&["x"][..]];
    }

    #[test]
    fn union() {
        let mut t1 = OrdTrie::new();
        t1.insert(&["a"], 1i32);

        let mut t2 = OrdTrie::new();
        t2.insert(&["b"], 2i32);

        let combined = t1.union(t2);
        assert_eq!(combined.len(), 2);
        assert_eq!(combined.get(&["a"]), Some(&1));
        assert_eq!(combined.get(&["b"]), Some(&2));
    }

    #[test]
    fn difference() {
        let mut t1 = OrdTrie::new();
        t1.insert(&["a"], 1i32);
        t1.insert(&["b"], 2);

        let mut t2 = OrdTrie::new();
        t2.insert(&["b"], 2i32);

        let diff = t1.difference(&t2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.get(&["a"]), Some(&1));
    }

    #[test]
    fn intersection() {
        let mut t1 = OrdTrie::new();
        t1.insert(&["a"], 1i32);
        t1.insert(&["b"], 2);

        let mut t2 = OrdTrie::new();
        t2.insert(&["b"], 99i32);
        t2.insert(&["c"], 3);

        let inter = t1.intersection(&t2);
        assert_eq!(inter.len(), 1);
        assert_eq!(inter.get(&["b"]), Some(&2)); // self's value kept
    }

    #[test]
    fn symmetric_difference() {
        let mut t1 = OrdTrie::new();
        t1.insert(&["a"], 1i32);
        t1.insert(&["b"], 2);

        let mut t2 = OrdTrie::new();
        t2.insert(&["b"], 99i32);
        t2.insert(&["c"], 3);

        let sd = t1.symmetric_difference(&t2);
        assert_eq!(sd.len(), 2);
        assert!(sd.contains_path(&["a"]));
        assert!(sd.contains_path(&["c"]));
        assert!(!sd.contains_path(&["b"]));
    }

    #[test]
    fn prune_cleans_empty_nodes() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a", "b"], 1i32);
        trie.remove(&["a", "b"]);
        // "a" node still exists but has no value and no children with values.
        trie.prune();
        assert!(trie.is_empty());
    }

    #[test]
    fn value_and_child_count() {
        let mut trie = OrdTrie::new();
        trie.insert(&[] as &[&str], 1i32);
        assert_eq!(trie.value(), Some(&1));

        trie.insert(&["a"], 2);
        trie.insert(&["b"], 3);
        assert_eq!(trie.child_count(), 2);
    }

    #[test]
    fn get_mut() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a"], 1i32);
        *trie.get_mut(&["a"]).unwrap() = 99;
        assert_eq!(trie.get(&["a"]), Some(&99));
    }

    #[test]
    fn debug_format() {
        let mut trie = OrdTrie::new();
        trie.insert(&["a"], 1i32);
        let s = format!("{:?}", trie);
        assert!(s.contains("OrdTrie"));
    }
}
