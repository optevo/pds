//! RRB-tree node types and slab layout for folio-backed persistent vectors.
//!
//! This module provides the 512-byte [`VectorNodePage`] type — the raw page
//! payload stored in a folio slab — together with builder/reader pairs for
//! both the leaf and internal node layouts.
//!
//! # Layout
//!
//! Both node variants are packed into a `[u8; 512]` page using fixed-size
//! headers:
//!
//! **`VectorLeaf`** (`discriminant = 0x01`):
//!
//! ```text
//! Byte 0       : discriminant = 0x01
//! Byte 1       : count (number of entries, 0..=BRANCHING_FACTOR)
//! Bytes 2..66  : entry_offsets [u16; BRANCHING_FACTOR] — offsets[i] is the
//!                start of entry i within the data section; offsets[count]
//!                is the total number of data bytes written.
//! Bytes 66..512: data section (446 bytes); entries are packed tightly with
//!                no framing — each entry is decoded with its own length
//!                inferred from offsets[i+1] - offsets[i].
//! ```
//!
//! **`VectorInternal`** (`discriminant = 0x02`):
//!
//! ```text
//! Byte 0        : discriminant = 0x02
//! Byte 1        : count (number of children, 0..=BRANCHING_FACTOR)
//! Bytes 2..130  : sizes [u32; BRANCHING_FACTOR] — cumulative subtree element
//!                 counts; sizes[i] = total elements in children[0..=i].
//! Bytes 130..386: children [u64; BRANCHING_FACTOR] — folio page IDs of
//!                 child nodes.
//! Bytes 386..512: reserved / padding.
//! ```
//!
//! # Branching factor
//!
//! [`BRANCHING_FACTOR`] is 32.  At this factor:
//! - Depth ≈ 3 for 32 768 elements.
//! - Depth ≈ 4 for 1 048 576 elements.
//! - Index operations are `O(log_32 N)` ≈ O(1) for any practical N.
//!
//! # Size assertions
//!
//! A compile-time check asserts that [`VectorNodePage`] is exactly 512 bytes.

use bytemuck::{Pod, Zeroable};

use crate::codec::{CodecError, ValueCodec};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of bytes per [`VectorNodePage`].
pub const PAGE_BYTES: usize = 512;

/// Maximum number of children in an internal node, or entries in a leaf node.
pub const BRANCHING_FACTOR: usize = 32;

/// Discriminant byte indicating a leaf node.
pub const DISCRIMINANT_LEAF: u8 = 0x01;

/// Discriminant byte indicating an internal node.
pub const DISCRIMINANT_INTERNAL: u8 = 0x02;

/// Number of `u16` entry_offset slots in a leaf header.
///
/// We store `BRANCHING_FACTOR + 1` offsets so that `offsets[count]` is the
/// total number of data bytes written, giving us O(1) entry length via
/// `offsets[i+1] - offsets[i]` without any end-of-data tracking.
const LEAF_OFFSET_SLOTS: usize = BRANCHING_FACTOR + 1;

/// Byte offset of the entry_offsets array inside a leaf page.
const LEAF_OFFSETS_START: usize = 2;
/// Byte offset of the data section inside a leaf page.
const LEAF_DATA_START: usize = LEAF_OFFSETS_START + LEAF_OFFSET_SLOTS * 2;
// With BRANCHING_FACTOR=32: LEAF_DATA_START = 2 + 33*2 = 68.
// Data section: 512 - 68 = 444 bytes.

/// Number of bytes available in a leaf's data section.
pub const LEAF_DATA_CAPACITY: usize = PAGE_BYTES - LEAF_DATA_START;

/// Byte offset of the `sizes` array inside an internal page.
const INTERNAL_SIZES_START: usize = 2;
/// Byte offset of the `children` array inside an internal page.
const INTERNAL_CHILDREN_START: usize = INTERNAL_SIZES_START + BRANCHING_FACTOR * 4;
// sizes: 32 * 4 = 128 bytes → children start at 2 + 128 = 130.
// children: 32 * 8 = 256 bytes → end at 130 + 256 = 386 bytes (≤ 512 ✓).

// ---------------------------------------------------------------------------
// VectorNodePage
// ---------------------------------------------------------------------------

/// A 512-byte page payload for an RRB-tree node.
///
/// This is the raw byte array stored in the folio slab.  Byte 0 is the
/// discriminant: [`DISCRIMINANT_LEAF`] (`0x01`) or
/// [`DISCRIMINANT_INTERNAL`] (`0x02`).  All other bytes depend on the variant.
///
/// Use [`LeafBuilder`] / [`LeafReader`] and [`build_internal`] /
/// [`InternalReader`] to construct and read pages.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(transparent)]
pub struct VectorNodePage(pub [u8; PAGE_BYTES]);

impl Default for VectorNodePage {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

// Compile-time size assertion.
const _: () = assert!(
    std::mem::size_of::<VectorNodePage>() == PAGE_BYTES,
    "VectorNodePage must be exactly PAGE_BYTES bytes"
);

impl std::fmt::Debug for VectorNodePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VectorNodePage(discriminant={:#x}, ...)", self.0[0])
    }
}

// ---------------------------------------------------------------------------
// LeafBuilder — write a leaf node into a VectorNodePage
// ---------------------------------------------------------------------------

/// Builds a [`VectorNodePage`] leaf by appending encoded values one by one.
///
/// Allocate one builder per leaf page.  Call [`Self::push_encoded`] for each entry,
/// then [`Self::finish`] to get the completed page.
///
/// # Errors
///
/// [`Self::push_encoded`] returns [`CodecError`] if the encoded value would overflow
/// the data section (≥ [`LEAF_DATA_CAPACITY`] bytes total).
pub struct LeafBuilder {
    /// Mutable page buffer.
    page: VectorNodePage,
    /// Number of entries written so far.
    count: usize,
    /// Number of data bytes written so far.
    data_len: usize,
}

impl LeafBuilder {
    /// Creates an empty leaf builder.
    #[must_use]
    pub fn new() -> Self {
        let mut page = VectorNodePage::default();
        page.0[0] = DISCRIMINANT_LEAF;
        // offsets[0] = 0 (no data yet) — already zero from default.
        Self {
            page,
            count: 0,
            data_len: 0,
        }
    }

    /// Returns `true` if the leaf already has [`BRANCHING_FACTOR`] entries.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.count >= BRANCHING_FACTOR
    }

    /// Encodes `value` using `C` and appends it to the leaf.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if:
    /// - The leaf already has [`BRANCHING_FACTOR`] entries, or
    /// - The encoded value would overflow the data section.
    pub fn push_encoded<T, C: ValueCodec<T>>(&mut self, value: &T) -> Result<(), CodecError> {
        if self.count >= BRANCHING_FACTOR {
            return Err(CodecError::EncodeTooLarge);
        }

        // Encode the value into a temporary buffer.
        let mut buf: Vec<u8> = Vec::new();
        C::encode(value, &mut buf)?;

        // Check that it fits in the remaining data section.
        if self.data_len + buf.len() > LEAF_DATA_CAPACITY {
            return Err(CodecError::EncodeTooLarge);
        }

        // Write data bytes.
        let data_start = LEAF_DATA_START + self.data_len;
        let data_end = data_start + buf.len();
        self.page.0[data_start..data_end].copy_from_slice(&buf);
        self.data_len += buf.len();

        // Update entry_offsets[count+1] = current data_len.
        let next_slot = self.count + 1;
        let off_pos = LEAF_OFFSETS_START + next_slot * 2;
        let offset_val = self.data_len as u16;
        self.page.0[off_pos] = (offset_val & 0xff) as u8;
        self.page.0[off_pos + 1] = (offset_val >> 8) as u8;

        self.count += 1;
        self.page.0[1] = self.count as u8;

        Ok(())
    }

    /// Returns the completed leaf page.
    #[must_use]
    pub fn finish(self) -> VectorNodePage {
        self.page
    }
}

impl Default for LeafBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LeafReader — read entries from a leaf VectorNodePage
// ---------------------------------------------------------------------------

/// Reads entries from a [`VectorNodePage`] leaf.
///
/// Validity is not re-checked after construction — `page.0[0]` must equal
/// [`DISCRIMINANT_LEAF`].
pub struct LeafReader<'a> {
    /// Reference to the page data.
    page: &'a VectorNodePage,
    /// Cached entry count (from byte 1).
    count: usize,
}

impl<'a> LeafReader<'a> {
    /// Creates a `LeafReader` from a leaf page.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the discriminant is not [`DISCRIMINANT_LEAF`].
    #[must_use]
    pub fn new(page: &'a VectorNodePage) -> Self {
        debug_assert_eq!(
            page.0[0], DISCRIMINANT_LEAF,
            "LeafReader: expected leaf discriminant"
        );
        let count = page.0[1] as usize;
        Self { page, count }
    }

    /// Returns the number of entries in this leaf.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Returns the byte offset of entry `i` within the data section.
    #[must_use]
    fn entry_offset(&self, i: usize) -> usize {
        let pos = LEAF_OFFSETS_START + i * 2;
        let lo = self.page.0[pos] as usize;
        let hi = self.page.0[pos + 1] as usize;
        lo | (hi << 8)
    }

    /// Returns the raw byte slice for entry `i`.
    #[must_use]
    pub fn entry_bytes(&self, i: usize) -> &[u8] {
        debug_assert!(
            i < self.count,
            "entry index {i} out of bounds (count={})",
            self.count
        );
        let start = LEAF_DATA_START + self.entry_offset(i);
        let end = LEAF_DATA_START + self.entry_offset(i + 1);
        &self.page.0[start..end]
    }

    /// Decodes entry `i` using codec `C`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if decoding fails.
    pub fn get_entry<T, C: ValueCodec<T>>(&self, i: usize) -> Result<T, CodecError> {
        C::decode(self.entry_bytes(i))
    }
}

// ---------------------------------------------------------------------------
// build_internal / InternalReader
// ---------------------------------------------------------------------------

/// Constructs an internal [`VectorNodePage`] from a list of child page IDs and
/// their cumulative subtree sizes.
///
/// `children` and `cumulative_sizes` must have the same length, and that length
/// must not exceed [`BRANCHING_FACTOR`].
///
/// # Panics
///
/// In debug builds, panics if `children.len() != cumulative_sizes.len()` or
/// `children.len() > BRANCHING_FACTOR`.
#[must_use]
pub fn build_internal(children: &[u64], cumulative_sizes: &[u32]) -> VectorNodePage {
    debug_assert_eq!(
        children.len(),
        cumulative_sizes.len(),
        "build_internal: children and sizes must have the same length"
    );
    debug_assert!(
        children.len() <= BRANCHING_FACTOR,
        "build_internal: too many children ({})",
        children.len()
    );

    let mut page = VectorNodePage::default();
    page.0[0] = DISCRIMINANT_INTERNAL;
    page.0[1] = children.len() as u8;

    // Write cumulative sizes: [u32; count] at INTERNAL_SIZES_START.
    for (i, &sz) in cumulative_sizes.iter().enumerate() {
        let pos = INTERNAL_SIZES_START + i * 4;
        page.0[pos] = (sz & 0xff) as u8;
        page.0[pos + 1] = ((sz >> 8) & 0xff) as u8;
        page.0[pos + 2] = ((sz >> 16) & 0xff) as u8;
        page.0[pos + 3] = ((sz >> 24) & 0xff) as u8;
    }

    // Write children: [u64; count] at INTERNAL_CHILDREN_START.
    for (i, &child_id) in children.iter().enumerate() {
        let pos = INTERNAL_CHILDREN_START + i * 8;
        page.0[pos] = (child_id & 0xff) as u8;
        page.0[pos + 1] = ((child_id >> 8) & 0xff) as u8;
        page.0[pos + 2] = ((child_id >> 16) & 0xff) as u8;
        page.0[pos + 3] = ((child_id >> 24) & 0xff) as u8;
        page.0[pos + 4] = ((child_id >> 32) & 0xff) as u8;
        page.0[pos + 5] = ((child_id >> 40) & 0xff) as u8;
        page.0[pos + 6] = ((child_id >> 48) & 0xff) as u8;
        page.0[pos + 7] = ((child_id >> 56) & 0xff) as u8;
    }

    page
}

/// Reads the internal node fields from a [`VectorNodePage`].
///
/// Validity is not re-checked after construction — `page.0[0]` must equal
/// [`DISCRIMINANT_INTERNAL`].
pub struct InternalReader<'a> {
    /// Reference to the page data.
    page: &'a VectorNodePage,
    /// Cached child count.
    count: usize,
}

impl<'a> InternalReader<'a> {
    /// Creates an `InternalReader` from an internal page.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the discriminant is not [`DISCRIMINANT_INTERNAL`].
    #[must_use]
    pub fn new(page: &'a VectorNodePage) -> Self {
        debug_assert_eq!(
            page.0[0], DISCRIMINANT_INTERNAL,
            "InternalReader: expected internal discriminant"
        );
        let count = page.0[1] as usize;
        Self { page, count }
    }

    /// Returns the number of children in this internal node.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Returns the cumulative subtree size at child index `i`.
    ///
    /// `sizes[i]` = total number of elements in children `0..=i`.
    #[must_use]
    pub fn cumulative_size(&self, i: usize) -> u32 {
        debug_assert!(
            i < self.count,
            "size index {i} out of bounds (count={})",
            self.count
        );
        let pos = INTERNAL_SIZES_START + i * 4;
        let b0 = self.page.0[pos] as u32;
        let b1 = self.page.0[pos + 1] as u32;
        let b2 = self.page.0[pos + 2] as u32;
        let b3 = self.page.0[pos + 3] as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }

    /// Returns the page ID of child `i`.
    #[must_use]
    pub fn child_page_id(&self, i: usize) -> u64 {
        debug_assert!(
            i < self.count,
            "child index {i} out of bounds (count={})",
            self.count
        );
        let pos = INTERNAL_CHILDREN_START + i * 8;
        let mut id = 0u64;
        for j in 0..8 {
            id |= (self.page.0[pos + j] as u64) << (j * 8);
        }
        id
    }

    /// Returns the index of the child that contains element at position `pos`
    /// within this subtree.
    ///
    /// Uses the cumulative sizes table to locate the child in O(count) time
    /// (count ≤ 32).  Returns `None` if `pos` is out of range.
    #[must_use]
    pub fn find_child(&self, pos: usize) -> Option<(usize, usize)> {
        let mut prefix = 0usize;
        for i in 0..self.count {
            let cum = self.cumulative_size(i) as usize;
            if pos < cum {
                // Element is in child i; local position within that child.
                return Some((i, pos - prefix));
            }
            prefix = cum;
        }
        None
    }
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

    // --- Size assertions ---

    #[test]
    fn vector_node_page_is_512_bytes() {
        assert_eq!(std::mem::size_of::<VectorNodePage>(), 512);
    }

    #[test]
    fn vector_node_page_is_pod() {
        // If this compiles, the Pod + Zeroable derives are correct.
        let _: VectorNodePage = bytemuck::Zeroable::zeroed();
    }

    #[test]
    fn leaf_data_capacity_is_correct() {
        // LEAF_DATA_START = 2 + (32+1)*2 = 2 + 66 = 68; capacity = 512 - 68 = 444.
        assert_eq!(LEAF_DATA_START, 68);
        assert_eq!(LEAF_DATA_CAPACITY, 444);
    }

    #[test]
    fn internal_children_start_is_correct() {
        // sizes: 32 * 4 = 128 bytes; children start at 2 + 128 = 130.
        // children end: 130 + 32 * 8 = 130 + 256 = 386 ≤ 512.
        assert_eq!(INTERNAL_SIZES_START, 2);
        assert_eq!(INTERNAL_CHILDREN_START, 130);
        const { assert!(INTERNAL_CHILDREN_START + BRANCHING_FACTOR * 8 <= PAGE_BYTES) }
    }

    // --- Discriminant uniqueness ---

    #[test]
    fn discriminants_are_distinct() {
        assert_ne!(DISCRIMINANT_LEAF, DISCRIMINANT_INTERNAL);
    }

    // --- LeafBuilder / LeafReader round-trips ---

    #[test]
    fn leaf_empty_round_trip() {
        let builder: LeafBuilder = LeafBuilder::new();
        let page = builder.finish();
        assert_eq!(page.0[0], DISCRIMINANT_LEAF);
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 0);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn leaf_single_entry_postcard_u32() {
        let mut builder = LeafBuilder::new();
        builder.push_encoded::<u32, PostcardCodec>(&42u32).unwrap();
        let page = builder.finish();

        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 1);
        let val: u32 = reader.get_entry::<u32, PostcardCodec>(0).unwrap();
        assert_eq!(val, 42u32);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn leaf_multiple_entries_postcard_string() {
        let mut builder = LeafBuilder::new();
        let entries: &[&str] = &["alpha", "beta", "gamma", "delta"];
        for &s in entries {
            builder
                .push_encoded::<String, PostcardCodec>(&s.to_string())
                .unwrap();
        }
        let page = builder.finish();

        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), entries.len());
        for (i, &expected) in entries.iter().enumerate() {
            let got: String = reader.get_entry::<String, PostcardCodec>(i).unwrap();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn leaf_full_branching_factor_pod_u64() {
        let mut builder = LeafBuilder::new();
        for i in 0u64..BRANCHING_FACTOR as u64 {
            builder.push_encoded::<u64, PodCodec>(&i).unwrap();
        }
        let page = builder.finish();

        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), BRANCHING_FACTOR);
        for i in 0usize..BRANCHING_FACTOR {
            let got: u64 = reader.get_entry::<u64, PodCodec>(i).unwrap();
            assert_eq!(got, i as u64);
        }
    }

    #[test]
    fn leaf_push_fails_when_count_exceeds_branching_factor() {
        let mut builder = LeafBuilder::new();
        for i in 0u64..BRANCHING_FACTOR as u64 {
            builder.push_encoded::<u64, PodCodec>(&i).unwrap();
        }
        // One more must fail.
        assert!(builder.push_encoded::<u64, PodCodec>(&99u64).is_err());
    }

    #[test]
    fn leaf_is_full_flag() {
        let mut builder = LeafBuilder::new();
        assert!(!builder.is_full());
        for i in 0u64..BRANCHING_FACTOR as u64 {
            builder.push_encoded::<u64, PodCodec>(&i).unwrap();
        }
        assert!(builder.is_full());
    }

    // --- build_internal / InternalReader round-trips ---

    #[test]
    fn internal_empty_round_trip() {
        let page = build_internal(&[], &[]);
        assert_eq!(page.0[0], DISCRIMINANT_INTERNAL);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), 0);
        assert_eq!(reader.find_child(0), None);
    }

    #[test]
    fn internal_single_child_round_trip() {
        // One child with 5 elements: cumulative_sizes = [5].
        let page = build_internal(&[999u64], &[5u32]);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), 1);
        assert_eq!(reader.cumulative_size(0), 5);
        assert_eq!(reader.child_page_id(0), 999u64);

        // Elements 0..4 all in child 0, local positions 0..4.
        for pos in 0usize..5 {
            assert_eq!(reader.find_child(pos), Some((0, pos)));
        }
        // Position 5 is out of range.
        assert_eq!(reader.find_child(5), None);
    }

    #[test]
    fn internal_three_children_round_trip() {
        // Three children with 4, 3, 5 elements each.
        // cumulative_sizes = [4, 7, 12].
        let children = &[100u64, 200u64, 300u64];
        let cum_sizes = &[4u32, 7u32, 12u32];
        let page = build_internal(children, cum_sizes);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), 3);

        for (i, (&cid, &sz)) in children.iter().zip(cum_sizes.iter()).enumerate() {
            assert_eq!(reader.child_page_id(i), cid);
            assert_eq!(reader.cumulative_size(i), sz);
        }

        // find_child for elements in child 0 (positions 0..3 → local 0..3).
        for pos in 0usize..4 {
            assert_eq!(reader.find_child(pos), Some((0, pos)));
        }
        // find_child for elements in child 1 (positions 4..6 → local 0..2).
        for pos in 4usize..7 {
            assert_eq!(reader.find_child(pos), Some((1, pos - 4)));
        }
        // find_child for elements in child 2 (positions 7..11 → local 0..4).
        for pos in 7usize..12 {
            assert_eq!(reader.find_child(pos), Some((2, pos - 7)));
        }
        // Out of range.
        assert_eq!(reader.find_child(12), None);
    }

    #[test]
    fn internal_max_branching_factor_children() {
        let children: Vec<u64> = (0..BRANCHING_FACTOR as u64).collect();
        // Each subtree has exactly 1 element; cumulative = 1, 2, 3, ..., 32.
        let cum_sizes: Vec<u32> = (1..=BRANCHING_FACTOR as u32).collect();
        let page = build_internal(&children, &cum_sizes);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), BRANCHING_FACTOR);
        for i in 0..BRANCHING_FACTOR {
            assert_eq!(reader.child_page_id(i), i as u64);
            assert_eq!(reader.cumulative_size(i), (i + 1) as u32);
        }
        // Each position maps to its own child (1 element per child).
        for pos in 0..BRANCHING_FACTOR {
            assert_eq!(reader.find_child(pos), Some((pos, 0)));
        }
        assert_eq!(reader.find_child(BRANCHING_FACTOR), None);
    }
}
