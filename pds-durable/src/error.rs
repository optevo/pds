// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Error types for `pds-durable`.

/// All errors produced by `pds-durable` operations.
///
/// Errors fall into three categories: I/O failures from the underlying filesystem,
/// WAL format corruption (bad CRC, truncated header), and serialisation failures
/// when encoding or decoding collection snapshots or individual entries.
#[derive(Debug, thiserror::Error)]
pub enum DurableError {
    /// An I/O error from the underlying filesystem or file descriptor.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A WAL entry at `offset` is structurally invalid.
    ///
    /// This typically indicates a torn write (power-loss mid-entry) or
    /// filesystem corruption.  Recovery stops at the corrupt offset and
    /// truncates the file there.
    #[error("WAL corrupt at offset {offset}: {reason}")]
    Corrupt {
        /// Byte offset of the corrupt entry in the WAL file.
        offset: u64,
        /// Human-readable reason for the corruption.
        reason: &'static str,
    },

    /// A serialisation or deserialisation error from `postcard`.
    #[error("serialisation error: {0}")]
    Serde(String),

    /// The WAL file was written by a different (unsupported) version.
    #[error("WAL version mismatch: expected 1, got {0}")]
    VersionMismatch(u32),
}
