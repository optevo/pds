// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! WAL replay and recovery logic.
//!
//! [`recover_map`] rebuilds a `pds::HashMap` from a WAL by locating the last
//! `Checkpoint` entry and replaying all `Insert` / `Remove` entries that
//! follow it.

use std::hash::Hash;

use serde::de::DeserializeOwned;

use crate::error::DurableError;
use crate::wal::{Wal, WalEntry, WAL_HEADER_SIZE};

/// Recovers a `pds::HashMap<K, V>` from an existing WAL.
///
/// Algorithm:
/// 1. Scan for the last `Checkpoint` entry; deserialise its snapshot.
/// 2. Replay all `Insert` / `Remove` entries after the checkpoint offset.
/// 3. Stop at the first corrupt entry; truncate the WAL file there.
/// 4. Return the recovered collection and the last valid byte offset.
///
/// If no `Checkpoint` is present, starts from an empty map and replays
/// all entries from the beginning.
///
/// Time: O(n) where n is the number of WAL entries.
///
/// # Errors
///
/// Returns [`DurableError::Serde`] if the checkpoint snapshot cannot be
/// deserialised.  I/O errors during truncation are returned as
/// [`DurableError::Io`].
#[tracing::instrument(skip(wal))]
pub(crate) fn recover_map<K, V>(
    wal: &mut Wal,
) -> Result<(pds::HashMap<K, V>, u64), DurableError>
where
    K: Clone + Hash + Eq + DeserializeOwned,
    V: Clone + Hash + DeserializeOwned,
{
    // Phase 1 — find the last Checkpoint and its offset.
    let mut checkpoint_map: Option<pds::HashMap<K, V>> = None;
    let mut replay_from = WAL_HEADER_SIZE;

    for item in wal.entries_from(WAL_HEADER_SIZE) {
        let (offset, entry) = item?;
        if let WalEntry::Checkpoint { snapshot_bytes } = entry {
            let map: pds::HashMap<K, V> = postcard::from_bytes(&snapshot_bytes)
                .map_err(|e| DurableError::Serde(e.to_string()))?;
            checkpoint_map = Some(map);
            // Compute the offset of the entry *after* this checkpoint.
            // We need to re-derive it; store the checkpoint's own offset
            // and advance past it using entries_from which yields (offset, entry).
            replay_from = wal
                .entries_from(offset)
                .next()
                .map(|r| {
                    // We already consumed this entry; the *next* one starts after it.
                    // We need to find the size — re-scan from offset to get consumed bytes.
                    r.ok().map(|(o, _)| o)
                })
                .flatten()
                .unwrap_or(offset);
            // replay_from will be corrected in phase 2 scanning below.
        }
    }

    // Re-scan to find the correct replay_from (first entry after the last checkpoint).
    // We stored last_checkpoint_offset on the Wal, which was populated during open().
    let checkpoint_start = wal.last_checkpoint_offset;
    if checkpoint_start == 0 && checkpoint_map.is_none() {
        // No checkpoint at all: replay from the beginning.
        replay_from = WAL_HEADER_SIZE;
    } else if checkpoint_start > 0 {
        // Skip past the checkpoint entry itself.
        // entries_from yields (offset, entry); the first item at checkpoint_start
        // is the checkpoint entry. The next entry starts after it.
        let mut iter = wal.entries_from(checkpoint_start);
        if let Some(Ok((_, _))) = iter.next() {
            // iter.pos now points past the checkpoint entry.
            // Yield the next position by peeking.
            replay_from = iter.next().map(|r| r.ok().map(|(o, _)| o)).flatten()
                .unwrap_or_else(|| {
                    // No entries after checkpoint: replay nothing.
                    u64::MAX
                });
        }
    }

    // Phase 2 — replay Insert/Remove entries after the checkpoint.
    let mut map = checkpoint_map.unwrap_or_default();
    let mut last_valid_offset = replay_from;

    if replay_from != u64::MAX {
        for item in wal.entries_from(replay_from) {
            let (offset, entry) = item?;
            match entry {
                WalEntry::Insert {
                    key_bytes,
                    value_bytes,
                } => {
                    let k: K = postcard::from_bytes(&key_bytes)
                        .map_err(|e| DurableError::Serde(e.to_string()))?;
                    let v: V = postcard::from_bytes(&value_bytes)
                        .map_err(|e| DurableError::Serde(e.to_string()))?;
                    map.insert(k, v);
                    last_valid_offset = offset;
                }
                WalEntry::Remove { key_bytes } => {
                    let k: K = postcard::from_bytes(&key_bytes)
                        .map_err(|e| DurableError::Serde(e.to_string()))?;
                    map.remove(&k);
                    last_valid_offset = offset;
                }
                WalEntry::Checkpoint { .. } => {
                    // Should not happen during forward replay (we already found
                    // the last checkpoint), but harmless to skip.
                    last_valid_offset = offset;
                }
                WalEntry::FlushMarker => {
                    last_valid_offset = offset;
                }
            }
        }
    }

    // Phase 3 — truncate WAL to remove any corrupt tail.
    let file_len = wal.file_len()?;

    // Compute the actual byte offset just past the last valid entry.
    // We need to find the end of the last entry we processed.
    // Re-scan once more from `last_valid_offset` to get entry end.
    let truncate_to = if replay_from == u64::MAX {
        // No entries after checkpoint — truncate to end of checkpoint.
        find_entry_end(wal, checkpoint_start)
    } else if last_valid_offset == replay_from && map == pds::HashMap::default() && checkpoint_map.is_none() {
        // No entries replayed at all and no checkpoint.
        Some(WAL_HEADER_SIZE)
    } else {
        find_entry_end(wal, last_valid_offset)
    };

    if let Some(end) = truncate_to {
        if end < file_len {
            wal.truncate(end)?;
        }
    }

    let last_offset = truncate_to.unwrap_or(file_len);
    Ok((map, last_offset))
}

/// Finds the byte offset just past the entry that starts at `entry_offset`.
///
/// Returns `None` if `entry_offset` is past EOF or the entry is corrupt.
fn find_entry_end(wal: &Wal, entry_offset: u64) -> Option<u64> {
    wal.entries_from(entry_offset)
        .next()
        .and_then(|r| r.ok())
        .map(|(offset, _)| {
            // The iterator consumed the entry at `entry_offset` and advanced to the
            // next entry position; but our iterator yields (start_offset, entry).
            // We need to find the *end* of the entry at entry_offset.
            // That is: next_entry_start.
            // To find it, take the next entry's offset from the iterator.
            let _ = offset; // offset == entry_offset here (first item)
            // We need to peek at the next item to get the next start offset.
            // Since `entries_from` creates a fresh iterator, do it again.
            wal.entries_from(entry_offset)
                .take(2)
                .last()
                .and_then(|r| r.ok())
                .map(|(o, _)| o)
                .unwrap_or({
                    // There's no second entry; the file ends right after entry_offset's entry.
                    // We can get the end by: entry_offset + 8 (len_field) + entry_len.
                    // Instead, just return the file length — the entry is the last one.
                    u64::MAX
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::{Wal, WalEntry};
    use tempfile::tempdir;

    fn make_insert_entry(k: &str, v: i64) -> WalEntry {
        WalEntry::Insert {
            key_bytes: postcard::to_allocvec(k).unwrap(),
            value_bytes: postcard::to_allocvec(&v).unwrap(),
        }
    }

    fn make_remove_entry(k: &str) -> WalEntry {
        WalEntry::Remove {
            key_bytes: postcard::to_allocvec(k).unwrap(),
        }
    }

    #[test]
    fn empty_wal_returns_empty_map() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");
        let mut wal = Wal::create(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn n_inserts_no_checkpoint_full_replay() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            for i in 0..5i64 {
                wal.append(&make_insert_entry(&format!("k{}", i), i), false)
                    .unwrap();
            }
        }

        let mut wal = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        assert_eq!(map.len(), 5);
        for i in 0..5i64 {
            assert_eq!(map.get(&format!("k{}", i)), Some(&i));
        }
    }

    #[test]
    fn inserts_checkpoint_more_inserts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        // Build a reference map.
        let mut reference: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for i in 0..3i64 {
            reference.insert(format!("pre{}", i), i);
        }

        {
            let mut wal = Wal::create(&path).unwrap();
            // Pre-checkpoint inserts.
            for i in 0..3i64 {
                wal.append(&make_insert_entry(&format!("pre{}", i), i), false)
                    .unwrap();
            }
            // Checkpoint: serialise current state.
            let snap_map: pds::HashMap<String, i64> =
                reference.iter().map(|(k, v)| (k.clone(), *v)).collect();
            let snapshot_bytes = postcard::to_allocvec(&snap_map).unwrap();
            wal.append(&WalEntry::Checkpoint { snapshot_bytes }, false)
                .unwrap();
            // Post-checkpoint inserts.
            for i in 0..3i64 {
                reference.insert(format!("post{}", i), i + 10);
                wal.append(
                    &make_insert_entry(&format!("post{}", i), i + 10),
                    false,
                )
                .unwrap();
            }
        }

        let mut wal = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        assert_eq!(map.len(), reference.len());
        for (k, v) in &reference {
            assert_eq!(map.get(k), Some(v), "missing key {}", k);
        }
    }

    #[test]
    fn remove_after_insert() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert_entry("x", 99), false).unwrap();
            wal.append(&make_remove_entry("x"), false).unwrap();
        }

        let mut wal = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        assert!(map.is_empty());
    }
}
