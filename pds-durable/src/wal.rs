// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Append-only Write-Ahead Log (WAL) implementation.
//!
//! # File layout
//!
//! ```text
//! [File header]
//!   magic:   b"PDSW"   (4 bytes)
//!   version: 1u32 LE   (4 bytes)
//!
//! [Entry stream — repeated until EOF]
//!   entry_len: u64 LE  — byte count of (entry_type + payload + crc32c)
//!   entry_type: u8
//!   payload:   [entry_len - 5] bytes — postcard-encoded body
//!   crc32c:    u32 LE — CRC32C of (entry_type byte ++ payload bytes)
//! ```

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::DurableError;

/// Magic bytes written at the start of every WAL file.
pub(crate) const WAL_MAGIC: &[u8; 4] = b"PDSW";
/// WAL format version.
pub(crate) const WAL_VERSION: u32 = 1;
/// Byte size of the WAL file header (magic + version).
pub(crate) const WAL_HEADER_SIZE: u64 = 8;

/// Entry-type tag byte for an `Insert` entry.
const TAG_INSERT: u8 = 0x01;
/// Entry-type tag byte for a `Remove` entry.
const TAG_REMOVE: u8 = 0x02;
/// Entry-type tag byte for a `Checkpoint` entry.
const TAG_CHECKPOINT: u8 = 0x03;
/// Entry-type tag byte for a `FlushMarker` entry.
const TAG_FLUSH_MARKER: u8 = 0x04;

/// A single logical record in the WAL.
///
/// Each variant carries the postcard-serialised bytes of its payload,
/// not the decoded Rust values.  This keeps the WAL layer independent
/// of the concrete key and value types.
#[derive(Debug, Clone)]
pub(crate) enum WalEntry {
    /// A key–value insertion.
    ///
    /// `key_bytes` and `value_bytes` are each the output of
    /// `postcard::to_allocvec(&K)` / `postcard::to_allocvec(&V)`.
    Insert {
        /// Postcard-encoded key.
        key_bytes: Vec<u8>,
        /// Postcard-encoded value.
        value_bytes: Vec<u8>,
    },
    /// A key removal.
    ///
    /// `key_bytes` is the output of `postcard::to_allocvec(&K)`.
    Remove {
        /// Postcard-encoded key.
        key_bytes: Vec<u8>,
    },
    /// A full collection snapshot written at checkpoint time.
    ///
    /// `snapshot_bytes` is the output of `postcard::to_allocvec(&collection)`.
    Checkpoint {
        /// Postcard-encoded full collection.
        snapshot_bytes: Vec<u8>,
    },
    /// A boundary marker written by [`Wal::flush`] in Relaxed mode.
    ///
    /// Carries no payload; its presence in the WAL signals a flush boundary.
    FlushMarker,
}

/// Serialise a [`WalEntry`] to its on-disk `(tag, payload)` pair.
///
/// Returns `(tag_byte, payload_bytes)`.  The payload is the postcard
/// encoding of the entry's inner structure — **not** a postcard encoding
/// of the `WalEntry` enum itself.
fn encode_entry(entry: &WalEntry) -> Result<(u8, Vec<u8>), DurableError> {
    match entry {
        WalEntry::Insert {
            key_bytes,
            value_bytes,
        } => {
            let payload = postcard::to_allocvec(&(key_bytes.as_slice(), value_bytes.as_slice()))
                .map_err(|e| DurableError::Serde(e.to_string()))?;
            Ok((TAG_INSERT, payload))
        }
        WalEntry::Remove { key_bytes } => {
            let payload = postcard::to_allocvec(&key_bytes.as_slice())
                .map_err(|e| DurableError::Serde(e.to_string()))?;
            Ok((TAG_REMOVE, payload))
        }
        WalEntry::Checkpoint { snapshot_bytes } => {
            let payload = postcard::to_allocvec(&snapshot_bytes.as_slice())
                .map_err(|e| DurableError::Serde(e.to_string()))?;
            Ok((TAG_CHECKPOINT, payload))
        }
        WalEntry::FlushMarker => Ok((TAG_FLUSH_MARKER, Vec::new())),
    }
}

/// Deserialise a raw `(tag, payload)` pair back into a [`WalEntry`].
fn decode_entry(tag: u8, payload: &[u8], offset: u64) -> Result<WalEntry, DurableError> {
    match tag {
        TAG_INSERT => {
            let (key_bytes, value_bytes): (Vec<u8>, Vec<u8>) =
                postcard::from_bytes(payload).map_err(|e| DurableError::Serde(e.to_string()))?;
            Ok(WalEntry::Insert {
                key_bytes,
                value_bytes,
            })
        }
        TAG_REMOVE => {
            let key_bytes: Vec<u8> =
                postcard::from_bytes(payload).map_err(|e| DurableError::Serde(e.to_string()))?;
            Ok(WalEntry::Remove { key_bytes })
        }
        TAG_CHECKPOINT => {
            let snapshot_bytes: Vec<u8> =
                postcard::from_bytes(payload).map_err(|e| DurableError::Serde(e.to_string()))?;
            Ok(WalEntry::Checkpoint { snapshot_bytes })
        }
        TAG_FLUSH_MARKER => Ok(WalEntry::FlushMarker),
        _ => Err(DurableError::Corrupt {
            offset,
            reason: "unknown entry type tag",
        }),
    }
}

/// Append-only Write-Ahead Log backed by a single file.
///
/// # File layout
///
/// See the module-level documentation for the full on-disk format.
///
/// # Error handling
///
/// All I/O errors are returned as [`DurableError::Io`].  CRC mismatches
/// during reading are surfaced as [`DurableError::Corrupt`] with the byte
/// offset of the offending entry.
pub(crate) struct Wal {
    /// Open file handle.  Positioned at EOF after construction and each
    /// write; seeks are issued on-demand in `entries_from`.
    pub(crate) file: File,
    /// Absolute path to the WAL file on disk.
    pub(crate) path: PathBuf,
    /// Byte offset of the last valid `Checkpoint` entry (or 0 if none).
    ///
    /// Populated during [`Wal::open`] for diagnostic and incremental-replay use.
    #[allow(dead_code)] // populated by open(); used in tests and future incremental recovery
    pub(crate) last_checkpoint_offset: u64,
    /// Pending entries not yet written to disk (Relaxed mode only).
    ///
    /// In Strict mode this is always empty; callers must not push to it.
    pub(crate) pending: Vec<WalEntry>,
}

impl Wal {
    /// Creates a new WAL file at `path` and writes the file header.
    ///
    /// Fails if the file already exists.
    ///
    /// Time: O(1) — writes 8 bytes and fsyncs.
    #[tracing::instrument(skip(path), fields(path = %path.display()))]
    pub(crate) fn create(path: &Path) -> Result<Self, DurableError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        write_header(&mut file)?;
        file.flush()?;
        Ok(Self {
            file,
            path: path.to_owned(),
            last_checkpoint_offset: 0,
            pending: Vec::new(),
        })
    }

    /// Opens an existing WAL file, verifies its header, and scans forward
    /// to find the last valid `Checkpoint` offset.
    ///
    /// Time: O(n) where n is the number of entries in the WAL.
    #[tracing::instrument(skip(path), fields(path = %path.display()))]
    pub(crate) fn open(path: &Path) -> Result<Self, DurableError> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        verify_header(&mut file)?;

        // Scan all entries to find the last Checkpoint offset.
        let mut last_checkpoint_offset: u64 = 0;
        let mut pos = WAL_HEADER_SIZE;

        loop {
            match read_entry_at(&mut file, pos) {
                Ok(Some((entry, entry_total_len))) => {
                    if matches!(entry, WalEntry::Checkpoint { .. }) {
                        last_checkpoint_offset = pos;
                    }
                    pos += entry_total_len;
                }
                Ok(None) => break, // clean EOF
                Err(_) => break,   // corrupt tail — stop scan
            }
        }

        // Seek to EOF so subsequent appends go to the right place.
        file.seek(SeekFrom::End(0))?;

        Ok(Self {
            file,
            path: path.to_owned(),
            last_checkpoint_offset,
            pending: Vec::new(),
        })
    }

    /// Opens an existing WAL if it exists, or creates a new one.
    ///
    /// Time: O(n) on open (scans existing entries), O(1) on create.
    pub(crate) fn open_or_create(path: &Path) -> Result<Self, DurableError> {
        if path.exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }

    /// Appends a single entry to the WAL, optionally fsyncing afterward.
    ///
    /// In Strict mode, `fsync = true` ensures the entry is durable before
    /// control returns to the caller.  In Relaxed mode, pass `fsync = false`
    /// and call [`flush`][Self::flush] at a flush boundary.
    ///
    /// The WAL file position is advanced to EOF after the write.
    ///
    /// Time: O(|entry|) + fsync latency when `fsync = true`.
    #[tracing::instrument(skip(self, entry), fields(tag = entry_tag(entry)))]
    pub(crate) fn append(&mut self, entry: &WalEntry, fsync: bool) -> Result<(), DurableError> {
        write_entry(&mut self.file, entry)?;
        if fsync {
            self.file.sync_data()?;
        }
        Ok(())
    }

    /// Writes all pending entries (Relaxed mode buffer) to disk.
    ///
    /// Drains `self.pending`.  Does **not** fsync; the caller is responsible
    /// for deciding when to call `sync_data`.
    ///
    /// Time: O(Σ|entry|) for all pending entries.
    #[tracing::instrument(skip(self))]
    pub(crate) fn flush(&mut self) -> Result<(), DurableError> {
        let entries: Vec<WalEntry> = self.pending.drain(..).collect();
        for entry in &entries {
            write_entry(&mut self.file, entry)?;
        }
        Ok(())
    }

    /// Returns an iterator over all valid entries starting at `offset`.
    ///
    /// The iterator stops at the first entry whose CRC32C does not match
    /// its payload — i.e. at the first sign of a torn write.  All entries
    /// before the corrupt one are yielded as `Ok(_)`.  The corrupt entry
    /// itself is **not** yielded; the iterator simply stops.
    ///
    /// `offset` must be a valid entry boundary (e.g. `WAL_HEADER_SIZE` or
    /// a previously recorded checkpoint offset).
    ///
    /// Time: O(n) where n is the number of entries from `offset` to EOF.
    pub(crate) fn entries_from(
        &self,
        offset: u64,
    ) -> impl Iterator<Item = Result<(u64, WalEntry), DurableError>> + '_ {
        WalIterator {
            path: self.path.clone(),
            pos: offset,
            done: false,
        }
    }

    /// Returns the current byte length of the WAL file.
    ///
    /// Time: O(1).
    pub(crate) fn file_len(&self) -> Result<u64, DurableError> {
        Ok(self.file.metadata()?.len())
    }

    /// Truncates the WAL file to `len` bytes.
    ///
    /// Used during recovery to remove a corrupt tail entry.
    ///
    /// Time: O(1).
    pub(crate) fn truncate(&mut self, len: u64) -> Result<(), DurableError> {
        self.file.set_len(len)?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Writes the 8-byte WAL file header to `file`.
fn write_header(file: &mut File) -> Result<(), DurableError> {
    file.write_all(WAL_MAGIC)?;
    file.write_all(&WAL_VERSION.to_le_bytes())?;
    Ok(())
}

/// Reads and verifies the 8-byte WAL file header.
fn verify_header(file: &mut File) -> Result<(), DurableError> {
    let mut magic = [0u8; 4];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut magic).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            DurableError::Corrupt {
                offset: 0,
                reason: "WAL file too short to contain header",
            }
        } else {
            DurableError::Io(e)
        }
    })?;
    if &magic != WAL_MAGIC {
        return Err(DurableError::Corrupt {
            offset: 0,
            reason: "WAL magic bytes do not match",
        });
    }
    let mut version_buf = [0u8; 4];
    file.read_exact(&mut version_buf)?;
    let version = u32::from_le_bytes(version_buf);
    if version != WAL_VERSION {
        return Err(DurableError::VersionMismatch(version));
    }
    Ok(())
}

/// Serialises `entry` and writes one framed record to `file`.
///
/// Record layout: `entry_len (u64 LE) | tag (u8) | payload (N bytes) | crc32c (u32 LE)`
/// where `entry_len = 1 + N + 4`.
fn write_entry(file: &mut File, entry: &WalEntry) -> Result<(), DurableError> {
    let (tag, payload) = encode_entry(entry)?;

    // CRC covers the tag byte followed by the payload bytes.
    let crc = {
        let mut buf = Vec::with_capacity(1 + payload.len());
        buf.push(tag);
        buf.extend_from_slice(&payload);
        crc32c::crc32c(&buf)
    };

    // entry_len = tag(1) + payload(N) + crc(4)
    let entry_len: u64 = 1 + payload.len() as u64 + 4;
    file.write_all(&entry_len.to_le_bytes())?;
    file.write_all(&[tag])?;
    file.write_all(&payload)?;
    file.write_all(&crc.to_le_bytes())?;
    Ok(())
}

/// Reads one entry from `file` at byte position `pos`.
///
/// Returns `Ok(Some((entry, total_bytes_consumed)))` on success,
/// `Ok(None)` on a clean EOF at `pos`, and `Err(...)` on corruption.
fn read_entry_at(
    file: &mut File,
    pos: u64,
) -> Result<Option<(WalEntry, u64)>, DurableError> {
    file.seek(SeekFrom::Start(pos))?;

    // Read entry_len (u64 LE).
    let mut len_buf = [0u8; 8];
    match file.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(DurableError::Io(e)),
    }
    let entry_len = u64::from_le_bytes(len_buf);

    if entry_len < 5 {
        // Minimum: tag(1) + crc(4) = 5, no payload.
        return Err(DurableError::Corrupt {
            offset: pos,
            reason: "entry_len is too small (< 5)",
        });
    }

    // Read tag byte.
    let mut tag_buf = [0u8; 1];
    match file.read_exact(&mut tag_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(DurableError::Corrupt {
                offset: pos,
                reason: "unexpected EOF reading entry tag",
            });
        }
        Err(e) => return Err(DurableError::Io(e)),
    }
    let tag = tag_buf[0];

    // Read payload (entry_len - 5 bytes, where 5 = tag + crc).
    let payload_len = entry_len - 5;
    let mut payload = vec![0u8; payload_len as usize];
    if payload_len > 0 {
        match file.read_exact(&mut payload) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(DurableError::Corrupt {
                    offset: pos,
                    reason: "unexpected EOF reading entry payload",
                });
            }
            Err(e) => return Err(DurableError::Io(e)),
        }
    }

    // Read CRC (u32 LE).
    let mut crc_buf = [0u8; 4];
    match file.read_exact(&mut crc_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(DurableError::Corrupt {
                offset: pos,
                reason: "unexpected EOF reading entry CRC",
            });
        }
        Err(e) => return Err(DurableError::Io(e)),
    }
    let stored_crc = u32::from_le_bytes(crc_buf);

    // Verify CRC over (tag || payload).
    let computed_crc = {
        let mut buf = Vec::with_capacity(1 + payload.len());
        buf.push(tag);
        buf.extend_from_slice(&payload);
        crc32c::crc32c(&buf)
    };
    if computed_crc != stored_crc {
        return Err(DurableError::Corrupt {
            offset: pos,
            reason: "CRC32C mismatch — entry may be corrupt",
        });
    }

    let entry = decode_entry(tag, &payload, pos)?;
    // Total bytes consumed: entry_len_field(8) + entry_len
    let total = 8 + entry_len;
    Ok(Some((entry, total)))
}

/// Returns a short tag name for tracing.
fn entry_tag(entry: &WalEntry) -> &'static str {
    match entry {
        WalEntry::Insert { .. } => "insert",
        WalEntry::Remove { .. } => "remove",
        WalEntry::Checkpoint { .. } => "checkpoint",
        WalEntry::FlushMarker => "flush_marker",
    }
}

// ── Iterator ────────────────────────────────────────────────────────────────

/// Iterator returned by [`Wal::entries_from`].
///
/// Opens the WAL file independently so `Wal` does not need to be
/// `mut` during iteration.
struct WalIterator {
    path: PathBuf,
    pos: u64,
    done: bool,
}

impl Iterator for WalIterator {
    type Item = Result<(u64, WalEntry), DurableError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        // Re-open for each call to avoid holding a second file handle.
        // For WAL sizes in the tens of MB range this is acceptable;
        // the iterator is only used during recovery, not in hot paths.
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) => {
                self.done = true;
                return Some(Err(DurableError::Io(e)));
            }
        };

        let current_pos = self.pos;
        match read_entry_at(&mut file, current_pos) {
            Ok(None) => {
                self.done = true;
                None
            }
            Ok(Some((entry, consumed))) => {
                self.pos += consumed;
                Some(Ok((current_pos, entry)))
            }
            Err(_) => {
                // Stop at first corruption; do not yield the error.
                self.done = true;
                None
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use tempfile::tempdir;

    fn make_insert(k: &str, v: &str) -> WalEntry {
        WalEntry::Insert {
            key_bytes: postcard::to_allocvec(k).unwrap(),
            value_bytes: postcard::to_allocvec(v).unwrap(),
        }
    }

    fn make_remove(k: &str) -> WalEntry {
        WalEntry::Remove {
            key_bytes: postcard::to_allocvec(k).unwrap(),
        }
    }

    #[test]
    fn create_and_open_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        let wal = Wal::create(&path).unwrap();
        drop(wal);

        // Open should succeed.
        let wal2 = Wal::open(&path).unwrap();
        assert_eq!(wal2.last_checkpoint_offset, 0);
        let entries: Vec<_> = wal2.entries_from(WAL_HEADER_SIZE).collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn insert_remove_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert("alpha", "1"), false).unwrap();
            wal.append(&make_remove("beta"), false).unwrap();
            wal.append(&make_insert("gamma", "3"), false).unwrap();
        }

        let wal = Wal::open(&path).unwrap();
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(entries.len(), 3);

        match &entries[0].1 {
            WalEntry::Insert { key_bytes, value_bytes } => {
                assert_eq!(
                    postcard::from_bytes::<String>(key_bytes).unwrap(),
                    "alpha"
                );
                assert_eq!(
                    postcard::from_bytes::<String>(value_bytes).unwrap(),
                    "1"
                );
            }
            other => panic!("expected Insert, got {:?}", other),
        }
        assert!(matches!(entries[1].1, WalEntry::Remove { .. }));
        assert!(matches!(entries[2].1, WalEntry::Insert { .. }));
    }

    #[test]
    fn checkpoint_entry_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(
                &WalEntry::Checkpoint {
                    snapshot_bytes: vec![1, 2, 3],
                },
                false,
            )
            .unwrap();
        }

        let wal = Wal::open(&path).unwrap();
        assert_eq!(wal.last_checkpoint_offset, WAL_HEADER_SIZE);

        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
        match &entries[0].1 {
            WalEntry::Checkpoint { snapshot_bytes } => {
                assert_eq!(*snapshot_bytes, vec![1u8, 2, 3]);
            }
            other => panic!("expected Checkpoint, got {:?}", other),
        }
    }

    #[test]
    fn flush_marker_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&WalEntry::FlushMarker, false).unwrap();
        }

        let wal = Wal::open(&path).unwrap();
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].1, WalEntry::FlushMarker));
    }

    #[test]
    fn truncated_mid_entry_stops_cleanly() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert("a", "1"), false).unwrap();
            wal.append(&make_insert("b", "2"), false).unwrap();
        }

        // Truncate the file by a few bytes to corrupt the last entry.
        let meta = std::fs::metadata(&path).unwrap();
        let original_len = meta.len();
        let truncated_len = original_len - 4; // remove the last 4 bytes (mid-crc or mid-payload)
        {
            let f = OpenOptions::new().write(true).open(&path).unwrap();
            f.set_len(truncated_len).unwrap();
        }

        let wal = Wal::open(&path).unwrap();
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        // Only the first intact entry should be visible.
        assert_eq!(
            entries.len(),
            1,
            "should stop at truncated entry, got {} entries",
            entries.len()
        );
    }

    #[test]
    fn crc_mismatch_stops_before_corrupt_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        let second_entry_offset;
        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert("a", "1"), false).unwrap();
            // Record where the second entry starts.
            second_entry_offset = wal.file.seek(SeekFrom::Current(0)).unwrap();
            wal.append(&make_insert("b", "2"), false).unwrap();
        }

        // Flip a byte in the payload of the second entry (skip the 8-byte length prefix).
        {
            let mut f = OpenOptions::new().write(true).open(&path).unwrap();
            // The second entry starts at `second_entry_offset`.
            // Byte layout: entry_len(8) | tag(1) | payload | crc(4)
            // Flip byte at second_entry_offset + 8 + 1 (first byte of payload).
            let flip_offset = second_entry_offset + 8 + 1;
            f.seek(SeekFrom::Start(flip_offset)).unwrap();
            let mut byte = [0u8; 1];
            File::open(&path).unwrap().seek(SeekFrom::Start(flip_offset)).unwrap();
            std::fs::File::open(&path).unwrap().read_exact_at(&mut byte, flip_offset).unwrap();
            byte[0] ^= 0xFF;
            f.seek(SeekFrom::Start(flip_offset)).unwrap();
            f.write_all(&byte).unwrap();
        }

        let wal = Wal::open(&path).unwrap();
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        // Only the first (uncorrupted) entry should be yielded.
        assert_eq!(entries.len(), 1);
        match &entries[0].1 {
            WalEntry::Insert { key_bytes, .. } => {
                assert_eq!(
                    postcard::from_bytes::<String>(key_bytes).unwrap(),
                    "a"
                );
            }
            other => panic!("expected Insert, got {:?}", other),
        }
    }

    #[test]
    fn pending_flush_writes_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.pending.push(make_insert("x", "10"));
            wal.pending.push(make_remove("y"));
            wal.flush().unwrap();
        }

        let wal = Wal::open(&path).unwrap();
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn open_or_create_creates_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new.wal");
        assert!(!path.exists());
        let _wal = Wal::open_or_create(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn open_or_create_opens_when_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("existing.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert("k", "v"), false).unwrap();
        }

        let wal = Wal::open_or_create(&path).unwrap();
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
    }
}

// Platform-specific trait needed for `read_exact_at` in the test above.
#[cfg(test)]
#[cfg(unix)]
use std::os::unix::fs::FileExt;
