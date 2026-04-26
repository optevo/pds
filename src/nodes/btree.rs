// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::iter::{FromIterator, FusedIterator};
use core::mem;
use core::num::NonZeroUsize;
use core::ops::{Bound, RangeBounds};

use archery::{SharedPointer, SharedPointerKind};
use equivalent::Comparable;
use imbl_sized_chunks::Chunk;

pub(crate) use crate::config::ORD_CHUNK_SIZE as NODE_SIZE;

const MEDIAN: usize = NODE_SIZE / 2;
const THIRD: usize = NODE_SIZE / 3;
const NUM_CHILDREN: usize = NODE_SIZE + 1;

/// A node in a `B+Tree`.
///
/// The main tree representation uses [`Branch`] and [`Leaf`]; this is only used
/// in places that want to handle either a branch or a leaf.
#[derive(Debug)]
pub(crate) enum Node<K, V, P: SharedPointerKind> {
    Branch(SharedPointer<Branch<K, V, P>, P>),
    Leaf(SharedPointer<Leaf<K, V>, P>),
}

impl<K: Ord + core::fmt::Debug, V: core::fmt::Debug, P: SharedPointerKind> Branch<K, V, P> {
    #[cfg(any(test, fuzzing))]
    pub(crate) fn check_sane(&self, is_root: bool) -> usize {
        assert!(self.keys.len() >= if is_root { 1 } else { MEDIAN - 1 });
        assert_eq!(self.keys.len() + 1, self.children.len());
        assert!(self.keys.windows(2).all(|w| w[0] < w[1]));
        match &self.children {
            Children::Leaves { leaves } => {
                for i in 0..self.keys.len() {
                    let left = &leaves[i];
                    let right = &leaves[i + 1];
                    assert!(left.keys.last().unwrap().0 < right.keys.first().unwrap().0);
                }
                leaves.iter().map(|child| child.check_sane(false)).sum()
            }
            Children::Branches { branches, level } => {
                for i in 0..self.keys.len() {
                    let left = &branches[i];
                    let right = &branches[i + 1];
                    assert!(left.level() == level.get() - 1);
                    assert!(right.level() == level.get() - 1);
                }
                branches.iter().map(|child| child.check_sane(false)).sum()
            }
        }
    }
}
impl<K: Ord + core::fmt::Debug, V: core::fmt::Debug> Leaf<K, V> {
    #[cfg(any(test, fuzzing))]
    pub(crate) fn check_sane(&self, is_root: bool) -> usize {
        assert!(self.keys.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(self.keys.len() >= if is_root { 0 } else { THIRD });
        self.keys.len()
    }
}
impl<K: Ord + core::fmt::Debug, V: core::fmt::Debug, P: SharedPointerKind> Node<K, V, P> {
    /// Check invariants
    #[cfg(any(test, fuzzing))]
    pub(crate) fn check_sane(&self, is_root: bool) -> usize {
        match self {
            Node::Branch(branch) => branch.check_sane(is_root),
            Node::Leaf(leaf) => leaf.check_sane(is_root),
        }
    }
}

impl<K, V, P: SharedPointerKind> Node<K, V, P> {
    pub(crate) fn unit(key: K, value: V) -> Self {
        Node::Leaf(SharedPointer::new(Leaf {
            keys: Chunk::unit((key, value)),
        }))
    }

    fn level(&self) -> usize {
        match self {
            Node::Branch(branch) => branch.level(),
            Node::Leaf(_) => 0,
        }
    }

    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Node::Branch(a), Node::Branch(b)) => SharedPointer::ptr_eq(a, b),
            (Node::Leaf(a), Node::Leaf(b)) => SharedPointer::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Return a new tree node with all values transformed by `f(key, value)`.
    ///
    /// Keys are unchanged, so tree topology (key comparisons, ordering) is
    /// preserved exactly. Used by `par_map_values` in `ord/rayon.rs`.
    #[cfg(any(test, feature = "rayon"))]
    pub(crate) fn map_values<V2, F>(&self, f: &F) -> Node<K, V2, P>
    where
        K: Clone,
        V2: Clone,
        F: Fn(&K, &V) -> V2,
    {
        match self {
            Node::Branch(branch) => Node::Branch(SharedPointer::new(branch.map_values(f))),
            Node::Leaf(leaf) => Node::Leaf(SharedPointer::new(leaf.map_values(f))),
        }
    }
}

/// A branch node in a `B+Tree`.
/// Invariants:
/// * keys are ordered and unique
/// * keys.len() + 1 == children.len()
/// * all children have level = level - 1 (or level is 1 and all children are leaves)
/// * all keys in the subtree at children[i] are between keys[i - 1] (if i > 0) and keys[i] (if i < keys.len()).
/// * root branch must have at least 1 key, whereas non-root branches must have at least MEDIAN - 1 keys
#[derive(Debug)]
pub(crate) struct Branch<K, V, P: SharedPointerKind> {
    pub(crate) keys: Chunk<K, NODE_SIZE>,
    pub(crate) children: Children<K, V, P>,
}

#[derive(Debug)]
pub(crate) enum Children<K, V, P: SharedPointerKind> {
    /// implicitly level 1
    Leaves {
        leaves: Chunk<SharedPointer<Leaf<K, V>, P>, NUM_CHILDREN>,
    },
    /// level >= 2
    Branches {
        branches: Chunk<SharedPointer<Branch<K, V, P>, P>, NUM_CHILDREN>,
        /// The level of the tree node that contains these children.
        ///
        /// Leaves have level zero, so branches have level at least one. Since this is the
        /// level of something containing branches, it is at least two.
        level: NonZeroUsize,
    },
}

impl<K, V, P: SharedPointerKind> Children<K, V, P> {
    fn len(&self) -> usize {
        match self {
            Children::Leaves { leaves } => leaves.len(),
            Children::Branches { branches, .. } => branches.len(),
        }
    }
    fn drain_from_front(&mut self, other: &mut Self, count: usize) {
        match (self, other) {
            (
                Children::Leaves { leaves },
                Children::Leaves {
                    leaves: other_leaves,
                },
            ) => leaves.drain_from_front(other_leaves, count),
            (
                Children::Branches { branches, .. },
                Children::Branches {
                    branches: other_branches,
                    ..
                },
            ) => branches.drain_from_front(other_branches, count),
            _ => panic!("mismatched drain_from_front"),
        }
    }
    fn drain_from_back(&mut self, other: &mut Self, count: usize) {
        match (self, other) {
            (
                Children::Leaves { leaves },
                Children::Leaves {
                    leaves: other_leaves,
                },
            ) => leaves.drain_from_back(other_leaves, count),
            (
                Children::Branches { branches, .. },
                Children::Branches {
                    branches: other_branches,
                    ..
                },
            ) => branches.drain_from_back(other_branches, count),
            _ => panic!("mismatched drain_from_back"),
        }
    }
    fn extend(&mut self, other: &Self) {
        match (self, other) {
            (
                Children::Leaves { leaves },
                Children::Leaves {
                    leaves: other_leaves,
                },
            ) => leaves.extend(other_leaves.iter().cloned()),
            (
                Children::Branches { branches, .. },
                Children::Branches {
                    branches: other_branches,
                    ..
                },
            ) => branches.extend(other_branches.iter().cloned()),
            _ => panic!("mismatched extend"),
        }
    }
    fn insert_front(&mut self, other: &Self) {
        match (self, other) {
            (
                Children::Leaves { leaves },
                Children::Leaves {
                    leaves: other_leaves,
                },
            ) => leaves.insert_from(0, other_leaves.iter().cloned()),
            (
                Children::Branches { branches, .. },
                Children::Branches {
                    branches: other_branches,
                    ..
                },
            ) => branches.insert_from(0, other_branches.iter().cloned()),
            _ => panic!("mismatched insert_front"),
        }
    }
    fn insert(&mut self, index: usize, node: Node<K, V, P>) {
        match (self, node) {
            (Children::Leaves { leaves }, Node::Leaf(node)) => leaves.insert(index, node),
            (Children::Branches { branches, .. }, Node::Branch(node)) => {
                branches.insert(index, node)
            }
            _ => panic!("mismatched insert"),
        }
    }
    fn split_off(&mut self, at: usize) -> Self {
        match self {
            Children::Leaves { leaves } => Children::Leaves {
                leaves: leaves.split_off(at),
            },
            Children::Branches { branches, level } => Children::Branches {
                branches: branches.split_off(at),
                level: *level,
            },
        }
    }
}

impl<K, V, P: SharedPointerKind> Branch<K, V, P> {
    pub(crate) fn pop_single_child(&mut self) -> Option<Node<K, V, P>> {
        if self.children.len() == 1 {
            debug_assert_eq!(self.keys.len(), 0);
            Some(match &mut self.children {
                Children::Leaves { leaves } => Node::Leaf(leaves.pop_back()),
                Children::Branches { branches, .. } => Node::Branch(branches.pop_back()),
            })
        } else {
            None
        }
    }

    fn level(&self) -> usize {
        match &self.children {
            Children::Leaves { .. } => 1,
            Children::Branches { level, .. } => level.get(),
        }
    }
}

/// A leaf node in a `B+Tree`.
///
/// Invariants:
/// * keys are ordered and unique
/// * leaf is the lowest level in the tree (level 0)
/// * non-root leaves must have at least THIRD keys
#[derive(Debug)]
pub(crate) struct Leaf<K, V> {
    pub(crate) keys: Chunk<(K, V), NODE_SIZE>,
}

impl<K: Clone, V, P: SharedPointerKind> Branch<K, V, P> {
    /// Return a new branch with the same separator keys and all values
    /// transformed by `f`. Tree structure and key ordering are preserved.
    #[cfg(any(test, feature = "rayon"))]
    pub(crate) fn map_values<V2, F>(&self, f: &F) -> Branch<K, V2, P>
    where
        V2: Clone,
        F: Fn(&K, &V) -> V2,
    {
        let new_children = match &self.children {
            Children::Leaves { leaves } => {
                let mut new_leaves: Chunk<SharedPointer<Leaf<K, V2>, P>, NUM_CHILDREN> =
                    Chunk::new();
                for leaf_ptr in leaves.as_slice() {
                    new_leaves.push_back(SharedPointer::new(leaf_ptr.map_values(f)));
                }
                Children::Leaves { leaves: new_leaves }
            }
            Children::Branches { branches, level } => {
                let mut new_branches: Chunk<SharedPointer<Branch<K, V2, P>, P>, NUM_CHILDREN> =
                    Chunk::new();
                for branch_ptr in branches.as_slice() {
                    new_branches.push_back(SharedPointer::new(branch_ptr.map_values(f)));
                }
                Children::Branches {
                    branches: new_branches,
                    level: *level,
                }
            }
        };
        Branch {
            keys: self.keys.clone(),
            children: new_children,
        }
    }
}

impl<K: Clone, V> Leaf<K, V> {
    /// Return a new leaf with the same keys and all values transformed by `f`.
    #[cfg(any(test, feature = "rayon"))]
    pub(crate) fn map_values<V2, F>(&self, f: &F) -> Leaf<K, V2>
    where
        V2: Clone,
        F: Fn(&K, &V) -> V2,
    {
        let mut new_keys: Chunk<(K, V2), NODE_SIZE> = Chunk::new();
        for (k, v) in self.keys.as_slice() {
            new_keys.push_back((k.clone(), f(k, v)));
        }
        Leaf { keys: new_keys }
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Node<K, V, P> {
    /// Removes a key from the node or its children.
    /// Returns `true` if the node is underflowed and should be rebalanced.
    pub(crate) fn remove<Q>(&mut self, key: &Q, removed: &mut Option<(K, V)>) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        match self {
            Node::Branch(branch) => SharedPointer::make_mut(branch).remove(key, removed),
            Node::Leaf(leaf) => SharedPointer::make_mut(leaf).remove(key, removed),
        }
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Branch<K, V, P> {
    pub(crate) fn remove<Q>(&mut self, key: &Q, removed: &mut Option<(K, V)>) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        let i = slice_ext::binary_search_by(&self.keys, |k| key.compare(k).reverse())
            .map(|x| x + 1)
            .unwrap_or_else(|x| x);
        let rebalance = match &mut self.children {
            Children::Leaves { leaves } => {
                SharedPointer::make_mut(&mut leaves[i]).remove(key, removed)
            }
            Children::Branches { branches, .. } => {
                SharedPointer::make_mut(&mut branches[i]).remove(key, removed)
            }
        };
        if rebalance {
            self.branch_rebalance_children(i);
        }
        // Underflow if the branch is < 1/2 full. Since the branches are relatively
        // rarely rebalanced (given relaxed leaf underflow), we can afford to be
        // a bit more conservative here.
        self.keys.len() < MEDIAN
    }
}

impl<K: Ord + Clone, V: Clone> Leaf<K, V> {
    pub(crate) fn remove<Q>(&mut self, key: &Q, removed: &mut Option<(K, V)>) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        if let Ok(i) = slice_ext::binary_search_by(&self.keys, |(k, _)| key.compare(k).reverse()) {
            *removed = Some(self.keys.remove(i));
        }
        // Underflow if the leaf is < 1/3 full. This relaxed underflow (vs. 1/2 full) is
        // useful to prevent degenerate cases where a random insert/remove workload will
        // constantly merge/split a leaf.
        self.keys.len() < THIRD
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Branch<K, V, P> {
    #[cold]
    pub(crate) fn branch_rebalance_children(&mut self, underflow_idx: usize) {
        let left_idx = underflow_idx.saturating_sub(1);
        match &mut self.children {
            Children::Leaves { leaves } => {
                let (left, mid, right) = match &leaves[left_idx..] {
                    [left, mid, right, ..] => (&**left, &**mid, Some(&**right)),
                    [left, mid, ..] => (&**left, &**mid, None),
                    _ => return,
                };
                // Prefer merging two sibling children if we can fit them into a single node.
                // But also try to rebalance if the smallest child is small (< 1/3), to amortize the cost of rebalancing.
                // Since we prefer merging, for rebalancing to apply the the largest child will be least 2/3 full,
                // which results in two at least half full nodes after rebalancing.
                match (left, mid, right) {
                    (left, mid, _) if left.keys.len() + mid.keys.len() <= NODE_SIZE => {
                        Self::merge_leaves(leaves, &mut self.keys, left_idx, false);
                    }
                    (_, mid, Some(right)) if mid.keys.len() + right.keys.len() <= NODE_SIZE => {
                        Self::merge_leaves(leaves, &mut self.keys, left_idx + 1, true);
                    }
                    (left, mid, _) if mid.keys.len().min(left.keys.len()) < THIRD => {
                        Self::rebalance_leaves(leaves, &mut self.keys, left_idx);
                    }
                    (_, mid, Some(right)) if mid.keys.len().min(right.keys.len()) < THIRD => {
                        Self::rebalance_leaves(leaves, &mut self.keys, left_idx + 1);
                    }
                    _ => (),
                }
            }
            Children::Branches { branches, .. } => {
                let (left, mid, right) = match &branches[left_idx..] {
                    [left, mid, right, ..] => (&**left, &**mid, Some(&**right)),
                    [left, mid, ..] => (&**left, &**mid, None),
                    _ => return,
                };
                match (left, mid, right) {
                    (left, mid, _) if left.keys.len() + mid.keys.len() < NODE_SIZE => {
                        Self::merge_branches(branches, &mut self.keys, left_idx, false);
                    }
                    (_, mid, Some(right)) if mid.keys.len() + right.keys.len() < NODE_SIZE => {
                        Self::merge_branches(branches, &mut self.keys, left_idx + 1, true);
                    }
                    (left, mid, _) if mid.keys.len().min(left.keys.len()) < THIRD => {
                        Self::rebalance_branches(branches, &mut self.keys, left_idx);
                    }
                    (_, mid, Some(right)) if mid.keys.len().min(right.keys.len()) < THIRD => {
                        Self::rebalance_branches(branches, &mut self.keys, left_idx + 1);
                    }
                    _ => (),
                }
            }
        }
    }

    /// Merges two children leaves of this branch.
    ///
    /// Assumes that the two children can fit in a single leaf, panicking if not.
    fn merge_leaves(
        children: &mut Chunk<SharedPointer<Leaf<K, V>, P>, NUM_CHILDREN>,
        keys: &mut Chunk<K, NODE_SIZE>,
        left_idx: usize,
        keep_left: bool,
    ) {
        let [left, right, ..] = &mut children[left_idx..] else {
            unreachable!()
        };
        if keep_left {
            let left = SharedPointer::make_mut(left);
            let (left, right) = (left, &**right);
            left.keys.extend(right.keys.iter().cloned());
        } else {
            let right = SharedPointer::make_mut(right);
            let (left, right) = (&**left, right);
            right.keys.insert_from(0, left.keys.iter().cloned());
        }
        keys.remove(left_idx);
        children.remove(left_idx + (keep_left as usize));
        debug_assert_eq!(keys.len() + 1, children.len());
    }

    /// Rebalances two adjacent leaves so that they have the same
    /// number of keys (or differ by at most 1).
    fn rebalance_leaves(
        children: &mut Chunk<SharedPointer<Leaf<K, V>, P>, NUM_CHILDREN>,
        keys: &mut Chunk<K, NODE_SIZE>,
        left_idx: usize,
    ) {
        let [left, right, ..] = &mut children[left_idx..] else {
            unreachable!()
        };
        let (left, right) = (
            SharedPointer::make_mut(left),
            SharedPointer::make_mut(right),
        );
        let num_to_move = left.keys.len().abs_diff(right.keys.len()) / 2;
        if num_to_move == 0 {
            return;
        }
        if left.keys.len() > right.keys.len() {
            right.keys.drain_from_back(&mut left.keys, num_to_move);
        } else {
            left.keys.drain_from_front(&mut right.keys, num_to_move);
        }
        keys[left_idx] = right.keys.first().unwrap().0.clone();
        debug_assert_ne!(left.keys.len(), 0);
        debug_assert_ne!(right.keys.len(), 0);
    }

    /// Rebalances two adjacent child branches so that they have the same number of keys
    /// (or differ by at most 1). The separator key is rotated between the two branches.
    /// to keep the invariants of the parent branch.
    fn rebalance_branches(
        children: &mut Chunk<SharedPointer<Branch<K, V, P>, P>, NUM_CHILDREN>,
        keys: &mut Chunk<K, NODE_SIZE>,
        left_idx: usize,
    ) {
        let [left, right, ..] = &mut children[left_idx..] else {
            unreachable!()
        };
        let (left, right) = (
            SharedPointer::make_mut(left),
            SharedPointer::make_mut(right),
        );
        let num_to_move = left.keys.len().abs_diff(right.keys.len()) / 2;
        if num_to_move == 0 {
            return;
        }
        let separator = &mut keys[left_idx];
        if left.keys.len() > right.keys.len() {
            right.keys.push_front(separator.clone());
            right.keys.drain_from_back(&mut left.keys, num_to_move - 1);
            *separator = left.keys.pop_back();
            right
                .children
                .drain_from_back(&mut left.children, num_to_move);
        } else {
            left.keys.push_back(separator.clone());
            left.keys.drain_from_front(&mut right.keys, num_to_move - 1);
            *separator = right.keys.pop_front();
            left.children
                .drain_from_front(&mut right.children, num_to_move);
        }
        debug_assert_ne!(left.keys.len(), 0);
        debug_assert_eq!(left.children.len(), left.keys.len() + 1);
        debug_assert_ne!(right.keys.len(), 0);
        debug_assert_eq!(right.children.len(), right.keys.len() + 1);
    }

    /// Merges two children of this branch.
    ///
    /// Assumes that the two children can fit in a single branch, panicking if not.
    fn merge_branches(
        children: &mut Chunk<SharedPointer<Branch<K, V, P>, P>, NUM_CHILDREN>,
        keys: &mut Chunk<K, NODE_SIZE>,
        left_idx: usize,
        keep_left: bool,
    ) {
        let [left, right, ..] = &mut children[left_idx..] else {
            unreachable!()
        };
        let separator = keys.remove(left_idx);
        if keep_left {
            let left = SharedPointer::make_mut(left);
            let (left, right) = (left, &**right);
            left.keys.push_back(separator);
            left.keys.extend(right.keys.iter().cloned());
            left.children.extend(&right.children);
        } else {
            let right = SharedPointer::make_mut(right);
            let (left, right) = (&**left, right);
            right.keys.push_front(separator);
            right.keys.insert_from(0, left.keys.iter().cloned());
            right.children.insert_front(&left.children);
        }
        children.remove(left_idx + (keep_left as usize));
        debug_assert_eq!(keys.len() + 1, children.len());
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Branch<K, V, P> {
    pub(crate) fn insert(&mut self, key: K, value: V) -> InsertAction<K, V, P> {
        let i = slice_ext::binary_search_by(&self.keys, |k| k.cmp(&key))
            .map(|x| x + 1)
            .unwrap_or_else(|x| x);
        let insert_action = match &mut self.children {
            Children::Leaves { leaves } => {
                SharedPointer::make_mut(&mut leaves[i]).insert(key, value)
            }
            Children::Branches { branches, .. } => {
                SharedPointer::make_mut(&mut branches[i]).insert(key, value)
            }
        };
        match insert_action {
            InsertAction::Split(new_key, new_node) if self.keys.len() >= NODE_SIZE => {
                self.split_branch_insert(i, new_key, new_node)
            }
            InsertAction::Split(separator, new_node) => {
                self.keys.insert(i, separator);
                self.children.insert(i + 1, new_node);
                InsertAction::Inserted
            }
            action => action,
        }
    }
}
impl<K: Ord + Clone, V: Clone> Leaf<K, V> {
    pub(crate) fn insert<P: SharedPointerKind>(
        &mut self,
        key: K,
        value: V,
    ) -> InsertAction<K, V, P> {
        match slice_ext::binary_search_by(&self.keys, |(k, _)| k.cmp(&key)) {
            Ok(i) => {
                let (k, v) = mem::replace(&mut self.keys[i], (key, value));
                InsertAction::Replaced(k, v)
            }
            Err(i) if self.keys.len() >= NODE_SIZE => self.split_leaf_insert(i, key, value),
            Err(i) => {
                self.keys.insert(i, (key, value));
                InsertAction::Inserted
            }
        }
    }
}
impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Node<K, V, P> {
    pub(crate) fn insert(&mut self, key: K, value: V) -> InsertAction<K, V, P> {
        match self {
            Node::Branch(branch) => SharedPointer::make_mut(branch).insert(key, value),
            Node::Leaf(leaf) => SharedPointer::make_mut(leaf).insert(key, value),
        }
    }
}
impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Branch<K, V, P> {
    #[cold]
    fn split_branch_insert(
        &mut self,
        i: usize,
        new_key: K,
        new_node: Node<K, V, P>,
    ) -> InsertAction<K, V, P> {
        let split_idx = MEDIAN + (i > MEDIAN) as usize;
        let mut right_keys = self.keys.split_off(split_idx);
        let split_idx = MEDIAN + (i >= MEDIAN) as usize;
        let mut right_children = self.children.split_off(split_idx);
        let separator = if i == MEDIAN {
            right_children.insert(0, new_node.clone());
            new_key
        } else {
            if i < MEDIAN {
                self.keys.insert(i, new_key);
                self.children.insert(i + 1, new_node);
            } else {
                right_keys.insert(i - (MEDIAN + 1), new_key);
                right_children.insert(i - (MEDIAN + 1) + 1, new_node);
            }
            self.keys.pop_back()
        };
        debug_assert_eq!(self.keys.len(), right_keys.len());
        debug_assert_eq!(self.keys.len() + 1, self.children.len());
        debug_assert_eq!(right_keys.len() + 1, right_children.len());
        InsertAction::Split(
            separator,
            Node::Branch(SharedPointer::new(Branch {
                keys: right_keys,
                children: right_children,
            })),
        )
    }
}

impl<K: Ord + Clone, V: Clone> Leaf<K, V> {
    #[inline]
    fn split_leaf_insert<P: SharedPointerKind>(
        &mut self,
        i: usize,
        key: K,
        value: V,
    ) -> InsertAction<K, V, P> {
        let mut right_keys = self.keys.split_off(MEDIAN);
        if i < MEDIAN {
            self.keys.insert(i, (key, value));
        } else {
            right_keys.insert(i - MEDIAN, (key, value));
        }
        InsertAction::Split(
            right_keys.first().unwrap().0.clone(),
            Node::Leaf(SharedPointer::new(Leaf { keys: right_keys })),
        )
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Branch<K, V, P> {
    pub(crate) fn lookup_mut<Q>(&mut self, key: &Q) -> Option<(&K, &mut V)>
    where
        Q: Comparable<K> + ?Sized,
    {
        let i = slice_ext::binary_search_by(&self.keys, |k| key.compare(k).reverse())
            .map(|x| x + 1)
            .unwrap_or_else(|x| x);
        match &mut self.children {
            Children::Leaves { leaves } => SharedPointer::make_mut(&mut leaves[i]).lookup_mut(key),
            Children::Branches { branches, .. } => {
                SharedPointer::make_mut(&mut branches[i]).lookup_mut(key)
            }
        }
    }
}

impl<K: Ord + Clone, V: Clone> Leaf<K, V> {
    pub(crate) fn lookup_mut<Q>(&mut self, key: &Q) -> Option<(&K, &mut V)>
    where
        Q: Comparable<K> + ?Sized,
    {
        let keys = &mut self.keys;
        let i = slice_ext::binary_search_by(keys, |(k, _)| key.compare(k).reverse()).ok()?;
        keys.get_mut(i).map(|(k, v)| (&*k, v))
    }
}

impl<K: Ord + Clone, V: Clone, P: SharedPointerKind> Node<K, V, P> {
    pub(crate) fn lookup_mut<Q>(&mut self, key: &Q) -> Option<(&K, &mut V)>
    where
        Q: Comparable<K> + ?Sized,
    {
        match self {
            Node::Branch(branch) => SharedPointer::make_mut(branch).lookup_mut(key),
            Node::Leaf(leaf) => SharedPointer::make_mut(leaf).lookup_mut(key),
        }
    }

    pub(crate) fn new_from_split(left: Self, separator: K, right: Self) -> Self {
        Node::Branch(SharedPointer::new(Branch {
            keys: Chunk::unit(separator),
            children: match (left, right) {
                (Node::Branch(left), Node::Branch(right)) => Children::Branches {
                    level: NonZeroUsize::new(left.level() + 1).unwrap(),
                    branches: Chunk::from_iter([left, right]),
                },
                (Node::Leaf(left), Node::Leaf(right)) => Children::Leaves {
                    leaves: Chunk::from_iter([left, right]),
                },
                _ => panic!("mismatched split"),
            },
        }))
    }
}

impl<K: Ord, V, P: SharedPointerKind> Branch<K, V, P> {
    fn min(&self) -> Option<&(K, V)> {
        let mut node = self;
        loop {
            match &node.children {
                Children::Leaves { leaves } => return leaves.first()?.min(),
                Children::Branches { branches, .. } => node = branches.first()?,
            }
        }
    }
    fn max(&self) -> Option<&(K, V)> {
        let mut node = self;
        loop {
            match &node.children {
                Children::Leaves { leaves } => return leaves.last()?.max(),
                Children::Branches { branches, .. } => node = branches.last()?,
            }
        }
    }
    pub(crate) fn lookup<Q>(&self, key: &Q) -> Option<&(K, V)>
    where
        Q: Comparable<K> + ?Sized,
    {
        let mut node = self;
        loop {
            let i = slice_ext::binary_search_by(&node.keys, |k| key.compare(k).reverse())
                .map(|x| x + 1)
                .unwrap_or_else(|x| x);
            match &node.children {
                Children::Leaves { leaves } => return leaves[i].lookup(key),
                Children::Branches { branches, .. } => node = &branches[i],
            }
        }
    }
}

impl<K: Ord, V> Leaf<K, V> {
    fn min(&self) -> Option<&(K, V)> {
        self.keys.first()
    }
    fn max(&self) -> Option<&(K, V)> {
        self.keys.last()
    }
    fn lookup<Q>(&self, key: &Q) -> Option<&(K, V)>
    where
        Q: Comparable<K> + ?Sized,
    {
        let keys = &self.keys;
        let i = slice_ext::binary_search_by(keys, |(k, _)| key.compare(k).reverse()).ok()?;
        keys.get(i)
    }
}

impl<K: Ord, V, P: SharedPointerKind> Node<K, V, P> {
    pub(crate) fn min(&self) -> Option<&(K, V)> {
        match self {
            Node::Branch(branch) => branch.min(),
            Node::Leaf(leaf) => leaf.min(),
        }
    }

    pub(crate) fn max(&self) -> Option<&(K, V)> {
        match self {
            Node::Branch(branch) => branch.max(),
            Node::Leaf(leaf) => leaf.max(),
        }
    }

    pub(crate) fn lookup<Q>(&self, key: &Q) -> Option<&(K, V)>
    where
        Q: Comparable<K> + ?Sized,
    {
        match self {
            Node::Branch(branch) => branch.lookup(key),
            Node::Leaf(leaf) => leaf.lookup(key),
        }
    }
}

impl<K: Clone, V: Clone> Clone for Leaf<K, V> {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
        }
    }
}

impl<K: Clone, V: Clone, P: SharedPointerKind> Clone for Branch<K, V, P> {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            children: self.children.clone(),
        }
    }
}

impl<K: Clone, V: Clone, P: SharedPointerKind> Clone for Children<K, V, P> {
    fn clone(&self) -> Self {
        match self {
            Children::Leaves { leaves } => Children::Leaves {
                leaves: leaves.clone(),
            },
            Children::Branches { branches, level } => Children::Branches {
                branches: branches.clone(),
                level: *level,
            },
        }
    }
}

impl<K, V, P: SharedPointerKind> Clone for Node<K, V, P> {
    fn clone(&self) -> Self {
        match self {
            Node::Branch(branch) => Node::Branch(branch.clone()),
            Node::Leaf(leaf) => Node::Leaf(leaf.clone()),
        }
    }
}

pub(crate) enum InsertAction<K, V, P: SharedPointerKind> {
    Inserted,
    Replaced(K, V),
    Split(K, Node<K, V, P>),
}

impl<K, V, P: SharedPointerKind> Default for Node<K, V, P> {
    fn default() -> Self {
        Node::Leaf(SharedPointer::new(Leaf { keys: Chunk::new() }))
    }
}

#[derive(Debug)]
pub(crate) struct ConsumingIter<K, V, P: SharedPointerKind> {
    /// The leaves of the tree, in order, note that this will remain the shared ptr
    /// as it will allows us to have a smaller VecDeque allocation and avoid eagerly
    /// cloning the leaves, which defeats the purpose of this iterator.
    /// Leaves present in the VecDeque are guaranteed to be non-empty.
    leaves: VecDeque<SharedPointer<Leaf<K, V>, P>>,
    remaining: usize,
}

impl<K, V, P: SharedPointerKind> ConsumingIter<K, V, P> {
    pub(crate) fn new(node: Option<Node<K, V, P>>, size: usize) -> Self {
        fn push<K, V, P: SharedPointerKind>(
            out: &mut VecDeque<SharedPointer<Leaf<K, V>, P>>,
            node: SharedPointer<Branch<K, V, P>, P>,
        ) {
            match &node.children {
                Children::Leaves { leaves } => {
                    out.extend(leaves.iter().filter(|leaf| !leaf.keys.is_empty()).cloned())
                }
                Children::Branches { branches, .. } => {
                    for child in branches.iter() {
                        push(out, child.clone());
                    }
                }
            }
        }
        // preallocate the VecDeque assuming each leaf is half full
        let mut leaves = VecDeque::with_capacity(size.div_ceil(NODE_SIZE / 2));
        match node {
            Some(Node::Branch(b)) => push(&mut leaves, b),
            Some(Node::Leaf(l)) if !l.keys.is_empty() => leaves.push_back(l),
            _ => (),
        }
        Self {
            leaves,
            remaining: size,
        }
    }
}

impl<K: Clone, V: Clone, P: SharedPointerKind> Iterator for ConsumingIter<K, V, P> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.leaves.front_mut()?;
        let leaf = SharedPointer::make_mut(node);
        self.remaining -= 1;
        let item = leaf.keys.pop_front();
        if leaf.keys.is_empty() {
            self.leaves.pop_front();
        }
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K: Clone, V: Clone, P: SharedPointerKind> DoubleEndedIterator for ConsumingIter<K, V, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let node = self.leaves.back_mut()?;
        let leaf = SharedPointer::make_mut(node);
        self.remaining -= 1;
        let item = leaf.keys.pop_back();
        if leaf.keys.is_empty() {
            self.leaves.pop_back();
        }
        Some(item)
    }
}

#[derive(Debug)]
pub(crate) struct Iter<'a, K, V, P: SharedPointerKind> {
    /// The forward and backward cursors
    /// The cursors are lazily initialized if their corresponding bound is unbounded
    fwd: Cursor<'a, K, V, P>,
    bwd: Cursor<'a, K, V, P>,
    fwd_yielded: bool,
    bwd_yielded: bool,
    exhausted: bool,
    exact: bool,
    remaining: usize,
    root: Option<&'a Node<K, V, P>>,
}

impl<'a, K, V, P: SharedPointerKind> Iter<'a, K, V, P> {
    pub(crate) fn new<R, Q>(root: Option<&'a Node<K, V, P>>, len: usize, range: R) -> Self
    where
        R: RangeBounds<Q>,
        Q: Comparable<K> + ?Sized,
    {
        let mut fwd = Cursor::empty();
        let mut bwd = Cursor::empty();
        let mut exhausted = match range.start_bound() {
            Bound::Included(key) | Bound::Excluded(key) => {
                fwd.init(root);
                if fwd.seek_to_key(key, false) && matches!(range.start_bound(), Bound::Excluded(_))
                {
                    fwd.next().is_none()
                } else {
                    fwd.is_empty()
                }
            }
            Bound::Unbounded => false,
        };

        exhausted = match (exhausted, range.end_bound()) {
            (false, Bound::Included(key) | Bound::Excluded(key)) => {
                bwd.init(root);
                if bwd.seek_to_key(key, true) && matches!(range.end_bound(), Bound::Excluded(_)) {
                    bwd.prev().is_none()
                } else {
                    bwd.is_empty()
                }
            }
            (exhausted, _) => exhausted,
        };

        // Check if forward is > backward cursor to determine if we are exhausted
        // Due to the usage of zip this is correct even if the cursors are already or not initialized yet
        fn cursors_exhausted<K, V, P: SharedPointerKind>(
            fwd: &Cursor<'_, K, V, P>,
            bwd: &Cursor<'_, K, V, P>,
        ) -> bool {
            for (&(fi, f), &(bi, b)) in fwd.stack.iter().zip(bwd.stack.iter()) {
                if !core::ptr::eq(f, b) {
                    return false;
                }
                if fi > bi {
                    return true;
                }
            }
            if let (Some((fi, f)), Some((bi, b))) = (fwd.leaf, bwd.leaf) {
                if !core::ptr::eq(f, b) {
                    return false;
                }
                if fi > bi {
                    return true;
                }
            }
            false
        }
        exhausted = exhausted || cursors_exhausted(&fwd, &bwd);

        let exact = matches!(range.start_bound(), Bound::Unbounded)
            && matches!(range.end_bound(), Bound::Unbounded);

        Self {
            fwd,
            bwd,
            remaining: len,
            exact,
            exhausted,
            fwd_yielded: false,
            bwd_yielded: false,
            root,
        }
    }

    /// Updates the exhausted state of the iterator.
    /// Returns true if the iterator is immaterially exhausted, which implies ignoring the
    /// current next candidate, if any.
    #[inline]
    fn update_exhausted(&mut self, has_next: bool, other_side_yielded: bool) -> bool {
        debug_assert!(!self.exhausted);
        if !has_next {
            self.exhausted = true;
            return true;
        }
        // Check if the cursors are exhausted by checking their leaves
        // This is valid even if the cursors are empty due to not being initialized yet.
        // If they were empty because exhaustion we would not be in this function.
        if let (Some((fi, f)), Some((bi, b))) = (self.fwd.leaf, self.bwd.leaf) {
            if core::ptr::eq(f, b) && fi >= bi {
                self.exhausted = true;
                return fi == bi && other_side_yielded;
            }
        }
        false
    }

    #[cold]
    fn peek_initial(&mut self, fwd: bool) -> Option<&'a (K, V)> {
        debug_assert!(!self.exhausted);
        let cursor = if fwd {
            self.fwd_yielded = true;
            &mut self.fwd
        } else {
            self.bwd_yielded = true;
            &mut self.bwd
        };
        // If the cursor is empty we need to initialize it and seek to the first/last element.
        // If they were empty because exhaustion we would not be in this function.
        if cursor.is_empty() {
            cursor.init(self.root);
            if fwd {
                cursor.seek_to_first();
            } else {
                cursor.seek_to_last();
            }
        }
        cursor.peek()
    }
}

impl<'a, K, V, P: SharedPointerKind> Iterator for Iter<'a, K, V, P> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        let next = if self.fwd_yielded {
            self.fwd.next()
        } else {
            self.peek_initial(true)
        }
        .map(|(k, v)| (k, v));
        if self.update_exhausted(next.is_some(), self.bwd_yielded) {
            return None;
        }
        self.remaining -= 1;
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.exhausted {
            return (0, Some(0));
        }
        let lb = if self.exact { self.remaining } else { 0 };
        (lb, Some(self.remaining))
    }
}

impl<'a, K, V, P: SharedPointerKind> DoubleEndedIterator for Iter<'a, K, V, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        let next = if self.bwd_yielded {
            self.bwd.prev()
        } else {
            self.peek_initial(false)
        }
        .map(|(k, v)| (k, v));
        if self.update_exhausted(next.is_some(), self.fwd_yielded) {
            return None;
        }
        self.remaining -= 1;
        next
    }
}

impl<'a, K, V, P: SharedPointerKind> Clone for Iter<'a, K, V, P> {
    fn clone(&self) -> Self {
        Self {
            fwd: self.fwd.clone(),
            bwd: self.bwd.clone(),
            exact: self.exact,
            fwd_yielded: self.fwd_yielded,
            bwd_yielded: self.bwd_yielded,
            exhausted: self.exhausted,
            remaining: self.remaining,
            root: self.root,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Cursor<'a, K, V, P: SharedPointerKind> {
    // a sequence of nodes starting at the root
    stack: Vec<(usize, &'a Branch<K, V, P>)>,
    leaf: Option<(usize, &'a Leaf<K, V>)>,
}

impl<'a, K, V, P: SharedPointerKind> Clone for Cursor<'a, K, V, P> {
    fn clone(&self) -> Self {
        Self {
            stack: self.stack.clone(),
            leaf: self.leaf,
        }
    }
}

impl<'a, K, V, P: SharedPointerKind> Cursor<'a, K, V, P> {
    /// Creates a new empty cursor.
    /// The variety of methods is to allow for a more efficient initialization
    /// in all cases.
    pub(crate) fn empty() -> Self {
        Self {
            stack: Vec::new(),
            leaf: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.stack.is_empty() && self.leaf.is_none()
    }

    pub(crate) fn init(&mut self, node: Option<&'a Node<K, V, P>>) {
        if let Some(node) = node {
            self.stack.reserve_exact(node.level());
            match node {
                Node::Branch(branch) => self.stack.push((0, branch)),
                Node::Leaf(leaf) => {
                    debug_assert!(self.leaf.is_none());
                    self.leaf = Some((0, leaf))
                }
            }
        }
    }

    // pushes the `ix`th child of `branch` onto the stack, whether it's a leaf
    // or a branch
    fn push_child(&mut self, branch: &'a Branch<K, V, P>, ix: usize) {
        debug_assert!(
            self.leaf.is_none(),
            "it doesn't make sense to push when we're already at a leaf"
        );
        match &branch.children {
            Children::Leaves { leaves } => self.leaf = Some((0, &leaves[ix])),
            Children::Branches { branches, .. } => self.stack.push((0, &branches[ix])),
        }
    }

    pub(crate) fn seek_to_first(&mut self) -> Option<&'a (K, V)> {
        loop {
            if let Some((i, leaf)) = &self.leaf {
                debug_assert_eq!(i, &0);
                return leaf.keys.first();
            }
            let (i, branch) = self.stack.last()?;
            debug_assert_eq!(i, &0);
            self.push_child(branch, 0);
        }
    }

    fn seek_to_last(&mut self) -> Option<&'a (K, V)> {
        loop {
            if let Some((i, leaf)) = &mut self.leaf {
                debug_assert_eq!(i, &0);
                *i = leaf.keys.len().saturating_sub(1);
                return leaf.keys.last();
            }
            let (i, branch) = self.stack.last_mut()?;
            debug_assert_eq!(i, &0);
            *i = branch.children.len() - 1;
            let (i, branch) = (*i, *branch);
            self.push_child(branch, i);
        }
    }

    fn seek_to_key<Q>(&mut self, key: &Q, for_prev: bool) -> bool
    where
        Q: Comparable<K> + ?Sized,
    {
        loop {
            if let Some((i, leaf)) = &mut self.leaf {
                let search =
                    slice_ext::binary_search_by(&leaf.keys, |(k, _)| key.compare(k).reverse());
                *i = search.unwrap_or_else(|x| x);
                if for_prev {
                    if search.is_err() {
                        self.prev();
                    }
                } else if search == Err(leaf.keys.len()) {
                    self.next();
                }
                return search.is_ok();
            }
            let Some((i, branch)) = self.stack.last_mut() else {
                return false;
            };
            *i = slice_ext::binary_search_by(&branch.keys, |k| key.compare(k).reverse())
                .map(|x| x + 1)
                .unwrap_or_else(|x| x);
            let (i, branch) = (*i, *branch);
            self.push_child(branch, i);
        }
    }

    /// Advances this and another cursor to their next position.
    /// While doing so skip all shared nodes between them.
    pub(crate) fn advance_skipping_shared<'b>(&mut self, other: &mut Cursor<'b, K, V, P>) {
        // The current implementation is not optimal as it will still visit many nodes unnecessarily
        // before skipping them. But it requires very little additional code.
        // Nevertheless it will still improve performance when there are shared nodes.
        loop {
            let mut skipped_any = false;
            debug_assert!(self.leaf.is_some());
            debug_assert!(other.leaf.is_some());
            if let (Some(this), Some(that)) = (self.leaf, other.leaf) {
                if core::ptr::eq(this.1, that.1) {
                    self.leaf = None;
                    other.leaf = None;
                    skipped_any = true;
                    let shared_levels = self
                        .stack
                        .iter()
                        .rev()
                        .zip(other.stack.iter().rev())
                        .take_while(|(this, that)| core::ptr::eq(this.1, that.1))
                        .count();
                    if shared_levels != 0 {
                        self.stack.drain(self.stack.len() - shared_levels..);
                        other.stack.drain(other.stack.len() - shared_levels..);
                    }
                }
            }
            self.next();
            other.next();
            if !skipped_any || self.leaf.is_none() {
                break;
            }
        }
    }

    pub(crate) fn next(&mut self) -> Option<&'a (K, V)> {
        loop {
            if let Some((i, leaf)) = &mut self.leaf {
                if *i + 1 < leaf.keys.len() {
                    *i += 1;
                    return leaf.keys.get(*i);
                }
                self.leaf = None;
            }
            let Some((i, branch)) = self.stack.last_mut() else {
                break;
            };
            if *i + 1 < branch.children.len() {
                *i += 1;
                let (i, branch) = (*i, *branch);
                self.push_child(branch, i);
                break;
            }
            self.stack.pop();
        }
        self.seek_to_first()
    }

    fn prev(&mut self) -> Option<&'a (K, V)> {
        loop {
            if let Some((i, leaf)) = &mut self.leaf {
                if *i > 0 {
                    *i -= 1;
                    return leaf.keys.get(*i);
                }
                self.leaf = None;
            }
            let Some((i, branch)) = self.stack.last_mut() else {
                break;
            };
            if *i > 0 {
                *i -= 1;
                let (i, branch) = (*i, *branch);
                self.push_child(branch, i);
                break;
            }
            self.stack.pop();
        }
        self.seek_to_last()
    }

    pub(crate) fn peek(&self) -> Option<&'a (K, V)> {
        if let Some((i, leaf)) = &self.leaf {
            leaf.keys.get(*i)
        } else {
            None
        }
    }
}

/// A mutable iterator over a B+ tree, yielding `(&K, &mut V)` pairs.
///
/// Each node on the traversal path is made exclusive via `SharedPointer::make_mut`
/// (copy-on-write). This is the same pattern HashMap uses for its `IterMut`.
enum IterMutItem<'a, K, V, P: SharedPointerKind> {
    /// Iterating over leaf key-value pairs.
    LeafEntries(core::slice::IterMut<'a, (K, V)>),
    /// Iterating over leaf children of a branch node.
    LeafChildren(core::slice::IterMut<'a, SharedPointer<Leaf<K, V>, P>>),
    /// Iterating over branch children of a branch node.
    BranchChildren(core::slice::IterMut<'a, SharedPointer<Branch<K, V, P>, P>>),
}

pub(crate) struct IterMut<'a, K, V, P: SharedPointerKind> {
    count: usize,
    stack: Vec<IterMutItem<'a, K, V, P>>,
}

impl<'a, K, V, P> IterMut<'a, K, V, P>
where
    K: Ord + Clone + 'a,
    V: Clone + 'a,
    P: SharedPointerKind,
{
    pub(crate) fn new(root: Option<&'a mut Node<K, V, P>>, size: usize) -> Self {
        let mut result = IterMut {
            count: size,
            stack: Vec::new(),
        };
        if let Some(node) = root {
            result.push_node(node);
        }
        result
    }

    fn push_node(&mut self, node: &'a mut Node<K, V, P>) {
        match node {
            Node::Branch(branch_ref) => {
                let branch = SharedPointer::make_mut(branch_ref);
                self.push_branch_children(branch);
            }
            Node::Leaf(leaf_ref) => {
                let leaf = SharedPointer::make_mut(leaf_ref);
                self.stack.push(IterMutItem::LeafEntries(
                    leaf.keys.as_mut_slice().iter_mut(),
                ));
            }
        }
    }

    fn push_branch_children(&mut self, branch: &'a mut Branch<K, V, P>) {
        match &mut branch.children {
            Children::Leaves { leaves } => {
                self.stack
                    .push(IterMutItem::LeafChildren(leaves.as_mut_slice().iter_mut()));
            }
            Children::Branches { branches, .. } => {
                self.stack.push(IterMutItem::BranchChildren(
                    branches.as_mut_slice().iter_mut(),
                ));
            }
        }
    }
}

impl<'a, K, V, P> Iterator for IterMut<'a, K, V, P>
where
    K: Ord + Clone + 'a,
    V: Clone + 'a,
    P: SharedPointerKind,
{
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.stack.last_mut()? {
                IterMutItem::LeafEntries(iter) => {
                    if let Some((k, v)) = iter.next() {
                        self.count -= 1;
                        return Some((k, v));
                    }
                    self.stack.pop();
                }
                IterMutItem::LeafChildren(iter) => {
                    if let Some(leaf_ref) = iter.next() {
                        let leaf = SharedPointer::make_mut(leaf_ref);
                        self.stack.push(IterMutItem::LeafEntries(
                            leaf.keys.as_mut_slice().iter_mut(),
                        ));
                    } else {
                        self.stack.pop();
                    }
                }
                IterMutItem::BranchChildren(iter) => {
                    if let Some(branch_ref) = iter.next() {
                        let branch = SharedPointer::make_mut(branch_ref);
                        match &mut branch.children {
                            Children::Leaves { leaves } => {
                                self.stack.push(IterMutItem::LeafChildren(
                                    leaves.as_mut_slice().iter_mut(),
                                ));
                            }
                            Children::Branches { branches, .. } => {
                                self.stack.push(IterMutItem::BranchChildren(
                                    branches.as_mut_slice().iter_mut(),
                                ));
                            }
                        }
                    } else {
                        self.stack.pop();
                    }
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<'a, K, V, P: SharedPointerKind> ExactSizeIterator for IterMut<'a, K, V, P>
where
    K: Ord + Clone + 'a,
    V: Clone + 'a,
{
}

impl<'a, K, V, P: SharedPointerKind> FusedIterator for IterMut<'a, K, V, P>
where
    K: Ord + Clone + 'a,
    V: Clone + 'a,
{
}

// --- Split / concat primitives (rayon-gated) ---
//
// These two functions implement O(log n) structural split and O(log n) height-aware
// join for the B+ tree used by OrdMap and OrdSet. They are the building blocks for
// the join-based parallel set operations (par_union, par_intersection, etc.) in
// `ord/rayon.rs`.
//
// The output of `split_node` may contain underfull nodes on the split spine — this
// is intentional. `concat_node` accepts such trees and produces a valid tree.

/// Count the number of key-value entries in an optional tree node.
///
/// Used by [`split_at_key_consuming`][crate::ord::map::GenericOrdMap::split_at_key_consuming]
/// to recompute the `size` field after a structural split. O(n).
#[cfg(any(test, feature = "rayon"))]
pub(crate) fn count_entries<K, V, P: SharedPointerKind>(node: &Option<Node<K, V, P>>) -> usize {
    match node {
        None => 0,
        Some(Node::Leaf(leaf)) => leaf.keys.len(),
        Some(Node::Branch(branch)) => count_branch_entries(&**branch),
    }
}

#[cfg(any(test, feature = "rayon"))]
fn count_branch_entries<K, V, P: SharedPointerKind>(branch: &Branch<K, V, P>) -> usize {
    match &branch.children {
        Children::Leaves { leaves } => leaves.iter().map(|l| l.keys.len()).sum(),
        Children::Branches { branches, .. } => {
            branches.iter().map(|b| count_branch_entries(&**b)).sum()
        }
    }
}

/// Split `node` at `key`: returns `(left, exact_value, right)` where every key in
/// `left` is strictly less than `key`, every key in `right` is strictly greater, and
/// `exact_value` is `Some(v)` if `key` was present in the tree.
///
/// Runs in O(depth × NODE_SIZE) ≈ O(log n). The returned trees may have underfull
/// nodes on their right / left spine respectively; pass them to [`concat_node`] to
/// combine back into a valid tree.
#[cfg(any(test, feature = "rayon"))]
pub(crate) fn split_node<K, V, P, Q>(
    node: Node<K, V, P>,
    key: &Q,
) -> (Option<Node<K, V, P>>, Option<V>, Option<Node<K, V, P>>)
where
    K: Ord + Clone,
    V: Clone,
    P: SharedPointerKind,
    Q: Comparable<K> + ?Sized,
{
    match node {
        Node::Leaf(leaf) => {
            let leaf = SharedPointer::try_unwrap(leaf).unwrap_or_else(|p| (*p).clone());
            let pos = slice_ext::binary_search_by(&leaf.keys, |(k, _)| key.compare(k).reverse());
            let (found, split_at) = match pos {
                Ok(i) => (true, i),
                Err(i) => (false, i),
            };
            let value = if found {
                Some(leaf.keys[split_at].1.clone())
            } else {
                None
            };
            let right_start = split_at + found as usize;
            // Clone the original keys once and split it.
            let mut left_keys = leaf.keys;
            let right_keys = left_keys.split_off(split_at);
            // `right_keys` now starts at `split_at`; if found, drop the exact entry at [0].
            let right_keys = if found {
                let mut rk = right_keys;
                rk.pop_front(); // remove the matched key
                rk
            } else {
                right_keys
            };
            let _ = right_start; // computed above, used implicitly via split
            let left = if left_keys.is_empty() {
                None
            } else {
                Some(Node::Leaf(SharedPointer::new(Leaf { keys: left_keys })))
            };
            let right = if right_keys.is_empty() {
                None
            } else {
                Some(Node::Leaf(SharedPointer::new(Leaf { keys: right_keys })))
            };
            (left, value, right)
        }
        Node::Branch(branch) => {
            let branch = SharedPointer::try_unwrap(branch).unwrap_or_else(|p| (*p).clone());
            // Find which child to recurse into. `i` is the child index such that
            // children[i] is the subtree that could contain `key`.
            let i = slice_ext::binary_search_by(&branch.keys, |k| key.compare(k).reverse())
                .map(|x| x + 1)
                .unwrap_or_else(|x| x);
            // Extract child i as a Node, then recurse.
            let child = branch_child_node(&branch, i);
            // Split the branch keys and children arrays around index i.
            // Keys:    branch.keys[0..i]   go left, branch.keys[i..] go right.
            // Children: branch.children[0..i] go left (plus sub_left),
            //           branch.children[i+1..] go right (plus sub_right).
            let mut left_keys = branch.keys;
            let mut right_keys = left_keys.split_off(i); // left_keys=[0..i], right_keys=[i..]
            let mut left_children = branch.children;
            let right_children = left_children.split_off(i + 1); // left=[0..i+1], right=[i+1..]
                                                                 // Drop the last entry from left_children (it's child i which we recursed into).
            let _ = left_children.pop_last_node();
            // Recurse.
            let (sub_left, value, sub_right) = split_node(child, key);
            // If sub_left is None, the recursed child contributed nothing to the left side.
            // The rightmost key in left_keys (K[i-1]) was the separator between children[i-1]
            // and children[i]; with children[i]'s left part gone, that separator must be dropped.
            if sub_left.is_none() && !left_keys.is_empty() {
                left_keys.pop_back();
            }
            // If sub_right is None, similarly drop the leftmost key of right_keys (K[i]),
            // which was the separator between children[i] and children[i+1].
            if sub_right.is_none() && !right_keys.is_empty() {
                right_keys.pop_front();
            }
            // Build left tree: children[0..i] + sub_left, keys[0..i] (adjusted above).
            let left = build_branch_from_children(left_children, left_keys, sub_left, true);
            // Build right tree: sub_right + children[i+1..], keys[i..] (adjusted above).
            let right = build_branch_from_children(right_children, right_keys, sub_right, false);
            (left, value, right)
        }
    }
}

/// Extract child `i` from a Branch as a `Node`, cloning through the shared pointer.
#[cfg(any(test, feature = "rayon"))]
fn branch_child_node<K, V, P>(branch: &Branch<K, V, P>, i: usize) -> Node<K, V, P>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
    match &branch.children {
        Children::Leaves { leaves } => Node::Leaf(leaves[i].clone()),
        Children::Branches { branches, .. } => Node::Branch(branches[i].clone()),
    }
}

impl<K, V, P: SharedPointerKind> Children<K, V, P> {
    /// Remove and return the last child as a `Node`.
    #[cfg(any(test, feature = "rayon"))]
    fn pop_last_node(&mut self) -> Node<K, V, P> {
        match self {
            Children::Leaves { leaves } => Node::Leaf(leaves.pop_back()),
            Children::Branches { branches, .. } => Node::Branch(branches.pop_back()),
        }
    }
}

/// Assemble a partial branch after a split.
///
/// `children` and `keys` form the existing side (already split off from the full
/// branch). `extra` is the sub-tree returned by the recursive split call. If
/// `extra_is_right_of_children` is false, `extra` is prepended (it came from the
/// left side); if true, it is appended (right side).
///
/// Returns `None` if the result would be empty; returns `Some(Leaf)` if the
/// branch would have exactly one child (collapse); otherwise returns a `Branch`.
#[cfg(any(test, feature = "rayon"))]
fn build_branch_from_children<K, V, P>(
    mut children: Children<K, V, P>,
    keys: Chunk<K, NODE_SIZE>,
    extra: Option<Node<K, V, P>>,
    extra_appended: bool, // true = extra goes after children; false = before
) -> Option<Node<K, V, P>>
where
    K: Clone,
    V: Clone,
    P: SharedPointerKind,
{
    if let Some(node) = extra {
        let n = children.len();
        if extra_appended {
            children.insert(n, node);
        } else {
            children.insert(0, node);
        }
    }
    // Return None for empty, otherwise always a Branch (even single-child).
    // Collapsing a single-child Branch to its child would break the type invariant
    // when the parent has Children::Branches and the sole child is a Leaf — concat_node
    // handles underfull spine nodes without needing pre-collapse.
    if children.len() == 0 {
        None
    } else {
        Some(Node::Branch(SharedPointer::new(Branch { keys, children })))
    }
}

/// Height-aware O(log n) join of two trees where every key in `left` is strictly
/// less than every key in `right`.
///
/// Produces a single valid B+ tree (may increase height by at most 1). Both inputs
/// may contain underfull spine nodes (as produced by [`split_node`]).
#[cfg(any(test, feature = "rayon"))]
pub(crate) fn concat_node<K, V, P>(left: Node<K, V, P>, right: Node<K, V, P>) -> Node<K, V, P>
where
    K: Ord + Clone,
    V: Clone,
    P: SharedPointerKind,
{
    let h_left = left.level();
    let h_right = right.level();

    if h_left == h_right {
        // Equal heights: `new_from_split` creates a Branch one level above both.
        let sep = right.min().unwrap().0.clone();
        Node::new_from_split(left, sep, right)
    } else if h_left > h_right {
        // Left is taller: walk left's right spine and insert right at the correct depth.
        let sep = right.min().unwrap().0.clone();
        let mut left_branch = match left {
            Node::Branch(b) => SharedPointer::try_unwrap(b).unwrap_or_else(|p| (*p).clone()),
            Node::Leaf(_) => unreachable!("non-zero level must be a Branch"),
        };
        match concat_insert_right_spine(&mut left_branch, right, sep) {
            None => Node::Branch(SharedPointer::new(left_branch)),
            Some((overflow_sep, overflow_right)) => Node::new_from_split(
                Node::Branch(SharedPointer::new(left_branch)),
                overflow_sep,
                overflow_right,
            ),
        }
    } else {
        // Right is taller: walk right's left spine and insert left at the correct depth.
        let sep = right.min().unwrap().0.clone();
        let mut right_branch = match right {
            Node::Branch(b) => SharedPointer::try_unwrap(b).unwrap_or_else(|p| (*p).clone()),
            Node::Leaf(_) => unreachable!("non-zero level must be a Branch"),
        };
        match concat_insert_left_spine(&mut right_branch, left, sep) {
            None => Node::Branch(SharedPointer::new(right_branch)),
            Some((overflow_sep, overflow_left)) => Node::new_from_split(
                overflow_left,
                overflow_sep,
                Node::Branch(SharedPointer::new(right_branch)),
            ),
        }
    }
}

/// Insert `right` (which has level `branch.level() - 1` or less) at the rightmost
/// position of `branch`. Returns `Some((separator, overflow_node))` on overflow.
#[cfg(any(test, feature = "rayon"))]
fn concat_insert_right_spine<K, V, P>(
    branch: &mut Branch<K, V, P>,
    right: Node<K, V, P>,
    sep: K,
) -> Option<(K, Node<K, V, P>)>
where
    K: Ord + Clone,
    V: Clone,
    P: SharedPointerKind,
{
    if branch.level() == right.level() + 1 {
        // right is exactly one level below branch — insert directly.
        concat_append_child_right(branch, sep, right)
    } else {
        // Recurse into the rightmost child (must be a Branch at this point).
        let overflow = match &mut branch.children {
            Children::Branches { branches, .. } => {
                let last = branches.last_mut().expect("branch must have children");
                let child_branch = SharedPointer::make_mut(last);
                concat_insert_right_spine(child_branch, right, sep)
            }
            Children::Leaves { .. } => {
                unreachable!("leaf-level branch reached before target depth")
            }
        };
        match overflow {
            None => None,
            Some((overflow_sep, overflow_node)) => {
                concat_append_child_right(branch, overflow_sep, overflow_node)
            }
        }
    }
}

/// Append `sep` + `child` as the new rightmost child of `branch`. Splits on overflow.
#[cfg(any(test, feature = "rayon"))]
fn concat_append_child_right<K, V, P>(
    branch: &mut Branch<K, V, P>,
    sep: K,
    child: Node<K, V, P>,
) -> Option<(K, Node<K, V, P>)>
where
    K: Ord + Clone,
    V: Clone,
    P: SharedPointerKind,
{
    let n = branch.children.len();
    if n < NUM_CHILDREN {
        branch.keys.push_back(sep);
        branch.children.insert(n, child);
        None
    } else {
        // Full branch: split at MEDIAN to make room. The new child goes on the right half.
        // left:  keys[0..MEDIAN],    children[0..MEDIAN+1]
        // sep_key: keys[MEDIAN]
        // right: keys[MEDIAN+1..] ++ [sep],  children[MEDIAN+1..] ++ [child]
        let mut right_keys = branch.keys.split_off(MEDIAN + 1); // branch.keys = [0..MEDIAN+1]
        let split_sep = branch.keys.pop_back(); // branch.keys = [0..MEDIAN]
        right_keys.push_back(sep);
        let mut right_children = branch.children.split_off(MEDIAN + 1);
        let m = right_children.len();
        right_children.insert(m, child);
        let right_branch = Node::Branch(SharedPointer::new(Branch {
            keys: right_keys,
            children: right_children,
        }));
        Some((split_sep, right_branch))
    }
}

/// Insert `left` (which has level `branch.level() - 1` or less) at the leftmost
/// position of `branch`. Returns `Some((separator, overflow_node))` on overflow.
#[cfg(any(test, feature = "rayon"))]
fn concat_insert_left_spine<K, V, P>(
    branch: &mut Branch<K, V, P>,
    left: Node<K, V, P>,
    sep: K, // min_key(branch.children[0]) = separator between `left` and the old first child
) -> Option<(K, Node<K, V, P>)>
where
    K: Ord + Clone,
    V: Clone,
    P: SharedPointerKind,
{
    if branch.level() == left.level() + 1 {
        concat_prepend_child_left(branch, left, sep)
    } else {
        let overflow = match &mut branch.children {
            Children::Branches { branches, .. } => {
                let first = &mut branches[0];
                let child_branch = SharedPointer::make_mut(first);
                concat_insert_left_spine(child_branch, left, sep)
            }
            Children::Leaves { .. } => {
                unreachable!("leaf-level branch reached before target depth")
            }
        };
        match overflow {
            None => None,
            Some((overflow_sep, overflow_left)) => {
                concat_prepend_child_left(branch, overflow_left, overflow_sep)
            }
        }
    }
}

/// Prepend `child` + `sep` as the new leftmost child of `branch`. Splits on overflow.
/// `sep` is the separator between `child` and the current first child (i.e. the min key
/// of the current first child).
#[cfg(any(test, feature = "rayon"))]
fn concat_prepend_child_left<K, V, P>(
    branch: &mut Branch<K, V, P>,
    child: Node<K, V, P>,
    sep: K,
) -> Option<(K, Node<K, V, P>)>
where
    K: Ord + Clone,
    V: Clone,
    P: SharedPointerKind,
{
    let n = branch.children.len();
    if n < NUM_CHILDREN {
        branch.keys.push_front(sep);
        branch.children.insert(0, child);
        None
    } else {
        // Full branch: split. The new child goes on the left half.
        // After conceptual prepend: new_keys = [sep, K0..K{N-1}] (N+1 total, where N=NODE_SIZE),
        //   new_children = [child, C0..C{N}] (N+2 total, where N+1=NUM_CHILDREN).
        // Split at index MEDIAN so that left and right each get MEDIAN keys and MEDIAN+1 children:
        //   left_keys     = new_keys[0..MEDIAN]       = [sep, K0..K{MEDIAN-2}]
        //   split_sep     = new_keys[MEDIAN]           = original K{MEDIAN-1}
        //   right_keys    = new_keys[MEDIAN+1..]       = original K{MEDIAN}..
        //   left_children = new_children[0..MEDIAN+1]  = [child, C0..C{MEDIAN-1}]
        //   right_children = new_children[MEDIAN+1..]  = C{MEDIAN}..
        let right_keys = branch.keys.split_off(MEDIAN); // branch.keys=[K0..K{MEDIAN-1}], right_keys=[K{MEDIAN}..]
        let split_sep = branch.keys.pop_back(); // remove K{MEDIAN-1}; branch.keys=[K0..K{MEDIAN-2}]
        let old_left_keys = mem::replace(&mut branch.keys, right_keys);
        // branch.keys = right_keys (K{MEDIAN}..); old_left_keys = K0..K{MEDIAN-2} (MEDIAN-1 elements)
        let mut left_keys: Chunk<K, NODE_SIZE> = Chunk::unit(sep);
        left_keys.extend(old_left_keys.iter().cloned()); // [sep, K0..K{MEDIAN-2}] = MEDIAN elements
        let right_children = branch.children.split_off(MEDIAN); // branch.children=[C0..C{MEDIAN-1}], right_children=[C{MEDIAN}..]
        branch.children.insert(0, child); // branch.children = [child, C0..C{MEDIAN-1}] = MEDIAN+1 elements
        let left_children = mem::replace(&mut branch.children, right_children);
        let left_branch = Node::Branch(SharedPointer::new(Branch {
            keys: left_keys,         // MEDIAN elements
            children: left_children, // MEDIAN+1 elements → invariant: MEDIAN + 1 == MEDIAN+1 ✓
        }));
        // right (stays as branch): keys=right_keys=MEDIAN elements, children=right_children=MEDIAN+1 ✓
        Some((split_sep, left_branch))
    }
}

mod slice_ext {
    #[inline]
    #[allow(unsafe_code)] // Uses ptr::read to avoid branching in the binary search hot path; same technique as std's implementation.
    pub(super) fn binary_search_by<T, F>(slice: &[T], mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&T) -> core::cmp::Ordering,
    {
        // Optimization: defer to std-lib if we think we're comparing integers, in which case
        // the stdlib implementation optimizes better using a fully branchless approach.
        // This branch is fully resolved at compile-time and will not incur any space or runtime overhead.
        // There is a mild assumption that the std-lib implementation will remain optimized for primitive types.
        if !core::mem::needs_drop::<T>() && core::mem::size_of::<T>() <= 16 {
            return slice.binary_search_by(f);
        }

        // This binary search implementation will always perform the minimum number of
        // comparisons and also allows for early return from the search loop when the comparison
        // function returns `Equal`, which is best when the comparison function isn't trivial
        // (e.g. `memcmp` vs. integer comparison).

        use core::cmp::Ordering::*;
        let mut low = 0;
        let mut high = slice.len();
        // Compared to the stdlib this implementation perform early return when the comparison
        // function returns Equal and will perform the optimal number of comparisons.
        // This is a tradeoff when the comparisons aren't cheap, as is the case
        // when the comparison is a memcmp of the field name and CRDT type.
        while low < high {
            // the midpoint is biased (truncated) towards low so it will always be less than high
            let mid = low + (high - low) / 2;
            // SAFETY: mid is always in bounds because low < high <= slice.len(),
            // so mid = low + (high - low) / 2 < high <= slice.len().
            let cmp = f(unsafe { slice.get_unchecked(mid) });
            // TODO: Use select_unpredictable when min rustc_version >= 1.88
            // to guarantee conditional move optimization.
            // low can only get up to slice.len() as mid < slice.len()
            low = if cmp == Less { mid + 1 } else { low };
            high = if cmp == Greater { mid } else { high };
            if cmp == Equal {
                // SAFETY: mid < slice.len() as established above (bounds-checked
                // by the loop invariant). Hint enables the compiler to elide a
                // redundant bounds check on the return value.
                unsafe {
                    core::hint::assert_unchecked(mid < slice.len());
                }
                return Ok(mid);
            }
        }
        // SAFETY: low can only advance to mid + 1 where mid < slice.len(),
        // so low <= slice.len(). Hint enables the compiler to elide a
        // redundant bounds check on the Err return value.
        unsafe {
            core::hint::assert_unchecked(low <= slice.len());
        }
        Err(low)
    }
}
