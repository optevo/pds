//! Codec abstraction for encoding keys and values into folio node page bytes.
//!
//! The `C: ValueCodec<T>` bound on all pds-folio collections controls how
//! keys and values are serialised into the byte arrays stored in HAMT leaf
//! pages, vector leaf pages, and B-tree leaf pages.
//!
//! Two built-in implementations are provided:
//!
//! - [`PodCodec`] — zero-copy for [`bytemuck::Pod`] types; the raw bytes of
//!   the value are written directly with no framing overhead. Use for `u64`,
//!   `u32`, `i64`, `[u8; 32]`, and other fixed-size plain-data types. Always
//!   available; no feature flags required.
//! - [`PostcardCodec`] — compact variable-length encoding via the
//!   [`postcard`] crate. Supports any `#[derive(Serialize, Deserialize)]`
//!   type: `String`, enums, structs, `Vec<_>`, nested types. Enabled behind
//!   the `serde` feature flag.
//!
//! # Implementing a custom codec
//!
//! ```rust,ignore
//! use pds_folio::codec::{ValueCodec, CodecError};
//!
//! /// Custom pod codec for a specific type.
//! struct MyCodec;
//!
//! impl ValueCodec<u64> for MyCodec {
//!     fn encode(value: &u64, buf: &mut Vec<u8>) -> Result<(), CodecError> {
//!         buf.extend_from_slice(&value.to_le_bytes());
//!         Ok(())
//!     }
//!     fn decode(bytes: &[u8]) -> Result<u64, CodecError> {
//!         if bytes.len() != 8 {
//!             return Err(CodecError::Decode("expected 8 bytes".into()));
//!         }
//!         Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
//!     }
//!     fn take(bytes: &[u8]) -> Result<(u64, &[u8]), CodecError> {
//!         if bytes.len() < 8 {
//!             return Err(CodecError::Decode("need 8 bytes".into()));
//!         }
//!         let val = u64::from_le_bytes(bytes[..8].try_into().unwrap());
//!         Ok((val, &bytes[8..]))
//!     }
//! }
//! ```

/// Error returned by [`ValueCodec`] methods.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// Encoding failed.
    #[error("encode error: {0}")]
    Encode(String),
    /// Decoding failed.
    #[error("decode error: {0}")]
    Decode(String),
    /// Encoded value is too large to fit in the available page space.
    #[error("encoded value is too large for the available page space")]
    EncodeTooLarge,
}

/// Per-type codec: encodes and decodes values of type `T` into page bytes.
///
/// Implementors must be stateless — all state is carried by the encoded bytes.
/// The trait is expected to be implemented on zero-sized marker types.
///
/// # Methods
///
/// - `encode` — appends the encoded form of `value` to `buf`.
/// - `decode` — decodes a value from `bytes`, consuming exactly all bytes.
/// - `take` — decodes a value from the front of `bytes`, returning the decoded
///   value and the remaining unconsumed bytes. Used by the B-tree codec to
///   split a key from a concatenated key+value byte slice.
pub trait ValueCodec<T>: 'static {
    /// Encodes `value` and appends the bytes to `buf`.
    ///
    /// Appends rather than overwrites so callers can pack multiple values
    /// into a single allocation by calling `encode` sequentially.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Encode`] if serialisation fails.
    fn encode(value: &T, buf: &mut Vec<u8>) -> Result<(), CodecError>;

    /// Decodes a value from `bytes`.
    ///
    /// `bytes` must be exactly the bytes produced by a prior [`ValueCodec::encode`]
    /// call for the same type.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Decode`] if deserialisation fails.
    fn decode(bytes: &[u8]) -> Result<T, CodecError>;

    /// Decodes a value from the front of `bytes`, returning the value and the
    /// remaining unconsumed bytes.
    ///
    /// Used by the B-tree codec path to split a key from a concatenated
    /// key+value byte slice without a separate framing prefix.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Decode`] if deserialisation fails.
    fn take(bytes: &[u8]) -> Result<(T, &[u8]), CodecError>;
}

// --- PodCodec ---

/// Zero-copy codec for [`bytemuck::Pod`] types.
///
/// Encodes by writing the raw bytes of the value (via [`bytemuck::bytes_of`]),
/// and decodes by reading them back (via [`bytemuck::from_bytes`]). No framing,
/// no varint encoding — just a memcpy of the fixed-size value.
///
/// # Type constraints
///
/// `T` must implement both [`bytemuck::Pod`] and [`Copy`].  This is
/// automatically satisfied for all built-in `bytemuck::Pod` types.
///
/// In practice: use `PodCodec` for `u32`, `u64`, `i64`, `f64`, `[u8; N]`, and
/// other `#[derive(Pod)]` types. For strings, enums, or variable-length types,
/// use [`PostcardCodec`] (requires the `serde` feature).
///
/// # Examples
///
/// ```rust
/// use pds_folio::codec::{ValueCodec, PodCodec};
///
/// let mut buf = Vec::new();
/// PodCodec::encode(&42u64, &mut buf).unwrap();
/// let decoded: u64 = PodCodec::decode(&buf).unwrap();
/// assert_eq!(decoded, 42u64);
/// ```
pub struct PodCodec;

impl<T: bytemuck::Pod + Copy> ValueCodec<T> for PodCodec {
    fn encode(value: &T, buf: &mut Vec<u8>) -> Result<(), CodecError> {
        buf.extend_from_slice(bytemuck::bytes_of(value));
        Ok(())
    }

    fn decode(bytes: &[u8]) -> Result<T, CodecError> {
        let size = std::mem::size_of::<T>();
        if bytes.len() != size {
            return Err(CodecError::Decode(format!(
                "expected {} bytes, got {}",
                size,
                bytes.len()
            )));
        }
        // Use pod_read_unaligned because page data has no alignment guarantees.
        Ok(bytemuck::pod_read_unaligned(bytes))
    }

    fn take(bytes: &[u8]) -> Result<(T, &[u8]), CodecError> {
        let size = std::mem::size_of::<T>();
        if bytes.len() < size {
            return Err(CodecError::Decode(format!(
                "take: need {} bytes, got {}",
                size,
                bytes.len()
            )));
        }
        // Use pod_read_unaligned because page data has no alignment guarantees.
        let value = bytemuck::pod_read_unaligned(&bytes[..size]);
        Ok((value, &bytes[size..]))
    }
}

impl PodCodec {
    /// Encodes a [`bytemuck::Pod`] value as raw bytes — zero-copy, no framing.
    ///
    /// Time: O(`size_of::<T>()`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use pds_folio::codec::PodCodec;
    ///
    /// let mut buf = Vec::new();
    /// PodCodec::encode_pod(&1234u64, &mut buf);
    /// assert_eq!(buf, 1234u64.to_ne_bytes());
    /// ```
    pub fn encode_pod<T: bytemuck::Pod>(value: &T, buf: &mut Vec<u8>) {
        buf.extend_from_slice(bytemuck::bytes_of(value));
    }

    /// Decodes a [`bytemuck::Pod`] value from raw bytes — zero-copy.
    ///
    /// `bytes` must be exactly `size_of::<T>()` bytes long.
    ///
    /// # Panics
    ///
    /// Panics if `bytes.len() != size_of::<T>()`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use pds_folio::codec::PodCodec;
    ///
    /// let bytes = 42u64.to_ne_bytes();
    /// let decoded: u64 = PodCodec::decode_pod(&bytes);
    /// assert_eq!(decoded, 42u64);
    /// ```
    pub fn decode_pod<T: bytemuck::Pod + Copy>(bytes: &[u8]) -> T {
        bytemuck::pod_read_unaligned(bytes)
    }
}

// --- PostcardCodec ---

/// Compact variable-length codec using the [`postcard`] crate.
///
/// Supports any type that implements `#[derive(Serialize, Deserialize)]`:
/// strings, enums, structs, `Vec<T>`, nested types, and all numeric primitives.
///
/// # Encoding
///
/// Uses postcard's no-heap, no-framing encoding: varints for integers, length
/// prefixes for slices, recursion for structs. Output is typically 20–40% smaller
/// than bincode for string-heavy workloads.
///
/// # Feature flag
///
/// `PostcardCodec` is only available when the `serde` feature is enabled.
///
/// # Examples
///
/// ```rust,ignore
/// use pds_folio::codec::{ValueCodec, PostcardCodec};
///
/// let mut buf = Vec::new();
/// PostcardCodec::encode(&"hello".to_string(), &mut buf).unwrap();
/// let decoded: String = PostcardCodec::decode(&buf).unwrap();
/// assert_eq!(decoded, "hello");
/// ```
#[cfg(feature = "serde")]
pub struct PostcardCodec;

#[cfg(feature = "serde")]
impl<T: serde::Serialize + for<'de> serde::Deserialize<'de>> ValueCodec<T> for PostcardCodec {
    fn encode(value: &T, buf: &mut Vec<u8>) -> Result<(), CodecError> {
        let encoded =
            postcard::to_allocvec(value).map_err(|e| CodecError::Encode(e.to_string()))?;
        buf.extend_from_slice(&encoded);
        Ok(())
    }

    fn decode(bytes: &[u8]) -> Result<T, CodecError> {
        postcard::from_bytes(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }

    fn take(bytes: &[u8]) -> Result<(T, &[u8]), CodecError> {
        postcard::take_from_bytes(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // --- PodCodec ---

    #[test]
    fn pod_codec_u64_round_trip() {
        let mut buf = Vec::new();
        PodCodec::encode(&42u64, &mut buf).unwrap();
        let decoded: u64 = PodCodec::decode(&buf).unwrap();
        assert_eq!(decoded, 42u64);
    }

    #[test]
    fn pod_codec_take_u64() {
        let mut buf = Vec::new();
        PodCodec::encode(&10u64, &mut buf).unwrap();
        PodCodec::encode(&20u64, &mut buf).unwrap();
        let (a, rest) = <PodCodec as ValueCodec<u64>>::take(&buf).unwrap();
        let (b, leftover) = <PodCodec as ValueCodec<u64>>::take(rest).unwrap();
        assert_eq!(a, 10u64);
        assert_eq!(b, 20u64);
        assert!(leftover.is_empty());
    }

    #[test]
    fn pod_codec_decode_wrong_size() {
        let result: Result<u64, _> = PodCodec::decode(&[0u8; 4]);
        assert!(result.is_err());
        match result.unwrap_err() {
            CodecError::Decode(_) => {}
            other => panic!("expected Decode error, got {other:?}"),
        }
    }

    #[test]
    fn pod_codec_take_insufficient_bytes() {
        let result: Result<(u64, _), _> = PodCodec::take(&[0u8; 4]);
        assert!(result.is_err());
    }

    #[test]
    fn pod_codec_encode_decode_array() {
        let arr: [u8; 32] = [0xAB; 32];
        let mut buf = Vec::new();
        PodCodec::encode(&arr, &mut buf).unwrap();
        assert_eq!(buf.len(), 32);
        let decoded: [u8; 32] = PodCodec::decode(&buf).unwrap();
        assert_eq!(decoded, arr);
    }

    #[test]
    fn pod_codec_encode_zero() {
        let mut buf = Vec::new();
        PodCodec::encode(&0u64, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 8]);
        let decoded: u64 = PodCodec::decode(&buf).unwrap();
        assert_eq!(decoded, 0u64);
    }

    // --- PodCodec helpers ---

    #[test]
    fn pod_codec_encode_pod_u64() {
        let mut buf = Vec::new();
        PodCodec::encode_pod(&1234u64, &mut buf);
        assert_eq!(buf.len(), 8);
        let decoded = PodCodec::decode_pod::<u64>(&buf);
        assert_eq!(decoded, 1234u64);
    }

    // --- PostcardCodec (serde feature) ---

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_codec_u64_round_trip() {
        let mut buf = Vec::new();
        PostcardCodec::encode(&42u64, &mut buf).unwrap();
        let decoded: u64 = PostcardCodec::decode(&buf).unwrap();
        assert_eq!(decoded, 42u64);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_codec_string_round_trip() {
        let mut buf = Vec::new();
        PostcardCodec::encode(&"hello, world".to_string(), &mut buf).unwrap();
        let decoded: String = PostcardCodec::decode(&buf).unwrap();
        assert_eq!(decoded, "hello, world");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_codec_take_splits_key_value() {
        let mut buf = Vec::new();
        PostcardCodec::encode(&42u32, &mut buf).unwrap();
        PostcardCodec::encode(&"world".to_string(), &mut buf).unwrap();
        let (k, rest): (u32, _) = PostcardCodec::take(&buf).unwrap();
        let v: String = PostcardCodec::decode(rest).unwrap();
        assert_eq!(k, 42u32);
        assert_eq!(v, "world");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_codec_empty_string() {
        let mut buf = Vec::new();
        PostcardCodec::encode(&String::new(), &mut buf).unwrap();
        let decoded: String = PostcardCodec::decode(&buf).unwrap();
        assert!(decoded.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn postcard_codec_decode_error() {
        // Empty bytes → decode error, not panic.
        let result: Result<u64, _> = PostcardCodec::decode(&[]);
        assert!(result.is_err());
        match result.unwrap_err() {
            CodecError::Decode(_) => {}
            other => panic!("expected Decode error, got {other:?}"),
        }
    }
}
