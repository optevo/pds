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
/// 1. Scan the entire WAL, collecting all valid `(offset, entry)` pairs.
///    Stop at the first corrupt entry (CRC mismatch or truncated record).
/// 2. Find the last `Checkpoint` entry in the collected list.
/// 3. Deserialise its snapshot into an initial map (or use an empty map if
///    no checkpoint exists).
/// 4. Replay all `Insert` / `Remove` entries after the checkpoint.
/// 5. Truncate the WAL file to the byte immediately past the last valid entry,
///    removing any corrupt tail.
/// 6. Return the recovered collection and the truncation offset.
///
/// Time: O(n) where n is the number of WAL entries.
///
/// # Errors
///
/// Returns [`DurableError::Serde`] if the checkpoint snapshot cannot be
/// deserialised, or [`DurableError::Io`] for filesystem errors.
#[tracing::instrument(skip(wal))]
pub(crate) fn recover_map<K, V>(wal: &mut Wal) -> Result<(pds::HashMap<K, V>, u64), DurableError>
where
    K: Clone + Hash + Eq + DeserializeOwned,
    V: Clone + Hash + DeserializeOwned,
{
    // Phase 1 — collect all valid entries into memory.
    let mut all_entries: Vec<(u64, WalEntry)> = Vec::new();

    for item in wal.entries_from(WAL_HEADER_SIZE) {
        match item {
            Ok((offset, entry)) => {
                all_entries.push((offset, entry));
            }
            Err(_) => {
                // Stop at first corrupt or truncated entry.
                break;
            }
        }
    }

    // Compute the byte offset just past the last valid entry.
    let end_of_valid = if all_entries.is_empty() {
        WAL_HEADER_SIZE
    } else {
        let last_start = all_entries
            .last()
            .map(|(o, _)| *o)
            .unwrap_or(WAL_HEADER_SIZE);
        compute_entry_end(wal, last_start)
    };

    // Truncate corrupt tail if the file extends beyond the last valid entry.
    let file_len = wal.file_len()?;
    if end_of_valid < file_len {
        wal.truncate(end_of_valid)?;
    }

    // Phase 2 — find the last Checkpoint and build the initial map.
    let last_checkpoint_idx = all_entries
        .iter()
        .rposition(|(_, e)| matches!(e, WalEntry::Checkpoint { .. }));

    let mut map: pds::HashMap<K, V> = pds::HashMap::new();
    let replay_start_idx = match last_checkpoint_idx {
        Some(idx) => {
            if let WalEntry::Checkpoint { snapshot_bytes } = &all_entries[idx].1 {
                map = postcard::from_bytes(snapshot_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
            }
            idx + 1
        }
        None => 0,
    };

    // Phase 3 — replay Insert / Remove entries after the checkpoint.
    for (_, entry) in &all_entries[replay_start_idx..] {
        match entry {
            WalEntry::Insert {
                key_bytes,
                value_bytes,
            } => {
                let k: K = postcard::from_bytes(key_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                let v: V = postcard::from_bytes(value_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                map.insert(k, v);
            }
            WalEntry::Remove { key_bytes } => {
                let k: K = postcard::from_bytes(key_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                map.remove(&k);
            }
            WalEntry::Checkpoint { .. } | WalEntry::FlushMarker => {
                // No action during replay.
            }
        }
    }

    Ok((map, end_of_valid))
}

/// Returns the byte offset just past the WAL entry starting at `entry_start`.
///
/// Reads the 8-byte `entry_len` field from the file and returns
/// `entry_start + 8 + entry_len`.  Falls back to `WAL_HEADER_SIZE` on error.
fn compute_entry_end(wal: &Wal, entry_start: u64) -> u64 {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(&wal.path) else {
        return WAL_HEADER_SIZE;
    };
    if file.seek(SeekFrom::Start(entry_start)).is_err() {
        return WAL_HEADER_SIZE;
    }
    let mut len_buf = [0u8; 8];
    if file.read_exact(&mut len_buf).is_err() {
        return WAL_HEADER_SIZE;
    }
    let entry_len = u64::from_le_bytes(len_buf);
    entry_start + 8 + entry_len
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::Wal;
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

        let mut reference: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
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
                wal.append(&make_insert_entry(&format!("post{}", i), i + 10), false)
                    .unwrap();
            }
        }

        let mut wal = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        assert_eq!(map.len(), reference.len(), "recovered map has wrong length");
        for (k, v) in &reference {
            assert_eq!(map.get(k), Some(v), "missing key {}", k);
        }
    }

    #[test]
    fn remove_after_insert_produces_empty_map() {
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

    #[test]
    fn partial_tail_entry_truncated() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert_entry("a", 1), false).unwrap();
            wal.append(&make_insert_entry("b", 2), false).unwrap();
        }

        // Truncate the file by 4 bytes to corrupt the last entry.
        let original_len = std::fs::metadata(&path).unwrap().len();
        let truncated_len = original_len - 4;
        {
            let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.set_len(truncated_len).unwrap();
        }

        let mut wal = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        // Only the first intact entry should survive.
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a"), Some(&1));
        // WAL should have been truncated to remove the partial entry.
        let new_len = std::fs::metadata(&path).unwrap().len();
        assert!(
            new_len < original_len,
            "WAL should be shorter after truncation: new_len={} original_len={}",
            new_len,
            original_len
        );
    }

    #[test]
    fn corrupt_crc_stops_before_corrupt_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        // Write two entries; record where the second starts.
        let second_offset = {
            let mut wal = Wal::create(&path).unwrap();
            wal.append(&make_insert_entry("a", 1), false).unwrap();
            let off = wal.file_len().unwrap();
            wal.append(&make_insert_entry("b", 2), false).unwrap();
            off
        };

        // Flip a byte in the second entry's payload (skip len(8) + tag(1) = 9 bytes).
        {
            use std::io::{Read, Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let flip_pos = second_offset + 9;
            f.seek(SeekFrom::Start(flip_pos)).unwrap();
            let mut byte = [0u8; 1];
            f.read_exact(&mut byte).unwrap();
            byte[0] ^= 0xFF;
            f.seek(SeekFrom::Start(flip_pos)).unwrap();
            f.write_all(&byte).unwrap();
        }

        let mut wal = Wal::open(&path).unwrap();
        let (map, _): (pds::HashMap<String, i64>, _) = recover_map(&mut wal).unwrap();
        // Only the first (uncorrupted) entry should be present.
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a"), Some(&1));
    }
}
