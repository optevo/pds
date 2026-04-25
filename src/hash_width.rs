// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Configurable hash width for HAMT trie traversal.
//!
//! The [`HashWidth`] trait abstracts the hash representation used internally
//! by [`HashMap`](crate::hashmap::HashMap) and [`HashSet`](crate::hashset::HashSet).
//! The default implementation uses `u64` (12 usable trie levels with 5-bit
//! branching). A `u128` implementation provides 25 levels, virtually
//! eliminating collision nodes for any realistic collection size.
//!
//! # Choosing a hash width
//!
//! | Width | Trie levels | Collision threshold | Per-entry overhead |
//! |-------|-------------|--------------------|--------------------|
//! | `u64` | 12 | ~4 billion entries | 8 bytes |
//! | `u128` | 25 | ~10^19 entries | 16 bytes |
//!
//! Use `u64` (the default) unless your keys are inherently 128-bit values
//! (UUIDs, content hashes) and you want to exploit their full entropy via
//! an identity hasher.

use core::fmt::Debug;
use core::hash::Hash;

use crate::nodes::hamt::fmix64;

/// Trait for hash bit types used in HAMT trie traversal.
///
/// Implemented for [`u64`] (default) and [`u128`] (wide). The hash width
/// determines how many trie levels are available before hash exhaustion
/// triggers collision nodes.
pub trait HashWidth: Copy + Eq + Hash + Default + Debug + Send + Sync + 'static {
    /// Total number of usable hash bits.
    const BIT_COUNT: usize;

    /// Convert from the `u64` output of [`BuildHasher::hash_one()`].
    ///
    /// For `u64` this is the identity. For `u128` this extends the hash
    /// via mixing to fill the wider representation.
    ///
    /// [`BuildHasher::hash_one()`]: core::hash::BuildHasher::hash_one
    fn from_hash64(hash: u64) -> Self;

    /// Extract a trie level index: `(self >> shift) & width_mask`.
    ///
    /// `width_mask` is `HASH_WIDTH - 1` (e.g. 31 for 5-bit branching).
    fn trie_index(self, shift: usize, width_mask: usize) -> usize;

    /// Extract SIMD control byte from the high bits of the hash.
    /// Must return a non-zero value (used as a sentinel in SIMD probing).
    fn ctrl_byte(self) -> u8;

    /// Extract SIMD group index from the high bits of the hash.
    /// `num_groups` is the number of SIMD groups in the node (1 or 2).
    fn ctrl_group(self, num_groups: usize) -> usize;

    /// Convert to `u64` for Merkle hash computation. For `u64` this is
    /// identity. For wider types, truncation is acceptable since Merkle
    /// hashes are always `u64` regardless of trie hash width.
    fn to_u64(self) -> u64;
}

impl HashWidth for u64 {
    const BIT_COUNT: usize = 64;

    #[inline]
    fn from_hash64(hash: u64) -> Self {
        hash
    }

    #[inline]
    fn trie_index(self, shift: usize, width_mask: usize) -> usize {
        ((self >> shift) as usize) & width_mask
    }

    #[inline]
    fn ctrl_byte(self) -> u8 {
        ((self >> (u64::BITS - 8)) as u8).max(1)
    }

    #[inline]
    fn ctrl_group(self, num_groups: usize) -> usize {
        if num_groups == 1 {
            return 0;
        }
        (self >> (u64::BITS.saturating_sub(9))) as usize % num_groups
    }

    #[inline]
    fn to_u64(self) -> u64 {
        self
    }
}

impl HashWidth for u128 {
    const BIT_COUNT: usize = 128;

    #[inline]
    fn from_hash64(hash: u64) -> Self {
        // Extend u64 to u128 via wide-multiply mixing. The low 64 bits
        // preserve the original hash; the high 64 bits are independently
        // mixed to provide additional trie levels.
        let hi = fmix64(hash);
        ((hi as u128) << 64) | (hash as u128)
    }

    #[inline]
    fn trie_index(self, shift: usize, width_mask: usize) -> usize {
        ((self >> shift) as usize) & width_mask
    }

    #[inline]
    fn ctrl_byte(self) -> u8 {
        ((self >> (u128::BITS - 8)) as u8).max(1)
    }

    #[inline]
    fn ctrl_group(self, num_groups: usize) -> usize {
        if num_groups == 1 {
            return 0;
        }
        (self >> (u128::BITS.saturating_sub(9))) as usize % num_groups
    }

    #[inline]
    fn to_u64(self) -> u64 {
        self as u64
    }
}
