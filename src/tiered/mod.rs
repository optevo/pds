//! Tiered write-behind collections.
//!
//! A composable, configurable pipeline where hot writes land on a fast mutable
//! tier and propagate to progressively richer (but slower) persistent tiers.
//! Callers accept a bounded data-loss window in exchange for near-transient
//! write throughput plus "for free" access to structural sharing, disk
//! durability, and Merkle identity at whatever propagation lag they can tolerate.
//!
//! # Architecture
//!
//! Each tier implements [`CollectionBackend<K, V>`][backend::CollectionBackend].
//! A [`TieredCollection<K, V, Hot, Cold>`] is itself a `CollectionBackend`, so
//! stages compose recursively:
//!
//! ```text
//! // Two tiers:
//! TieredCollection<K, V, StdHashMapBackend<K, V>, PdsHashMapBackend<K, V>>
//!
//! // Three tiers (Std → Pds → MerkleWrapper<Pds>):
//! TieredCollection<K, V,
//!     StdHashMapBackend<K, V>,
//!     TieredCollection<K, V,
//!         PdsHashMapBackend<K, V>,
//!         MerkleWrapperBackend<K, V>>>
//! ```
//!
//! # Propagation policies
//!
//! The [`PropagationPolicy`] is set per tier boundary:
//!
//! | Policy | Trigger |
//! |--------|---------|
//! | [`Immediate`][PropagationPolicy::Immediate] | Every write |
//! | [`Batched(n)`][PropagationPolicy::Batched] | Every nth write |
//! | [`Timed(d)`][PropagationPolicy::Timed] | Background thread, every `d` |
//! | [`Manual`][PropagationPolicy::Manual] | Explicit [`flush`][TieredCollection::flush] |
//!
//! # Thread safety
//!
//! [`TieredCollection`] is cheaply `Clone` — cloning increments an `Arc` counter.
//! All operations acquire a `Mutex` lock, so clones of the same collection observe
//! the same state.
//!
//! # `len` approximation
//!
//! [`TieredCollection::len`] returns `hot.len() + cold.len()`. This **over-counts**
//! when the same key exists in both tiers (before a flush moves it to cold).
//! The approximation is documented and intentional — an exact deduplicated count
//! would require an O(n) scan.

pub mod backend;
pub mod backends;
pub mod bag;
pub mod bag_backend;
pub mod bag_backends;
pub mod multimap;
pub mod multimap_backend;
pub mod multimap_backends;
pub mod policy;
pub mod sequence;
pub mod sequence_backend;
pub mod sequence_backends;
pub mod set;
pub mod set_backend;
pub mod set_backends;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_bag;
#[cfg(test)]
mod tests_compose;
#[cfg(test)]
mod tests_multimap;
#[cfg(test)]
mod tests_ord;
#[cfg(test)]
mod tests_seq;
#[cfg(test)]
mod tests_set;

pub use backend::{CollectionBackend, OrderedCollectionBackend};
pub use bag::{TieredBag, TieredOrdBag};
pub use bag_backend::{BagBackend, OrderedBagBackend};
pub use multimap::TieredMultiMap;
pub use multimap_backend::MultiMapBackend;
pub use policy::PropagationPolicy;
pub use sequence::TieredSequence;
pub use sequence::TieredVector;
pub use set::{TieredOrdSet, TieredSet, TieredSetOrdExt};
pub use set_backend::{OrderedSetBackend, SetBackend};

use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredCollection`].
///
/// Wrapped in `Arc<Mutex<…>>` so that clones of the owning `TieredCollection`
/// share state.
struct TieredState<K, V, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Keys deleted from the hot tier that have not yet been removed from cold.
    ///
    /// Checked during `get` to suppress cold-tier lookups for recently deleted
    /// keys, and applied to cold during `flush`.
    pending_deletes: HashSet<K>,
    /// Number of writes since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
    /// Phantom: `V` is a logical part of the collection's type but is only
    /// stored inside `Hot` and `Cold`, not in this struct directly.
    _v: PhantomData<V>,
}

impl<K, V, Hot, Cold> TieredState<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
    Hot: CollectionBackend<K, V>,
    Cold: CollectionBackend<K, V>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// Uses per-entry inserts — O(hot × log cold) — instead of the previous
    /// O(hot + cold) drain-and-reload approach. Pending deletes are applied
    /// first, then each hot entry is inserted individually into cold. This
    /// avoids allocating an intermediate Vec of all cold entries and performing
    /// a full cold drain, at the cost of O(log cold) per hot entry.
    fn flush(&mut self) {
        // Apply pending deletes to cold before inserting hot entries, so that a
        // delete which occurred after the last flush is not resurrected.
        for key in self.pending_deletes.drain() {
            self.cold.remove(&key);
        }
        // Insert each hot entry directly into cold. Hot wins on key conflicts
        // because these inserts happen after the pending-delete pass.
        for (k, v) in self.hot.drain() {
            self.cold.insert(k, v);
        }
        self.write_count = 0;
    }

    /// Records a write and auto-flushes if the policy demands it.
    fn record_write(&mut self) {
        self.write_count += 1;
        match &self.policy {
            PropagationPolicy::Immediate => {
                self.flush();
            }
            PropagationPolicy::Batched(n) => {
                let threshold = if *n == 0 { 1 } else { *n };
                if self.write_count >= threshold {
                    self.flush();
                }
            }
            PropagationPolicy::Timed(_) | PropagationPolicy::Manual => {
                // No automatic flush on write.
            }
        }
    }
}

// --- PropagationHandle ---

/// A handle to a background propagation thread spawned by
/// [`TieredCollection::start_background_propagation`].
///
/// Dropping the handle sends a stop signal to the background thread and waits
/// for it to finish.
pub struct PropagationHandle {
    /// Sender used to signal the background thread to stop.
    stop: std::sync::mpsc::Sender<()>,
    /// The background thread's join handle. Taken on drop.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PropagationHandle {
    /// Signals the background thread to stop and joins it.
    ///
    /// The send may fail if the thread has already exited; that is not an error.
    fn drop(&mut self) {
        // Ignore error — thread may have already exited.
        let _ = self.stop.send(());
        if let Some(handle) = self.thread.take() {
            // Ignore join errors (the thread may have panicked).
            let _ = handle.join();
        }
    }
}

// --- TieredCollection ---

/// A two-tier write-behind collection.
///
/// `TieredCollection<K, V, Hot, Cold>` routes writes to the `Hot` backend and
/// reads from `Hot` first, falling back to `Cold` when the key is not in the
/// hot tier. Hot entries are propagated to `Cold` according to the
/// [`PropagationPolicy`].
///
/// # Cheap `Clone`
///
/// Cloning a `TieredCollection` is O(1): it clones the inner `Arc` so both the
/// original and the clone share the same state. This makes it safe and cheap to
/// pass clones to other threads.
///
/// # `len` approximation
///
/// `TieredCollection::len` returns `hot.len() + cold.len()`. This over-counts
/// when the same key exists in both tiers simultaneously (before the hot entry
/// is flushed into cold). An exact count would require an O(n) set difference.
pub struct TieredCollection<K, V, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredState<K, V, Hot, Cold>>>,
}

impl<K, V, Hot, Cold> Clone for TieredCollection<K, V, Hot, Cold> {
    /// Clones the collection by cloning the inner `Arc` — O(1).
    ///
    /// The clone shares state with the original: mutations through either
    /// handle are visible to the other.
    fn clone(&self) -> Self {
        TieredCollection {
            state: Arc::clone(&self.state),
        }
    }
}

impl<K, V, Hot, Cold> TieredCollection<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Send + 'static,
    Hot: CollectionBackend<K, V>,
    Cold: CollectionBackend<K, V>,
{
    /// Creates a new `TieredCollection` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredCollection {
            state: Arc::new(Mutex::new(TieredState {
                hot,
                cold,
                pending_deletes: HashSet::new(),
                write_count: 0,
                policy,
                _v: PhantomData,
            })),
        }
    }

    /// Inserts `key` → `value` into the hot tier, returning the previous value.
    ///
    /// The previous value is the value that was in the hot tier (or, if the key
    /// was not in the hot tier, the value in the cold tier). Keys masked by
    /// `pending_deletes` are treated as absent.
    ///
    /// After the insert, the write counter is incremented and, depending on the
    /// [`PropagationPolicy`], the hot tier may be flushed to cold immediately.
    ///
    /// Time: O(1) amortised for `StdHashMapBackend`; O(log N) for pds-backed tiers.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let mut guard = self.state.lock().expect("TieredCollection mutex poisoned");
        // Find the previous value: hot takes precedence over cold.
        let prev = if let Some(v) = guard.hot.get(&key) {
            Some(v)
        } else if guard.pending_deletes.contains(&key) {
            // Key was deleted and not yet flushed — treat as absent.
            None
        } else {
            guard.cold.get(&key)
        };
        // Remove from pending deletes (re-inserting a deleted key un-deletes it).
        guard.pending_deletes.remove(&key);
        guard.hot.insert(key, value);
        guard.record_write();
        prev
    }

    /// Returns a clone of the value associated with `key`.
    ///
    /// Checks the hot tier first. If the key is not in hot and is not in
    /// `pending_deletes`, falls back to the cold tier.
    ///
    /// Time: O(1) amortised for `StdHashMapBackend`; O(log N) for pds-backed tiers.
    pub fn get(&self, key: &K) -> Option<V> {
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        if let Some(v) = guard.hot.get(key) {
            return Some(v);
        }
        // Key deleted from hot and not yet flushed — suppress cold lookup.
        if guard.pending_deletes.contains(key) {
            return None;
        }
        guard.cold.get(key)
    }

    /// Removes `key`, returning the previous value.
    ///
    /// Removes `key` from the hot tier (if present) and adds it to
    /// `pending_deletes` so that the cold-tier fallback is suppressed until the
    /// next flush. The previous value is the value from hot, or — if the key
    /// was not in hot — the value from cold.
    ///
    /// Time: O(1) amortised for `StdHashMapBackend`; O(log N) for pds-backed tiers.
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut guard = self.state.lock().expect("TieredCollection mutex poisoned");
        // Determine the previous value before removing.
        let prev = if let Some(v) = guard.hot.remove(key) {
            Some(v)
        } else if guard.pending_deletes.contains(key) {
            None
        } else {
            guard.cold.get(key)
        };
        // Mark for deletion from cold on next flush.
        if prev.is_some() {
            guard.pending_deletes.insert(key.clone());
        }
        if prev.is_some() {
            guard.record_write();
        }
        prev
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// All hot entries are moved to cold (merging with existing cold state),
    /// pending deletes are applied, and the write counter is reset.
    ///
    /// This is always safe to call regardless of the [`PropagationPolicy`].
    ///
    /// Time: O(n) where n is the number of entries in hot + cold.
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredCollection mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// The snapshot is independent: subsequent mutations to `self` do not affect
    /// it. For `PdsHashMapBackend` the clone is O(1) via structural sharing; for
    /// `StdHashMapBackend` it is O(n).
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        guard.hot.clone()
    }

    /// Returns an approximation of the total number of entries.
    ///
    /// Returns `hot.len() + cold.len()`. This **over-counts** when the same key
    /// exists in both tiers simultaneously (i.e. the hot entry has not yet been
    /// flushed into cold). An exact deduplicated count would require an O(n)
    /// scan.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both the hot and cold tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Creates a `TieredCollection` with a [`Timed`][PropagationPolicy::Timed] policy
    /// and immediately starts the background propagation thread.
    ///
    /// Returns `(collection, handle)`. Drop `handle` to stop the background thread.
    ///
    /// This is a convenience constructor that combines [`new`][Self::new] with
    /// [`start_background_propagation`][Self::start_background_propagation] in a single
    /// call, ensuring the policy and the thread interval always match.
    ///
    /// # Example
    ///
    /// ```
    /// # use pds::tiered::{TieredCollection, PropagationPolicy};
    /// # use pds::tiered::backends::{StdHashMapBackend, PdsHashMapBackend};
    /// let (tc, _handle) = TieredCollection::<i32, i32, StdHashMapBackend<i32, i32>, PdsHashMapBackend<i32, i32>>::with_timed_propagation(
    ///     StdHashMapBackend::new(),
    ///     PdsHashMapBackend::new(),
    ///     std::time::Duration::from_millis(50),
    /// );
    /// tc.insert(1, 42);
    /// // Background thread will flush to cold within ~50 ms.
    /// ```
    ///
    /// Time: O(1).
    pub fn with_timed_propagation(
        hot: Hot,
        cold: Cold,
        interval: std::time::Duration,
    ) -> (Self, PropagationHandle) {
        let tc = Self::new(hot, cold, PropagationPolicy::Timed(interval));
        let handle = tc.start_background_propagation();
        (tc, handle)
    }

    /// Spawns a background thread that flushes on the given interval.
    ///
    /// Only meaningful with the
    /// [`Timed`][PropagationPolicy::Timed] policy, but safe to call with any policy.
    /// The thread wakes every `duration` and calls `flush`.
    ///
    /// Drop the returned [`PropagationHandle`] to stop the thread.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is poisoned when the background thread runs.
    pub fn start_background_propagation(&self) -> PropagationHandle {
        let state_clone = Arc::clone(&self.state);

        // Determine the interval: use the policy's duration if Timed, else 1 second
        // as a reasonable default (caller is responsible for using the right policy).
        let duration = {
            let guard = state_clone.lock().expect("TieredCollection mutex poisoned");
            match &guard.policy {
                PropagationPolicy::Timed(d) => *d,
                _ => std::time::Duration::from_secs(1),
            }
        };

        let (tx, rx) = std::sync::mpsc::channel::<()>();

        let thread = std::thread::spawn(move || {
            loop {
                // Wait for either a stop signal or the interval to elapse.
                match rx.recv_timeout(duration) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // Stop signal received or sender dropped.
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Interval elapsed — flush.
                        let mut guard =
                            state_clone.lock().expect("TieredCollection mutex poisoned");
                        guard.flush();
                    }
                }
            }
        });

        PropagationHandle {
            stop: tx,
            thread: Some(thread),
        }
    }
}

// --- Type alias: TieredOrdMap ---

/// A [`TieredCollection`] constrained to ordered backends.
///
/// `TieredOrdMap<K, V, Hot, Cold>` is a type alias for
/// `TieredCollection<K, V, Hot, Cold>` with the constraint that both `Hot` and
/// `Cold` implement [`OrderedCollectionBackend`].
/// This enables the extension methods on [`TieredCollectionOrdExt`].
///
/// # Example
///
/// ```
/// # use pds::tiered::{TieredOrdMap, PropagationPolicy};
/// # use pds::tiered::backends::{StdBTreeMapBackend, PdsOrdMapBackend};
/// let tc: TieredOrdMap<i32, &str, StdBTreeMapBackend<i32, &str>, PdsOrdMapBackend<i32, &str>> =
///     TieredOrdMap::new(StdBTreeMapBackend::new(), PdsOrdMapBackend::new(), PropagationPolicy::Manual);
/// ```
pub type TieredOrdMap<K, V, Hot, Cold> = TieredCollection<K, V, Hot, Cold>;

// --- Extension trait: TieredCollectionOrdExt ---

/// Extension methods for [`TieredCollection`] when both tiers implement
/// [`OrderedCollectionBackend`].
///
/// These methods merge hot and cold tier results in key order, with hot entries
/// winning on duplicate keys. Pending deletes are excluded from the results.
pub trait TieredCollectionOrdExt<K, V> {
    /// Returns all key-value pairs whose keys lie within `range`, in ascending
    /// key order.
    ///
    /// Hot and cold tiers are merged: hot wins on key conflicts. Pending deletes
    /// are excluded.
    ///
    /// Time: O(n) where n is the total number of entries in both tiers.
    fn range(&self, range: impl std::ops::RangeBounds<K>) -> Vec<(K, V)>;

    /// Returns all key-value pairs in ascending key order.
    ///
    /// Hot and cold tiers are merged: hot wins on key conflicts. Pending deletes
    /// are excluded.
    ///
    /// Time: O(n) where n is the total number of entries in both tiers.
    fn iter_ordered(&self) -> Vec<(K, V)>;
}
// Note: `TieredCollectionOrdExt` is intentionally separate from
// [`OrderedCollectionBackend`] — it is an extension on `TieredCollection`
// itself, not a trait required of its tier backends individually.

impl<K, V, Hot, Cold> TieredCollectionOrdExt<K, V> for TieredCollection<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Ord + Send + 'static,
    V: Clone + Send + 'static,
    Hot: backend::OrderedCollectionBackend<K, V> + Clone,
    Cold: backend::OrderedCollectionBackend<K, V> + Clone,
{
    fn range(&self, range: impl std::ops::RangeBounds<K>) -> Vec<(K, V)> {
        use std::ops::Bound;
        // Materialise bounds so we can query both tiers without consuming `range`.
        let start = match range.start_bound() {
            Bound::Included(k) => Bound::Included(k.clone()),
            Bound::Excluded(k) => Bound::Excluded(k.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match range.end_bound() {
            Bound::Included(k) => Bound::Included(k.clone()),
            Bound::Excluded(k) => Bound::Excluded(k.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        let hot_results = guard.hot.range((start.clone(), end.clone()));
        let cold_results = guard.cold.range((start, end));
        merge_ordered_results(hot_results, cold_results, &guard.pending_deletes)
    }

    fn iter_ordered(&self) -> Vec<(K, V)> {
        let guard = self.state.lock().expect("TieredCollection mutex poisoned");
        merge_ordered_results(
            guard.hot.iter_ordered(),
            guard.cold.iter_ordered(),
            &guard.pending_deletes,
        )
    }
}

/// Merges hot and cold ordered results, with hot winning on duplicates and
/// pending deletes excluded from the output.
///
/// Both `hot` and `cold` slices must be in ascending key order (invariant of
/// `OrderedCollectionBackend`). The output is in ascending key order.
fn merge_ordered_results<K, V>(
    hot: Vec<(K, V)>,
    cold: Vec<(K, V)>,
    pending_deletes: &HashSet<K>,
) -> Vec<(K, V)>
where
    K: Clone + Ord + Eq + std::hash::Hash,
    V: Clone,
{
    // Collect hot keys for O(1) duplicate detection.
    // Clone the keys into the set so we can still move `hot` later.
    let hot_keys: HashSet<K> = hot.iter().map(|(k, _)| k.clone()).collect();

    // Merge: cold entries not in hot, followed by all hot entries, then sort.
    let mut result: Vec<(K, V)> = cold
        .into_iter()
        .filter(|(k, _)| !hot_keys.contains(k) && !pending_deletes.contains(k))
        .chain(
            hot.into_iter()
                .filter(|(k, _)| !pending_deletes.contains(k)),
        )
        .collect();

    result.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
    result
}

// --- CollectionBackend impl for TieredCollection ---
//
// This makes TieredCollection itself usable as a tier inside another
// TieredCollection, enabling three-tier (and deeper) compositions.

impl<K, V, Hot, Cold> CollectionBackend<K, V> for TieredCollection<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Send + 'static,
    Hot: CollectionBackend<K, V>,
    Cold: CollectionBackend<K, V>,
{
    fn get(&self, key: &K) -> Option<V> {
        TieredCollection::get(self, key)
    }

    fn insert(&mut self, key: K, value: V) -> Option<V> {
        TieredCollection::insert(self, key, value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        TieredCollection::remove(self, key)
    }

    fn len(&self) -> usize {
        TieredCollection::len(self)
    }

    fn is_empty(&self) -> bool {
        TieredCollection::is_empty(self)
    }

    /// Loads entries into the hot tier.
    ///
    /// Entries will propagate to cold according to the policy on the next flush.
    ///
    /// Time: O(n log n) — each entry is a hot insert.
    fn load_from(&mut self, iter: impl Iterator<Item = (K, V)>) {
        let mut guard = self.state.lock().expect("TieredCollection mutex poisoned");
        let entries: Vec<(K, V)> = iter.collect();
        guard.hot.load_from(entries.into_iter());
        // Do not trigger a flush here — the caller controls timing.
    }

    /// Flushes, then drains the cold tier.
    ///
    /// After this call both tiers are empty.
    ///
    /// Time: O(n) where n is the total number of entries.
    fn drain(&mut self) -> Vec<(K, V)> {
        let mut guard = self.state.lock().expect("TieredCollection mutex poisoned");
        guard.flush();
        guard.cold.drain()
    }
}
