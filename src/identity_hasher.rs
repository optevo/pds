// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! An identity hasher for integer keys that are already well-distributed.
//!
//! [`IdentityHasher`] passes key bytes directly as the hash value, eliminating
//! hash computation entirely. It is designed for keys whose bits are already
//! uniformly distributed — UUIDs, content hashes, random `u64`/`u128` values,
//! and similar.
//!
//! # When to use
//!
//! Use the identity hasher when:
//! - Keys are `u64`, `u32`, `usize`, or `u128` values with good bit distribution
//! - Keys are cryptographic or content-derived hashes (e.g. SHA-256 truncated to u64)
//! - Keys are UUIDs or other random-looking identifiers
//!
//! Do **not** use it when:
//! - Keys are small sequential integers (0, 1, 2, …) — the HAMT will skew because
//!   low bits cluster in the same trie level
//! - Keys are strings, structs, or any non-integer type — the XOR-fold fallback in
//!   `write()` is not a quality hash
//!
//! # Cross-session consistency
//!
//! Because `IdentityHasher` applies no randomisation, the same integer key always
//! produces the same hash value across processes and restarts. This property enables
//! several advanced use cases:
//!
//! - **Cross-session `InternPool` merging.** If a `HashMap` is serialised via the
//!   `persist` feature and later deserialised, the reconstructed HAMT nodes have the
//!   same trie paths as nodes built at runtime from the same keys. The `hash-intern`
//!   feature can then merge both pools by Merkle hash comparison — identical subtrees
//!   are deduplicated by pointer, saving memory without a full structural comparison.
//! - **Reproducible Merkle hashes.** The HAMT's per-node Merkle hashes depend on key
//!   hashes. With `IdentityBuildHasher` those hashes are deterministic, so a snapshot
//!   taken in one run can be diff-ed against a snapshot from another run using only
//!   the root Merkle hash — equal roots mean byte-for-byte identical trees.
//! - **Reproducible tests.** Tests that assert on serialised bytes or internal node
//!   structure pass consistently across machines and CI runs without controlling the
//!   random seed externally.
//!
//! **Security caveat:** `IdentityBuildHasher` provides no hash randomisation and is
//! therefore vulnerable to Hash DoS from untrusted input (an attacker who can choose
//! keys can force all keys into the same trie bucket). Use it only when all keys come
//! from a trusted source — your own code, a closed serialisation format, or internal
//! integer identifiers. For untrusted user input, keep the default `RandomState`.
//!
//! # Example
//!
//! ```
//! use pds::GenericHashMap;
//! use pds::shared_ptr::DefaultSharedPtr;
//! use pds::identity_hasher::IdentityBuildHasher;
//!
//! // Content hashes as keys — already random, no hashing needed.
//! let mut map: GenericHashMap<u64, String, IdentityBuildHasher, DefaultSharedPtr> =
//!     GenericHashMap::with_hasher(IdentityBuildHasher);
//! map.insert(0xdeadbeef_cafebabe_u64, "block-A".into());
//! map.insert(0x01234567_89abcdef_u64, "block-B".into());
//!
//! assert_eq!(map.get(&0xdeadbeef_cafebabe_u64), Some(&"block-A".into()));
//! ```

use core::hash::{BuildHasher, Hasher};

// ─── IdentityHasher ──────────────────────────────────────────────────

/// A [`Hasher`] that returns the key value directly as the hash.
///
/// Specialised `write_*` methods for all integer types store the value
/// unchanged. The generic `write()` fallback XOR-folds bytes into a `u64`
/// state — it is correct but not a quality hash. Use the identity hasher
/// only with integer keys.
///
/// For `u128` keys, the two 64-bit halves are XOR-folded, producing a `u64`
/// that preserves entropy from both halves. This loses information relative
/// to a full 128-bit representation, which is unavoidable because [`Hasher::finish`]
/// returns `u64`. For maximum bit-width on 128-bit keys, pair with a `u128`
/// [`HashWidth`](crate::hash_width::HashWidth) parameter on the collection.
#[derive(Clone, Debug, Default)]
pub struct IdentityHasher {
    value: u64,
}

impl Hasher for IdentityHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Fallback path for non-integer types. XOR-fold 8-byte chunks into
        // the state. This is not a quality hash — IdentityHasher is designed
        // for integer keys where the specific write_* methods are called.
        for chunk in bytes.chunks(8) {
            let mut buf = [0u8; 8];
            buf[..chunk.len()].copy_from_slice(chunk);
            self.value ^= u64::from_le_bytes(buf);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.value = i as u64;
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.value = i as u64;
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.value = i as u64;
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.value = i;
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        // XOR the two 64-bit halves to produce a u64. Both halves contribute
        // entropy to every trie level. This is the best available approximation
        // within the Hasher::finish() → u64 constraint.
        self.value = (i as u64) ^ ((i >> 64) as u64);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.value = i as u64;
    }

    #[inline]
    fn write_i8(&mut self, i: i8) {
        self.value = i as u64;
    }

    #[inline]
    fn write_i16(&mut self, i: i16) {
        self.value = i as u64;
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.value = i as u64;
    }

    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.value = i as u64;
    }

    #[inline]
    fn write_i128(&mut self, i: i128) {
        self.value = (i as u64) ^ ((i >> 64) as u64);
    }

    #[inline]
    fn write_isize(&mut self, i: isize) {
        self.value = i as u64;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.value
    }
}

// ─── IdentityBuildHasher ─────────────────────────────────────────────

/// A [`BuildHasher`] that constructs [`IdentityHasher`] instances.
///
/// Use this as the `S` type parameter on [`HashMap`](crate::HashMap) and
/// [`HashSet`](crate::HashSet) when keys are well-distributed integers.
/// `IdentityBuildHasher` is zero-sized and `Copy`.
///
/// # Example
///
/// ```
/// use pds::GenericHashSet;
/// use pds::shared_ptr::DefaultSharedPtr;
/// use pds::identity_hasher::IdentityBuildHasher;
///
/// let mut set: GenericHashSet<u64, IdentityBuildHasher, DefaultSharedPtr> =
///     GenericHashSet::with_hasher(IdentityBuildHasher);
/// set.insert(0x_feed_face_dead_beef_u64);
/// assert!(set.contains(&0x_feed_face_dead_beef_u64));
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityBuildHasher;

impl BuildHasher for IdentityBuildHasher {
    type Hasher = IdentityHasher;

    #[inline]
    fn build_hasher(&self) -> IdentityHasher {
        IdentityHasher::default()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use core::hash::{BuildHasher, Hash, Hasher};

    use super::*;

    fn hash_value<T: Hash>(bh: &IdentityBuildHasher, val: T) -> u64 {
        bh.hash_one(val)
    }

    #[test]
    fn u64_is_identity() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 0u64), 0);
        assert_eq!(hash_value(&bh, 1u64), 1);
        assert_eq!(hash_value(&bh, u64::MAX), u64::MAX);
        assert_eq!(
            hash_value(&bh, 0xdeadbeef_cafebabe_u64),
            0xdeadbeef_cafebabe_u64
        );
    }

    #[test]
    fn u32_is_identity() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 0u32), 0);
        assert_eq!(hash_value(&bh, 42u32), 42);
        assert_eq!(hash_value(&bh, u32::MAX), u32::MAX as u64);
    }

    #[test]
    fn usize_is_identity() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 99usize), 99);
    }

    #[test]
    fn u128_xor_folds() {
        let bh = IdentityBuildHasher;
        let val: u128 = 0x0102030405060708_090a0b0c0d0e0f10;
        let expected = (val as u64) ^ ((val >> 64) as u64);
        assert_eq!(hash_value(&bh, val), expected);
    }

    #[test]
    fn write_u64_direct() {
        let mut h = IdentityHasher::default();
        h.write_u64(12345);
        assert_eq!(h.finish(), 12345);
    }

    #[test]
    fn write_overrides_previous_state() {
        // Each write_* replaces (not combines) the state — this is intentional
        // for integer keys where only one write_* call is made per key.
        let mut h = IdentityHasher::default();
        h.write_u64(999);
        h.write_u64(42);
        assert_eq!(h.finish(), 42);
    }

    #[test]
    fn identity_hasher_in_hashmap() {
        use crate::shared_ptr::DefaultSharedPtr;
        use crate::GenericHashMap;
        let mut map: GenericHashMap<u64, &str, IdentityBuildHasher, DefaultSharedPtr> =
            GenericHashMap::with_hasher(IdentityBuildHasher);
        map.insert(1_u64, "one");
        map.insert(2_u64, "two");
        map.insert(u64::MAX, "max");

        assert_eq!(map.get(&1_u64), Some(&"one"));
        assert_eq!(map.get(&2_u64), Some(&"two"));
        assert_eq!(map.get(&u64::MAX), Some(&"max"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn identity_hasher_in_hashset() {
        use crate::shared_ptr::DefaultSharedPtr;
        use crate::GenericHashSet;
        let mut set: GenericHashSet<u64, IdentityBuildHasher, DefaultSharedPtr> =
            GenericHashSet::with_hasher(IdentityBuildHasher);
        set.insert(100_u64);
        set.insert(200_u64);
        set.insert(300_u64);

        assert!(set.contains(&100_u64));
        assert!(set.contains(&200_u64));
        assert!(!set.contains(&999_u64));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn large_map_with_identity_hasher() {
        // Use well-distributed keys (multiply by a mixing constant) to
        // exercise the HAMT at scale without sequential clustering.
        use crate::shared_ptr::DefaultSharedPtr;
        use crate::GenericHashMap;
        let mut map: GenericHashMap<u64, u64, IdentityBuildHasher, DefaultSharedPtr> =
            GenericHashMap::with_hasher(IdentityBuildHasher);
        for i in 0u64..1000 {
            let key = i.wrapping_mul(0x9e3779b97f4a7c15);
            map.insert(key, i);
        }
        assert_eq!(map.len(), 1000);
        for i in 0u64..1000 {
            let key = i.wrapping_mul(0x9e3779b97f4a7c15);
            assert_eq!(map.get(&key), Some(&i));
        }
    }

    #[test]
    fn write_u16_is_identity() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 0u16), 0);
        assert_eq!(hash_value(&bh, 1000u16), 1000);
        assert_eq!(hash_value(&bh, u16::MAX), u16::MAX as u64);
    }

    #[test]
    fn write_i8_is_cast() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 0i8), 0);
        assert_eq!(hash_value(&bh, 1i8), 1);
        // Negative values cast to u64 via as-cast (wrapping)
        assert_eq!(hash_value(&bh, -1i8), (-1i8 as u64));
    }

    #[test]
    fn write_i16_is_cast() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 0i16), 0);
        assert_eq!(hash_value(&bh, -1i16), (-1i16 as u64));
    }

    #[test]
    fn write_i32_is_cast() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 42i32), 42);
        assert_eq!(hash_value(&bh, -42i32), (-42i32 as u64));
    }

    #[test]
    fn write_i64_is_cast() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 0i64), 0);
        assert_eq!(hash_value(&bh, i64::MIN), (i64::MIN as u64));
    }

    #[test]
    fn write_i128_xor_folds() {
        let bh = IdentityBuildHasher;
        let val: i128 = 0x0102030405060708_090a0b0c0d0e0f10_i128;
        let expected = (val as u64) ^ ((val >> 64) as u64);
        assert_eq!(hash_value(&bh, val), expected);
    }

    #[test]
    fn write_isize_is_cast() {
        let bh = IdentityBuildHasher;
        assert_eq!(hash_value(&bh, 123isize), 123);
    }
}
