// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::hash::{BuildHasher, Hash};
use core::iter::FusedIterator;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::slice::{Iter as SliceIter, IterMut as SliceIterMut};
use core::{fmt, mem};

use archery::{SharedPointer, SharedPointerKind};
use bitmaps::{Bits, BitsImpl};
use equivalent::Equivalent;
use imbl_sized_chunks::inline_array::InlineArray;
use imbl_sized_chunks::sparse_chunk::{Iter as ChunkIter, IterMut as ChunkIterMut, SparseChunk};

use crate::config::HASH_LEVEL_SIZE as HASH_SHIFT;
use crate::hash_width::HashWidth;

pub(crate) const HASH_WIDTH: usize = 2_usize.pow(HASH_SHIFT as u32);
/// Fixed upper bound for iterator stack capacity. Sufficient for u128
/// (128 / 5 + 1 = 27 levels). Wastes ~13 slots for u64 iterators but
/// avoids needing generic_const_exprs.
const ITER_STACK_CAPACITY: usize = 128_usize.div_ceil(HASH_SHIFT) + 1;
const SMALL_NODE_WIDTH: usize = HASH_WIDTH / 2;
const GROUP_WIDTH: usize = HASH_WIDTH / 2;

type SimdGroup = wide::u8x16;
type GroupBitmap = bitmaps::Bitmap<GROUP_WIDTH>;

/// Wide-multiply mixer — a single 128-bit multiply followed by a fold-XOR.
/// Produces excellent avalanche in just 2 operations. Used to compute Merkle
/// hash contributions so that commutative addition of similar inputs doesn't
/// cancel information. Based on wyhash mixing (Wang Yi, 2019).
#[inline]
pub(crate) fn fmix64(h: u64) -> u64 {
    let full = (h as u128).wrapping_mul(0x9E3779B97F4A7C15_u128);
    (full as u64) ^ ((full >> 64) as u64)
}

const _: () = {
    // Limitations of the current implementation, can only handle up to 2 groups,
    // but can be lifted with further code changes
    assert!(HASH_SHIFT <= 5, "HASH_LEVEL_SIZE must be at most 5");
    assert!(HASH_SHIFT >= 3, "HASH_LEVEL_SIZE must be at least 3");
};

#[inline]
pub(crate) fn hash_key<K: Hash + ?Sized, S: BuildHasher, H: HashWidth>(bh: &S, key: &K) -> H {
    H::from_hash64(bh.hash_one(key))
}

#[inline]
fn group_find_empty(control: &SimdGroup) -> Option<usize> {
    let idx = group_find(control, 0).first_index();
    // if the GROUP_WIDTH != SimdGroup lanes, we need to handle finding
    // a zero in an index outside the valid range
    if GROUP_WIDTH != size_of::<SimdGroup>() {
        idx.filter(|&i| i < GROUP_WIDTH)
    } else {
        idx
    }
}

#[inline]
fn group_find(control: &SimdGroup, value: u8) -> GroupBitmap {
    let mask = control.cmp_eq(SimdGroup::splat(value)).move_mask();
    GroupBitmap::from_value(mask as _)
}

/// Construct a node directly inside a SharedPointer allocation to avoid memcpy.
/// Nodes can be large (SparseChunk with SIMD control bytes), and the safe path
/// (construct on stack → clone into Arc) measurably slows down insert-heavy workloads.
#[inline]
fn node_with<T, P: SharedPointerKind>(with: impl FnOnce(&mut T)) -> SharedPointer<T, P>
where
    T: Default,
{
    // SAFETY: We allocate a SharedPointer<UnsafeCell<MaybeUninit<T>>> to get a
    // heap-allocated slot, write T::default() into it, let the callback mutate it,
    // then transmute the pointer type from UnsafeCell<MaybeUninit<T>> to T.
    // This is sound because:
    // - UnsafeCell<MaybeUninit<T>> and T have identical layout (UnsafeCell and
    //   MaybeUninit are both #[repr(transparent)])
    // - The value is fully initialised before the transmute (write + callback)
    // - ManuallyDrop prevents double-free of the source pointer
    // - The SharedPointer refcount is 1 (just created), so no aliasing concerns
    let result: SharedPointer<UnsafeCell<mem::MaybeUninit<T>>, P> =
        SharedPointer::new(UnsafeCell::new(mem::MaybeUninit::uninit()));
    #[allow(unsafe_code)]
    unsafe {
        (&mut *result.get()).write(T::default());
        let mut_ptr = &mut *UnsafeCell::raw_get(&*result);
        let mut_ptr = MaybeUninit::as_mut_ptr(mut_ptr);
        with(&mut *mut_ptr);
        let result = ManuallyDrop::new(result);
        mem::transmute_copy(&result)
    }
}

pub trait HashValue {
    type Key: Eq;

    fn extract_key(&self) -> &Self::Key;
    fn ptr_eq(&self, other: &Self) -> bool;
}

/// Generic SIMD node that stores leaf values only (no child nodes).
/// Uses SIMD control bytes for fast parallel lookup.
pub(crate) struct GenericSimdNode<A, H: HashWidth, const WIDTH: usize, const GROUPS: usize>
where
    BitsImpl<WIDTH>: Bits,
{
    /// Stores value-hash pairs directly (leaf-only)
    pub(crate) data: SparseChunk<(A, H), WIDTH>,

    /// SIMD control bytes for fast parallel lookup.
    /// Each byte corresponds to the u8 suffix of the hash.
    /// 0 indicates an empty slot, 1-255 are valid hash prefixes.
    control: [SimdGroup; GROUPS],

    /// Merkle fingerprint: commutative sum of fmix64(entry_hash) for all
    /// entries. Used for fast negative equality/diff checks — if two nodes
    /// have different merkle_hash values, their key sets definitely differ.
    pub(crate) merkle_hash: u64,
}

/// HAMT node that stores Entry enum (can contain values or child nodes).
/// Uses classic HAMT bitmap-indexed structure without SIMD.
pub(crate) struct HamtNode<A, P: SharedPointerKind, H: HashWidth = u64>
where
    BitsImpl<HASH_WIDTH>: Bits,
{
    /// Stores Entry enum which can contain values, collision nodes, or child nodes
    pub(crate) data: SparseChunk<Entry<A, P, H>, HASH_WIDTH>,

    /// Merkle fingerprint: commutative sum of fmix64(contribution) for all
    /// entries, where contribution is the entry's key hash (for values) or
    /// child merkle_hash (for sub-nodes). See `Entry::merkle_contribution`.
    pub(crate) merkle_hash: u64,
}

impl<A: Clone, H: HashWidth, const WIDTH: usize, const GROUPS: usize> Clone
    for GenericSimdNode<A, H, WIDTH, GROUPS>
where
    BitsImpl<WIDTH>: Bits,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            control: self.control,
            merkle_hash: self.merkle_hash,
        }
    }
}

impl<A: Clone, P: SharedPointerKind, H: HashWidth> Clone for HamtNode<A, P, H>
where
    BitsImpl<HASH_WIDTH>: Bits,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            merkle_hash: self.merkle_hash,
        }
    }
}

pub(crate) type SmallSimdNode<A, H> = GenericSimdNode<A, H, SMALL_NODE_WIDTH, 1>;
pub(crate) type LargeSimdNode<A, H> = GenericSimdNode<A, H, HASH_WIDTH, 2>;

// Legacy type alias for compatibility
pub(crate) type Node<A, P, H = u64> = HamtNode<A, P, H>;

impl<A, H: HashWidth, const WIDTH: usize, const GROUPS: usize> Default
    for GenericSimdNode<A, H, WIDTH, GROUPS>
where
    BitsImpl<WIDTH>: Bits,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> Default for HamtNode<A, P, H>
where
    BitsImpl<HASH_WIDTH>: Bits,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<A, H: HashWidth, const WIDTH: usize, const GROUPS: usize>
    GenericSimdNode<A, H, WIDTH, GROUPS>
where
    BitsImpl<WIDTH>: Bits,
{
    #[inline(always)]
    pub(crate) fn new() -> Self {
        GenericSimdNode {
            data: SparseChunk::new(),
            control: [SimdGroup::default(); GROUPS],
            merkle_hash: 0,
        }
    }

    #[inline]
    fn with<P: SharedPointerKind>(with: impl FnOnce(&mut Self)) -> SharedPointer<Self, P> {
        node_with(with)
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    fn pop_value<P: SharedPointerKind>(&mut self) -> Entry<A, P, H> {
        let (val, hash) = self.data.pop().unwrap();
        Entry::Value(val, hash)
    }
}

impl<A: HashValue, H: HashWidth, const WIDTH: usize, const GROUPS: usize>
    GenericSimdNode<A, H, WIDTH, GROUPS>
where
    BitsImpl<WIDTH>: Bits,
{
    #[inline]
    pub(crate) fn get<Q>(&self, hash: H, key: &Q) -> Option<&A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        let search = hash.ctrl_byte();
        let group = hash.ctrl_group(GROUPS);
        let mut bitmap = group_find(&self.control[group], search);

        while let Some(offset) = bitmap.first_index() {
            let index = group * GROUP_WIDTH + offset;
            let (ref value, value_hash) = self.data.get(index).unwrap();
            if hash_may_eq::<A, H>(hash, *value_hash) && key.equivalent(value.extract_key()) {
                return Some(value);
            }
            bitmap.set(offset, false);
        }
        None
    }

    #[allow(unsafe_code)]
    pub(crate) fn get_mut<Q>(&mut self, hash: H, key: &Q) -> Option<&mut A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        let search = hash.ctrl_byte();
        let group = hash.ctrl_group(GROUPS);
        let mut bitmap = group_find(&self.control[group], search);
        // SAFETY: The loop needs to re-borrow self.data on each iteration, but
        // the returned &mut A extends the borrow of self.data beyond the loop.
        // The borrow checker cannot express this (the returned reference is to a
        // single slot, not the whole chunk). The raw pointer breaks the borrow
        // chain while preserving the guarantee that only one &mut A is returned.
        let this = self as *mut Self;
        #[allow(dropping_references)]
        drop(self);

        while let Some(offset) = bitmap.first_index() {
            let index = group * GROUP_WIDTH + offset;
            let this = unsafe { &mut *this };
            let (ref mut value, value_hash) = this.data.get_mut(index).unwrap();
            if hash_may_eq::<A, H>(hash, *value_hash) && key.equivalent(value.extract_key()) {
                return Some(value);
            }
            bitmap.set(offset, false);
        }
        None
    }

    pub(crate) fn remove<Q>(&mut self, hash: H, key: &Q) -> Option<A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        let search = hash.ctrl_byte();
        let group = hash.ctrl_group(GROUPS);
        let mut bitmap = group_find(&self.control[group], search);

        while let Some(offset) = bitmap.first_index() {
            let index = group * GROUP_WIDTH + offset;
            let (ref value, value_hash) = self.data.get(index).unwrap();
            if hash_may_eq::<A, H>(hash, *value_hash) && key.equivalent(value.extract_key()) {
                let removed_hash = *value_hash;
                let mut ctrl_array = self.control[group].to_array();
                ctrl_array[offset] = 0;
                self.control[group] = SimdGroup::from(ctrl_array);
                let removed = self.data.remove(index).map(|(v, _)| v);
                self.merkle_hash = self.merkle_hash.wrapping_sub(fmix64(removed_hash.to_u64()));
                return removed;
            }
            bitmap.set(offset, false);
        }
        None
    }

    pub(crate) fn insert(&mut self, hash: H, value: A) -> Result<Option<A>, A> {
        let search = hash.ctrl_byte();
        let group = hash.ctrl_group(GROUPS);
        // First check if we're updating an existing value in the group
        let mut bitmap = group_find(&self.control[group], search);
        while let Some(offset) = bitmap.first_index() {
            let index = group * GROUP_WIDTH + offset;
            let (current, current_hash) = self.data.get_mut(index).unwrap();
            if hash_may_eq::<A, H>(hash, *current_hash)
                && current.extract_key() == value.extract_key()
            {
                // Key hash unchanged — merkle_hash stays the same.
                return Ok(Some(mem::replace(current, value)));
            }
            bitmap.set(offset, false);
        }

        // Try to insert into the designated group
        if let Some(offset) = group_find_empty(&self.control[group]) {
            let index = group * GROUP_WIDTH + offset;
            self.data.insert(index, (value, hash));
            let mut ctrl_array = self.control[group].to_array();
            ctrl_array[offset] = search;
            self.control[group] = SimdGroup::from(ctrl_array);
            self.merkle_hash = self.merkle_hash.wrapping_add(fmix64(hash.to_u64()));
            return Ok(None);
        }

        // Group is full, need to upgrade
        Err(value)
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> HamtNode<A, P, H>
where
    BitsImpl<HASH_WIDTH>: Bits,
{
    #[inline(always)]
    pub(crate) fn new() -> Self {
        HamtNode {
            data: SparseChunk::new(),
            merkle_hash: 0,
        }
    }

    #[inline]
    fn with(with: impl FnOnce(&mut Self)) -> SharedPointer<Self, P> {
        node_with(with)
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    fn pop(&mut self) -> Entry<A, P, H> {
        self.data.pop().unwrap()
    }
}

impl<A: HashValue, H: HashWidth> SmallSimdNode<A, H> {
    #[cold]
    fn upgrade_to_large<P: SharedPointerKind>(
        &mut self,
        hash: H,
        shift: usize,
        value: A,
    ) -> Entry<A, P, H>
    where
        A: Clone,
    {
        // Move all small node entries into a LargeSimdNode and try to insert the new value
        // Existing entries are guaranteed to fit since SmallNode has 16 entries max
        // and LargeSimdNode has 2 groups of 16 (32 total)
        let old_merkle = self.merkle_hash;
        let mut remaining_value = None;
        let mut large_node = LargeSimdNode::with(|node| {
            // Carry over the merkle hash from the small node — same entries.
            node.merkle_hash = old_merkle;
            let mut group_offsets = [0; 2];
            while let Some((val, entry_hash)) = self.data.pop() {
                let search = entry_hash.ctrl_byte();
                let group = entry_hash.ctrl_group(2);
                let group_offset = group_offsets[group];
                group_offsets[group] += 1;
                let data_offset = group * GROUP_WIDTH + group_offset;
                let mut ctrl_array = node.control[group].to_array();
                ctrl_array[group_offset] = search;
                node.control[group] = SimdGroup::from(ctrl_array);
                node.data.insert(data_offset, (val, entry_hash));
            }
            // node.insert updates merkle_hash for the new entry.
            if let Err(val) = node.insert(hash, value) {
                // Put it back if insert failed
                remaining_value = Some(val);
            }
        });

        // Check if insertion succeeded
        if let Some(value) = remaining_value {
            // LargeSimdNode group is full, upgrade to HamtNode
            let large_mut = SharedPointer::make_mut(&mut large_node);
            large_mut.upgrade_to_hamt(hash, shift, value)
        } else {
            // Successfully inserted into LargeSimdNode
            Entry::LargeSimdNode(large_node)
        }
    }
}

impl<A: HashValue, H: HashWidth> LargeSimdNode<A, H> {
    #[cold]
    fn upgrade_to_hamt<P: SharedPointerKind>(
        &mut self,
        hash: H,
        shift: usize,
        value: A,
    ) -> Entry<A, P, H>
    where
        A: Clone,
    {
        let hamt_node = HamtNode::with(|node| {
            // Relocate all existing values to their correct HAMT positions
            while let Some((value, hash)) = self.data.pop() {
                node.insert(hash, shift, value);
            }
            // Insert the new value
            node.insert(hash, shift, value);
        });
        Entry::HamtNode(hamt_node)
    }
}

impl<A: HashValue, P: SharedPointerKind, H: HashWidth> HamtNode<A, P, H> {
    pub(crate) fn get<Q>(&self, hash: H, shift: usize, key: &Q) -> Option<&A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        let mut node = self;
        let mut shift = shift;

        loop {
            let index = hash.trie_index(shift, HASH_WIDTH - 1);
            let entry = node.data.get(index)?;

            // Check HamtNode and Value first and check the others
            // in a cold function that's also inlined.
            // This prevents the compiler from putting all node types
            // in a jump table, which makes things slower.
            // This is less relevant in other code paths that may include
            // atomics, memory allocation (e.g. insert, remove) etc..
            match entry {
                Entry::HamtNode(ref child) => {
                    node = child;
                    shift += HASH_SHIFT;
                    continue;
                }
                Entry::Value(ref value, value_hash) => {
                    return if hash_may_eq::<A, H>(hash, *value_hash)
                        && key.equivalent(value.extract_key())
                    {
                        Some(value)
                    } else {
                        None
                    };
                }
                // Note: tried a bunch of things here, like (un)likely intrinsics,
                // but none of them worked as reliably as the cold function
                // that is also inlined.
                _ => return Self::get_terminal(entry, hash, key),
            }
        }
    }

    #[cold]
    #[inline(always)]
    fn get_terminal<'a, Q>(entry: &'a Entry<A, P, H>, hash: H, key: &Q) -> Option<&'a A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        match entry {
            Entry::SmallSimdNode(ref small) => small.get(hash, key),
            Entry::LargeSimdNode(ref large) => large.get(hash, key),
            Entry::Collision(ref coll) => coll.get(key),
            _ => unreachable!(),
        }
    }

    pub(crate) fn get_mut<Q>(&mut self, hash: H, shift: usize, key: &Q) -> Option<&mut A>
    where
        A: Clone,
        Q: Equivalent<A::Key> + ?Sized,
    {
        let index = hash.trie_index(shift, HASH_WIDTH - 1);
        match self.data.get_mut(index) {
            Some(Entry::HamtNode(ref mut child_ref)) => {
                SharedPointer::make_mut(child_ref).get_mut(hash, shift + HASH_SHIFT, key)
            }
            Some(Entry::SmallSimdNode(ref mut small_ref)) => {
                SharedPointer::make_mut(small_ref).get_mut(hash, key)
            }
            Some(Entry::LargeSimdNode(ref mut large_ref)) => {
                SharedPointer::make_mut(large_ref).get_mut(hash, key)
            }
            Some(Entry::Value(ref mut value, value_hash)) => {
                if hash_may_eq::<A, H>(hash, *value_hash) && key.equivalent(value.extract_key()) {
                    Some(value)
                } else {
                    None
                }
            }
            Some(Entry::Collision(ref mut coll_ref)) => {
                SharedPointer::make_mut(coll_ref).get_mut(key)
            }
            None => None,
        }
    }

    fn merge_values(value1: A, hash1: H, value2: A, hash2: H) -> Entry<A, P, H> {
        let small_node = SmallSimdNode::with(|node| {
            node.data.insert(0, (value1, hash1));
            node.data.insert(1, (value2, hash2));
            let mut ctrl_array = node.control[0].to_array();
            ctrl_array[0] = hash1.ctrl_byte();
            ctrl_array[1] = hash2.ctrl_byte();
            node.control[0] = SimdGroup::from(ctrl_array);
            node.merkle_hash = fmix64(hash1.to_u64()).wrapping_add(fmix64(hash2.to_u64()));
        });
        Entry::SmallSimdNode(small_node)
    }

    pub(crate) fn insert(&mut self, hash: H, shift: usize, value: A) -> Option<A>
    where
        A: Clone,
    {
        let index = hash.trie_index(shift, HASH_WIDTH - 1);

        let Some(entry) = self.data.get_mut(index) else {
            // Insert at empty HAMT position
            self.data.insert(index, Entry::Value(value, hash));
            self.merkle_hash = self.merkle_hash.wrapping_add(fmix64(hash.to_u64()));
            return None;
        };

        // old_contrib is captured inline per arm to avoid a separate
        // upfront lookup + merkle_contribution() dispatch. Only the
        // Value fall-through case needs it after the match.
        let old_contrib = match entry {
            Entry::HamtNode(child_ref) => {
                let child = SharedPointer::make_mut(child_ref);
                let old_m = child.merkle_hash;
                let result = child.insert(hash, shift + HASH_SHIFT, value);
                self.merkle_hash = self
                    .merkle_hash
                    .wrapping_sub(old_m)
                    .wrapping_add(child.merkle_hash);
                return result;
            }
            Entry::SmallSimdNode(small_ref) => {
                let small = SharedPointer::make_mut(small_ref);
                let old_m = small.merkle_hash;
                match small.insert(hash, value) {
                    Ok(result) => {
                        self.merkle_hash = self
                            .merkle_hash
                            .wrapping_sub(old_m)
                            .wrapping_add(small.merkle_hash);
                        return result;
                    }
                    Err(value) => {
                        // Small SIMD node is full, upgrade to LargeSimdNode
                        let new_entry =
                            small.upgrade_to_large(hash, shift + HASH_SHIFT, value);
                        let new_m = new_entry.merkle_contribution();
                        *entry = new_entry;
                        self.merkle_hash = self
                            .merkle_hash
                            .wrapping_sub(old_m)
                            .wrapping_add(new_m);
                        return None;
                    }
                }
            }
            Entry::LargeSimdNode(large_ref) => {
                let large = SharedPointer::make_mut(large_ref);
                let old_m = large.merkle_hash;
                match large.insert(hash, value) {
                    Ok(result) => {
                        self.merkle_hash = self
                            .merkle_hash
                            .wrapping_sub(old_m)
                            .wrapping_add(large.merkle_hash);
                        return result;
                    }
                    Err(value) => {
                        // Large SIMD node is full, upgrade to HamtNode
                        let new_entry =
                            large.upgrade_to_hamt(hash, shift + HASH_SHIFT, value);
                        let new_m = new_entry.merkle_contribution();
                        *entry = new_entry;
                        self.merkle_hash = self
                            .merkle_hash
                            .wrapping_sub(old_m)
                            .wrapping_add(new_m);
                        return None;
                    }
                }
            }
            // Update value or create a subtree
            Entry::Value(current, current_hash) => {
                if hash_may_eq::<A, H>(hash, *current_hash)
                    && current.extract_key() == value.extract_key()
                {
                    // Same key → same hash → merkle unchanged.
                    return Some(mem::replace(current, value));
                }
                fmix64((*current_hash).to_u64())
            }
            Entry::Collision(collision) => {
                let coll = SharedPointer::make_mut(collision);
                let old_m = (coll.data.len() as u64).wrapping_mul(fmix64(coll.hash.to_u64()));
                let result = coll.insert(value);
                let new_m = (coll.data.len() as u64).wrapping_mul(fmix64(coll.hash.to_u64()));
                self.merkle_hash = self
                    .merkle_hash
                    .wrapping_sub(old_m)
                    .wrapping_add(new_m);
                return result;
            }
        };

        // Only reachable from Entry::Value when keys don't match (hash collision).
        // Remove the old entry, build the merged node, and insert it back.
        let Entry::Value(old_value, old_hash) = self.data.remove(index).unwrap() else {
            unreachable!()
        };
        let new_entry = if shift + HASH_SHIFT >= H::BIT_COUNT {
            // We're at the lowest level, need to set up a collision node.
            Entry::from(CollisionNode::new(hash, old_value, value))
        } else {
            Self::merge_values(old_value, old_hash, value, hash)
        };
        let new_m = new_entry.merkle_contribution();
        self.data.insert(index, new_entry);
        self.merkle_hash = self
            .merkle_hash
            .wrapping_sub(old_contrib)
            .wrapping_add(new_m);
        None
    }

    pub(crate) fn remove<Q>(&mut self, hash: H, shift: usize, key: &Q) -> Option<A>
    where
        A: Clone,
        Q: Equivalent<A::Key> + ?Sized,
    {
        let index = hash.trie_index(shift, HASH_WIDTH - 1);

        let removed;
        // old_m and demotion value are captured inline per arm to avoid a
        // separate upfront lookup + merkle_contribution() dispatch.
        let (old_m, new_node) = match self.data.get_mut(index)? {
            Entry::HamtNode(child_ref) => {
                let child = SharedPointer::make_mut(child_ref);
                let old_m = child.merkle_hash;
                removed = child.remove(hash, shift + HASH_SHIFT, key);
                if child.len() == 1 && child.data.iter().next().is_some_and(|e| e.is_value()) {
                    (old_m, Some(child.pop()))
                } else {
                    self.merkle_hash = self
                        .merkle_hash
                        .wrapping_sub(old_m)
                        .wrapping_add(child.merkle_hash);
                    return removed;
                }
            }
            Entry::SmallSimdNode(small_ref) => {
                let small = SharedPointer::make_mut(small_ref);
                let old_m = small.merkle_hash;
                removed = small.remove(hash, key);
                if small.len() == 1 {
                    (old_m, Some(small.pop_value()))
                } else {
                    self.merkle_hash = self
                        .merkle_hash
                        .wrapping_sub(old_m)
                        .wrapping_add(small.merkle_hash);
                    return removed;
                }
            }
            Entry::LargeSimdNode(large_ref) => {
                let large = SharedPointer::make_mut(large_ref);
                let old_m = large.merkle_hash;
                removed = large.remove(hash, key);
                if large.len() == 1 {
                    (old_m, Some(large.pop_value()))
                } else {
                    self.merkle_hash = self
                        .merkle_hash
                        .wrapping_sub(old_m)
                        .wrapping_add(large.merkle_hash);
                    return removed;
                }
            }
            Entry::Value(value, value_hash) => {
                if hash_may_eq::<A, H>(hash, *value_hash) && key.equivalent(value.extract_key()) {
                    let old_m = fmix64((*value_hash).to_u64());
                    let result = self.data.remove(index).map(Entry::unwrap_value);
                    self.merkle_hash = self.merkle_hash.wrapping_sub(old_m);
                    return result;
                } else {
                    return None;
                }
            }
            Entry::Collision(coll_ref) => {
                let coll = SharedPointer::make_mut(coll_ref);
                let old_m = (coll.data.len() as u64).wrapping_mul(fmix64(coll.hash.to_u64()));
                removed = coll.remove(key);
                if coll.len() == 1 {
                    (old_m, Some(coll.pop_value()))
                } else {
                    let new_m = (coll.data.len() as u64).wrapping_mul(fmix64(coll.hash.to_u64()));
                    self.merkle_hash = self
                        .merkle_hash
                        .wrapping_sub(old_m)
                        .wrapping_add(new_m);
                    return removed;
                }
            }
        };
        // Node was demoted to a single value — replace entry.
        if let Some(new_node) = new_node {
            let new_m = new_node.merkle_contribution();
            self.data.insert(index, new_node);
            self.merkle_hash = self
                .merkle_hash
                .wrapping_sub(old_m)
                .wrapping_add(new_m);
        }
        removed
    }
}

#[derive(Clone)]
pub(crate) struct CollisionNode<A, H: HashWidth = u64> {
    pub(crate) hash: H,
    pub(crate) data: Vec<A>,
}

pub(crate) enum Entry<A, P: SharedPointerKind, H: HashWidth = u64> {
    HamtNode(SharedPointer<HamtNode<A, P, H>, P>),
    SmallSimdNode(SharedPointer<SmallSimdNode<A, H>, P>),
    LargeSimdNode(SharedPointer<LargeSimdNode<A, H>, P>),
    Value(A, H),
    Collision(SharedPointer<CollisionNode<A, H>, P>),
}

impl<A: Clone, P: SharedPointerKind, H: HashWidth> Clone for Entry<A, P, H> {
    fn clone(&self) -> Self {
        match self {
            Entry::HamtNode(node) => Entry::HamtNode(node.clone()),
            Entry::SmallSimdNode(node) => Entry::SmallSimdNode(node.clone()),
            Entry::LargeSimdNode(node) => Entry::LargeSimdNode(node.clone()),
            Entry::Value(value, hash) => Entry::Value(value.clone(), *hash),
            Entry::Collision(coll) => Entry::Collision(coll.clone()),
        }
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> Entry<A, P, H> {
    fn is_value(&self) -> bool {
        matches!(self, Entry::Value(_, _))
    }

    /// Return this entry's contribution to its parent's Merkle hash.
    /// For leaf values this is fmix64(key_hash). For child nodes the
    /// merkle_hash is passed through directly — the root hash is a flat
    /// sum of fmix64(leaf_hash) values, independent of tree structure.
    /// For collision nodes we include the entry count so that
    /// adding/removing colliding keys is detected.
    #[inline]
    pub(crate) fn merkle_contribution(&self) -> u64 {
        match self {
            Entry::Value(_, hash) => fmix64((*hash).to_u64()),
            Entry::SmallSimdNode(node) => node.merkle_hash,
            Entry::LargeSimdNode(node) => node.merkle_hash,
            Entry::HamtNode(node) => node.merkle_hash,
            Entry::Collision(coll) => {
                (coll.data.len() as u64).wrapping_mul(fmix64(coll.hash.to_u64()))
            }
        }
    }

    #[inline(always)]
    fn unwrap_value(self) -> A {
        match self {
            Entry::Value(a, _) => a,
            _ => panic!("nodes::hamt::Entry::unwrap_value: unwrapped a non-value"),
        }
    }

    /// Check whether two entries point to the same allocation (for node
    /// variants) or are the exact same value pointer. Returns false for
    /// Value entries (use PartialEq instead) and for entries of different
    /// types.
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Entry::HamtNode(a), Entry::HamtNode(b)) => SharedPointer::ptr_eq(a, b),
            (Entry::SmallSimdNode(a), Entry::SmallSimdNode(b)) => SharedPointer::ptr_eq(a, b),
            (Entry::LargeSimdNode(a), Entry::LargeSimdNode(b)) => SharedPointer::ptr_eq(a, b),
            (Entry::Collision(a), Entry::Collision(b)) => SharedPointer::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Collect all values reachable from this entry into the provided Vec.
    pub(crate) fn collect_values<'a>(&'a self, out: &mut Vec<&'a A>) {
        match self {
            Entry::Value(a, _) => out.push(a),
            Entry::HamtNode(node) => {
                for entry in node.data.iter() {
                    entry.collect_values(out);
                }
            }
            Entry::SmallSimdNode(node) => {
                for (a, _) in node.data.iter() {
                    out.push(a);
                }
            }
            Entry::LargeSimdNode(node) => {
                for (a, _) in node.data.iter() {
                    out.push(a);
                }
            }
            Entry::Collision(coll) => {
                for a in &coll.data {
                    out.push(a);
                }
            }
        }
    }
}

#[cfg(feature = "hash-intern")]
impl<A: Clone + PartialEq, P: SharedPointerKind, H: HashWidth> Entry<A, P, H> {
    /// Recursively intern this entry's child nodes bottom-up. Children
    /// are interned before parents so that parent equality checks can
    /// use `ptr_eq` on interned children.
    pub(crate) fn intern(&mut self, pool: &mut crate::intern::InternPool<A, P, H>) {
        match self {
            Entry::HamtNode(node_ptr) => {
                // First, intern children recursively
                let node = SharedPointer::make_mut(node_ptr);
                for entry in node.data.iter_mut() {
                    entry.intern(pool);
                }
                // Then intern this node itself
                *node_ptr = pool.intern_hamt(node_ptr.clone());
            }
            Entry::SmallSimdNode(node_ptr) => {
                // Leaf-only node — no children to recurse into
                *node_ptr = pool.intern_small(node_ptr.clone());
            }
            Entry::LargeSimdNode(node_ptr) => {
                // Leaf-only node — no children to recurse into
                *node_ptr = pool.intern_large(node_ptr.clone());
            }
            Entry::Collision(node_ptr) => {
                *node_ptr = pool.intern_collision(node_ptr.clone());
            }
            Entry::Value(_, _) => {
                // Leaf values are not interned — they are stored inline
            }
        }
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> From<CollisionNode<A, H>> for Entry<A, P, H> {
    fn from(node: CollisionNode<A, H>) -> Self {
        Entry::Collision(SharedPointer::new(node))
    }
}

/// Compare two hashes, returning true if the keys may be equal.
/// This function will always return true if it thinks keys may be cheap to compare.
#[inline]
fn hash_may_eq<A: HashValue, H: HashWidth>(hash: H, other_hash: H) -> bool {
    (!mem::needs_drop::<A::Key>() && mem::size_of::<A::Key>() <= 16) || hash == other_hash
}

impl<A: HashValue, H: HashWidth> CollisionNode<A, H> {
    #[cold]
    fn new(hash: H, value1: A, value2: A) -> Self {
        CollisionNode {
            hash,
            data: vec![value1, value2],
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    #[cold]
    fn get<Q>(&self, key: &Q) -> Option<&A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        self.data
            .iter()
            .find(|&entry| key.equivalent(entry.extract_key()))
    }

    #[cold]
    fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        self.data
            .iter_mut()
            .find(|entry| key.equivalent(entry.extract_key()))
    }

    #[cold]
    fn insert(&mut self, value: A) -> Option<A> {
        for item in &mut self.data {
            if value.extract_key() == item.extract_key() {
                return Some(mem::replace(item, value));
            }
        }
        self.data.push(value);
        None
    }

    #[cold]
    fn remove<Q>(&mut self, key: &Q) -> Option<A>
    where
        Q: Equivalent<A::Key> + ?Sized,
    {
        for (index, item) in self.data.iter().enumerate() {
            if key.equivalent(item.extract_key()) {
                return Some(self.data.swap_remove(index));
            }
        }
        None
    }

    #[inline]
    fn pop_value<P: SharedPointerKind>(&mut self) -> Entry<A, P, H> {
        Entry::Value(self.data.pop().unwrap(), self.hash)
    }
}

#[cfg(test)]
impl<A, P: SharedPointerKind, H: HashWidth> Node<A, P, H> {
    /// Analyze the node structure for debugging/statistics
    pub(crate) fn analyze_structure<F>(&self, mut visitor: F)
    where
        F: FnMut(&Entry<A, P, H>),
    {
        for i in self.data.indices() {
            visitor(&self.data[i]);
        }
    }
}

/// An allocation-free stack for iterators.
type InlineStack<T> = InlineArray<T, (usize, [T; ITER_STACK_CAPACITY])>;

#[allow(clippy::enum_variant_names)] // mirrors Entry enum variant names
enum IterItem<'a, A, P: SharedPointerKind, H: HashWidth = u64> {
    SmallSimdNode(ChunkIter<'a, (A, H), SMALL_NODE_WIDTH>),
    LargeSimdNode(ChunkIter<'a, (A, H), HASH_WIDTH>),
    HamtNode(ChunkIter<'a, Entry<A, P, H>, HASH_WIDTH>),
    CollisionNode(H, SliceIter<'a, A>),
}

// We manually impl Clone for IterItem to allow cloning even when A isn't Clone
// This works because the iterators hold references, not owned values
impl<'a, A, P: SharedPointerKind, H: HashWidth> Clone for IterItem<'a, A, P, H> {
    fn clone(&self) -> Self {
        match self {
            IterItem::SmallSimdNode(iter) => IterItem::SmallSimdNode(iter.clone()),
            IterItem::LargeSimdNode(iter) => IterItem::LargeSimdNode(iter.clone()),
            IterItem::HamtNode(iter) => IterItem::HamtNode(iter.clone()),
            IterItem::CollisionNode(hash, iter) => IterItem::CollisionNode(*hash, iter.clone()),
        }
    }
}

// Ref iterator

pub(crate) struct Iter<'a, A, P: SharedPointerKind, H: HashWidth = u64> {
    count: usize,
    stack: InlineStack<IterItem<'a, A, P, H>>,
}

// We impl Clone instead of deriving it, because we want Clone even if K and V aren't.
impl<'a, A, P: SharedPointerKind, H: HashWidth> Clone for Iter<'a, A, P, H> {
    fn clone(&self) -> Self {
        Self {
            count: self.count,
            stack: self.stack.clone(),
        }
    }
}

impl<'a, A, P, H> Iter<'a, A, P, H>
where
    A: 'a,
    P: SharedPointerKind,
    H: HashWidth,
{
    pub(crate) fn new(root: Option<&'a Node<A, P, H>>, size: usize) -> Self {
        let mut result = Iter {
            count: size,
            stack: InlineStack::new(),
        };
        if let Some(node) = root {
            result.stack.push(IterItem::HamtNode(node.data.iter()));
        }
        result
    }
}

impl<'a, A, P, H> Iterator for Iter<'a, A, P, H>
where
    A: 'a,
    P: SharedPointerKind,
    H: HashWidth,
{
    type Item = (&'a A, H);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(current) = self.stack.last_mut() {
            match current {
                IterItem::SmallSimdNode(iter) => {
                    if let Some((value, hash)) = iter.next() {
                        self.count -= 1;
                        return Some((value, *hash));
                    }
                }
                IterItem::LargeSimdNode(iter) => {
                    if let Some((value, hash)) = iter.next() {
                        self.count -= 1;
                        return Some((value, *hash));
                    }
                }
                IterItem::HamtNode(iter) => {
                    if let Some(entry) = iter.next() {
                        let iter_item = match entry {
                            Entry::Value(value, hash) => {
                                self.count -= 1;
                                return Some((value, *hash));
                            }
                            Entry::HamtNode(child) => IterItem::HamtNode(child.data.iter()),
                            Entry::SmallSimdNode(small) => {
                                IterItem::SmallSimdNode(small.data.iter())
                            }
                            Entry::LargeSimdNode(large) => {
                                IterItem::LargeSimdNode(large.data.iter())
                            }
                            Entry::Collision(coll) => {
                                IterItem::CollisionNode(coll.hash, coll.data.iter())
                            }
                        };
                        self.stack.push(iter_item);
                        continue;
                    }
                }
                IterItem::CollisionNode(hash, iter) => {
                    if let Some(value) = iter.next() {
                        self.count -= 1;
                        return Some((value, *hash));
                    }
                }
            }
            self.stack.pop();
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<'a, A, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for Iter<'a, A, P, H> where
    A: 'a
{
}

impl<'a, A, P: SharedPointerKind, H: HashWidth> FusedIterator for Iter<'a, A, P, H> where A: 'a {}

// Mut ref iterator

#[allow(clippy::enum_variant_names)] // mirrors Entry enum variant names
enum IterMutItem<'a, A, P: SharedPointerKind, H: HashWidth = u64> {
    SmallSimdNode(ChunkIterMut<'a, (A, H), SMALL_NODE_WIDTH>),
    LargeSimdNode(ChunkIterMut<'a, (A, H), HASH_WIDTH>),
    HamtNode(ChunkIterMut<'a, Entry<A, P, H>, HASH_WIDTH>),
    CollisionNode(H, SliceIterMut<'a, A>),
}

pub(crate) struct IterMut<'a, A, P: SharedPointerKind, H: HashWidth = u64> {
    count: usize,
    stack: InlineStack<IterMutItem<'a, A, P, H>>,
}

impl<'a, A, P, H> IterMut<'a, A, P, H>
where
    A: 'a,
    P: SharedPointerKind,
    H: HashWidth,
{
    pub(crate) fn new(root: Option<&'a mut Node<A, P, H>>, size: usize) -> Self {
        let mut result = IterMut {
            count: size,
            stack: InlineStack::new(),
        };
        if let Some(node) = root {
            result
                .stack
                .push(IterMutItem::HamtNode(node.data.iter_mut()));
        }
        result
    }
}

impl<'a, A, P, H> Iterator for IterMut<'a, A, P, H>
where
    A: Clone + 'a,
    P: SharedPointerKind,
    H: HashWidth,
{
    type Item = (&'a mut A, H);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(current) = self.stack.last_mut() {
            match current {
                IterMutItem::SmallSimdNode(iter) => {
                    if let Some((value, hash)) = iter.next() {
                        self.count -= 1;
                        return Some((value, *hash));
                    }
                }
                IterMutItem::LargeSimdNode(iter) => {
                    if let Some((value, hash)) = iter.next() {
                        self.count -= 1;
                        return Some((value, *hash));
                    }
                }
                IterMutItem::HamtNode(iter) => {
                    if let Some(entry) = iter.next() {
                        let iter_item = match entry {
                            Entry::Value(value, hash) => {
                                self.count -= 1;
                                return Some((value, *hash));
                            }
                            Entry::HamtNode(child_ref) => {
                                let child = SharedPointer::make_mut(child_ref);
                                IterMutItem::HamtNode(child.data.iter_mut())
                            }
                            Entry::SmallSimdNode(small_ref) => {
                                let small = SharedPointer::make_mut(small_ref);
                                IterMutItem::SmallSimdNode(small.data.iter_mut())
                            }
                            Entry::LargeSimdNode(large_ref) => {
                                let large = SharedPointer::make_mut(large_ref);
                                IterMutItem::LargeSimdNode(large.data.iter_mut())
                            }
                            Entry::Collision(coll_ref) => {
                                let coll = SharedPointer::make_mut(coll_ref);
                                IterMutItem::CollisionNode(coll.hash, coll.data.iter_mut())
                            }
                        };
                        self.stack.push(iter_item);
                        continue;
                    }
                }
                IterMutItem::CollisionNode(hash, iter) => {
                    if let Some(value) = iter.next() {
                        self.count -= 1;
                        return Some((value, *hash));
                    }
                }
            }
            self.stack.pop();
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<'a, A, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for IterMut<'a, A, P, H> where
    A: Clone + 'a
{
}

impl<'a, A, P: SharedPointerKind, H: HashWidth> FusedIterator for IterMut<'a, A, P, H> where
    A: Clone + 'a
{
}

// Consuming iterator

enum DrainItem<A, P: SharedPointerKind, H: HashWidth = u64> {
    SmallSimdNode(SharedPointer<SmallSimdNode<A, H>, P>),
    LargeSimdNode(SharedPointer<LargeSimdNode<A, H>, P>),
    HamtNode(SharedPointer<HamtNode<A, P, H>, P>),
    Collision(SharedPointer<CollisionNode<A, H>, P>),
}

pub(crate) struct Drain<A, P: SharedPointerKind, H: HashWidth = u64> {
    count: usize,
    stack: InlineStack<DrainItem<A, P, H>>,
}

impl<A, P: SharedPointerKind, H: HashWidth> Drain<A, P, H> {
    pub(crate) fn new(root: Option<SharedPointer<Node<A, P, H>, P>>, size: usize) -> Self {
        let mut result = Drain {
            count: size,
            stack: InlineStack::new(),
        };
        if let Some(root) = root {
            result.stack.push(DrainItem::HamtNode(root));
        }
        result
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> Iterator for Drain<A, P, H>
where
    A: Clone,
{
    type Item = (A, H);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(current) = self.stack.last_mut() {
            match current {
                DrainItem::SmallSimdNode(small_ref) => {
                    if let Some((value, hash)) = SharedPointer::make_mut(small_ref).data.pop() {
                        self.count -= 1;
                        return Some((value, hash));
                    }
                }
                DrainItem::LargeSimdNode(large_ref) => {
                    if let Some((value, hash)) = SharedPointer::make_mut(large_ref).data.pop() {
                        self.count -= 1;
                        return Some((value, hash));
                    }
                }
                DrainItem::HamtNode(node_ref) => {
                    if let Some(entry) = SharedPointer::make_mut(node_ref).data.pop() {
                        let drain_item = match entry {
                            Entry::Value(value, hash) => {
                                self.count -= 1;
                                return Some((value, hash));
                            }
                            Entry::HamtNode(child) => DrainItem::HamtNode(child),
                            Entry::SmallSimdNode(small) => DrainItem::SmallSimdNode(small),
                            Entry::LargeSimdNode(large) => DrainItem::LargeSimdNode(large),
                            Entry::Collision(coll) => DrainItem::Collision(coll),
                        };
                        self.stack.push(drain_item);
                        continue;
                    }
                }
                DrainItem::Collision(coll_ref) => {
                    let coll = SharedPointer::make_mut(coll_ref);
                    if let Some(value) = coll.data.pop() {
                        self.count -= 1;
                        return Some((value, coll.hash));
                    }
                }
            }
            self.stack.pop();
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<A, P: SharedPointerKind, H: HashWidth> ExactSizeIterator for Drain<A, P, H> where A: Clone {}

impl<A, P: SharedPointerKind, H: HashWidth> FusedIterator for Drain<A, P, H> where A: Clone {}

impl<A: fmt::Debug, P: SharedPointerKind, H: HashWidth> fmt::Debug for HamtNode<A, P, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "HamtNode[ ")?;
        for i in self.data.indices() {
            write!(f, "{}: ", i)?;
            match &self.data[i] {
                Entry::Value(v, _h) => write!(f, "{:?}, ", v)?,
                Entry::Collision(c) => write!(f, "Coll{:?}, ", c.data)?,
                Entry::HamtNode(n) => write!(f, "{:?}, ", n)?,
                Entry::SmallSimdNode(s) => write!(f, "{:?}, ", s)?,
                Entry::LargeSimdNode(l) => write!(f, "{:?}, ", l)?,
            }
        }
        write!(f, " ]")
    }
}

impl<A: fmt::Debug, H: HashWidth, const WIDTH: usize, const GROUPS: usize> fmt::Debug
    for GenericSimdNode<A, H, WIDTH, GROUPS>
where
    BitsImpl<WIDTH>: Bits,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "SimdNode<{}, {}>[ ", WIDTH, GROUPS)?;
        for i in self.data.indices() {
            write!(f, "{}: ", i)?;
            let (v, _h) = &self.data[i];
            write!(f, "{:?}, ", v)?;
        }
        write!(f, " ]")
    }
}
