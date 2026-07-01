//! HAMT node types and 512-byte slab slot layout.
//!
//! Every HAMT node — whether a leaf or an internal node — is stored in a
//! fixed-size 512-byte slab slot ([`HamtNodePage`]).  The first byte is a
//! discriminant that identifies which variant occupies the slot.
//!
//! # Node variants
//!
//! ## Leaf nodes
//!
//! A leaf holds up to [`LEAF_CAP`] key/value pairs.  The layout inside the
//! 512-byte slot uses a fixed-size header (to avoid shifting data when entries
//! are added) followed by a data section:
//!
//! ```text
//! Offset   Size   Field
//!      0      1   discriminant     u8 — DISCRIMINANT_LEAF (0x01)
//!      1      1   count            u8 — number of entries (0..=LEAF_CAP)
//!      2  8·16   key_hashes       [u64; LEAF_CAP] — stored in slots 0..count
//!    130  2·17   entry_offsets    [u16; LEAF_CAP+1] — offsets[i] = start of
//!                                 entry i in data; offsets[count] = total bytes
//!    164    348   data             packed framed (key, value) pairs
//! ```
//!
//! Each entry in `data` is framed as:
//! `[key_len: u16 LE][encoded key bytes][encoded value bytes]`
//!
//! This framing lets [`LeafReader::get_entry`] split the key from the value
//! without a second offset table.
//!
//! With `LEAF_CAP = 16`:
//! - Header: 2 + 128 + 34 = 164 bytes (discriminant + count + hashes + offsets)
//! - Data: 512 − 164 = 348 bytes available for entries
//!
//! ## Internal nodes
//!
//! An internal node uses a 32-bit bitmap (one bit per 5-bit HAMT path
//! component — 2^5 = 32 possible children) and a compressed child array.
//!
//! ```text
//! Offset   Size   Field
//!      0      1   discriminant   u8 — DISCRIMINANT_INTERNAL (0x02)
//!      1      3   _pad           [u8; 3] — reserved, must be zero
//!      4      4   bitmap         u32 LE — one bit per child position (0..32)
//!      8  8·P    children       [u64; popcount(bitmap)] LE — slab page IDs
//! ```
//!
//! `P = popcount(bitmap)` ≤ 32.  Maximum node size: 8 + 256 = 264 bytes,
//! comfortably within the 512-byte slot.
//!
//! # HAMT branching
//!
//! Each HAMT level consumes [`BRANCH_BITS`] = 5 bits of the key hash,
//! giving up to 12 levels for a 64-bit hash.
//!
//! # Slab slot type
//!
//! [`HamtNodePage`] is a `[u8; PAGE_BYTES]` newtype deriving [`bytemuck::Pod`]
//! and [`bytemuck::Zeroable`] via `bytemuck`'s proc-macro (which generates the
//! `unsafe impl` internally, keeping this crate free of `unsafe` code).

use bytemuck::{Pod, Zeroable};

use crate::codec::{CodecError, ValueCodec};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Size of each slab slot in bytes.
///
/// A power of two large enough to fit both node variants with comfortable
/// headroom.  Eight slots fit in a 4 KiB folio page.
pub const PAGE_BYTES: usize = 512;

/// Discriminant value stored at byte 0 of a leaf slot.
pub const DISCRIMINANT_LEAF: u8 = 0x01;

/// Discriminant value stored at byte 0 of an internal slot.
pub const DISCRIMINANT_INTERNAL: u8 = 0x02;

/// Maximum number of entries a leaf can hold before a split is required.
///
/// With `LEAF_CAP = 16` the fixed header occupies 164 bytes, leaving
/// 348 bytes for serialised entry data.
pub const LEAF_CAP: usize = 16;

/// HAMT branching factor in bits.  Five bits per level → 32 children.
pub const BRANCH_BITS: u32 = 5;

/// Mask for the 5-bit path component extracted at each HAMT level.
pub const BRANCH_MASK: u32 = (1 << BRANCH_BITS) - 1;

// ---------------------------------------------------------------------------
// Byte offsets derived from LEAF_CAP (compile-time constants)
// ---------------------------------------------------------------------------

/// Byte offset at which `key_hashes` begins (= 2).
const HASH_BASE: usize = 2;

/// Byte offset at which `entry_offsets` begins (= 2 + LEAF_CAP * 8 = 130).
const OFFSET_BASE: usize = 2 + LEAF_CAP * 8;

/// Byte offset at which the data section begins (= OFFSET_BASE + (LEAF_CAP+1)*2 = 164).
const DATA_START: usize = OFFSET_BASE + (LEAF_CAP + 1) * 2;

// ---------------------------------------------------------------------------
// HamtNodePage — slab slot type
// ---------------------------------------------------------------------------

/// A fixed-size 512-byte slab slot containing one HAMT node.
///
/// The discriminant at byte 0 identifies the node type:
/// - `0x01` ([`DISCRIMINANT_LEAF`]) — interpreted by [`LeafReader`] /
///   [`LeafBuilder`]
/// - `0x02` ([`DISCRIMINANT_INTERNAL`]) — interpreted by [`InternalReader`] /
///   [`build_internal`]
/// - `0x00` — unallocated (all-zero) slot
///
/// [`Pod`] and [`Zeroable`] are derived via `bytemuck`'s proc-macro;
/// no `unsafe` code is required in this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
#[repr(transparent)]
pub struct HamtNodePage(pub [u8; PAGE_BYTES]);

impl Default for HamtNodePage {
    /// Returns an unallocated (all-zero) page.
    fn default() -> Self {
        Self([0u8; PAGE_BYTES])
    }
}

// ---------------------------------------------------------------------------
// LeafBuilder
// ---------------------------------------------------------------------------

/// Constructs a [`HamtNodePage`] containing a leaf node.
///
/// Entries are appended in the order they are pushed.  The caller is
/// responsible for ordering entries by key hash (ascending) if sorted
/// traversal or binary search is required.
///
/// After all entries have been added, call [`LeafBuilder::finish`] to obtain
/// the completed page.
///
/// # Entry framing
///
/// Each entry is written to the data section as:
/// `[key_len: u16 LE][encoded key bytes][encoded value bytes]`
#[derive(Debug)]
pub struct LeafBuilder {
    /// Scratch buffer — becomes the finished page on [`LeafBuilder::finish`].
    page: [u8; PAGE_BYTES],
    /// Number of entries written so far.
    count: usize,
    /// Total bytes written to the data section.
    data_written: usize,
}

impl LeafBuilder {
    /// Creates a new, empty leaf builder.
    ///
    /// Time: O(PAGE_BYTES) — zeroes the scratch buffer.
    #[must_use]
    pub fn new() -> Self {
        let mut page = [0u8; PAGE_BYTES];
        page[0] = DISCRIMINANT_LEAF;
        // count (byte 1) starts at 0 — already zeroed.
        Self {
            page,
            count: 0,
            data_written: 0,
        }
    }

    /// Returns the number of bytes remaining in the data section after a
    /// hypothetical push with `entry_len` encoded bytes (key-length prefix
    /// included).
    ///
    /// Time: O(1).
    #[must_use]
    fn bytes_available(&self, entry_len: usize) -> Option<usize> {
        let end = DATA_START + self.data_written + entry_len;
        PAGE_BYTES.checked_sub(end)
    }

    /// Appends one key/value entry.
    ///
    /// Encodes the key and value using codec `C`, writes a 2-byte key-length
    /// prefix, and stores the hash and offset in the header.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Encode`] if:
    /// - The leaf already holds [`LEAF_CAP`] entries.
    /// - The encoded entry (including the 2-byte prefix) does not fit in the
    ///   remaining data space.
    /// - Codec encoding fails.
    ///
    /// Time: O(key_size + value_size).
    pub fn push_framed<K, V, C>(
        &mut self,
        key_hash: u64,
        key: &K,
        value: &V,
    ) -> Result<(), CodecError>
    where
        C: ValueCodec<K> + ValueCodec<V>,
    {
        if self.count >= LEAF_CAP {
            return Err(CodecError::Encode(format!(
                "leaf full: count {} >= LEAF_CAP {}",
                self.count, LEAF_CAP
            )));
        }

        let mut key_bytes = Vec::new();
        <C as ValueCodec<K>>::encode(key, &mut key_bytes)?;
        let mut val_bytes = Vec::new();
        <C as ValueCodec<V>>::encode(value, &mut val_bytes)?;

        if key_bytes.len() > u16::MAX as usize {
            return Err(CodecError::Encode(
                "key encoding exceeds u16::MAX bytes".into(),
            ));
        }

        // Entry layout: [key_len: u16][key_bytes][val_bytes]
        let entry_len = 2 + key_bytes.len() + val_bytes.len();

        if self.bytes_available(entry_len).is_none() {
            return Err(CodecError::Encode(format!(
                "leaf data section full: need {entry_len} bytes but only {} remaining",
                PAGE_BYTES.saturating_sub(DATA_START + self.data_written)
            )));
        }

        let count = self.count;

        // Write key_hash at HASH_BASE + count*8.
        let hash_offset = HASH_BASE + count * 8;
        self.page[hash_offset..hash_offset + 8].copy_from_slice(&key_hash.to_le_bytes());

        // Write entry_offsets[count] = data_written (start of this entry).
        let offset_pos = OFFSET_BASE + count * 2;
        self.page[offset_pos..offset_pos + 2]
            .copy_from_slice(&(self.data_written as u16).to_le_bytes());

        // Write the framed entry.
        let data_pos = DATA_START + self.data_written;
        let key_len = key_bytes.len() as u16;
        self.page[data_pos..data_pos + 2].copy_from_slice(&key_len.to_le_bytes());
        self.page[data_pos + 2..data_pos + 2 + key_bytes.len()].copy_from_slice(&key_bytes);
        self.page[data_pos + 2 + key_bytes.len()..data_pos + entry_len].copy_from_slice(&val_bytes);

        self.data_written += entry_len;
        self.count += 1;

        // Write sentinel: entry_offsets[count] = data_written (total bytes written).
        let sentinel_pos = OFFSET_BASE + self.count * 2;
        self.page[sentinel_pos..sentinel_pos + 2]
            .copy_from_slice(&(self.data_written as u16).to_le_bytes());

        // Update count byte.
        self.page[1] = self.count as u8;

        Ok(())
    }

    /// Finalises the builder and returns the completed [`HamtNodePage`].
    ///
    /// Time: O(1).
    #[must_use]
    pub fn finish(self) -> HamtNodePage {
        HamtNodePage(self.page)
    }
}

impl Default for LeafBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LeafReader
// ---------------------------------------------------------------------------

/// A read-only view over a [`HamtNodePage`] that holds a leaf node.
///
/// The layout matches the fixed-header format produced by [`LeafBuilder`].
/// Accessor methods decode fields on the fly from the backing bytes; no
/// heap allocation occurs during reads.
pub struct LeafReader<'a> {
    /// Raw bytes of the slot.
    bytes: &'a [u8; PAGE_BYTES],
    /// Cached entry count.
    count: usize,
}

impl<'a> LeafReader<'a> {
    /// Borrows `page` as a leaf reader.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if the discriminant is not [`DISCRIMINANT_LEAF`].
    #[inline]
    #[must_use]
    pub fn new(page: &'a HamtNodePage) -> Self {
        debug_assert_eq!(
            page.0[0], DISCRIMINANT_LEAF,
            "LeafReader::new called on non-leaf page (discriminant = {:#x})",
            page.0[0]
        );
        let count = page.0[1] as usize;
        Self {
            bytes: &page.0,
            count,
        }
    }

    /// Returns the number of entries.
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Returns the full 64-bit key hash for entry `i`.
    ///
    /// # Panics
    ///
    /// Panics if `i >= self.count()`.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn key_hash(&self, i: usize) -> u64 {
        assert!(
            i < self.count,
            "LeafReader::key_hash: index {i} >= count {}",
            self.count
        );
        let base = HASH_BASE + i * 8;
        u64::from_le_bytes(self.bytes[base..base + 8].try_into().expect("8-byte slice"))
    }

    /// Decodes the key/value pair at entry `i` using codec `C`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if decoding fails or the entry bytes are
    /// malformed.
    ///
    /// # Panics
    ///
    /// Panics if `i >= self.count()`.
    ///
    /// Time: O(entry_size).
    pub fn get_entry<K, V, C>(&self, i: usize) -> Result<(K, V), CodecError>
    where
        C: ValueCodec<K> + ValueCodec<V>,
    {
        assert!(
            i < self.count,
            "LeafReader::get_entry: index {i} >= count {}",
            self.count
        );

        let start = u16::from_le_bytes(
            self.bytes[OFFSET_BASE + i * 2..OFFSET_BASE + i * 2 + 2]
                .try_into()
                .expect("2-byte slice"),
        ) as usize;
        let end = u16::from_le_bytes(
            self.bytes[OFFSET_BASE + (i + 1) * 2..OFFSET_BASE + (i + 1) * 2 + 2]
                .try_into()
                .expect("2-byte slice"),
        ) as usize;

        let entry_bytes = &self.bytes[DATA_START + start..DATA_START + end];

        if entry_bytes.len() < 2 {
            return Err(CodecError::Decode(
                "entry too short: missing key-length prefix".into(),
            ));
        }
        let key_len = u16::from_le_bytes([entry_bytes[0], entry_bytes[1]]) as usize;
        if 2 + key_len > entry_bytes.len() {
            return Err(CodecError::Decode(format!(
                "key_len {key_len} exceeds entry length {}",
                entry_bytes.len()
            )));
        }

        let key_bytes = &entry_bytes[2..2 + key_len];
        let val_bytes = &entry_bytes[2 + key_len..];
        let key = <C as ValueCodec<K>>::decode(key_bytes)?;
        let value = <C as ValueCodec<V>>::decode(val_bytes)?;
        Ok((key, value))
    }
}

// ---------------------------------------------------------------------------
// InternalReader / build_internal
// ---------------------------------------------------------------------------

/// A read-only view over a [`HamtNodePage`] that holds an internal HAMT node.
///
/// The layout is described in the module documentation.
pub struct InternalReader<'a> {
    /// Raw bytes of the slot.
    bytes: &'a [u8; PAGE_BYTES],
}

impl<'a> InternalReader<'a> {
    /// Borrows `page` as an internal node reader.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if the discriminant is not [`DISCRIMINANT_INTERNAL`].
    #[inline]
    #[must_use]
    pub fn new(page: &'a HamtNodePage) -> Self {
        debug_assert_eq!(
            page.0[0], DISCRIMINANT_INTERNAL,
            "InternalReader::new called on non-internal page (discriminant = {:#x})",
            page.0[0]
        );
        Self { bytes: &page.0 }
    }

    /// Returns the 32-bit child bitmap.
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn bitmap(&self) -> u32 {
        u32::from_le_bytes(self.bytes[4..8].try_into().expect("4-byte slice"))
    }

    /// Returns the number of children (popcount of the bitmap).
    ///
    /// Time: O(1).
    #[inline]
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.bitmap().count_ones() as usize
    }

    /// Returns the compressed child index for HAMT path component `bit_pos`
    /// (0..32), or `None` if that child does not exist.
    ///
    /// The compressed index is the popcount of bits strictly below `bit_pos`.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn child_index(&self, bit_pos: u32) -> Option<usize> {
        debug_assert!(bit_pos < 32, "bit_pos must be < 32");
        let bm = self.bitmap();
        if bm & (1 << bit_pos) == 0 {
            return None;
        }
        let mask = (1u32 << bit_pos) - 1;
        Some((bm & mask).count_ones() as usize)
    }

    /// Returns the slab page ID of the compressed child at index `idx`.
    ///
    /// `idx` is the compressed index (not the bit position).
    ///
    /// # Panics
    ///
    /// Panics if `idx >= child_count()`.
    ///
    /// Time: O(1).
    #[must_use]
    pub fn child_page_id(&self, idx: usize) -> u64 {
        let cc = self.child_count();
        assert!(
            idx < cc,
            "InternalReader::child_page_id: index {idx} >= count {cc}"
        );
        let base = 8 + idx * 8;
        u64::from_le_bytes(self.bytes[base..base + 8].try_into().expect("8-byte slice"))
    }
}

/// Builds an internal HAMT node page from a bitmap and compressed child array.
///
/// # Panics
///
/// Panics if `children.len() != bitmap.count_ones() as usize` or if the
/// node would exceed [`PAGE_BYTES`] (impossible with a 32-bit bitmap and
/// 64-bit page IDs, but checked for safety).
#[must_use]
pub fn build_internal(bitmap: u32, children: &[u64]) -> HamtNodePage {
    let expected = bitmap.count_ones() as usize;
    assert_eq!(
        children.len(),
        expected,
        "build_internal: bitmap popcount {expected} != children.len() {}",
        children.len()
    );
    let required = 8 + children.len() * 8;
    assert!(
        required <= PAGE_BYTES,
        "build_internal: node size {required} exceeds PAGE_BYTES {PAGE_BYTES}"
    );

    let mut page = [0u8; PAGE_BYTES];
    page[0] = DISCRIMINANT_INTERNAL;
    page[4..8].copy_from_slice(&bitmap.to_le_bytes());
    for (i, &child_id) in children.iter().enumerate() {
        let base = 8 + i * 8;
        page[base..base + 8].copy_from_slice(&child_id.to_le_bytes());
    }
    HamtNodePage(page)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::PodCodec;
    #[cfg(feature = "serde")]
    use crate::codec::PostcardCodec;

    // -----------------------------------------------------------------------
    // Size / layout assertions
    // -----------------------------------------------------------------------

    #[test]
    fn hamt_node_page_is_512_bytes() {
        assert_eq!(core::mem::size_of::<HamtNodePage>(), PAGE_BYTES);
    }

    #[test]
    fn hamt_node_page_is_pod() {
        let page = HamtNodePage::default();
        let bytes = bytemuck::bytes_of(&page);
        assert_eq!(bytes.len(), PAGE_BYTES);
        assert!(
            bytes.iter().all(|&b| b == 0),
            "default page should be all-zero"
        );
    }

    #[test]
    fn leaf_header_constants_are_correct() {
        // HASH_BASE = 2, OFFSET_BASE = 130, DATA_START = 164.
        assert_eq!(HASH_BASE, 2);
        assert_eq!(OFFSET_BASE, 130);
        assert_eq!(DATA_START, 164);
        // Data section is 348 bytes.
        assert_eq!(PAGE_BYTES - DATA_START, 348);
    }

    #[test]
    fn internal_node_max_size_fits_in_slot() {
        // 8-byte header + 32 children × 8 bytes = 264 bytes ≤ PAGE_BYTES.
        const { assert!(8 + 32 * 8 <= PAGE_BYTES) }
    }

    // -----------------------------------------------------------------------
    // LeafBuilder / LeafReader round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn leaf_empty() {
        let page = LeafBuilder::new().finish();
        assert_eq!(page.0[0], DISCRIMINANT_LEAF);
        assert_eq!(page.0[1], 0);
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 0);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn leaf_postcard_single_entry_round_trip() {
        let mut builder = LeafBuilder::new();
        builder
            .push_framed::<_, _, PostcardCodec>(
                0xDEAD_BEEF_1234_5678u64,
                &"hello".to_string(),
                &"world".to_string(),
            )
            .expect("push must succeed");
        let page = builder.finish();

        assert_eq!(page.0[0], DISCRIMINANT_LEAF);
        assert_eq!(page.0[1], 1);

        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 1);
        assert_eq!(reader.key_hash(0), 0xDEAD_BEEF_1234_5678u64);

        let (k, v) = reader
            .get_entry::<String, String, PostcardCodec>(0)
            .expect("get_entry must succeed");
        assert_eq!(k, "hello");
        assert_eq!(v, "world");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn leaf_postcard_multiple_entries_round_trip() {
        let mut builder = LeafBuilder::new();
        for i in 0u64..8 {
            builder
                .push_framed::<_, _, PostcardCodec>(i, &format!("key{i}"), &(i * 10u64))
                .expect("push must succeed");
        }
        let page = builder.finish();

        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 8);
        for i in 0u64..8 {
            assert_eq!(reader.key_hash(i as usize), i);
            let (k, v) = reader
                .get_entry::<String, u64, PostcardCodec>(i as usize)
                .expect("get_entry must succeed");
            assert_eq!(k, format!("key{i}"));
            assert_eq!(v, i * 10);
        }
    }

    #[test]
    fn leaf_pod_u64_keys_and_values_round_trip() {
        let mut builder = LeafBuilder::new();
        for i in 0u64..4 {
            builder
                .push_framed::<_, _, PodCodec>(i * 100, &i, &(i * i))
                .expect("push must succeed");
        }
        let page = builder.finish();

        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 4);
        for i in 0u64..4 {
            let (k, v) = reader
                .get_entry::<u64, u64, PodCodec>(i as usize)
                .expect("get_entry must succeed");
            assert_eq!(k, i);
            assert_eq!(v, i * i);
        }
    }

    #[test]
    fn leaf_rejects_overflow_at_cap() {
        let mut builder = LeafBuilder::new();
        for i in 0..LEAF_CAP {
            // Use u64 pairs (fixed size, always fit) to fill up to LEAF_CAP.
            builder
                .push_framed::<u64, u64, PodCodec>(i as u64, &(i as u64), &(i as u64))
                .expect("entry must fit");
        }
        // LEAF_CAP + 1 must be rejected (leaf is full at LEAF_CAP entries).
        let result = builder.push_framed::<u64, u64, PodCodec>(LEAF_CAP as u64, &0u64, &0u64);
        assert!(result.is_err(), "push beyond LEAF_CAP must fail");
    }

    // -----------------------------------------------------------------------
    // InternalReader / build_internal round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn internal_node_single_child_round_trip() {
        let bitmap: u32 = 1 << 7;
        let page = build_internal(bitmap, &[0xABCD_1234_5678_9ABCu64]);

        assert_eq!(page.0[0], DISCRIMINANT_INTERNAL);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.bitmap(), bitmap);
        assert_eq!(reader.child_count(), 1);
        assert_eq!(reader.child_index(7), Some(0));
        assert_eq!(reader.child_index(0), None);
        assert_eq!(reader.child_page_id(0), 0xABCD_1234_5678_9ABCu64);
    }

    #[test]
    fn internal_node_three_children_round_trip() {
        // Bits 0, 2, 5 set → 3 children.
        let bitmap: u32 = (1 << 0) | (1 << 2) | (1 << 5);
        let page = build_internal(bitmap, &[100u64, 200u64, 300u64]);

        let reader = InternalReader::new(&page);
        assert_eq!(reader.child_count(), 3);
        assert_eq!(reader.child_index(0), Some(0));
        assert_eq!(reader.child_index(2), Some(1));
        assert_eq!(reader.child_index(5), Some(2));
        assert_eq!(reader.child_index(1), None);
        assert_eq!(reader.child_page_id(0), 100);
        assert_eq!(reader.child_page_id(1), 200);
        assert_eq!(reader.child_page_id(2), 300);
    }

    #[test]
    fn internal_node_all_32_children_round_trip() {
        let bitmap: u32 = u32::MAX;
        let children: Vec<u64> = (0..32).map(|i: u64| i * 1000 + 1).collect();
        let page = build_internal(bitmap, &children);

        let reader = InternalReader::new(&page);
        assert_eq!(reader.child_count(), 32);
        for i in 0..32u32 {
            assert_eq!(reader.child_index(i), Some(i as usize));
            assert_eq!(reader.child_page_id(i as usize), i as u64 * 1000 + 1);
        }
    }

    #[test]
    fn discriminants_are_distinct_and_non_zero() {
        assert_ne!(DISCRIMINANT_LEAF, DISCRIMINANT_INTERNAL);
        assert_ne!(DISCRIMINANT_LEAF, 0u8);
        assert_ne!(DISCRIMINANT_INTERNAL, 0u8);
    }
}
