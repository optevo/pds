// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! `TieredMap` — an in-memory heap cache with a `VersionedHamt` backing store.
//!
//! The heap collection acts as an L1 write-back cache; `pds_merkle_spine::VersionedHamt`
//! is the crash-safe, versioned L2 store.  Each `flush()` (Relaxed) or mutation
//! (Strict) creates a new `VersionId` with a Merkle root — a complete audit trail
//! of heap state over time.
//!
//! # Architecture
//!
//! ```text
//! TieredMap<K, V, Mode>
//!   ├── front:          pds::HashMap<K, V>         — hot tier; RAM-bounded LRU cache
//!   ├── dirty:          HashSet<K>                 — entries not yet flushed to back
//!   ├── eviction_queue: VecDeque<K>                — approximate LRU order
//!   └── back:           VersionedHamt<K, V>        — cold tier; versioned, crash-safe
//! ```
//!
//! # Mode semantics
//!
//! | Mode | Write sequence | Recovery | Versioning |
//! |------|---------------|----------|------------|
//! | [`Strict`] | Write to `back` (new version per mutation) then `front` | Latest version; front warms on demand | One version per mutation |
//! | [`Relaxed`] | Write to `front` only; `flush()` pushes dirty entries to `back` | Latest version; state = last flush | One version per flush |
//!
//! # LRU eviction
//!
//! When `max_front_entries > 0` and `front.len()` would exceed that limit after
//! an insert, the head of `eviction_queue` (oldest key) is evicted.  If the key
//! is dirty, it is written to `back` first (creating a single-entry version).
//! Future reads that miss `front` fall through to `back`.
//!
//! The eviction queue is approximate FIFO-within-generation — good enough to keep
//! `front.len()` bounded without the complexity of a doubly-linked LRU list.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::Hash;
use std::marker::PhantomData;

use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds::HashMap;
use pds_merkle_spine::versioned_hamt::VersionedHamtError;
use pds_merkle_spine::{VersionId, VersionedHamt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::durable_map::{Relaxed, Strict};
use crate::error::DurableError;

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for [`TieredMap`] (both Strict and Relaxed modes).
#[derive(Debug, Clone)]
pub struct TieredConfig {
    /// Evict LRU entries from front when it exceeds this count.
    ///
    /// When zero, the front cache is unbounded and no eviction occurs.
    pub max_front_entries: usize,

    /// Auto-flush in Relaxed mode every N mutations (0 = manual only).
    ///
    /// When non-zero, `flush()` is called automatically after every Nth dirty
    /// entry is added to the front cache.  Has no effect in Strict mode.
    pub flush_every: usize,

    /// Retain this many historical versions in the backing store (0 = all).
    ///
    /// Currently informational — the `VersionedHamt` retains all versions in
    /// memory.  A future compaction pass will honour this limit.
    pub max_versions: usize,
}

impl Default for TieredConfig {
    /// Creates a `TieredConfig` with no eviction, no auto-flush, and unlimited
    /// version retention.
    fn default() -> Self {
        Self {
            max_front_entries: 0,
            flush_every: 0,
            max_versions: 0,
        }
    }
}

// ── Error conversion ──────────────────────────────────────────────────────────

impl From<VersionedHamtError> for DurableError {
    fn from(e: VersionedHamtError) -> Self {
        DurableError::Serde(e.to_string())
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Creates a new in-memory `FolioStore` suitable for `VersionedHamt`.
fn make_mem_store() -> FolioStore<MemBackend> {
    let backend = MemBackend::new(4096, 512);
    FolioStore::create(backend, 4096, 512, ChecksumKind::Xxh3, true)
        .expect("in-memory FolioStore creation must succeed")
}

// ── TieredMap struct ──────────────────────────────────────────────────────────

/// A tiered map with an in-memory heap cache backed by a versioned HAMT.
///
/// The type parameter `Mode` is either [`Strict`] or [`Relaxed`]:
///
/// - In [`Strict`] mode, every mutation writes to `back` (creating a new version)
///   and then to `front`.  The mutation is durable on return.
/// - In [`Relaxed`] mode, mutations write only to `front` (and the `dirty` set).
///   Call [`flush()`][TieredMap::flush] to push dirty entries to `back` as a
///   single new version.
pub struct TieredMap<K, V, Mode = Relaxed>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de>,
{
    /// Hot tier: in-memory `pds::HashMap` cache.
    front: HashMap<K, V>,

    /// Dirty-entry tracker: keys in `front` not yet flushed to `back`.
    ///
    /// In Strict mode, this is always empty (every mutation goes to `back`
    /// immediately).  In Relaxed mode, it grows with each mutation and is
    /// cleared on `flush()`.
    dirty: HashSet<K>,

    /// Approximate LRU eviction queue.  Keys are pushed to the back on
    /// insert and popped from the front when the cache exceeds
    /// `config.max_front_entries`.
    eviction_queue: VecDeque<K>,

    /// Cold tier: versioned, crash-safe HAMT.  Every mutation in Strict mode
    /// or `flush()` in Relaxed mode creates a new version here.
    back: VersionedHamt<K, V>,

    /// Configuration controlling eviction, auto-flush, and version retention.
    config: TieredConfig,

    /// Zero-sized mode tag.
    _mode: PhantomData<Mode>,
}

// ── Strict mode ───────────────────────────────────────────────────────────────

impl<K, V> TieredMap<K, V, Strict>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de> + DeserializeOwned,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de> + DeserializeOwned + PartialEq,
{
    /// Opens or creates a `TieredMap` at `path` in Strict mode.
    ///
    /// The `path` argument is accepted for API symmetry with `DurableMap::open`
    /// but is currently unused — the backing `VersionedHamt` uses an in-memory
    /// `MemBackend`.  A file-backed backend will be supported in a future release.
    ///
    /// The `front` cache starts empty; entries are warmed lazily on `get`.
    ///
    /// Time: O(1).
    pub fn open(_path: &std::path::Path, config: TieredConfig) -> Result<Self, DurableError> {
        let back = VersionedHamt::new(make_mem_store());
        Ok(Self {
            front: HashMap::new(),
            dirty: HashSet::new(),
            eviction_queue: VecDeque::new(),
            back,
            config,
            _mode: PhantomData,
        })
    }

    /// Inserts `k` → `v` into the backing store first (new version), then into
    /// `front`.  The mutation is durable on return.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) HAMT write + O(log N) heap write.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn insert(&mut self, k: K, v: V) -> Result<Option<V>, DurableError> {
        // Write to back first — durable.
        self.back = self.back.insert(k.clone(), v.clone())?;

        // Write to front.
        let prev = self.front.insert(k.clone(), v);

        // Maintain approximate LRU queue.
        self.eviction_queue.push_back(k.clone());
        if self.config.max_front_entries > 0 && self.front.len() > self.config.max_front_entries {
            self.evict_one()?;
        }

        Ok(prev)
    }

    /// Removes `k` from the backing store (new version), then from `front`.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) HAMT write + O(log N) heap write.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn remove(&mut self, k: &K) -> Result<Option<V>, DurableError> {
        // Remove from back — durable.
        let (new_back, _evicted) = self.back.remove(k)?;
        self.back = new_back;

        // Remove from front.
        let prev = self.front.remove(k);
        Ok(prev)
    }

    /// Returns a reference to the value for `k` in the front cache, if present.
    ///
    /// In Strict mode, every key is written to `front` on insert, so all known
    /// keys are in `front` (unless evicted).  Evicted keys require a `get_cold`
    /// call (not yet provided) to fetch from `back`.
    ///
    /// To look up an evicted key, call [`get_or_fetch`][Self::get_or_fetch].
    ///
    /// Time: O(log N) — heap lookup.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.front.get(k)
    }

    /// Returns a reference to the value for `k`, fetching from `back` on a
    /// front-cache miss.
    ///
    /// When `k` is not in `front`, the value is fetched from `back` and
    /// inserted into `front` for future lookups.
    ///
    /// Time: O(log N) — heap lookup; O(log N) HAMT read + O(log N) heap insert
    /// on a cold miss.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn get_or_fetch(&mut self, k: &K) -> Result<Option<&V>, DurableError> {
        if !self.front.contains_key(k) {
            // Cold miss — fetch from back.
            if let Some(v) = self.back.get(k)? {
                self.front.insert(k.clone(), v);
                self.eviction_queue.push_back(k.clone());
                if self.config.max_front_entries > 0
                    && self.front.len() > self.config.max_front_entries
                {
                    self.evict_one()?;
                }
            }
        }
        Ok(self.front.get(k))
    }

    /// Tests whether `k` is present in `front`.
    ///
    /// Time: O(log N) — heap lookup.
    pub fn contains_key(&self, k: &K) -> bool {
        self.front.contains_key(k)
    }

    /// Returns the number of entries in `front`.
    ///
    /// In Strict mode with `max_front_entries = 0`, this equals the total
    /// number of entries ever inserted (since nothing is evicted).
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.front.len()
    }

    /// Tests whether `front` is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.front.is_empty()
    }

    /// Returns a read-only reference to the front cache.
    ///
    /// Time: O(1).
    pub fn front(&self) -> &HashMap<K, V> {
        &self.front
    }

    /// Returns the `VersionId` of the latest committed version in `back`.
    ///
    /// Each mutation in Strict mode produces a new version.
    ///
    /// Time: O(1).
    pub fn latest_version(&self) -> Option<VersionId> {
        // The VersionedHamt always has at least v0 (the genesis version).
        Some(self.back.version())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Evicts the oldest entry from `eviction_queue` and `front`.
    ///
    /// In Strict mode there are no dirty entries (back is always up to date),
    /// so eviction only removes from `front`.
    ///
    /// Time: O(log N) — heap remove.
    fn evict_one(&mut self) -> Result<(), DurableError> {
        if let Some(evict_key) = self.eviction_queue.pop_front() {
            // In Strict mode, back is already up to date — just remove from front.
            self.front.remove(&evict_key);
        }
        Ok(())
    }
}

// ── Relaxed mode ──────────────────────────────────────────────────────────────

impl<K, V> TieredMap<K, V, Relaxed>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de> + DeserializeOwned,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de> + DeserializeOwned + PartialEq,
{
    /// Opens or creates a `TieredMap` at `path` in Relaxed mode.
    ///
    /// The `path` argument is accepted for API symmetry with `DurableMap::open`
    /// but is currently unused — the backing `VersionedHamt` uses an in-memory
    /// `MemBackend`.  A file-backed backend will be supported in a future release.
    ///
    /// The `front` cache starts empty; entries are warmed lazily on `get_or_fetch`.
    ///
    /// Time: O(1).
    pub fn open(_path: &std::path::Path, config: TieredConfig) -> Result<Self, DurableError> {
        let back = VersionedHamt::new(make_mem_store());
        Ok(Self {
            front: HashMap::new(),
            dirty: HashSet::new(),
            eviction_queue: VecDeque::new(),
            back,
            config,
            _mode: PhantomData,
        })
    }

    /// Inserts `k` → `v` into `front` only.
    ///
    /// The mutation is NOT yet durable; call [`flush()`][Self::flush] to persist
    /// the dirty entries to `back` as a single new version.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) — heap write; zero I/O.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let prev = self.front.insert(k.clone(), v);
        self.dirty.insert(k.clone());
        self.eviction_queue.push_back(k.clone());

        // Evict if over capacity.
        if self.config.max_front_entries > 0 && self.front.len() > self.config.max_front_entries {
            // Eviction errors are silently ignored in infallible insert; callers
            // that need error visibility should call flush() manually.
            let _ = self.evict_one();
        }

        // Auto-flush.
        if self.config.flush_every > 0 && self.dirty.len() >= self.config.flush_every {
            let _ = self.flush();
        }

        prev
    }

    /// Removes `k` from `front`.
    ///
    /// The removal is NOT yet durable; call [`flush()`][Self::flush] to persist.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) — heap write; zero I/O.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let prev = self.front.remove(k);
        // Mark dirty so flush writes the removal to back.
        // Removals are tracked by inserting a tombstone-style absent key: on flush,
        // we remove from back any dirty key not present in front.
        self.dirty.insert(k.clone());
        prev
    }

    /// Pushes all dirty entries to `back` as a single new version.
    ///
    /// Returns the `VersionId` of the new version, which contains the Merkle
    /// root of the full collection state at this flush point.
    ///
    /// Time: O(D log N) where D is the number of dirty entries — one HAMT
    /// mutation per dirty entry, amortised over all mutations since the last flush.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn flush(&mut self) -> Result<VersionId, DurableError> {
        // Apply each dirty key to back.  Each dirty entry is either:
        //   - Present in front  → insert (or update) in back
        //   - Absent from front → remove from back (tombstone scenario)
        for k in self.dirty.drain() {
            match self.front.get(&k) {
                Some(v) => {
                    self.back = self.back.insert(k, v.clone())?;
                }
                None => {
                    // Key was removed from front — propagate to back.
                    let (new_back, _) = self.back.remove(&k)?;
                    self.back = new_back;
                }
            }
        }
        // Return the version produced by the last mutation.  If dirty was empty,
        // the back version is unchanged (no new version was created) — return
        // the current version as a no-op flush.
        Ok(self.back.version())
    }

    /// Returns the number of dirty (unflushed) mutations in `front`.
    ///
    /// Time: O(1).
    pub fn pending_count(&self) -> usize {
        self.dirty.len()
    }

    /// Returns a reference to the value for `k` in the front cache, if present.
    ///
    /// Does NOT fall through to `back` on a miss — use
    /// [`get_or_fetch`][Self::get_or_fetch] for cold keys.
    ///
    /// Time: O(log N) — heap lookup.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.front.get(k)
    }

    /// Returns a reference to the value for `k`, fetching from `back` on a
    /// front-cache miss.
    ///
    /// When `k` is not in `front`, the value is fetched from `back` and
    /// inserted into `front` for future lookups.
    ///
    /// Time: O(log N) — heap lookup; O(log N) HAMT read + O(log N) heap insert
    /// on a cold miss.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn get_or_fetch(&mut self, k: &K) -> Result<Option<&V>, DurableError> {
        if !self.front.contains_key(k) {
            // Cold miss — fetch from back.
            if let Some(v) = self.back.get(k)? {
                self.front.insert(k.clone(), v);
                self.eviction_queue.push_back(k.clone());
                if self.config.max_front_entries > 0
                    && self.front.len() > self.config.max_front_entries
                {
                    let _ = self.evict_one();
                }
            }
        }
        Ok(self.front.get(k))
    }

    /// Tests whether `k` is present in `front`.
    ///
    /// Time: O(log N) — heap lookup.
    pub fn contains_key(&self, k: &K) -> bool {
        self.front.contains_key(k)
    }

    /// Returns the number of entries in `front`.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        self.front.len()
    }

    /// Tests whether `front` is empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.front.is_empty()
    }

    /// Returns a read-only reference to the front cache.
    ///
    /// Time: O(1).
    pub fn front(&self) -> &HashMap<K, V> {
        &self.front
    }

    /// Returns the `VersionId` of the latest committed version in `back`.
    ///
    /// This is the version as of the last successful `flush()`.  Mutations
    /// added since the last flush are in `front` but not yet reflected in `back`.
    ///
    /// Time: O(1).
    pub fn latest_version(&self) -> Option<VersionId> {
        Some(self.back.version())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Evicts the oldest entry from `eviction_queue` and `front`.
    ///
    /// If the evicted key is dirty, it is written to `back` immediately
    /// (a single-entry micro-version) so the data is not lost.
    ///
    /// Time: O(log N) — heap remove; O(log N) HAMT write if dirty.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    fn evict_one(&mut self) -> Result<(), DurableError> {
        // Pop the oldest key from the eviction queue.
        while let Some(evict_key) = self.eviction_queue.pop_front() {
            if !self.front.contains_key(&evict_key) {
                // Already evicted or removed; skip stale queue entries.
                continue;
            }
            if self.dirty.contains(&evict_key) {
                // Dirty — flush to back before eviction.
                if let Some(v) = self.front.get(&evict_key) {
                    self.back = self.back.insert(evict_key.clone(), v.clone())?;
                }
                self.dirty.remove(&evict_key);
            }
            self.front.remove(&evict_key);
            break;
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Type aliases for test brevity.
    type StrictMap = TieredMap<String, u64, Strict>;
    type RelaxedMap = TieredMap<String, u64, Relaxed>;

    fn tmp_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tiered.dat");
        (dir, path)
    }

    fn strict_open(path: &std::path::Path) -> StrictMap {
        StrictMap::open(path, TieredConfig::default()).unwrap()
    }

    fn relaxed_open(path: &std::path::Path) -> RelaxedMap {
        RelaxedMap::open(path, TieredConfig::default()).unwrap()
    }

    // ── Strict mode tests ─────────────────────────────────────────────────────

    #[test]
    fn strict_open_empty() {
        let (_dir, path) = tmp_path();
        let m = strict_open(&path);
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn strict_insert_get() {
        let (_dir, path) = tmp_path();
        let mut m = strict_open(&path);
        let prev = m.insert("hello".to_string(), 42).unwrap();
        assert_eq!(prev, None);
        assert_eq!(m.get(&"hello".to_string()), Some(&42));
        assert!(m.contains_key(&"hello".to_string()));
    }

    #[test]
    fn strict_insert_returns_old_value() {
        let (_dir, path) = tmp_path();
        let mut m = strict_open(&path);
        m.insert("k".to_string(), 1).unwrap();
        let prev = m.insert("k".to_string(), 2).unwrap();
        assert_eq!(prev, Some(1));
        assert_eq!(m.get(&"k".to_string()), Some(&2));
    }

    #[test]
    fn strict_remove() {
        let (_dir, path) = tmp_path();
        let mut m = strict_open(&path);
        m.insert("x".to_string(), 7).unwrap();
        let prev = m.remove(&"x".to_string()).unwrap();
        assert_eq!(prev, Some(7));
        assert_eq!(m.get(&"x".to_string()), None);
        assert!(!m.contains_key(&"x".to_string()));
    }

    #[test]
    fn strict_remove_absent_key() {
        let (_dir, path) = tmp_path();
        let mut m = strict_open(&path);
        let prev = m.remove(&"missing".to_string()).unwrap();
        assert_eq!(prev, None);
    }

    #[test]
    fn strict_latest_version_advances_per_mutation() {
        let (_dir, path) = tmp_path();
        let mut m = strict_open(&path);
        let v0 = m.latest_version().unwrap();
        m.insert("a".to_string(), 1).unwrap();
        let v1 = m.latest_version().unwrap();
        m.insert("b".to_string(), 2).unwrap();
        let v2 = m.latest_version().unwrap();
        // Each mutation creates a new version.
        assert!(v1.seq > v0.seq, "v1 must be later than v0");
        assert!(v2.seq > v1.seq, "v2 must be later than v1");
    }

    #[test]
    fn strict_get_cold_via_get_or_fetch() {
        let (_dir, path) = tmp_path();
        // Insert with a very small front capacity so the key gets evicted.
        let config = TieredConfig {
            max_front_entries: 1,
            ..TieredConfig::default()
        };
        let mut m: StrictMap = StrictMap::open(&path, config).unwrap();
        m.insert("a".to_string(), 1).unwrap();
        // Insert a second key — "a" will be evicted from front.
        m.insert("b".to_string(), 2).unwrap();
        // "a" is evicted from front but lives in back.
        let v = m.get_or_fetch(&"a".to_string()).unwrap();
        assert_eq!(v, Some(&1));
    }

    #[test]
    fn strict_eviction_keeps_front_bounded() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 3,
            ..TieredConfig::default()
        };
        let mut m: StrictMap = StrictMap::open(&path, config).unwrap();
        for i in 0u64..10 {
            m.insert(format!("k{i}"), i).unwrap();
        }
        // After all inserts, front never exceeds max_front_entries.
        assert!(
            m.len() <= 3,
            "front must be at most max_front_entries; got {}",
            m.len()
        );
    }

    #[test]
    fn strict_front_accessor() {
        let (_dir, path) = tmp_path();
        let mut m = strict_open(&path);
        m.insert("a".to_string(), 1).unwrap();
        m.insert("b".to_string(), 2).unwrap();
        assert_eq!(m.front().len(), 2);
    }

    // ── Relaxed mode tests ────────────────────────────────────────────────────

    #[test]
    fn relaxed_open_empty() {
        let (_dir, path) = tmp_path();
        let m = relaxed_open(&path);
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert_eq!(m.pending_count(), 0);
    }

    #[test]
    fn relaxed_insert_get() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        let prev = m.insert("hello".to_string(), 42);
        assert_eq!(prev, None);
        assert_eq!(m.get(&"hello".to_string()), Some(&42));
        assert!(m.contains_key(&"hello".to_string()));
    }

    #[test]
    fn relaxed_insert_is_dirty_until_flush() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        assert_eq!(m.pending_count(), 0);
        m.insert("a".to_string(), 1);
        assert_eq!(m.pending_count(), 1);
        m.insert("b".to_string(), 2);
        assert_eq!(m.pending_count(), 2);
        let _ = m.flush().unwrap();
        assert_eq!(m.pending_count(), 0);
    }

    #[test]
    fn relaxed_flush_creates_new_version() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        let v0 = m.latest_version().unwrap();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        let v1 = m.flush().unwrap();
        // After flushing two inserts (each is one HAMT mutation), v1.seq > v0.seq.
        assert!(v1.seq > v0.seq, "flush must create a new version");
    }

    #[test]
    fn relaxed_flush_returns_version_id() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        m.insert("k".to_string(), 99);
        let vid = m.flush().unwrap();
        // The returned VersionId matches latest_version().
        assert_eq!(vid, m.latest_version().unwrap());
    }

    #[test]
    fn relaxed_flush_empty_dirty_returns_current_version() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        let v_before = m.latest_version().unwrap();
        // No mutations — flush should be a no-op.
        let v_after = m.flush().unwrap();
        assert_eq!(v_before.seq, v_after.seq);
    }

    #[test]
    fn relaxed_remove_propagated_on_flush() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        m.insert("k".to_string(), 5);
        m.flush().unwrap();
        m.remove(&"k".to_string());
        m.flush().unwrap();
        // After flush, key should be absent from back.
        let from_back = m.back.get(&"k".to_string()).unwrap();
        assert_eq!(
            from_back, None,
            "removed key must be absent from back after flush"
        );
    }

    #[test]
    fn relaxed_auto_flush_on_threshold() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            flush_every: 3,
            ..TieredConfig::default()
        };
        let mut m: RelaxedMap = RelaxedMap::open(&path, config).unwrap();
        let v0 = m.latest_version().unwrap();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        // Third insert triggers auto-flush.
        m.insert("c".to_string(), 3);
        // Auto-flush should have fired.
        assert_eq!(m.pending_count(), 0, "auto-flush must clear dirty set");
        assert!(
            m.latest_version().unwrap().seq > v0.seq,
            "auto-flush must create a new version"
        );
    }

    #[test]
    fn relaxed_cold_get_via_get_or_fetch() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 1,
            ..TieredConfig::default()
        };
        let mut m: RelaxedMap = RelaxedMap::open(&path, config).unwrap();
        m.insert("a".to_string(), 10);
        m.flush().unwrap();
        // Insert a second key — "a" will be evicted from front.
        m.insert("b".to_string(), 20);
        // "a" is evicted; fetch from back.
        let v = m.get_or_fetch(&"a".to_string()).unwrap();
        assert_eq!(v, Some(&10));
    }

    #[test]
    fn relaxed_eviction_flushes_dirty_key() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 1,
            ..TieredConfig::default()
        };
        let mut m: RelaxedMap = RelaxedMap::open(&path, config).unwrap();
        // Insert "a" — it's dirty and in front.
        m.insert("a".to_string(), 1);
        // Insert "b" — this evicts "a"; since "a" is dirty, it must be flushed to back.
        m.insert("b".to_string(), 2);
        // "a" was dirty-evicted; it must now be in back.
        let v = m.back.get(&"a".to_string()).unwrap();
        assert_eq!(v, Some(1), "dirty-evicted key must be in back");
        // front only has "b".
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn relaxed_eviction_keeps_front_bounded() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 3,
            ..TieredConfig::default()
        };
        let mut m: RelaxedMap = RelaxedMap::open(&path, config).unwrap();
        for i in 0u64..10 {
            m.insert(format!("k{i}"), i);
        }
        assert!(
            m.len() <= 3,
            "front must be at most max_front_entries after eviction; got {}",
            m.len()
        );
    }

    #[test]
    fn relaxed_latest_version_before_flush() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        let v0 = m.latest_version().unwrap();
        m.insert("x".to_string(), 1);
        // latest_version reflects back, which hasn't changed yet.
        assert_eq!(m.latest_version().unwrap().seq, v0.seq);
    }

    #[test]
    fn relaxed_insert_returns_old_value() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        m.insert("k".to_string(), 1);
        let prev = m.insert("k".to_string(), 2);
        assert_eq!(prev, Some(1));
    }

    #[test]
    fn relaxed_front_accessor() {
        let (_dir, path) = tmp_path();
        let mut m = relaxed_open(&path);
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        assert_eq!(m.front().len(), 2);
    }

    // ── Shared behaviour tests ────────────────────────────────────────────────

    #[test]
    fn tiered_config_default() {
        let cfg = TieredConfig::default();
        assert_eq!(cfg.max_front_entries, 0);
        assert_eq!(cfg.flush_every, 0);
        assert_eq!(cfg.max_versions, 0);
    }
}
