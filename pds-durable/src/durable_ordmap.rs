// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Durable wrapper around `pds::OrdMap` (feature `durable-ordmap`).

use std::marker::PhantomData;
use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::checkpoint::write_checkpoint;
use crate::durable_map::{DurableConfig, Relaxed, Strict};
use crate::error::DurableError;
use crate::wal::{Wal, WalEntry};

/// Recovers a `pds::OrdMap<K, V>` from a WAL.
fn recover_ord_map<K, V>(wal: &mut Wal) -> Result<(pds::OrdMap<K, V>, u64), DurableError>
where
    K: Clone + Ord + DeserializeOwned,
    V: Clone + DeserializeOwned,
{
    use crate::wal::WAL_HEADER_SIZE;

    let mut map: pds::OrdMap<K, V> = pds::OrdMap::new();
    let mut last_offset = WAL_HEADER_SIZE;

    for item in wal.entries_from(WAL_HEADER_SIZE) {
        let (offset, entry) = item?;
        match entry {
            WalEntry::Checkpoint { snapshot_bytes } => {
                map = postcard::from_bytes(&snapshot_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                last_offset = offset;
            }
            WalEntry::Insert {
                key_bytes,
                value_bytes,
            } => {
                let k: K = postcard::from_bytes(&key_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                let v: V = postcard::from_bytes(&value_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                map.insert(k, v);
                last_offset = offset;
            }
            WalEntry::Remove { key_bytes } => {
                let k: K = postcard::from_bytes(&key_bytes)
                    .map_err(|e| DurableError::Serde(e.to_string()))?;
                map.remove(&k);
                last_offset = offset;
            }
            WalEntry::FlushMarker => {
                last_offset = offset;
            }
        }
    }

    Ok((map, last_offset))
}

/// A `pds::OrdMap` wrapped with a WAL for crash-safe durability.
///
/// See [`DurableMap`][crate::DurableMap] for durability semantics.
pub struct DurableOrdMap<K, V, Mode = Strict> {
    inner: pds::OrdMap<K, V>,
    wal: Wal,
    config: DurableConfig,
    checkpoint_counter: usize,
    _mode: PhantomData<Mode>,
}

impl<K, V> DurableOrdMap<K, V, Strict>
where
    K: Clone + Ord + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
{
    /// Opens or creates a durable ordered map at `path`.
    ///
    /// Time: O(n) on open; O(1) on create.
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError> {
        let mut wal = Wal::open_or_create(path)?;
        let (inner, _) = recover_ord_map::<K, V>(&mut wal)?;
        Ok(Self {
            inner,
            wal,
            config,
            checkpoint_counter: 0,
            _mode: PhantomData,
        })
    }

    /// Inserts a key–value pair, fsyncing the WAL before returning.
    ///
    /// Time: O(log N) + WAL append + fsync.
    pub fn insert(&mut self, k: K, v: V) -> Result<Option<V>, DurableError> {
        let key_bytes =
            postcard::to_allocvec(&k).map_err(|e| DurableError::Serde(e.to_string()))?;
        let value_bytes =
            postcard::to_allocvec(&v).map_err(|e| DurableError::Serde(e.to_string()))?;
        self.wal.append(
            &WalEntry::Insert {
                key_bytes,
                value_bytes,
            },
            true,
        )?;
        let prev = self.inner.insert(k, v);
        self.checkpoint_counter += 1;
        self.maybe_checkpoint()?;
        Ok(prev)
    }

    /// Removes a key, fsyncing the WAL before returning.
    ///
    /// Time: O(log N) + WAL append + fsync.
    pub fn remove(&mut self, k: &K) -> Result<Option<V>, DurableError> {
        let key_bytes = postcard::to_allocvec(k).map_err(|e| DurableError::Serde(e.to_string()))?;
        self.wal.append(&WalEntry::Remove { key_bytes }, true)?;
        let prev = self.inner.remove(k);
        if prev.is_some() {
            self.checkpoint_counter += 1;
            self.maybe_checkpoint()?;
        }
        Ok(prev)
    }

    /// Returns a reference to the value for `k`, or `None`.
    ///
    /// Time: O(log N).
    pub fn get(&self, k: &K) -> Option<&V> {
        self.inner.get(k)
    }

    /// Tests whether the map contains `k`.
    ///
    /// Time: O(log N).
    pub fn contains_key(&self, k: &K) -> bool {
        self.inner.contains_key(k)
    }

    /// Returns the number of entries.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the map is empty.
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
    pub fn inner(&self) -> &pds::OrdMap<K, V> {
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

impl<K, V> DurableOrdMap<K, V, Relaxed>
where
    K: Clone + Ord + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
{
    /// Opens or creates a relaxed durable ordered map at `path`.
    ///
    /// Time: O(n) on open; O(1) on create.
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError> {
        let mut wal = Wal::open_or_create(path)?;
        let (inner, _) = recover_ord_map::<K, V>(&mut wal)?;
        Ok(Self {
            inner,
            wal,
            config,
            checkpoint_counter: 0,
            _mode: PhantomData,
        })
    }

    /// Inserts a key–value pair into the heap and buffers a WAL entry.
    ///
    /// Time: O(log N) heap + O(1) buffer.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let prev = self.inner.insert(k.clone(), v.clone());
        let key_bytes = postcard::to_allocvec(&k).unwrap_or_default();
        let value_bytes = postcard::to_allocvec(&v).unwrap_or_default();
        self.wal.pending.push(WalEntry::Insert {
            key_bytes,
            value_bytes,
        });
        self.checkpoint_counter += 1;
        prev
    }

    /// Removes a key from the heap and buffers a WAL entry.
    ///
    /// Time: O(log N) heap + O(1) buffer.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let prev = self.inner.remove(k);
        if prev.is_some() {
            let key_bytes = postcard::to_allocvec(k).unwrap_or_default();
            self.wal.pending.push(WalEntry::Remove { key_bytes });
            self.checkpoint_counter += 1;
        }
        prev
    }

    /// Returns a reference to the value for `k`, or `None`.
    ///
    /// Time: O(log N).
    pub fn get(&self, k: &K) -> Option<&V> {
        self.inner.get(k)
    }

    /// Tests whether the map contains `k`.
    ///
    /// Time: O(log N).
    pub fn contains_key(&self, k: &K) -> bool {
        self.inner.contains_key(k)
    }

    /// Returns the number of entries.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Tests whether the map is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Flushes buffered entries and writes a `FlushMarker`.
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
    pub fn inner(&self) -> &pds::OrdMap<K, V> {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    type StrictOrdMap = DurableOrdMap<String, i64, Strict>;
    type RelaxedOrdMap = DurableOrdMap<i32, String, Relaxed>;

    #[test]
    fn durable_ordmap_strict_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ordmap.wal");

        {
            let mut map = StrictOrdMap::open(&path, DurableConfig::default()).unwrap();
            map.insert("alpha".to_owned(), 1).unwrap();
            map.insert("beta".to_owned(), 2).unwrap();
            map.remove(&"alpha".to_owned()).unwrap();
        }

        let map = StrictOrdMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&"beta".to_owned()), Some(&2));
        assert!(!map.contains_key(&"alpha".to_owned()));
    }

    #[test]
    fn durable_ordmap_relaxed_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ordmap.wal");

        {
            let mut map = RelaxedOrdMap::open(&path, DurableConfig::default()).unwrap();
            map.insert(1, "one".to_owned());
            map.insert(2, "two".to_owned());
            map.flush().unwrap();
        }

        let map = RelaxedOrdMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&1), Some(&"one".to_owned()));
        assert_eq!(map.get(&2), Some(&"two".to_owned()));
    }
}
