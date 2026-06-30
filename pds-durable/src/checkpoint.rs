// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Checkpoint writing and WAL compaction.
//!
//! A checkpoint serialises the full current state of the collection into the WAL
//! as a `Checkpoint` entry, then compacts the WAL by atomically replacing the
//! file with a new one that contains only the checkpoint (all preceding entries
//! become redundant).

use std::hash::Hash;
use std::path::Path;

use serde::Serialize;

use crate::error::DurableError;
use crate::wal::{Wal, WalEntry};

/// Writes a checkpoint entry and compacts the WAL.
///
/// Steps:
/// 1. Serialise `collection` to `snapshot_bytes` via `postcard::to_allocvec`.
/// 2. Append a `Checkpoint` entry to `wal` with fsync.
/// 3. Write a new WAL at `<path>.wal.tmp` (header + checkpoint entry only).
/// 4. `std::fs::rename(tmp, path)` — atomic replace on POSIX.
/// 5. Update `wal.last_checkpoint_offset` and reset `checkpoint_counter`.
///
/// Time: O(N) where N is the serialised size of the collection.
///
/// # Errors
///
/// Returns [`DurableError::Serde`] if serialisation fails, or
/// [`DurableError::Io`] for any filesystem error.
#[tracing::instrument(skip(wal, collection, path), fields(path = %path.display()))]
pub(crate) fn write_checkpoint<C>(
    wal: &mut Wal,
    collection: &C,
    path: &Path,
) -> Result<(), DurableError>
where
    C: Serialize,
{
    // Step 1 — serialise the full collection.
    let snapshot_bytes =
        postcard::to_allocvec(collection).map_err(|e| DurableError::Serde(e.to_string()))?;

    let checkpoint_entry = WalEntry::Checkpoint {
        snapshot_bytes: snapshot_bytes.clone(),
    };

    // Step 2 — append checkpoint entry with fsync.
    wal.append(&checkpoint_entry, true)?;

    // Steps 3 & 4 — compact: write tmp WAL, then atomically rename.
    compact_wal(wal, path, &snapshot_bytes)?;

    Ok(())
}

/// Replaces the WAL at `path` with a new file containing only the latest
/// `Checkpoint` entry.
///
/// The new file is written to `<path>.tmp`, then renamed atomically to
/// `path`.  On POSIX this rename is atomic; on other platforms it may not be.
///
/// Time: O(|snapshot_bytes|).
pub(crate) fn compact_wal(
    wal: &mut Wal,
    path: &Path,
    snapshot_bytes: &[u8],
) -> Result<(), DurableError> {
    let tmp_path = path.with_extension("wal.tmp");

    // Write new WAL with header + checkpoint entry only.
    {
        let mut tmp_wal = Wal::create(&tmp_path)?;
        tmp_wal.append(
            &WalEntry::Checkpoint {
                snapshot_bytes: snapshot_bytes.to_vec(),
            },
            true,
        )?;
    }

    // Atomic rename.
    std::fs::rename(&tmp_path, path)?;

    // Re-open the compacted WAL so `wal` points to the new file.
    *wal = Wal::open(path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::{WalEntry, WAL_HEADER_SIZE};
    use tempfile::tempdir;

    #[test]
    fn checkpoint_compacts_wal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        let collection: pds::HashMap<String, i64> =
            vec![("a".to_owned(), 1i64), ("b".to_owned(), 2)]
                .into_iter()
                .collect();

        let mut wal = Wal::create(&path).unwrap();
        // Write some entries first.
        wal.append(
            &WalEntry::Insert {
                key_bytes: postcard::to_allocvec("a").unwrap(),
                value_bytes: postcard::to_allocvec(&1i64).unwrap(),
            },
            false,
        )
        .unwrap();

        write_checkpoint(&mut wal, &collection, &path).unwrap();

        // After compaction: WAL should contain header + one Checkpoint entry only.
        let entries: Vec<_> = wal
            .entries_from(WAL_HEADER_SIZE)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].1, WalEntry::Checkpoint { .. }));
    }

    #[test]
    fn checkpoint_then_more_inserts_recovers_all() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        let pre_map: pds::HashMap<String, i64> =
            vec![("k1".to_owned(), 1i64)].into_iter().collect();

        let mut wal = Wal::create(&path).unwrap();
        write_checkpoint(&mut wal, &pre_map, &path).unwrap();

        // Append more inserts after the checkpoint.
        wal.append(
            &WalEntry::Insert {
                key_bytes: postcard::to_allocvec("k2").unwrap(),
                value_bytes: postcard::to_allocvec(&2i64).unwrap(),
            },
            false,
        )
        .unwrap();

        // Recover.
        let mut wal2 = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) =
            crate::recovery::recover_map(&mut wal2).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("k1"), Some(&1));
        assert_eq!(map.get("k2"), Some(&2));
    }
}
