//! B+ tree node types and slab layout for [`FolioOrdMap`] and [`FolioOrdSet`].
//!
//! Each node occupies one 512-byte folio page (`BTreeNodePage`).  Two
//! discriminant values distinguish leaf nodes from internal (separator) nodes.
//!
//! # Leaf layout
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │ discriminant : u8   = DISCRIMINANT_LEAF (0x01)                      │
//! │ count        : u8   — number of key-value entries (≤ BTREE_ORDER)   │
//! │ next_leaf    : u64  — folio page ID of next leaf, or 0 for None     │
//! │ entry_offsets: [u16; BTREE_ORDER + 1]  — entry_offsets[i] = start  │
//! │                of entry i in data; entry_offsets[count] = total     │
//! │                data bytes.  Indices 0..=BTREE_ORDER (33 entries).   │
//! │ data         : [u8; DATA_SECTION_LEAF]  — codec-encoded K||V pairs │
//! │                in ascending key order (no internal framing).        │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Internal layout
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │ discriminant : u8   = DISCRIMINANT_INTERNAL (0x02)                  │
//! │ count        : u8   — number of separator keys (children = count+1) │
//! │ _pad         : u16  — reserved, always zero                         │
//! │ children     : [u64; BTREE_ORDER]  — child folio page IDs           │
//! │ key_offsets  : [u16; BTREE_ORDER]  — key_offsets[i] = start of     │
//! │                separator key i in key_data                          │
//! │ key_data     : [u8; DATA_SECTION_INTERNAL]  — codec-encoded keys   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Constants
//!
//! - `BTREE_ORDER = 32` — max separator keys in an internal node; max entries
//!   in a leaf node.  Fits exactly in 512 bytes for both layouts.
//! - An internal node has at most `BTREE_ORDER + 1 = 33` children.
//!
//! [`FolioOrdMap`]: crate::folio_ordmap::FolioOrdMap
//! [`FolioOrdSet`]: crate::folio_ordset::FolioOrdSet

use crate::codec::{CodecError, ValueCodec};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of separator keys in a B+ tree internal node, and the
/// maximum number of entries in a leaf node.
///
/// Chosen so that both `BTreeLeaf` and `BTreeInternal` fit within 512 bytes.
pub const BTREE_ORDER: usize = 32;

/// Discriminant byte for leaf nodes.
pub const DISCRIMINANT_LEAF: u8 = 0x01;

/// Discriminant byte for internal nodes.
pub const DISCRIMINANT_INTERNAL: u8 = 0x02;

// ---------------------------------------------------------------------------
// Page type
// ---------------------------------------------------------------------------

/// A 512-byte opaque page payload for B+ tree nodes.
///
/// Guaranteed `#[repr(transparent)]` over `[u8; 512]`.  Implements
/// [`bytemuck::Pod`] and [`bytemuck::Zeroable`] so it can be written to
/// and read from folio page bytes without unsafe code.
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(transparent)]
pub struct BTreeNodePage(pub [u8; 512]);

impl std::fmt::Debug for BTreeNodePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BTreeNodePage")
            .field(&format_args!("[u8; 512]"))
            .finish()
    }
}

impl Default for BTreeNodePage {
    /// Returns a zeroed page (discriminant 0x00 = unallocated).
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

// ---------------------------------------------------------------------------
// Leaf node layout
// ---------------------------------------------------------------------------
//
// Byte map (512 bytes total):
//   [0]       discriminant: u8
//   [1]       count:        u8
//   [2..10]   next_leaf:    u64 (little-endian; 0 = None)
//   [10..76]  entry_offsets: [u16; 33]  (66 bytes: (BTREE_ORDER+1) * 2)
//   [76..512] data:          [u8; 436]
//
// Verify: 1 + 1 + 8 + 66 + 436 = 512 ✓

const LEAF_DISCRIMINANT_OFFSET: usize = 0;
const LEAF_COUNT_OFFSET: usize = 1;
const LEAF_NEXT_LEAF_OFFSET: usize = 2; // u64 little-endian
const LEAF_ENTRY_OFFSETS_START: usize = 10; // [u16; BTREE_ORDER + 1]
const LEAF_ENTRY_OFFSETS_END: usize = 10 + (BTREE_ORDER + 1) * 2; // 10 + 66 = 76
const LEAF_DATA_START: usize = LEAF_ENTRY_OFFSETS_END; // 76
const LEAF_DATA_END: usize = 512; // 436 bytes of data

/// Number of data bytes available in a leaf page.
pub const DATA_SECTION_LEAF: usize = LEAF_DATA_END - LEAF_DATA_START; // 436

/// Builder for a B+ tree leaf page.
///
/// Entries must be pushed in ascending key order.  Each call to
/// [`Self::push_encoded`] appends one key-value pair.
pub struct LeafBuilder {
    page: BTreeNodePage,
    count: usize,
    data_cursor: usize,
}

impl LeafBuilder {
    /// Creates an empty leaf builder with `next_leaf = None`.
    pub fn new() -> Self {
        let mut b = Self {
            page: BTreeNodePage::default(),
            count: 0,
            data_cursor: 0,
        };
        b.page.0[LEAF_DISCRIMINANT_OFFSET] = DISCRIMINANT_LEAF;
        b
    }

    /// Returns `true` if no more entries can be pushed.
    pub fn is_full(&self) -> bool {
        self.count >= BTREE_ORDER
    }

    /// Returns the number of entries pushed so far.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Encodes `key` and `value` using codec `C` and appends them to this leaf.
    ///
    /// Returns [`CodecError::EncodeTooLarge`] if the encoded pair does not fit
    /// in the remaining data section space.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if encoding fails or if the page has no room.
    pub fn push_encoded<K, V, C>(&mut self, key: &K, value: &V) -> Result<(), CodecError>
    where
        C: ValueCodec<K> + ValueCodec<V>,
    {
        if self.is_full() {
            return Err(CodecError::EncodeTooLarge);
        }
        let mut buf = Vec::new();
        <C as ValueCodec<K>>::encode(key, &mut buf)?;
        <C as ValueCodec<V>>::encode(value, &mut buf)?;
        let needed = buf.len();
        if self.data_cursor + needed > DATA_SECTION_LEAF {
            return Err(CodecError::EncodeTooLarge);
        }
        // Record start offset of this entry.
        let offset_byte = LEAF_ENTRY_OFFSETS_START + self.count * 2;
        let offset_val = self.data_cursor as u16;
        self.page.0[offset_byte] = (offset_val & 0xFF) as u8;
        self.page.0[offset_byte + 1] = (offset_val >> 8) as u8;
        // Write data.
        let dst = LEAF_DATA_START + self.data_cursor;
        self.page.0[dst..dst + needed].copy_from_slice(&buf);
        self.data_cursor += needed;
        self.count += 1;
        // Update sentinel (offsets[count] = total data so far).
        let sentinel_byte = LEAF_ENTRY_OFFSETS_START + self.count * 2;
        let sentinel_val = self.data_cursor as u16;
        self.page.0[sentinel_byte] = (sentinel_val & 0xFF) as u8;
        self.page.0[sentinel_byte + 1] = (sentinel_val >> 8) as u8;
        Ok(())
    }

    /// Sets the `next_leaf` pointer (folio page ID of the next leaf).
    ///
    /// Pass `0` or leave unset for "no next leaf".
    pub fn set_next_leaf(&mut self, page_id: u64) {
        let bytes = page_id.to_le_bytes();
        self.page.0[LEAF_NEXT_LEAF_OFFSET..LEAF_NEXT_LEAF_OFFSET + 8].copy_from_slice(&bytes);
    }

    /// Finalises the page, writing the count byte, and returns it.
    pub fn finish(mut self) -> BTreeNodePage {
        self.page.0[LEAF_COUNT_OFFSET] = self.count as u8;
        self.page
    }
}

impl Default for LeafBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Reader for a B+ tree leaf page.
///
/// Provides O(1) access to the count, `next_leaf` pointer, and byte slices
/// for individual entries.
pub struct LeafReader<'a> {
    page: &'a BTreeNodePage,
}

impl<'a> LeafReader<'a> {
    /// Creates a reader for `page`.
    ///
    /// # Panics
    ///
    /// Panics if `page.0[0] != DISCRIMINANT_LEAF` (programmer error).
    pub fn new(page: &'a BTreeNodePage) -> Self {
        assert_eq!(
            page.0[LEAF_DISCRIMINANT_OFFSET], DISCRIMINANT_LEAF,
            "BTreeNodePage is not a leaf"
        );
        Self { page }
    }

    /// Returns the number of key-value entries.
    pub fn count(&self) -> usize {
        self.page.0[LEAF_COUNT_OFFSET] as usize
    }

    /// Returns the next-leaf folio page ID, or `0` if there is no next leaf.
    pub fn next_leaf(&self) -> u64 {
        let bytes: [u8; 8] = self.page.0[LEAF_NEXT_LEAF_OFFSET..LEAF_NEXT_LEAF_OFFSET + 8]
            .try_into()
            .unwrap();
        u64::from_le_bytes(bytes)
    }

    /// Returns the raw bytes for entry `i` (key followed immediately by value).
    ///
    /// # Panics
    ///
    /// Panics if `i >= self.count()`.
    pub fn entry_bytes(&self, i: usize) -> &[u8] {
        let count = self.count();
        assert!(i < count, "entry index {i} out of bounds (count={count})");
        let start = self.offset_at(i) as usize;
        let end = self.offset_at(i + 1) as usize;
        &self.page.0[LEAF_DATA_START + start..LEAF_DATA_START + end]
    }

    /// Decodes the key of entry `i` using codec `C`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if decoding fails.
    pub fn decode_key<K, C>(&self, i: usize) -> Result<(K, &[u8]), CodecError>
    where
        C: ValueCodec<K>,
    {
        // Decode only the key and return remaining bytes for the value.
        // Delegates to C::take so PodCodec uses size_of::<K>() and
        // PostcardCodec uses postcard::take_from_bytes.
        let bytes = self.entry_bytes(i);
        <C as ValueCodec<K>>::take(bytes)
    }

    /// Decodes both key and value of entry `i` using codec `C`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if decoding fails.
    pub fn decode_kv<K, V, C>(&self, i: usize) -> Result<(K, V), CodecError>
    where
        C: ValueCodec<K> + ValueCodec<V>,
    {
        let bytes = self.entry_bytes(i);
        let (key, rest) = <C as ValueCodec<K>>::take(bytes)?;
        let value = <C as ValueCodec<V>>::decode(rest)?;
        Ok((key, value))
    }

    /// Returns the data-section offset for entry `i` (or the sentinel at `count`).
    fn offset_at(&self, i: usize) -> u16 {
        let byte = LEAF_ENTRY_OFFSETS_START + i * 2;
        let lo = self.page.0[byte] as u16;
        let hi = (self.page.0[byte + 1] as u16) << 8;
        lo | hi
    }
}

// ---------------------------------------------------------------------------
// Internal node layout
// ---------------------------------------------------------------------------
//
// Byte map (512 bytes total):
//   [0]        discriminant: u8
//   [1]        count:        u8  (number of separator keys; children = count+1)
//   [2..4]     _pad:         [u8; 2]
//   [4..268]   children:     [u64; BTREE_ORDER+1] = [u64; 33]  (264 bytes)
//   [268..332] key_offsets:  [u16; BTREE_ORDER]   = [u16; 32]  (64 bytes)
//   [332..512] key_data:     [u8; 180]
//
// Verify: 1 + 1 + 2 + 264 + 64 + 180 = 512 ✓
//
// NOTE: the children array must hold BTREE_ORDER+1 entries (not BTREE_ORDER)
// because an internal node with N separator keys has N+1 children.  The
// original layout used [u64; BTREE_ORDER] which was one slot short, causing
// the 33rd child to overlap the key_offsets region and corrupt page IDs.

const INTERNAL_DISCRIMINANT_OFFSET: usize = 0;
const INTERNAL_COUNT_OFFSET: usize = 1;
const INTERNAL_CHILDREN_START: usize = 4; // [u64; BTREE_ORDER+1]
const INTERNAL_CHILDREN_END: usize = 4 + (BTREE_ORDER + 1) * 8; // 4 + 264 = 268
const INTERNAL_KEY_OFFSETS_START: usize = INTERNAL_CHILDREN_END; // 268
const INTERNAL_KEY_OFFSETS_END: usize = INTERNAL_KEY_OFFSETS_START + BTREE_ORDER * 2; // 268 + 64 = 332
const INTERNAL_KEY_DATA_START: usize = INTERNAL_KEY_OFFSETS_END; // 332
const INTERNAL_KEY_DATA_END: usize = 512;

/// Number of bytes available for separator key data in an internal node.
pub const DATA_SECTION_INTERNAL: usize = INTERNAL_KEY_DATA_END - INTERNAL_KEY_DATA_START; // 180

/// Builds a B+ tree internal (separator) node page.
///
/// `children` must have length `separator_keys.len() + 1`.
///
/// # Panics
///
/// Panics if the encoded separator keys exceed [`DATA_SECTION_INTERNAL`] bytes,
/// or if `children.len() != separator_keys.len() + 1`.
pub fn build_internal_node<K, C>(children: &[u64], separator_keys: &[K]) -> BTreeNodePage
where
    C: ValueCodec<K>,
{
    assert_eq!(
        children.len(),
        separator_keys.len() + 1,
        "children.len() must be separator_keys.len() + 1"
    );
    assert!(
        separator_keys.len() <= BTREE_ORDER,
        "too many separator keys for BTREE_ORDER={BTREE_ORDER}"
    );
    let count = separator_keys.len();
    let mut page = BTreeNodePage::default();
    page.0[INTERNAL_DISCRIMINANT_OFFSET] = DISCRIMINANT_INTERNAL;
    page.0[INTERNAL_COUNT_OFFSET] = count as u8;

    // Write children (count + 1 entries).
    let mut cursor = INTERNAL_CHILDREN_START;
    for &cid in children {
        let bytes = cid.to_le_bytes();
        page.0[cursor..cursor + 8].copy_from_slice(&bytes);
        cursor += 8;
    }

    // Encode separator keys and write offsets + key_data.
    let mut data_cursor: usize = 0;
    for (i, key) in separator_keys.iter().enumerate() {
        let mut buf = Vec::new();
        <C as ValueCodec<K>>::encode(key, &mut buf).expect("separator key encoding failed");
        let needed = buf.len();
        assert!(
            data_cursor + needed <= DATA_SECTION_INTERNAL,
            "separator keys exceed DATA_SECTION_INTERNAL bytes"
        );
        // Write offset for this key.
        let off_byte = INTERNAL_KEY_OFFSETS_START + i * 2;
        let off_val = data_cursor as u16;
        page.0[off_byte] = (off_val & 0xFF) as u8;
        page.0[off_byte + 1] = (off_val >> 8) as u8;
        // Write key data.
        let dst = INTERNAL_KEY_DATA_START + data_cursor;
        page.0[dst..dst + needed].copy_from_slice(&buf);
        data_cursor += needed;
    }
    page
}

/// Reader for a B+ tree internal (separator) node page.
pub struct InternalReader<'a> {
    page: &'a BTreeNodePage,
}

impl<'a> InternalReader<'a> {
    /// Creates a reader for `page`.
    ///
    /// # Panics
    ///
    /// Panics if `page.0[0] != DISCRIMINANT_INTERNAL`.
    pub fn new(page: &'a BTreeNodePage) -> Self {
        assert_eq!(
            page.0[INTERNAL_DISCRIMINANT_OFFSET], DISCRIMINANT_INTERNAL,
            "BTreeNodePage is not internal"
        );
        Self { page }
    }

    /// Returns the number of separator keys.
    pub fn count(&self) -> usize {
        self.page.0[INTERNAL_COUNT_OFFSET] as usize
    }

    /// Returns the number of children (`count + 1`).
    pub fn child_count(&self) -> usize {
        self.count() + 1
    }

    /// Returns the folio page ID of child `i`.
    ///
    /// # Panics
    ///
    /// Panics if `i >= child_count()`.
    pub fn child_page_id(&self, i: usize) -> u64 {
        let cc = self.child_count();
        assert!(i < cc, "child index {i} out of bounds (child_count={cc})");
        let byte = INTERNAL_CHILDREN_START + i * 8;
        let arr: [u8; 8] = self.page.0[byte..byte + 8].try_into().unwrap();
        u64::from_le_bytes(arr)
    }

    /// Decodes separator key `i` using codec `C`.
    ///
    /// Separator key `i` is the boundary between children `i` and `i + 1`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if decoding fails.
    ///
    /// # Panics
    ///
    /// Panics if `i >= count()`.
    pub fn decode_separator_key<K, C>(&self, i: usize) -> Result<K, CodecError>
    where
        C: ValueCodec<K>,
    {
        let count = self.count();
        assert!(
            i < count,
            "separator index {i} out of bounds (count={count})"
        );
        let start = self.key_offset_at(i) as usize;
        let end = if i + 1 < count {
            self.key_offset_at(i + 1) as usize
        } else {
            // Last key: the stored key_offsets table has no sentinel entry for
            // the end of the last key, so we use C::take to consume the key
            // from the start of its byte region — the remaining bytes are discarded.
            let key_bytes = &self.page.0[INTERNAL_KEY_DATA_START + start..INTERNAL_KEY_DATA_END];
            return <C as ValueCodec<K>>::take(key_bytes).map(|(k, _)| k);
        };
        <C as ValueCodec<K>>::decode(
            &self.page.0[INTERNAL_KEY_DATA_START + start..INTERNAL_KEY_DATA_START + end],
        )
    }

    /// Returns the data-section key offset for separator `i`.
    fn key_offset_at(&self, i: usize) -> u16 {
        let byte = INTERNAL_KEY_OFFSETS_START + i * 2;
        let lo = self.page.0[byte] as u16;
        let hi = (self.page.0[byte + 1] as u16) << 8;
        lo | hi
    }

    /// Finds the child index for a key by binary-searching the separator keys.
    ///
    /// Returns the child index `c` such that `children[c]` is the subtree
    /// that could contain `key`.  If `key < separators[0]`, returns 0.
    /// If `key >= separators[count-1]`, returns `count`.
    ///
    /// Time: O(count) — count ≤ BTREE_ORDER.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] if any separator key decoding fails.
    pub fn find_child<K, C>(&self, key: &K) -> Result<usize, CodecError>
    where
        K: Ord,
        C: ValueCodec<K>,
    {
        let count = self.count();
        // Linear scan: find first separator >= key, use the child to the left.
        for i in 0..count {
            let sep: K = self.decode_separator_key::<K, C>(i)?;
            if key < &sep {
                return Ok(i);
            }
        }
        Ok(count)
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

    // --- Size and layout ---

    #[test]
    fn btree_node_page_size() {
        assert_eq!(std::mem::size_of::<BTreeNodePage>(), 512);
    }

    #[test]
    fn btree_node_page_is_pod() {
        // bytemuck::Pod requires the type to be bit-valid for all byte patterns.
        // This confirms the impl compiles and the size is correct.
        let page = BTreeNodePage::default();
        let _bytes: &[u8] = bytemuck::bytes_of(&page);
        assert_eq!(_bytes.len(), 512);
    }

    #[test]
    fn discriminant_values_distinct() {
        assert_ne!(DISCRIMINANT_LEAF, DISCRIMINANT_INTERNAL);
        assert_ne!(DISCRIMINANT_LEAF, 0x00);
        assert_ne!(DISCRIMINANT_INTERNAL, 0x00);
    }

    #[test]
    fn data_section_sizes_correct() {
        // Leaf layout:  1 + 1 + 8 + (BTREE_ORDER+1)*2 + DATA_SECTION_LEAF = 512
        let header: usize = 1 + 1 + 8 + (BTREE_ORDER + 1) * 2;
        assert_eq!(header + DATA_SECTION_LEAF, 512);

        // Internal layout: 1 + 1 + 2 + (BTREE_ORDER+1)*8 + BTREE_ORDER*2 + DATA_SECTION_INTERNAL = 512
        // Children array is BTREE_ORDER+1 (one more than separator count) to hold N+1 children.
        let internal_fixed: usize = 1 + 1 + 2 + (BTREE_ORDER + 1) * 8 + BTREE_ORDER * 2;
        assert_eq!(internal_fixed + DATA_SECTION_INTERNAL, 512);
    }

    // --- Leaf builder / reader ---

    #[test]
    fn leaf_empty() {
        let builder = LeafBuilder::new();
        let page = builder.finish();
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 0);
        assert_eq!(reader.next_leaf(), 0);
    }

    // --- Leaf builder / reader (PodCodec — no serde feature required) ---

    #[test]
    fn leaf_single_entry_pod() {
        let mut b = LeafBuilder::new();
        b.push_encoded::<u32, u64, PodCodec>(&42u32, &100u64)
            .unwrap();
        let page = b.finish();
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 1);
        let (k, v) = reader.decode_kv::<u32, u64, PodCodec>(0).unwrap();
        assert_eq!(k, 42u32);
        assert_eq!(v, 100u64);
    }

    #[test]
    fn leaf_multiple_entries_in_order() {
        let mut b = LeafBuilder::new();
        for i in 0u32..10 {
            b.push_encoded::<u32, u32, PodCodec>(&i, &(i * 2)).unwrap();
        }
        let page = b.finish();
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 10);
        for i in 0u32..10 {
            let (k, v) = reader.decode_kv::<u32, u32, PodCodec>(i as usize).unwrap();
            assert_eq!(k, i);
            assert_eq!(v, i * 2);
        }
    }

    #[test]
    fn leaf_full_at_btree_order() {
        let mut b = LeafBuilder::new();
        let mut last_ok = 0usize;
        for i in 0..BTREE_ORDER {
            // Use short keys and values to avoid data overflow.
            let result = b.push_encoded::<u8, u8, PodCodec>(&(i as u8), &(i as u8));
            assert!(result.is_ok(), "push {i} failed unexpectedly");
            last_ok = i;
        }
        assert_eq!(last_ok, BTREE_ORDER - 1);
        assert!(b.is_full());
        // One more push should fail.
        let result = b.push_encoded::<u8, u8, PodCodec>(&0u8, &0u8);
        assert!(result.is_err());
    }

    #[test]
    fn leaf_next_leaf_pointer() {
        let mut b = LeafBuilder::new();
        b.push_encoded::<u32, u32, PodCodec>(&1u32, &1u32).unwrap();
        b.set_next_leaf(12345u64);
        let page = b.finish();
        let reader = LeafReader::new(&page);
        assert_eq!(reader.next_leaf(), 12345u64);
    }

    #[test]
    fn leaf_decode_key_only_pod() {
        let mut b = LeafBuilder::new();
        b.push_encoded::<u64, u64, PodCodec>(&999u64, &888u64)
            .unwrap();
        let page = b.finish();
        let reader = LeafReader::new(&page);
        let (k, rest) = reader.decode_key::<u64, PodCodec>(0).unwrap();
        assert_eq!(k, 999u64);
        // rest should decode as the value.
        let v: u64 = PodCodec::decode(rest).unwrap();
        assert_eq!(v, 888u64);
    }

    // --- Leaf builder / reader (PostcardCodec — requires serde feature) ---

    #[cfg(feature = "serde")]
    #[test]
    fn leaf_single_entry_postcard() {
        let mut b = LeafBuilder::new();
        b.push_encoded::<u32, u64, PostcardCodec>(&42u32, &100u64)
            .unwrap();
        let page = b.finish();
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 1);
        let (k, v) = reader.decode_kv::<u32, u64, PostcardCodec>(0).unwrap();
        assert_eq!(k, 42u32);
        assert_eq!(v, 100u64);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn leaf_string_kv_round_trip() {
        let mut b = LeafBuilder::new();
        b.push_encoded::<String, String, PostcardCodec>(
            &"alpha".to_string(),
            &"value1".to_string(),
        )
        .unwrap();
        b.push_encoded::<String, String, PostcardCodec>(&"beta".to_string(), &"value2".to_string())
            .unwrap();
        let page = b.finish();
        let reader = LeafReader::new(&page);
        assert_eq!(reader.count(), 2);
        let (k0, v0) = reader
            .decode_kv::<String, String, PostcardCodec>(0)
            .unwrap();
        let (k1, v1) = reader
            .decode_kv::<String, String, PostcardCodec>(1)
            .unwrap();
        assert_eq!(k0, "alpha");
        assert_eq!(v0, "value1");
        assert_eq!(k1, "beta");
        assert_eq!(v1, "value2");
    }

    // --- Internal node builder / reader (PodCodec) ---

    #[test]
    fn internal_single_separator() {
        let children = [10u64, 20u64];
        let separators = [5u32];
        let page = build_internal_node::<u32, PodCodec>(&children, &separators);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), 1);
        assert_eq!(reader.child_count(), 2);
        assert_eq!(reader.child_page_id(0), 10u64);
        assert_eq!(reader.child_page_id(1), 20u64);
        let sep: u32 = reader.decode_separator_key::<u32, PodCodec>(0).unwrap();
        assert_eq!(sep, 5u32);
    }

    #[test]
    fn internal_three_separators() {
        let children = [1u64, 2, 3, 4];
        let separators = [10u32, 20, 30];
        let page = build_internal_node::<u32, PodCodec>(&children, &separators);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), 3);
        assert_eq!(reader.child_count(), 4);
        for (i, &expected) in [1u64, 2, 3, 4].iter().enumerate() {
            assert_eq!(reader.child_page_id(i), expected);
        }
        for (i, &expected) in [10u32, 20, 30].iter().enumerate() {
            let sep: u32 = reader.decode_separator_key::<u32, PodCodec>(i).unwrap();
            assert_eq!(sep, expected, "separator {i} mismatch");
        }
    }

    #[test]
    fn internal_find_child_routing() {
        let children = [1u64, 2, 3, 4];
        let separators = [10u32, 20, 30];
        let page = build_internal_node::<u32, PodCodec>(&children, &separators);
        let reader = InternalReader::new(&page);

        assert_eq!(reader.find_child::<u32, PodCodec>(&5).unwrap(), 0);
        assert_eq!(reader.find_child::<u32, PodCodec>(&10).unwrap(), 1);
        assert_eq!(reader.find_child::<u32, PodCodec>(&15).unwrap(), 1);
        assert_eq!(reader.find_child::<u32, PodCodec>(&20).unwrap(), 2);
        assert_eq!(reader.find_child::<u32, PodCodec>(&25).unwrap(), 2);
        assert_eq!(reader.find_child::<u32, PodCodec>(&30).unwrap(), 3);
        assert_eq!(reader.find_child::<u32, PodCodec>(&99).unwrap(), 3);
    }

    #[test]
    fn internal_max_order() {
        let children: Vec<u64> = (0..=(BTREE_ORDER as u64)).collect();
        // Use u8 keys to keep data section small.
        let separators: Vec<u8> = (1u8..=(BTREE_ORDER as u8)).collect();
        let page = build_internal_node::<u8, PodCodec>(&children, &separators);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), BTREE_ORDER);
        assert_eq!(reader.child_count(), BTREE_ORDER + 1);
        for i in 0..BTREE_ORDER {
            let sep: u8 = reader.decode_separator_key::<u8, PodCodec>(i).unwrap();
            assert_eq!(sep, (i + 1) as u8);
        }
    }

    // --- Internal node builder / reader (PostcardCodec — requires serde feature) ---

    #[cfg(feature = "serde")]
    #[test]
    fn internal_single_separator_postcard() {
        let children = [10u64, 20u64];
        let separators = [5u32];
        let page = build_internal_node::<u32, PostcardCodec>(&children, &separators);
        let reader = InternalReader::new(&page);
        assert_eq!(reader.count(), 1);
        let sep: u32 = reader
            .decode_separator_key::<u32, PostcardCodec>(0)
            .unwrap();
        assert_eq!(sep, 5u32);
    }
}
