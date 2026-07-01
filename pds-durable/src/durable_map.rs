// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Durable wrapper around `pds::HashMap`.
//!
//! [`DurableMap<K, V, Strict>`] fsyncs the WAL before every mutation returns —
//! no data loss on crash.  [`DurableMap<K, V, Relaxed>`] buffers mutations in
//! memory and persists them only on an explicit [`flush()`][DurableMap::flush]
//! or after reaching an auto-flush threshold.

use std::hash::Hash;
use std::marker::PhantomData;
use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::checkpoint::write_checkpoint;
use crate::error::DurableError;
use crate::recovery::recover_map;
use crate::wal::{Wal, WalEntry};

// ── Mode tags ────────────────────────────────────────────────────────────────

/// Zero-sized mode tag: WAL entry is fsynced before each mutation returns.
///
/// Every successful `insert`/`remove` is durable on return.  Use this when
/// data loss is unacceptable (e.g. financial records, configuration stores).
pub struct Strict;

/// Zero-sized mode tag: mutations are buffered; call [`DurableMap::flush`]
/// to persist them.
///
/// The write-behind buffer may lose mutations made since the last flush on
/// crash.  Use this for high-throughput workloads where some data loss is
/// acceptable (e.g. caches, derived data).
pub struct Relaxed;

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for [`DurableMap`] (both Strict and Relaxed modes).
#[derive(Debug, Clone)]
pub struct DurableConfig {
    /// Auto-checkpoint every N mutations (0 = manual only).
    ///
    /// When non-zero, a checkpoint is triggered automatically after every Nth
    /// mutation.  This keeps the WAL compact and speeds up recovery.
    pub checkpoint_every: usize,

    /// Auto-flush every N buffered mutations in Relaxed mode (0 = manual only).
    ///
    /// Ignored in Strict mode.  When non-zero, `flush()` is called
    /// automatically once `pending_count()` reaches this threshold.
    pub flush_every: usize,

    /// Compact the WAL when it exceeds this byte size.
    ///
    /// After a checkpoint, if the WAL file is larger than this value, compaction
    /// runs automatically.  Default: 64 MiB.
    pub wal_max_bytes: u64,
}

impl Default for DurableConfig {
    fn default() -> Self {
        Self {
            checkpoint_every: 0,
            flush_every: 0,
            wal_max_bytes: 64 * 1024 * 1024,
        }
    }
}

// ── DurableMap ───────────────────────────────────────────────────────────────

/// A `pds::HashMap` wrapped with a WAL for crash-safe durability.
///
/// The mode parameter `Mode` is either [`Strict`] or [`Relaxed`] and
/// determines the write durability semantics.
///
/// # Type parameters
///
/// - `K` — key type; must be `Clone + Hash + Eq + Serialize + DeserializeOwned`
/// - `V` — value type; must be `Clone + Hash + Serialize + DeserializeOwned`
///   (the `Hash` bound is required by `pds::HashMap`'s serde implementation)
/// - `Mode` — durability mode; defaults to [`Strict`]
pub struct DurableMap<K, V, Mode = Strict> {
    pub(crate) inner: pds::HashMap<K, V>,
    pub(crate) wal: Wal,
    pub(crate) config: DurableConfig,
    pub(crate) checkpoint_counter: usize,
    pub(crate) _mode: PhantomData<Mode>,
}

// ── Strict impl ──────────────────────────────────────────────────────────────

impl<K, V> DurableMap<K, V, Strict>
where
    K: Clone + Hash + Eq + Serialize + DeserializeOwned,
    V: Clone + Hash + Serialize + DeserializeOwned,
{
    /// Opens an existing WAL at `path` (replaying it) or creates a new one.
    ///
    /// Time: O(n) on open (scans WAL); O(1) on create.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`] if the file cannot be opened or created,
    /// [`DurableError::Corrupt`] if the WAL header is invalid, or
    /// [`DurableError::Serde`] if a checkpoint snapshot cannot be decoded.
    #[tracing::instrument(skip(path, config), fields(path = %path.display()))]
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError> {
        let mut wal = Wal::open_or_create(path)?;
        let (inner, _) = recover_map::<K, V>(&mut wal)?;
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
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) heap + O(|entry|) WAL append + fsync.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`] if the WAL write or fsync fails, or
    /// [`DurableError::Serde`] if serialisation fails.
    #[tracing::instrument(skip(self, k, v))]
    pub fn insert(&mut self, k: K, v: V) -> Result<Option<V>, DurableError> {
        let key_bytes =
            postcard::to_allocvec(&k).map_err(|e| DurableError::Serde(e.to_string()))?;
        let value_bytes =
            postcard::to_allocvec(&v).map_err(|e| DurableError::Serde(e.to_string()))?;

        // WAL write + fsync before heap mutation.
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

    /// Inserts multiple key–value pairs atomically, with a single fsync for the batch.
    ///
    /// This is the **group-commit** API for Strict mode.  All entries are serialised
    /// into one in-memory buffer, written to disk in a single `write_all`, then
    /// fsynced once — giving `O(|batch|)` I/O cost plus **one** fsync regardless of
    /// batch size.  Compare to N separate `insert` calls, which each pay one fsync.
    ///
    /// Returns a `Vec` of previous values (one `Option<V>` per pair, in order).
    ///
    /// Time: O(N log M) heap + O(Σ|entry|) WAL write + 1 fsync, where N = batch
    /// length and M = current map size.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`] if the write or fsync fails, or
    /// [`DurableError::Serde`] if any key or value fails to serialise.
    #[tracing::instrument(skip(self, pairs))]
    pub fn insert_batch(
        &mut self,
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Vec<Option<V>>, DurableError> {
        let pairs: Vec<(K, V)> = pairs.into_iter().collect();
        if pairs.is_empty() {
            return Ok(Vec::new());
        }

        // Build WAL entries first; fail before touching disk if serialisation fails.
        let mut wal_entries = Vec::with_capacity(pairs.len());
        for (k, v) in &pairs {
            let key_bytes =
                postcard::to_allocvec(k).map_err(|e| DurableError::Serde(e.to_string()))?;
            let value_bytes =
                postcard::to_allocvec(v).map_err(|e| DurableError::Serde(e.to_string()))?;
            wal_entries.push(WalEntry::Insert {
                key_bytes,
                value_bytes,
            });
        }

        // Single write + single fsync for the entire batch.
        self.wal.append_batch(&wal_entries, true)?;

        // Apply all heap mutations after the WAL write is durable.
        let mut prev_values = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let prev = self.inner.insert(k, v);
            prev_values.push(prev);
            self.checkpoint_counter += 1;
        }
        self.maybe_checkpoint()?;
        Ok(prev_values)
    }

    /// Removes a key, fsyncing the WAL before returning.
    ///
    /// Returns the previous value if the key was present.
    ///
    /// Time: O(log N) heap + O(|entry|) WAL append + fsync.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`] or [`DurableError::Serde`].
    #[tracing::instrument(skip(self, k))]
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

    /// Returns a reference to the value associated with `k`, or `None`.
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

    /// Returns the number of entries in the map.
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

    /// Snapshots the current state to the WAL and compacts it.
    ///
    /// After this call the WAL file contains only the header and this
    /// checkpoint entry; all previous entries are discarded.
    ///
    /// Time: O(N) where N is the serialised size of the collection.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Serde`] or [`DurableError::Io`].
    #[tracing::instrument(skip(self))]
    pub fn checkpoint(&mut self) -> Result<(), DurableError> {
        let path = self.wal.path.clone();
        write_checkpoint(&mut self.wal, &self.inner, &path)?;
        self.checkpoint_counter = 0;
        Ok(())
    }

    /// Returns a read-only reference to the underlying heap collection.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &pds::HashMap<K, V> {
        &self.inner
    }

    /// Triggers a checkpoint if `checkpoint_every > 0` and the counter has reached the threshold.
    fn maybe_checkpoint(&mut self) -> Result<(), DurableError> {
        if self.config.checkpoint_every > 0
            && self.checkpoint_counter >= self.config.checkpoint_every
        {
            self.checkpoint()?;
        }
        Ok(())
    }
}

// ── Relaxed impl ─────────────────────────────────────────────────────────────

impl<K, V> DurableMap<K, V, Relaxed>
where
    K: Clone + Hash + Eq + Serialize + DeserializeOwned,
    V: Clone + Hash + Serialize + DeserializeOwned,
{
    /// Opens an existing WAL at `path` (replaying it) or creates a new one.
    ///
    /// Time: O(n) on open (scans WAL); O(1) on create.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`], [`DurableError::Corrupt`], or
    /// [`DurableError::Serde`].
    #[tracing::instrument(skip(path, config), fields(path = %path.display()))]
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError> {
        let mut wal = Wal::open_or_create(path)?;
        let (inner, _) = recover_map::<K, V>(&mut wal)?;
        Ok(Self {
            inner,
            wal,
            config,
            checkpoint_counter: 0,
            _mode: PhantomData,
        })
    }

    /// Inserts a key–value pair into the heap collection immediately, then
    /// pushes a WAL entry to the in-memory pending buffer.
    ///
    /// The mutation is **not** durable until [`flush()`][Self::flush] is called.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) heap + O(1) buffer append.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        // Heap first — O(log N).
        let prev = self.inner.insert(k.clone(), v.clone());

        // Serialise and push to pending buffer.
        let key_bytes = postcard::to_allocvec(&k).unwrap_or_default();
        let value_bytes = postcard::to_allocvec(&v).unwrap_or_default();
        self.wal.pending.push(WalEntry::Insert {
            key_bytes,
            value_bytes,
        });

        self.checkpoint_counter += 1;
        self.maybe_auto_flush_and_checkpoint();
        prev
    }

    /// Removes a key from the heap collection and pushes a WAL remove entry
    /// to the pending buffer.
    ///
    /// The removal is **not** durable until [`flush()`][Self::flush] is called.
    ///
    /// Returns the previous value if the key was present.
    ///
    /// Time: O(log N) heap + O(1) buffer append.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let prev = self.inner.remove(k);
        if prev.is_some() {
            let key_bytes = postcard::to_allocvec(k).unwrap_or_default();
            self.wal.pending.push(WalEntry::Remove { key_bytes });
            self.checkpoint_counter += 1;
            self.maybe_auto_flush_and_checkpoint();
        }
        prev
    }

    /// Returns a reference to the value associated with `k`, or `None`.
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

    /// Returns the number of entries in the map.
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

    /// Flushes all buffered mutations to the WAL file and writes a `FlushMarker`.
    ///
    /// Does not fsync; the OS may still buffer the writes.  Call
    /// [`checkpoint()`][Self::checkpoint] for full durability.
    ///
    /// Time: O(Σ|entry|) for all pending entries.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`] if the write fails.
    #[tracing::instrument(skip(self))]
    pub fn flush(&mut self) -> Result<(), DurableError> {
        self.wal.flush()?;
        self.wal.append(&WalEntry::FlushMarker, false)?;
        self.wal.file.sync_data()?;
        Ok(())
    }

    /// Returns the number of mutations buffered but not yet flushed.
    ///
    /// Time: O(1).
    pub fn pending_count(&self) -> usize {
        self.wal.pending.len()
    }

    /// Flushes all buffered mutations, writes a checkpoint, and compacts the WAL.
    ///
    /// After this call the map state is fully durable.
    ///
    /// Time: O(N + Σ|pending entry|).
    ///
    /// # Errors
    ///
    /// Returns [`DurableError::Io`] or [`DurableError::Serde`].
    #[tracing::instrument(skip(self))]
    pub fn checkpoint(&mut self) -> Result<(), DurableError> {
        // Flush pending buffer first.
        self.wal.flush()?;
        let path = self.wal.path.clone();
        write_checkpoint(&mut self.wal, &self.inner, &path)?;
        self.checkpoint_counter = 0;
        Ok(())
    }

    /// Returns a read-only reference to the underlying heap collection.
    ///
    /// Time: O(1).
    pub fn inner(&self) -> &pds::HashMap<K, V> {
        &self.inner
    }

    /// Runs auto-flush and/or auto-checkpoint if their thresholds are met.
    ///
    /// Errors from flush/checkpoint are silently ignored to keep the
    /// Relaxed `insert`/`remove` methods infallible.  Any I/O failure
    /// will resurface on the next explicit `flush()` or `checkpoint()` call.
    fn maybe_auto_flush_and_checkpoint(&mut self) {
        if self.config.flush_every > 0 && self.wal.pending.len() >= self.config.flush_every {
            let _ = self.flush();
        }
        if self.config.checkpoint_every > 0
            && self.checkpoint_counter >= self.config.checkpoint_every
        {
            let _ = self.checkpoint();
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // Convenience aliases to avoid turbofish repetition in tests.
    type StrictMap = DurableMap<String, i64, Strict>;
    type RelaxedMap = DurableMap<String, i64, Relaxed>;

    // ── Strict tests ──────────────────────────────────────────────────────────

    #[test]
    fn strict_insert_batch_reopen_all_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            let pairs: Vec<(String, i64)> =
                (0..10i64).map(|i| (format!("batch{}", i), i)).collect();
            let prev = map.insert_batch(pairs).unwrap();
            // First insert: all previous values should be None.
            assert!(prev.iter().all(|v| v.is_none()));
            assert_eq!(map.len(), 10);
        }

        // Reopen: WAL replay must reconstruct all 10 entries.
        let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 10);
        for i in 0..10i64 {
            assert_eq!(map.get(&format!("batch{}", i)), Some(&i));
        }
    }

    #[test]
    fn strict_insert_batch_empty_is_noop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        let prev = map.insert_batch(Vec::<(String, i64)>::new()).unwrap();
        assert!(prev.is_empty());
        assert!(map.is_empty());
    }

    #[test]
    fn strict_insert_batch_returns_previous_values() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        // Insert "a" = 1 individually first.
        map.insert("a".to_owned(), 1).unwrap();

        // Batch insert overwriting "a" and adding "b".
        let pairs = vec![("a".to_owned(), 99i64), ("b".to_owned(), 2)];
        let prev = map.insert_batch(pairs).unwrap();
        assert_eq!(prev[0], Some(1)); // "a" was 1
        assert_eq!(prev[1], None); // "b" was absent
        assert_eq!(map.get(&"a".to_owned()), Some(&99));
        assert_eq!(map.get(&"b".to_owned()), Some(&2));
    }

    #[test]
    fn strict_open_empty_insert_reopen_all_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..10i64 {
                map.insert(format!("key{}", i), i).unwrap();
            }
        }

        let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 10);
        for i in 0..10i64 {
            assert_eq!(map.get(&format!("key{}", i)), Some(&i));
        }
    }

    #[test]
    fn strict_insert_remove_reopen_key_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            map.insert("x".to_owned(), 42).unwrap();
            map.remove(&"x".to_owned()).unwrap();
        }

        let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn strict_checkpoint_every_fires() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let config = DurableConfig {
            checkpoint_every: 5,
            ..Default::default()
        };
        let mut map = StrictMap::open(&path, config).unwrap();
        for i in 0..5i64 {
            map.insert(format!("k{}", i), i).unwrap();
        }
        // After 5 inserts, checkpoint_counter should have been reset.
        assert_eq!(map.checkpoint_counter, 0);
    }

    #[test]
    fn strict_len_is_empty_consistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.insert("a".to_owned(), 1).unwrap();
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);
        assert_eq!(map.len(), map.inner().len());
    }

    #[test]
    fn strict_contains_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        assert!(!map.contains_key(&"z".to_owned()));
        map.insert("z".to_owned(), 9).unwrap();
        assert!(map.contains_key(&"z".to_owned()));
    }

    #[test]
    fn strict_checkpoint_then_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = StrictMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..5i64 {
                map.insert(format!("k{}", i), i).unwrap();
            }
            map.checkpoint().unwrap();
        }

        let map = StrictMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 5);
        for i in 0..5i64 {
            assert_eq!(map.get(&format!("k{}", i)), Some(&i));
        }
    }

    // ── Relaxed tests ─────────────────────────────────────────────────────────

    #[test]
    fn relaxed_no_flush_empty_on_recovery() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..5i64 {
                map.insert(format!("k{}", i), i);
            }
            // Drop without flushing — simulate crash.
        }

        let map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
        assert!(
            map.is_empty(),
            "unflushed mutations should be lost on crash"
        );
    }

    #[test]
    fn relaxed_flush_then_recovery_all_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..5i64 {
                map.insert(format!("k{}", i), i);
            }
            map.flush().unwrap();
            // Drop after flush.
        }

        let map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 5);
        for i in 0..5i64 {
            assert_eq!(map.get(&format!("k{}", i)), Some(&i));
        }
    }

    #[test]
    fn relaxed_auto_flush_at_threshold() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let config = DurableConfig {
            flush_every: 3,
            ..Default::default()
        };

        {
            let mut map = RelaxedMap::open(&path, config).unwrap();
            for i in 0..9i64 {
                map.insert(format!("k{}", i), i);
            }
            // Auto-flush fires at 3, 6, 9.
        }

        let map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 9);
    }

    #[test]
    fn relaxed_pending_count() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.pending_count(), 0);
        map.insert("a".to_owned(), 1);
        assert_eq!(map.pending_count(), 1);
        map.insert("b".to_owned(), 2);
        assert_eq!(map.pending_count(), 2);
        map.flush().unwrap();
        assert_eq!(map.pending_count(), 0);
    }

    #[test]
    fn relaxed_checkpoint_makes_durable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("map.wal");

        {
            let mut map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
            for i in 0..4i64 {
                map.insert(format!("k{}", i), i);
            }
            map.checkpoint().unwrap();
        }

        let map = RelaxedMap::open(&path, DurableConfig::default()).unwrap();
        assert_eq!(map.len(), 4);
    }
}
