// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Durable wrapper around `pds::HashSet` (feature `durable-set`).

use std::hash::Hash;
use std::marker::PhantomData;
use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::checkpoint::write_checkpoint;
use crate::error::DurableError;
use crate::wal::{Wal, WalEntry};
use crate::durable_map::{DurableConfig, Strict, Relaxed};

// ── WAL entry helpers for sets ───────────────────────────────────────────────

fn set_insert_entry<T: Serialize>(elem: &T) -> Result<WalEntry, DurableError> {
    let elem_bytes = postcard::to_allocvec(elem).map_err(|e| DurableError::Serde(e.to_string()))?;
    // Reuse Insert with empty value_bytes to distinguish from Remove.
    Ok(WalEntry::Insert {
        key_bytes: elem_bytes,
        value_bytes: Vec::new(),
    })
}

fn set_remove_entry<T: Serialize>(elem: &T) -> Result<WalEntry, DurableError> {
    let elem_bytes = postcard::to_allocvec(elem).map_err(|e| DurableError::Serde(e.to_string()))?;
    Ok(WalEntry::Remove { key_bytes: elem_bytes })
}

/// Recovers a `pds::HashSet<T>` from a WAL by replaying Insert/Remove entries.
fn recover_set<T>(wal: &mut Wal) -> Result<(pds::HashSet<T>, u64), DurableError>
where
    T: Clone + Hash + Eq + DeserializeOwned,
{
    use crate::wal::WAL_HEADER_SIZE;

    let mut set: pds::HashSet<T> = pds::HashSet::new();
    let mut last_offset = WAL_HEADER_SIZE;

    for item in wal.entries_from(WAL_HEADER_SIZE) {
        let (offset, entry) = item?;
        match entry {
            WalEntry::Checkpoint { snapshot_bytes } => {
                set = postcard::from_bytes(&snapshot_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                last_offset = offset;
            }
            WalEntry::Insert { key_bytes, .. } => {
                let elem: T = postcard::from_bytes(&key_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                set.insert(elem);
                last_offset = offset;
            }
            WalEntry::Remove { key_bytes } => {
                let elem: T = postcard::from_bytes(&key_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                set.remove(&elem);
                last_offset = offset;
            }
            WalEntry::FlushMarker => {
                last_offset = offset;
            }
        }
    }

    Ok((set, last_offset))
}

// ── DurableSet ───────────────────────────────────────────────────────────────

/// A `pds::HashSet` wrapped with a WAL for crash-safe durability.
///
/// See [`DurableMap`][crate::DurableMap] for durability semantics.
pub struct DurableSet<T, Mode = Strict> {
    inner: pds::HashSet<T>,
    wal: Wal,
    config: DurableConfig,
    checkpoint_counter: usize,
    _mode: PhantomData<Mode>,
}

impl<T> DurableSet<T, Strict>
where
    T: Clone + Hash + Eq + Serialize + DeserializeOwned,
{
    /// Opens or creates a durable set at `path`.
    ///
    /// Time: O(n) on open; O(1) on create.
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError> {
        let mut wal = Wal::open_or_create(path)?;
        let (inner, _) = recover_set::<T>(&mut wal)?;
        Ok(Self {
            inner,
            wal,
            config,
            checkpoint_counter: 0,
            _mode: PhantomData,
        })
    }

    /// Inserts an element, fsyncing the WAL before returning.
    ///
    /// Returns `true` if the element was newly inserted, `false` if it was
    /// already present.
    ///
    /// Time: O(log N) + WAL append + fsync.
    pub fn insert(&mut self, elem: T) -> Result<bool, DurableError> {
        let entry = set_insert_entry(&elem)?;
        self.wal.append(&entry, true)?;
        let is_new = self.inner.insert(elem).is_none();
        if is_new {
            self.checkpoint_counter += 1;
            self.maybe_checkpoint()?;
        }
        Ok(is_new)
    }

    /// Removes an element, fsyncing the WAL before returning.
    ///
    /// Returns `true` if the element was present and removed.
    ///
    /// Time: O(log N) + WAL append + fsync.
    pub fn remove(&mut self, elem: &T) -> Result<bool, DurableError> {
        let entry = set_remove_entry(elem)?;
        self.wal.append(&entry, true)?;
        let removed = self.inner.remove(elem).is_some();
        if removed {
            self.checkpoint_counter += 1;
            self.maybe_checkpoint()?;
        }
        Ok(removed)
    }

    /// Tests whether `elem` is in the set.
    ///
    /// Time: O(log N).
    pub fn contains(&self, elem: &T) -> bool {
        self.inner.contains(elem)
    }

    /// Returns the number of elements in the set.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Writes a checkpoint and compacts the WAL.
    ///
    /// Time: O(N).
    pub fn checkpoint(&mut self) -> Result<(), DurableError> {
        let path = self.wal.path.clone();
        write_checkpoint(&mut self.wal, &self.inner, &path)?;
        self.checkpoint_counter = 0;
        Ok(())
    }

    /// Returns a read-only reference to the underlying collection.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &pds::HashSet<T> {
        &self.inner
    }

    fn maybe_checkpoint(&mut self) -> Result<(), DurableError> {
        if self.config.checkpoint_every > 0
            && self.checkpoint_counter >= self.config.checkpoint_every
        {
            self.checkpoint()?;
        }
        Ok(())
    }
}

impl<T> DurableSet<T, Relaxed>
where
    T: Clone + Hash + Eq + Serialize + DeserializeOwned,
{
    /// Opens or creates a relaxed durable set at `path`.
    ///
    /// Time: O(n) on open; O(1) on create.
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError> {
        let mut wal = Wal::open_or_create(path)?;
        let (inner, _) = recover_set::<T>(&mut wal)?;
        Ok(Self {
            inner,
            wal,
            config,
            checkpoint_counter: 0,
            _mode: PhantomData,
        })
    }

    /// Inserts an element into the heap set and buffers a WAL entry.
    ///
    /// Time: O(log N) heap + O(1) buffer.
    pub fn insert(&mut self, elem: T) -> bool {
        let is_new = self.inner.insert(elem.clone()).is_none();
        if is_new {
            let key_bytes = postcard::to_allocvec(&elem).unwrap_or_default();
            self.wal.pending.push(WalEntry::Insert {
                key_bytes,
                value_bytes: Vec::new(),
            });
            self.checkpoint_counter += 1;
        }
        is_new
    }

    /// Removes an element from the heap set and buffers a WAL entry.
    ///
    /// Time: O(log N) heap + O(1) buffer.
    pub fn remove(&mut self, elem: &T) -> bool {
        let removed = self.inner.remove(elem).is_some();
        if removed {
            let key_bytes = postcard::to_allocvec(elem).unwrap_or_default();
            self.wal.pending.push(WalEntry::Remove { key_bytes });
            self.checkpoint_counter += 1;
        }
        removed
    }

    /// Tests whether `elem` is in the set.
    ///
    /// Time: O(log N).
    pub fn contains(&self, elem: &T) -> bool {
        self.inner.contains(elem)
    }

    /// Returns the number of elements in the set.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the set is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Flushes buffered entries to disk and writes a `FlushMarker`.
    ///
    /// Time: O(Σ|entry|).
    pub fn flush(&mut self) -> Result<(), DurableError> {
        self.wal.flush()?;
        self.wal.append(&WalEntry::FlushMarker, false)?;
        self.wal.file.sync_data()?;
        Ok(())
    }

    /// Flushes and checkpoints.
    ///
    /// Time: O(N).
    pub fn checkpoint(&mut self) -> Result<(), DurableError> {
        self.wal.flush()?;
        let path = self.wal.path.clone();
        write_checkpoint(&mut self.wal, &self.inner, &path)?;
        self.checkpoint_counter = 0;
        Ok(())
    }

    /// Returns a read-only reference to the underlying collection.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &pds::HashSet<T> {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn durable_set_strict_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("set.wal");

        {
            let mut set: DurableSet<String, Strict> =
                DurableSet::open(&path, DurableConfig::default()).unwrap();
            set.insert("apple".to_owned()).unwrap();
            set.insert("banana".to_owned()).unwrap();
            set.remove(&"banana".to_owned()).unwrap();
        }

        let set: DurableSet<String, Strict> =
            DurableSet::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(set.len(), 1);
        assert!(set.contains(&"apple".to_owned()));
        assert!(!set.contains(&"banana".to_owned()));
    }

    #[test]
    fn durable_set_relaxed_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("set.wal");

        {
            let mut set: DurableSet<i32, Relaxed> =
                DurableSet::open(&path, DurableConfig::default()).unwrap();
            set.insert(1);
            set.insert(2);
            set.insert(3);
            set.flush().unwrap();
        }

        let set: DurableSet<i32, Relaxed> =
            DurableSet::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(set.len(), 3);
        assert!(set.contains(&1));
        assert!(set.contains(&2));
        assert!(set.contains(&3));
    }
}
