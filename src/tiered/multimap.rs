//! [`TieredMultiMap`] — a two-tier write-behind multimap collection.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for multimaps. The `pending_entry_removes` and `pending_key_removes` sets
//! track deletions until the next flush, when they are applied to the cold tier
//! before hot elements are merged in.

use super::multimap_backend::MultiMapBackend;
use super::policy::PropagationPolicy;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredMultiMap`].
struct TieredMultiMapState<K, V, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Exact (key, value) pairs removed from hot that have not yet been applied
    /// to cold.
    pending_entry_removes: HashSet<(K, V)>,
    /// Keys whose entire value set has been removed. Applied to cold on flush
    /// before entry removes and hot merges.
    pending_key_removes: HashSet<K>,
    /// Number of writes since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
    /// Phantom: K, V are logical parts of the type.
    _kv: PhantomData<(K, V)>,
}

impl<K, V, Hot, Cold> TieredMultiMapState<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone + Eq + std::hash::Hash,
    Hot: MultiMapBackend<K, V>,
    Cold: MultiMapBackend<K, V>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// Order: key removes → entry removes → hot merge.
    fn flush(&mut self) {
        // Apply pending key removes to cold.
        for key in self.pending_key_removes.drain() {
            self.cold.remove_key(&key);
        }
        // Apply pending entry removes to cold.
        for (k, v) in self.pending_entry_removes.drain() {
            self.cold.remove_entry(&k, &v);
        }
        // Merge hot into cold (union semantics per key).
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
            PropagationPolicy::Timed(_) | PropagationPolicy::Manual => {}
        }
    }
}

// --- TieredMultiMap ---

/// A two-tier write-behind multimap.
///
/// `TieredMultiMap<K, V, Hot, Cold>` routes writes to the `Hot` backend.
/// Reads merge results from both tiers: hot values take precedence. Deleted
/// entries and keys are tracked in `pending_entry_removes` and
/// `pending_key_removes` until the next flush.
///
/// # Flush semantics
///
/// On flush, values from hot are **unioned** into cold — they are added to
/// cold's existing value set for each key rather than replacing it.
///
/// # Cheap `Clone`
///
/// Cloning a `TieredMultiMap` is O(1): it clones the inner `Arc` so both
/// handles share the same state.
pub struct TieredMultiMap<K, V, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredMultiMapState<K, V, Hot, Cold>>>,
}

impl<K, V, Hot, Cold> Clone for TieredMultiMap<K, V, Hot, Cold> {
    /// Clones the multimap by cloning the inner `Arc` — O(1).
    fn clone(&self) -> Self {
        TieredMultiMap {
            state: Arc::clone(&self.state),
        }
    }
}

impl<K, V, Hot, Cold> TieredMultiMap<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: MultiMapBackend<K, V>,
    Cold: MultiMapBackend<K, V>,
{
    /// Creates a new `TieredMultiMap` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredMultiMap {
            state: Arc::new(Mutex::new(TieredMultiMapState {
                hot,
                cold,
                pending_entry_removes: HashSet::new(),
                pending_key_removes: HashSet::new(),
                write_count: 0,
                policy,
                _kv: PhantomData,
            })),
        }
    }

    /// Creates a `TieredMultiMap` with a [`Timed`][PropagationPolicy::Timed]
    /// policy and immediately starts the background propagation thread.
    ///
    /// Returns `(multimap, handle)`. Drop `handle` to stop the background thread.
    ///
    /// Time: O(1).
    pub fn with_timed_propagation(
        hot: Hot,
        cold: Cold,
        interval: std::time::Duration,
    ) -> (Self, super::PropagationHandle) {
        let tm = Self::new(hot, cold, PropagationPolicy::Timed(interval));
        let handle = tm.start_background_propagation();
        (tm, handle)
    }

    /// Inserts a (key, value) pair into the hot tier.
    ///
    /// Also removes any pending key-level or entry-level remove that would
    /// shadow this pair.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn insert(&self, key: K, value: V) {
        let mut guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        guard.pending_key_removes.remove(&key);
        guard.pending_entry_removes.remove(&(key.clone(), value.clone()));
        guard.hot.insert(key, value);
        guard.record_write();
    }

    /// Removes a single (key, value) pair.
    ///
    /// Returns `true` if the pair was logically present (in hot or cold, not
    /// shadowed by pending removes).
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn remove_entry(&self, key: &K, value: &V) -> bool {
        let mut guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        let in_hot = guard.hot.contains(key, value);
        let key_removed = guard.pending_key_removes.contains(key);
        let entry_removed = guard.pending_entry_removes.contains(&(key.clone(), value.clone()));
        let in_cold = !key_removed && !entry_removed && guard.cold.contains(key, value);
        let was_present = in_hot || in_cold;
        if was_present {
            if guard.hot.remove_entry(key, value) || !in_hot {
                // Add to pending entry removes if it needs to be removed from cold.
                if in_cold {
                    guard
                        .pending_entry_removes
                        .insert((key.clone(), value.clone()));
                }
            }
            guard.record_write();
        }
        was_present
    }

    /// Removes all values associated with `key`.
    ///
    /// Returns `true` if the key had any values (in hot or cold).
    ///
    /// Time: O(k) where k is the number of values for the key.
    pub fn remove_key(&self, key: &K) -> bool {
        let mut guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        let hot_had = !guard.hot.get_all(key).is_empty();
        let cold_had = !guard.cold.get_all(key).is_empty();
        let was_present = hot_had || cold_had;
        if was_present {
            guard.hot.remove_key(key);
            guard.pending_key_removes.insert(key.clone());
            // Remove any pending entry removes for this key (superseded by key remove).
            guard
                .pending_entry_removes
                .retain(|(k, _)| k != key);
            guard.record_write();
        }
        was_present
    }

    /// Returns all values associated with `key`, merging hot and cold tiers.
    ///
    /// Hot values are union'd with cold values. Values pending removal are
    /// excluded.
    ///
    /// Time: O(n) where n is the total number of values for the key.
    pub fn get_all(&self, key: &K) -> Vec<V> {
        let guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        if guard.pending_key_removes.contains(key) {
            return guard.hot.get_all(key);
        }
        let hot_vals = guard.hot.get_all(key);
        let cold_vals = guard.cold.get_all(key);
        // Union hot and cold, excluding pending entry removes.
        let mut result: Vec<V> = hot_vals.clone();
        for v in cold_vals {
            let pending = guard
                .pending_entry_removes
                .contains(&(key.clone(), v.clone()));
            if !pending && !hot_vals.contains(&v) {
                result.push(v);
            }
        }
        result
    }

    /// Tests whether the exact (key, value) pair is logically present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn contains(&self, key: &K, value: &V) -> bool {
        !self.get_all(key).is_empty() && self.get_all(key).contains(value)
    }

    /// Returns the total number of (key, value) pairs across both tiers.
    ///
    /// This is an approximation: it may over-count when the same pair exists
    /// in both tiers simultaneously.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Key removes, entry removes, and hot insertions are applied to cold in
    /// that order.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredMultiMap mutex poisoned");
        guard.hot.clone()
    }

    /// Spawns a background thread that flushes on the given interval.
    ///
    /// Drop the returned [`PropagationHandle`][super::PropagationHandle] to
    /// stop the thread.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is poisoned when the background thread runs.
    pub fn start_background_propagation(&self) -> super::PropagationHandle {
        let state_clone = Arc::clone(&self.state);

        let duration = {
            let guard = state_clone.lock().expect("TieredMultiMap mutex poisoned");
            match &guard.policy {
                PropagationPolicy::Timed(d) => *d,
                _ => std::time::Duration::from_secs(1),
            }
        };

        let (tx, rx) = std::sync::mpsc::channel::<()>();

        let thread = std::thread::spawn(move || loop {
            match rx.recv_timeout(duration) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let mut guard =
                        state_clone.lock().expect("TieredMultiMap mutex poisoned");
                    guard.flush();
                }
            }
        });

        super::PropagationHandle {
            stop: tx,
            thread: Some(thread),
        }
    }
}
