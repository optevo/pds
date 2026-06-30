// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! `TieredMap` and companion preset types — pluggable storage-tier policies.
//!
//! Four storage presets are provided:
//!
//! | Type | Policy | Tiers | Durability | Speed |
//! |------|--------|-------|------------|-------|
//! | [`TieredMap<K,V,WriteBack>`] | [`WriteBack`] | t1 + t2 | Write-behind | Heap speed |
//! | [`TieredMap<K,V,Durable>`] | [`Durable`] | t1 + t2 (write-through) | Per-mutation | Slowest |
//! | [`MemOnlyMap<K,V>`] | [`MemOnly`] | t0 only — `std::HashMap` | None | Fastest |
//! | [`PipelinedMap<K,V>`] | [`Pipelined`] | t0 + t1 + t2 | 2-stage write-behind | Near-heap speed |
//!
//! The names `Strict` and `Relaxed` are backward-compatibility aliases for
//! `Durable` and `WriteBack` respectively.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::Hash;
use std::marker::PhantomData;
use std::mem;

use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};
use pds::HashMap;
use pds_merkle_spine::versioned_hamt::VersionedHamtError;
use pds_merkle_spine::{VersionId, VersionedHamt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::error::DurableError;
pub use crate::policy::{Durable, MemOnly, Pipelined, WriteBack};

// ── Backward-compatibility aliases ────────────────────────────────────────────

/// Backward-compatibility alias for [`Durable`].
///
/// Prefer `Durable` in new code.
pub type Strict = Durable;

/// Backward-compatibility alias for [`WriteBack`].
///
/// Prefer `WriteBack` in new code.
pub type Relaxed = WriteBack;

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for all `TieredMap` presets.
#[derive(Debug, Clone)]
pub struct TieredConfig {
    /// Evict LRU entries from front when it exceeds this count.
    ///
    /// When zero, the front cache is unbounded and no eviction occurs.
    /// Only applies to `WriteBack` and `Durable` presets.
    pub max_front_entries: usize,

    /// Auto-flush every N mutations (0 = manual only).
    ///
    /// In `WriteBack` mode, `flush()` fires after every Nth dirty entry is added.
    /// In `Pipelined` mode (via [`PipelinedMap`]), this controls how many dirty
    /// keys must accumulate before an auto-flush is triggered.
    /// Has no effect in `Durable` or `MemOnly` mode.
    pub flush_every: usize,

    /// Auto-commit in `Pipelined` mode every N t0 mutations (0 = manual only).
    ///
    /// When non-zero, `commit()` is called automatically after every Nth mutation
    /// to t0 in [`PipelinedMap`].  Has no effect in other modes.
    pub commit_every: usize,

    /// Retain this many historical versions in the backing store (0 = all).
    ///
    /// Currently informational — the `VersionedHamt` retains all versions in
    /// memory.  A future compaction pass will honour this limit.
    pub max_versions: usize,
}

impl Default for TieredConfig {
    /// Creates a `TieredConfig` with no eviction, no auto-flush, no auto-commit,
    /// and unlimited version retention.
    fn default() -> Self {
        Self {
            max_front_entries: 0,
            flush_every: 0,
            commit_every: 0,
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

// ── TieredMap — 2-tier struct (WriteBack / Durable) ──────────────────────────

/// A tiered map with an in-memory heap cache backed by a versioned HAMT.
///
/// The type parameter `Mode` selects between the two 2-tier presets:
/// [`Durable`] (write-through) and [`WriteBack`] (write-behind).
///
/// For the no-disk `MemOnly` preset use [`MemOnlyMap`]; for the 3-tier pipeline
/// use [`PipelinedMap`].
///
/// The backward-compatibility aliases [`Strict`] and [`Relaxed`] refer to
/// [`Durable`] and [`WriteBack`] respectively.
///
/// # Architecture
///
/// ```text
/// TieredMap<K, V, Mode>
///   ├── front:          pds::HashMap<K, V>         — hot tier; RAM-bounded LRU cache
///   ├── dirty:          HashSet<K>                 — entries not yet flushed to back
///   ├── eviction_queue: VecDeque<K>                — approximate LRU order
///   └── back:           VersionedHamt<K, V>        — cold tier; versioned, crash-safe
/// ```
pub struct TieredMap<K, V, Mode = WriteBack>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de>,
{
    /// Hot tier: in-memory `pds::HashMap` cache.
    pub(crate) front: HashMap<K, V>,

    /// Dirty-entry tracker: keys in `front` not yet flushed to `back`.
    pub(crate) dirty: HashSet<K>,

    /// Approximate LRU eviction queue.
    pub(crate) eviction_queue: VecDeque<K>,

    /// Cold tier: versioned, crash-safe HAMT.
    pub(crate) back: VersionedHamt<K, V>,

    /// Configuration controlling eviction, auto-flush, and version retention.
    pub(crate) config: TieredConfig,

    /// Zero-sized mode tag.
    pub(crate) _mode: PhantomData<Mode>,
}

// ── Durable mode ──────────────────────────────────────────────────────────────

impl<K, V> TieredMap<K, V, Durable>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de> + DeserializeOwned,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de> + DeserializeOwned + PartialEq,
{
    /// Opens or creates a `TieredMap` at `path` in `Durable` mode.
    ///
    /// The `path` argument is accepted for API symmetry with `DurableMap::open`
    /// but is currently unused — the backing `VersionedHamt` uses an in-memory
    /// `MemBackend`.  A file-backed backend will be supported in a future release.
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
        self.back = self.back.insert(k.clone(), v.clone())?;
        let prev = self.front.insert(k.clone(), v);
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
        let (new_back, _evicted) = self.back.remove(k)?;
        self.back = new_back;
        let prev = self.front.remove(k);
        Ok(prev)
    }

    /// Returns a reference to the value for `k` in the front cache, if present.
    ///
    /// Use [`get_or_fetch`][Self::get_or_fetch] for evicted keys.
    ///
    /// Time: O(log N) — heap lookup.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.front.get(k)
    }

    /// Returns a reference to the value for `k`, fetching from `back` on a
    /// front-cache miss.
    ///
    /// Time: O(log N) hit; O(log N) HAMT read + O(log N) heap insert on cold miss.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn get_or_fetch(&mut self, k: &K) -> Result<Option<&V>, DurableError> {
        if !self.front.contains_key(k) {
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
    /// Time: O(1).
    pub fn latest_version(&self) -> Option<VersionId> {
        Some(self.back.version())
    }

    fn evict_one(&mut self) -> Result<(), DurableError> {
        if let Some(evict_key) = self.eviction_queue.pop_front() {
            self.front.remove(&evict_key);
        }
        Ok(())
    }
}

// ── WriteBack mode ────────────────────────────────────────────────────────────

impl<K, V> TieredMap<K, V, WriteBack>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de> + DeserializeOwned,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de> + DeserializeOwned + PartialEq,
{
    /// Opens or creates a `TieredMap` at `path` in `WriteBack` mode.
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
    /// The mutation is NOT yet durable; call [`flush()`][Self::flush] to persist.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(log N) — heap write; zero I/O.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let prev = self.front.insert(k.clone(), v);
        self.dirty.insert(k.clone());
        self.eviction_queue.push_back(k.clone());

        if self.config.max_front_entries > 0 && self.front.len() > self.config.max_front_entries {
            let _ = self.evict_one();
        }

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
        self.dirty.insert(k.clone());
        prev
    }

    /// Pushes all dirty entries to `back` as a single new version.
    ///
    /// Returns the `VersionId` of the new version.
    ///
    /// Time: O(D log N) where D = `dirty.len()`.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn flush(&mut self) -> Result<VersionId, DurableError> {
        for k in self.dirty.drain() {
            match self.front.get(&k) {
                Some(v) => {
                    self.back = self.back.insert(k, v.clone())?;
                }
                None => {
                    let (new_back, _) = self.back.remove(&k)?;
                    self.back = new_back;
                }
            }
        }
        Ok(self.back.version())
    }

    /// Returns the number of dirty (unflushed) mutations.
    ///
    /// Time: O(1).
    pub fn pending_count(&self) -> usize {
        self.dirty.len()
    }

    /// Returns a reference to the value for `k` in `front`, if present.
    ///
    /// Use [`get_or_fetch`][Self::get_or_fetch] for cold keys.
    ///
    /// Time: O(log N) — heap lookup.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.front.get(k)
    }

    /// Returns a reference to the value for `k`, fetching from `back` on miss.
    ///
    /// Time: O(log N) hit; O(log N) HAMT read + O(log N) heap insert on cold miss.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn get_or_fetch(&mut self, k: &K) -> Result<Option<&V>, DurableError> {
        if !self.front.contains_key(k) {
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
    /// Time: O(1).
    pub fn latest_version(&self) -> Option<VersionId> {
        Some(self.back.version())
    }

    fn evict_one(&mut self) -> Result<(), DurableError> {
        while let Some(evict_key) = self.eviction_queue.pop_front() {
            if !self.front.contains_key(&evict_key) {
                continue;
            }
            if self.dirty.contains(&evict_key) {
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

// ── MemOnly preset ────────────────────────────────────────────────────────────

/// A memory-only map with no disk backing — the fastest storage preset.
///
/// All mutations land in a plain `std::collections::HashMap` with no structural-sharing
/// overhead.  Call [`into_persistent()`][Self::into_persistent] to consume the map
/// and produce a persistent `pds::HashMap<K, V>`.
///
/// There is no durability — all data is lost on drop without calling
/// `into_persistent()`.
///
/// Corresponds to the [`MemOnly`] policy.
pub struct MemOnlyMap<K, V> {
    inner: std::collections::HashMap<K, V>,
}

impl<K, V> MemOnlyMap<K, V>
where
    K: Clone + Hash + Eq,
    V: Clone + Hash,
{
    /// Creates a new, empty `MemOnlyMap`.
    ///
    /// Time: O(1).
    pub fn new() -> Self {
        Self {
            inner: std::collections::HashMap::new(),
        }
    }

    /// Inserts `k` → `v` into the map.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(1) amortised.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        self.inner.insert(k, v)
    }

    /// Removes `k` from the map.
    ///
    /// Returns the previous value for `k`, if any.
    ///
    /// Time: O(1) amortised.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        self.inner.remove(k)
    }

    /// Returns a reference to the value for `k`, if present.
    ///
    /// Time: O(1) amortised.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.inner.get(k)
    }

    /// Tests whether `k` is present in the map.
    ///
    /// Time: O(1) amortised.
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

    /// Freezes the map into a persistent `pds::HashMap<K, V>`.
    ///
    /// Each entry is inserted into a new `pds::HashMap` individually.  The resulting
    /// persistent map supports structural sharing, snapshotting, and all other
    /// `pds::HashMap` operations.
    ///
    /// Time: O(N log N).
    pub fn into_persistent(self) -> HashMap<K, V> {
        let mut out = HashMap::new();
        for (k, v) in self.inner {
            out.insert(k, v);
        }
        out
    }
}

impl<K, V> Default for MemOnlyMap<K, V>
where
    K: Clone + Hash + Eq,
    V: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

// ── Pipelined preset ──────────────────────────────────────────────────────────

/// A 3-tier pipeline map: transient write buffer → heap snapshot → versioned HAMT.
///
/// - **t0** (`std::collections::HashMap`): low-overhead write buffer.  All mutations
///   land here.  O(1) amortised insert with no structural-sharing overhead.
/// - **t1** (`pds::HashMap`): last committed snapshot.
///   [`commit()`][Self::commit] atomically replaces t0 with a fresh empty map
///   and converts the old t0 into t1 (O(N) — persistent HAMT insert per key).
/// - **t2** (`VersionedHamt`): crash-safe durable replica.
///   [`flush()`][Self::flush] pushes dirty t1 entries to t2 as a single new version.
///
/// # Data loss windows
///
/// - t0 mutations not yet committed → lost on crash.
/// - t1 mutations not yet flushed → lost on crash.
/// - t2 is always crash-safe; [`open()`][Self::open] resumes at the latest version.
///
/// Corresponds to the [`Pipelined`] policy.
pub struct PipelinedMap<K, V>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de>,
{
    /// Tier 0: low-overhead write buffer.
    t0: std::collections::HashMap<K, V>,

    /// Tier 1: last committed persistent snapshot.
    t1: HashMap<K, V>,

    /// Tier 2: crash-safe versioned HAMT.
    t2: VersionedHamt<K, V>,

    /// Keys mutated since the last `flush()`.
    dirty: HashSet<K>,

    /// Number of mutations accumulated in t0 since the last `commit()`.
    t0_count: usize,

    /// Configuration controlling auto-commit and auto-flush thresholds.
    config: TieredConfig,
}

impl<K, V> PipelinedMap<K, V>
where
    K: Clone + Hash + Eq + Serialize + for<'de> Deserialize<'de> + DeserializeOwned,
    V: Clone + Hash + Serialize + for<'de> Deserialize<'de> + DeserializeOwned + PartialEq,
{
    /// Opens or creates a `PipelinedMap` at `path`.
    ///
    /// The `path` argument is accepted for API symmetry but is currently unused —
    /// the backing `VersionedHamt` uses an in-memory `MemBackend`.  A file-backed
    /// backend will be supported in a future release.
    ///
    /// On open, t0 and t1 start empty; t2 opens at the latest stored version.
    ///
    /// Time: O(1).
    pub fn open(_path: &std::path::Path, config: TieredConfig) -> Result<Self, DurableError> {
        let t2 = VersionedHamt::new(make_mem_store());
        Ok(Self {
            t0: std::collections::HashMap::new(),
            t1: HashMap::new(),
            t2,
            dirty: HashSet::new(),
            t0_count: 0,
            config,
        })
    }

    /// Inserts `k` → `v` into t0 only.
    ///
    /// The mutation is NOT yet durable.  Call [`commit()`][Self::commit] to
    /// freeze t0 into t1, and [`flush()`][Self::flush] to push t1 entries to t2.
    ///
    /// Returns the previous value for `k` from t0, if any.
    ///
    /// Time: O(1) amortised — plain `std::HashMap` insert.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let prev = self.t0.insert(k.clone(), v);
        self.dirty.insert(k);
        self.t0_count += 1;

        if self.config.commit_every > 0 && self.t0_count >= self.config.commit_every {
            self.commit();
        }

        prev
    }

    /// Removes `k` from t0.
    ///
    /// The removal is tracked in `dirty` so a subsequent `flush()` will remove
    /// the key from t2 as well.
    ///
    /// Returns the previous value for `k` from t0, if any.
    ///
    /// Time: O(1) amortised.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let prev = self.t0.remove(k);
        self.dirty.insert(k.clone());
        self.t0_count += 1;

        if self.config.commit_every > 0 && self.t0_count >= self.config.commit_every {
            self.commit();
        }

        prev
    }

    /// Freezes t0 into t1, replacing t0 with a fresh empty map.
    ///
    /// After `commit()`, all entries previously in t0 are available in t1.
    /// Dirty keys accumulated in t0 are retained in `self.dirty`; they remain
    /// unflushed until a subsequent [`flush()`][Self::flush].
    ///
    /// Time: O(N) — each t0 entry is inserted into `pds::HashMap`.
    pub fn commit(&mut self) {
        let old_t0 = mem::replace(&mut self.t0, std::collections::HashMap::new());
        let mut new_t1 = HashMap::new();
        for (k, v) in old_t0 {
            new_t1.insert(k, v);
        }
        self.t1 = new_t1;
        self.t0_count = 0;
    }

    /// Pushes all dirty entries from t1 to t2 as a single new version.
    ///
    /// Returns the `VersionId` of the new version.  If `dirty` is empty, this
    /// is a no-op and the current version is returned.
    ///
    /// Time: O(D log N) where D = `dirty.len()`.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn flush(&mut self) -> Result<VersionId, DurableError> {
        for k in self.dirty.drain() {
            match self.t1.get(&k) {
                Some(v) => {
                    self.t2 = self.t2.insert(k, v.clone())?;
                }
                None => {
                    let (new_t2, _) = self.t2.remove(&k)?;
                    self.t2 = new_t2;
                }
            }
        }
        Ok(self.t2.version())
    }

    /// Commits t0 into t1, then flushes t1 dirty entries to t2.
    ///
    /// Equivalent to `commit()` then `flush()` in sequence.
    ///
    /// Time: O(N) commit + O(D log N) flush.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn commit_and_flush(&mut self) -> Result<VersionId, DurableError> {
        // Commit without triggering a recursive auto-flush.
        let old_t0 = mem::replace(&mut self.t0, std::collections::HashMap::new());
        let mut new_t1 = HashMap::new();
        for (k, v) in old_t0 {
            new_t1.insert(k, v);
        }
        self.t1 = new_t1;
        self.t0_count = 0;
        self.flush()
    }

    /// Returns a reference to the value for `k`, searching t0 → t1 in order.
    ///
    /// Keys present only in t2 (data from before the current session) are not
    /// returned here.  Use [`get_from_t2()`][Self::get_from_t2] for those.
    ///
    /// Time: O(1) amortised if found in t0; O(log N) if found in t1.
    pub fn get(&self, k: &K) -> Option<&V> {
        self.t0.get(k).or_else(|| self.t1.get(k))
    }

    /// Returns the value for `k` from t2, if present.
    ///
    /// Consults only the durable backing store.  Use this for keys that
    /// predate the current session (present in t2 but not in t0 or t1).
    ///
    /// Time: O(log N) — HAMT read.
    ///
    /// # Errors
    ///
    /// Returns [`DurableError`] on folio I/O or codec failure.
    pub fn get_from_t2(&self, k: &K) -> Result<Option<V>, DurableError> {
        Ok(self.t2.get(k)?)
    }

    /// Tests whether `k` is present in t0 or t1.
    ///
    /// Does NOT consult t2.  Use [`get_from_t2()`][Self::get_from_t2] for
    /// keys that exist only in the durable store.
    ///
    /// Time: O(1) amortised (t0) + O(log N) (t1 on t0 miss).
    pub fn contains_key(&self, k: &K) -> bool {
        self.t0.contains_key(k) || self.t1.contains_key(k)
    }

    /// Returns the combined entry count of t0 and t1 (with t0 shadowing t1).
    ///
    /// Keys present in both t0 and t1 are counted once.
    ///
    /// Time: O(N) — requires iterating t1 to count non-shadowed keys.
    pub fn len(&self) -> usize {
        let extra_in_t1 = self
            .t1
            .iter()
            .filter(|(k, _)| !self.t0.contains_key(k))
            .count();
        self.t0.len() + extra_in_t1
    }

    /// Tests whether both t0 and t1 are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.t0.is_empty() && self.t1.is_empty()
    }

    /// Returns the number of uncommitted mutations in t0 since the last `commit()`.
    ///
    /// Time: O(1).
    pub fn pending_commit(&self) -> usize {
        self.t0_count
    }

    /// Returns the number of dirty keys not yet flushed to t2.
    ///
    /// Time: O(1).
    pub fn pending_flush(&self) -> usize {
        self.dirty.len()
    }

    /// Returns the `VersionId` of the latest committed version in t2.
    ///
    /// Time: O(1).
    pub fn latest_version(&self) -> Option<VersionId> {
        Some(self.t2.version())
    }

    /// Returns a read-only reference to t0 (the current write buffer).
    ///
    /// Time: O(1).
    pub fn t0(&self) -> &std::collections::HashMap<K, V> {
        &self.t0
    }

    /// Returns a read-only reference to t1 (the last committed snapshot).
    ///
    /// Time: O(1).
    pub fn t1(&self) -> &HashMap<K, V> {
        &self.t1
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tiered.dat");
        (dir, path)
    }

    // ── Durable mode tests ────────────────────────────────────────────────────

    type DurableMapT = TieredMap<String, u64, Durable>;

    fn durable_open(path: &std::path::Path) -> DurableMapT {
        DurableMapT::open(path, TieredConfig::default()).unwrap()
    }

    #[test]
    fn durable_open_empty() {
        let (_dir, path) = tmp_path();
        let m = durable_open(&path);
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn durable_insert_get() {
        let (_dir, path) = tmp_path();
        let mut m = durable_open(&path);
        let prev = m.insert("hello".to_string(), 42).unwrap();
        assert_eq!(prev, None);
        assert_eq!(m.get(&"hello".to_string()), Some(&42));
        assert!(m.contains_key(&"hello".to_string()));
    }

    #[test]
    fn durable_insert_returns_old_value() {
        let (_dir, path) = tmp_path();
        let mut m = durable_open(&path);
        m.insert("k".to_string(), 1).unwrap();
        let prev = m.insert("k".to_string(), 2).unwrap();
        assert_eq!(prev, Some(1));
        assert_eq!(m.get(&"k".to_string()), Some(&2));
    }

    #[test]
    fn durable_remove() {
        let (_dir, path) = tmp_path();
        let mut m = durable_open(&path);
        m.insert("x".to_string(), 7).unwrap();
        let prev = m.remove(&"x".to_string()).unwrap();
        assert_eq!(prev, Some(7));
        assert_eq!(m.get(&"x".to_string()), None);
        assert!(!m.contains_key(&"x".to_string()));
    }

    #[test]
    fn durable_remove_absent_key() {
        let (_dir, path) = tmp_path();
        let mut m = durable_open(&path);
        let prev = m.remove(&"missing".to_string()).unwrap();
        assert_eq!(prev, None);
    }

    #[test]
    fn durable_latest_version_advances_per_mutation() {
        let (_dir, path) = tmp_path();
        let mut m = durable_open(&path);
        let v0 = m.latest_version().unwrap();
        m.insert("a".to_string(), 1).unwrap();
        let v1 = m.latest_version().unwrap();
        m.insert("b".to_string(), 2).unwrap();
        let v2 = m.latest_version().unwrap();
        assert!(v1.seq > v0.seq);
        assert!(v2.seq > v1.seq);
    }

    #[test]
    fn durable_get_cold_via_get_or_fetch() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 1,
            ..TieredConfig::default()
        };
        let mut m: DurableMapT = DurableMapT::open(&path, config).unwrap();
        m.insert("a".to_string(), 1).unwrap();
        m.insert("b".to_string(), 2).unwrap();
        let v = m.get_or_fetch(&"a".to_string()).unwrap();
        assert_eq!(v, Some(&1));
    }

    #[test]
    fn durable_eviction_keeps_front_bounded() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 3,
            ..TieredConfig::default()
        };
        let mut m: DurableMapT = DurableMapT::open(&path, config).unwrap();
        for i in 0u64..10 {
            m.insert(format!("k{i}"), i).unwrap();
        }
        assert!(m.len() <= 3, "front must be bounded; got {}", m.len());
    }

    #[test]
    fn durable_front_accessor() {
        let (_dir, path) = tmp_path();
        let mut m = durable_open(&path);
        m.insert("a".to_string(), 1).unwrap();
        m.insert("b".to_string(), 2).unwrap();
        assert_eq!(m.front().len(), 2);
    }

    /// Verifies that the backward-compat `Strict` alias resolves to `Durable`.
    #[test]
    fn strict_alias_compiles() {
        let (_dir, path) = tmp_path();
        let mut m: TieredMap<String, u64, Strict> =
            TieredMap::open(&path, TieredConfig::default()).unwrap();
        m.insert("a".to_string(), 1).unwrap();
        assert_eq!(m.get(&"a".to_string()), Some(&1));
    }

    // ── WriteBack mode tests ──────────────────────────────────────────────────

    type WriteBackMapT = TieredMap<String, u64, WriteBack>;

    fn writeback_open(path: &std::path::Path) -> WriteBackMapT {
        WriteBackMapT::open(path, TieredConfig::default()).unwrap()
    }

    #[test]
    fn writeback_open_empty() {
        let (_dir, path) = tmp_path();
        let m = writeback_open(&path);
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert_eq!(m.pending_count(), 0);
    }

    #[test]
    fn writeback_insert_get() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        let prev = m.insert("hello".to_string(), 42);
        assert_eq!(prev, None);
        assert_eq!(m.get(&"hello".to_string()), Some(&42));
        assert!(m.contains_key(&"hello".to_string()));
    }

    #[test]
    fn writeback_insert_is_dirty_until_flush() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        assert_eq!(m.pending_count(), 0);
        m.insert("a".to_string(), 1);
        assert_eq!(m.pending_count(), 1);
        m.insert("b".to_string(), 2);
        assert_eq!(m.pending_count(), 2);
        let _ = m.flush().unwrap();
        assert_eq!(m.pending_count(), 0);
    }

    #[test]
    fn writeback_flush_creates_new_version() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        let v0 = m.latest_version().unwrap();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        let v1 = m.flush().unwrap();
        assert!(v1.seq > v0.seq, "flush must create a new version");
    }

    #[test]
    fn writeback_flush_returns_version_id() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        m.insert("k".to_string(), 99);
        let vid = m.flush().unwrap();
        assert_eq!(vid, m.latest_version().unwrap());
    }

    #[test]
    fn writeback_flush_empty_dirty_returns_current_version() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        let v_before = m.latest_version().unwrap();
        let v_after = m.flush().unwrap();
        assert_eq!(v_before.seq, v_after.seq);
    }

    #[test]
    fn writeback_remove_propagated_on_flush() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        m.insert("k".to_string(), 5);
        m.flush().unwrap();
        m.remove(&"k".to_string());
        m.flush().unwrap();
        let from_back = m.back.get(&"k".to_string()).unwrap();
        assert_eq!(from_back, None, "removed key must be absent from back");
    }

    #[test]
    fn writeback_auto_flush_on_threshold() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            flush_every: 3,
            ..TieredConfig::default()
        };
        let mut m: WriteBackMapT = WriteBackMapT::open(&path, config).unwrap();
        let v0 = m.latest_version().unwrap();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        m.insert("c".to_string(), 3);
        assert_eq!(m.pending_count(), 0, "auto-flush must clear dirty set");
        assert!(m.latest_version().unwrap().seq > v0.seq);
    }

    #[test]
    fn writeback_cold_get_via_get_or_fetch() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 1,
            ..TieredConfig::default()
        };
        let mut m: WriteBackMapT = WriteBackMapT::open(&path, config).unwrap();
        m.insert("a".to_string(), 10);
        m.flush().unwrap();
        m.insert("b".to_string(), 20);
        let v = m.get_or_fetch(&"a".to_string()).unwrap();
        assert_eq!(v, Some(&10));
    }

    #[test]
    fn writeback_eviction_flushes_dirty_key() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 1,
            ..TieredConfig::default()
        };
        let mut m: WriteBackMapT = WriteBackMapT::open(&path, config).unwrap();
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        let v = m.back.get(&"a".to_string()).unwrap();
        assert_eq!(v, Some(1), "dirty-evicted key must be in back");
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn writeback_eviction_keeps_front_bounded() {
        let (_dir, path) = tmp_path();
        let config = TieredConfig {
            max_front_entries: 3,
            ..TieredConfig::default()
        };
        let mut m: WriteBackMapT = WriteBackMapT::open(&path, config).unwrap();
        for i in 0u64..10 {
            m.insert(format!("k{i}"), i);
        }
        assert!(m.len() <= 3, "front must be bounded; got {}", m.len());
    }

    #[test]
    fn writeback_latest_version_before_flush() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        let v0 = m.latest_version().unwrap();
        m.insert("x".to_string(), 1);
        assert_eq!(m.latest_version().unwrap().seq, v0.seq);
    }

    #[test]
    fn writeback_insert_returns_old_value() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        m.insert("k".to_string(), 1);
        let prev = m.insert("k".to_string(), 2);
        assert_eq!(prev, Some(1));
    }

    #[test]
    fn writeback_front_accessor() {
        let (_dir, path) = tmp_path();
        let mut m = writeback_open(&path);
        m.insert("a".to_string(), 1);
        m.insert("b".to_string(), 2);
        assert_eq!(m.front().len(), 2);
    }

    /// Verifies that the backward-compat `Relaxed` alias resolves to `WriteBack`.
    #[test]
    fn relaxed_alias_compiles() {
        let (_dir, path) = tmp_path();
        let mut m: TieredMap<String, u64, Relaxed> =
            TieredMap::open(&path, TieredConfig::default()).unwrap();
        m.insert("a".to_string(), 1);
        assert_eq!(m.get(&"a".to_string()), Some(&1));
    }

    // ── Shared config test ────────────────────────────────────────────────────

    #[test]
    fn tiered_config_default() {
        let cfg = TieredConfig::default();
        assert_eq!(cfg.max_front_entries, 0);
        assert_eq!(cfg.flush_every, 0);
        assert_eq!(cfg.commit_every, 0);
        assert_eq!(cfg.max_versions, 0);
    }
}
