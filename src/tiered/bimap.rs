//! [`TieredBiMap`] — a two-tier write-behind bijection map.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for bijection maps. Pending key-removes and value-removes suppress cold-tier
//! lookups until the next flush, at which point they are applied before hot
//! entries are merged in.
//!
//! # Bijection invariant
//!
//! The bijection (each key ↔ exactly one value) is guaranteed globally only
//! **after** a flush. Between flushes the hot tier may transiently hold a
//! key or value that also exists in the cold tier with a different partner.
//! Reads always prefer hot over cold, so the observable behaviour is
//! consistent, but callers that need a strict bijection across both tiers at
//! all times should use [`PropagationPolicy::Immediate`].

use super::bimap_backend::BiMapBackend;
use super::policy::PropagationPolicy;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredBiMap`].
struct TieredBiMapState<K, V, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Keys removed from hot (or via `remove_by_key`) not yet applied to cold.
    pending_key_removes: HashSet<K>,
    /// Values removed from hot (or via `remove_by_value`) not yet applied to cold.
    pending_value_removes: HashSet<V>,
    /// Number of writes since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
}

impl<K, V, Hot, Cold> TieredBiMapState<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone + Eq + std::hash::Hash,
    Hot: BiMapBackend<K, V>,
    Cold: BiMapBackend<K, V>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// Order: pending key removes → pending value removes → hot merge.
    fn flush(&mut self) {
        for key in self.pending_key_removes.drain() {
            self.cold.remove_by_key(&key);
        }
        for value in self.pending_value_removes.drain() {
            self.cold.remove_by_value(&value);
        }
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

// --- TieredBiMap ---

/// A two-tier write-behind bijection map.
///
/// `TieredBiMap<K, V, Hot, Cold>` routes writes to the `Hot` backend.
/// Reads check hot first, then fall back to cold (suppressing cold when the
/// key or value is covered by pending removes).
///
/// # Bijection invariant
///
/// The bijection is guaranteed globally only after a flush — see the module
/// documentation for details.
///
/// # Cheap `Clone`
///
/// Cloning a `TieredBiMap` is O(1): it clones the inner `Arc` so both handles
/// share the same state.
pub struct TieredBiMap<K, V, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredBiMapState<K, V, Hot, Cold>>>,
}

impl<K, V, Hot, Cold> Clone for TieredBiMap<K, V, Hot, Cold> {
    /// Clones the bimap by cloning the inner `Arc` — O(1).
    fn clone(&self) -> Self {
        TieredBiMap {
            state: Arc::clone(&self.state),
        }
    }
}

impl<K, V, Hot, Cold> TieredBiMap<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: BiMapBackend<K, V>,
    Cold: BiMapBackend<K, V>,
{
    /// Creates a new `TieredBiMap` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredBiMap {
            state: Arc::new(Mutex::new(TieredBiMapState {
                hot,
                cold,
                pending_key_removes: HashSet::new(),
                pending_value_removes: HashSet::new(),
                write_count: 0,
                policy,
            })),
        }
    }

    /// Creates a `TieredBiMap` with a [`Timed`][PropagationPolicy::Timed]
    /// policy and immediately starts the background propagation thread.
    ///
    /// Returns `(bimap, handle)`. Drop `handle` to stop the background thread.
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

    /// Inserts a `(key, value)` pair into the hot tier.
    ///
    /// Clears any pending removes that would shadow this pair. Returns the
    /// previous value associated with `key` (from hot or cold), if any.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let mut guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        // Find the previous value: hot wins over cold.
        let prev = if let Some(v) = guard.hot.get_by_key(&key) {
            Some(v)
        } else if guard.pending_key_removes.contains(&key) {
            None
        } else {
            guard.cold.get_by_key(&key)
        };
        // Un-shadow the pair — inserting cancels any pending removes.
        guard.pending_key_removes.remove(&key);
        guard.pending_value_removes.remove(&value);
        guard.hot.insert(key, value);
        guard.record_write();
        prev
    }

    /// Returns the value associated with `key`.
    ///
    /// Checks hot first; falls back to cold unless the key is covered by a
    /// pending remove.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn get_by_key(&self, key: &K) -> Option<V> {
        let guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        if let Some(v) = guard.hot.get_by_key(key) {
            return Some(v);
        }
        if guard.pending_key_removes.contains(key) {
            return None;
        }
        guard.cold.get_by_key(key)
    }

    /// Returns the key associated with `value`.
    ///
    /// Checks hot first; falls back to cold unless the value is covered by a
    /// pending remove.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn get_by_value(&self, value: &V) -> Option<K> {
        let guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        if let Some(k) = guard.hot.get_by_value(value) {
            return Some(k);
        }
        if guard.pending_value_removes.contains(value) {
            return None;
        }
        guard.cold.get_by_value(value)
    }

    /// Tests whether `key` is logically present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn contains_key(&self, key: &K) -> bool {
        self.get_by_key(key).is_some()
    }

    /// Tests whether `value` is logically present.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn contains_value(&self, value: &V) -> bool {
        self.get_by_value(value).is_some()
    }

    /// Removes the pair associated with `key`, returning the displaced value.
    ///
    /// Adds `key` to `pending_key_removes` so the cold tier is suppressed on
    /// subsequent lookups.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn remove_by_key(&self, key: &K) -> Option<V> {
        let mut guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        let prev = if let Some(v) = guard.hot.remove_by_key(key) {
            // Also track the value so its cold entry is suppressed.
            guard.pending_value_removes.insert(v.clone());
            Some(v)
        } else if guard.pending_key_removes.contains(key) {
            None
        } else if let Some(v) = guard.cold.get_by_key(key) {
            guard.pending_value_removes.insert(v.clone());
            Some(v)
        } else {
            None
        };
        if prev.is_some() {
            guard.pending_key_removes.insert(key.clone());
            guard.record_write();
        }
        prev
    }

    /// Removes the pair associated with `value`, returning the displaced key.
    ///
    /// Adds `value` to `pending_value_removes` so the cold tier is suppressed on
    /// subsequent lookups.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn remove_by_value(&self, value: &V) -> Option<K> {
        let mut guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        let prev = if let Some(k) = guard.hot.remove_by_value(value) {
            guard.pending_key_removes.insert(k.clone());
            Some(k)
        } else if guard.pending_value_removes.contains(value) {
            None
        } else if let Some(k) = guard.cold.get_by_value(value) {
            guard.pending_key_removes.insert(k.clone());
            Some(k)
        } else {
            None
        };
        if prev.is_some() {
            guard.pending_value_removes.insert(value.clone());
            guard.record_write();
        }
        prev
    }

    /// Returns the total number of pairs across both tiers (approximate).
    ///
    /// Over-counts when the same pair exists in both tiers before a flush.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Pending removes, then hot inserts, are applied to cold in that order.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredBiMap mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredBiMap mutex poisoned");
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
            let guard = state_clone.lock().expect("TieredBiMap mutex poisoned");
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
                    let mut guard = state_clone.lock().expect("TieredBiMap mutex poisoned");
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

// --- Type alias ---

/// A [`TieredBiMap`] using pds ordered backends for both tiers.
///
/// `TieredOrdBiMap<K, V>` is a convenience alias — use it when the pds
/// `OrdBiMap` backend is the right choice for both hot and cold tiers.
pub type TieredOrdBiMap<K, V> = TieredBiMap<
    K,
    V,
    super::bimap_backends::PdsOrdBiMapBackend<K, V>,
    super::bimap_backends::PdsOrdBiMapBackend<K, V>,
>;
