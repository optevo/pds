//! [`TieredInsertionOrderMap`] and [`TieredInsertionOrderSet`] — two-tier
//! write-behind collections that preserve insertion order.
//!
//! # Insertion-order semantics across tiers
//!
//! Insertion order is a cross-tier property. On flush, hot entries are appended
//! to cold in hot's insertion order. The combined iteration order is therefore
//! cold-first then hot-second, matching the append-log model of
//! [`TieredSequence`][super::sequence::TieredSequence]. Entries in cold retain
//! their committed insertion order; new entries from hot are appended in the
//! order they were written to hot.
//!
//! Keys that are updated in hot (re-inserted without a remove) retain their
//! original insertion position in the tier where they first appeared.

use super::insertion_order_backend::{InsertionOrderMapBackend, InsertionOrderSetBackend};
use super::policy::PropagationPolicy;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- TieredInsertionOrderMap ---

/// Internal state for [`TieredInsertionOrderMap`].
struct TieredIOMapState<K, V, Hot, Cold> {
    /// Fast, mutable hot tier.
    hot: Hot,
    /// Slower cold tier.
    cold: Cold,
    /// Keys removed from hot not yet applied to cold.
    pending_deletes: HashSet<K>,
    /// Write counter for `Batched` policy.
    write_count: usize,
    /// Propagation policy.
    policy: PropagationPolicy,
    /// Phantom: `V` is a logical part of the type but stored inside Hot/Cold.
    _v: PhantomData<V>,
}

impl<K, V, Hot, Cold> TieredIOMapState<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
    Hot: InsertionOrderMapBackend<K, V>,
    Cold: InsertionOrderMapBackend<K, V>,
{
    /// Flushes hot into cold.
    ///
    /// Pending deletes are applied first; then hot entries are appended to cold
    /// in insertion order.
    fn flush(&mut self) {
        for key in self.pending_deletes.drain() {
            self.cold.remove(&key);
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

/// A two-tier write-behind map that preserves insertion order.
///
/// Writes go to the hot tier. `iter_insertion_order` returns cold entries
/// followed by hot entries (each in their own insertion order), deduplicating
/// by key (hot wins for keys present in both tiers).
///
/// # Cheap `Clone`
///
/// Cloning is O(1) — clones the inner `Arc`.
pub struct TieredInsertionOrderMap<K, V, Hot, Cold> {
    state: Arc<Mutex<TieredIOMapState<K, V, Hot, Cold>>>,
}

impl<K, V, Hot, Cold> Clone for TieredInsertionOrderMap<K, V, Hot, Cold> {
    fn clone(&self) -> Self {
        TieredInsertionOrderMap {
            state: Arc::clone(&self.state),
        }
    }
}

impl<K, V, Hot, Cold> TieredInsertionOrderMap<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Send + 'static,
    Hot: InsertionOrderMapBackend<K, V>,
    Cold: InsertionOrderMapBackend<K, V>,
{
    /// Creates a new `TieredInsertionOrderMap` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredInsertionOrderMap {
            state: Arc::new(Mutex::new(TieredIOMapState {
                hot,
                cold,
                pending_deletes: HashSet::new(),
                write_count: 0,
                policy,
                _v: PhantomData,
            })),
        }
    }

    /// Creates a `TieredInsertionOrderMap` with a
    /// [`Timed`][PropagationPolicy::Timed] policy and immediately starts the
    /// background propagation thread.
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

    /// Inserts `(key, value)` into the hot tier, returning the previous value.
    ///
    /// If the key is already in hot, the value is updated in place (insertion
    /// position unchanged). If it was pending-deleted, the delete is cancelled.
    ///
    /// Time: O(1) amortised.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let mut guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        let prev = if let Some(v) = guard.hot.get(&key) {
            Some(v)
        } else if guard.pending_deletes.contains(&key) {
            None
        } else {
            guard.cold.get(&key)
        };
        guard.pending_deletes.remove(&key);
        guard.hot.insert(key, value);
        guard.record_write();
        prev
    }

    /// Returns the value associated with `key`.
    ///
    /// Checks hot first; falls back to cold unless the key is in
    /// `pending_deletes`.
    ///
    /// Time: O(1) amortised.
    pub fn get(&self, key: &K) -> Option<V> {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        if let Some(v) = guard.hot.get(key) {
            return Some(v);
        }
        if guard.pending_deletes.contains(key) {
            return None;
        }
        guard.cold.get(key)
    }

    /// Removes `key`, returning the previous value.
    ///
    /// Time: O(1) amortised.
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        let prev = if let Some(v) = guard.hot.remove(key) {
            Some(v)
        } else if guard.pending_deletes.contains(key) {
            None
        } else {
            guard.cold.get(key)
        };
        if prev.is_some() {
            guard.pending_deletes.insert(key.clone());
            guard.record_write();
        }
        prev
    }

    /// Tests whether `key` is logically present.
    ///
    /// Time: O(1) amortised.
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Returns the number of entries across both tiers (approximate — may
    /// over-count when the same key is in both tiers before a flush).
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        guard.flush();
    }

    /// Returns all entries in cross-tier insertion order: cold first, then hot.
    ///
    /// Hot entries take precedence for keys that exist in both tiers — cold
    /// entries whose key is also in hot (or pending-deleted) are excluded.
    ///
    /// Time: O(n).
    pub fn iter_insertion_order(&self) -> Vec<(K, V)> {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        let hot_pairs = guard.hot.iter_insertion_order();
        let hot_keys: HashSet<K> = hot_pairs.iter().map(|(k, _)| k.clone()).collect();

        let mut result: Vec<(K, V)> = guard
            .cold
            .iter_insertion_order()
            .into_iter()
            .filter(|(k, _)| !hot_keys.contains(k) && !guard.pending_deletes.contains(k))
            .collect();
        // Append hot entries in hot's insertion order.
        for (k, v) in hot_pairs {
            if !guard.pending_deletes.contains(&k) {
                result.push((k, v));
            }
        }
        result
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderMap mutex poisoned");
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
            let guard = state_clone
                .lock()
                .expect("TieredInsertionOrderMap mutex poisoned");
            match &guard.policy {
                PropagationPolicy::Timed(d) => *d,
                _ => std::time::Duration::from_secs(1),
            }
        };
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || loop {
            match rx.recv_timeout(duration) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let mut guard = state_clone
                        .lock()
                        .expect("TieredInsertionOrderMap mutex poisoned");
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

// --- TieredInsertionOrderSet ---

/// Internal state for [`TieredInsertionOrderSet`].
struct TieredIOSetState<A, Hot, Cold> {
    hot: Hot,
    cold: Cold,
    pending_removes: HashSet<A>,
    write_count: usize,
    policy: PropagationPolicy,
}

impl<A, Hot, Cold> TieredIOSetState<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash,
    Hot: InsertionOrderSetBackend<A>,
    Cold: InsertionOrderSetBackend<A>,
{
    fn flush(&mut self) {
        for elem in self.pending_removes.drain() {
            self.cold.remove(&elem);
        }
        for elem in self.hot.drain() {
            self.cold.insert(elem);
        }
        self.write_count = 0;
    }

    fn record_write(&mut self) {
        self.write_count += 1;
        match &self.policy {
            PropagationPolicy::Immediate => self.flush(),
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

/// A two-tier write-behind set that preserves insertion order.
///
/// Writes go to the hot tier. `iter_insertion_order` returns cold elements
/// followed by hot elements (each in their own insertion order), excluding
/// elements in `pending_removes`.
///
/// # Cheap `Clone`
///
/// Cloning is O(1) — clones the inner `Arc`.
pub struct TieredInsertionOrderSet<A, Hot, Cold> {
    state: Arc<Mutex<TieredIOSetState<A, Hot, Cold>>>,
}

impl<A, Hot, Cold> Clone for TieredInsertionOrderSet<A, Hot, Cold> {
    fn clone(&self) -> Self {
        TieredInsertionOrderSet {
            state: Arc::clone(&self.state),
        }
    }
}

impl<A, Hot, Cold> TieredInsertionOrderSet<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: InsertionOrderSetBackend<A>,
    Cold: InsertionOrderSetBackend<A>,
{
    /// Creates a new `TieredInsertionOrderSet` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredInsertionOrderSet {
            state: Arc::new(Mutex::new(TieredIOSetState {
                hot,
                cold,
                pending_removes: HashSet::new(),
                write_count: 0,
                policy,
            })),
        }
    }

    /// Creates a `TieredInsertionOrderSet` with a
    /// [`Timed`][PropagationPolicy::Timed] policy and starts the background
    /// propagation thread.
    ///
    /// Time: O(1).
    pub fn with_timed_propagation(
        hot: Hot,
        cold: Cold,
        interval: std::time::Duration,
    ) -> (Self, super::PropagationHandle) {
        let ts = Self::new(hot, cold, PropagationPolicy::Timed(interval));
        let handle = ts.start_background_propagation();
        (ts, handle)
    }

    /// Inserts `elem` into the hot tier.
    ///
    /// Returns `true` if the element was newly inserted (not already present
    /// in hot or cold).
    ///
    /// Time: O(1) amortised.
    pub fn insert(&self, elem: A) -> bool {
        let mut guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        let in_hot = guard.hot.contains(&elem);
        let in_cold = !guard.pending_removes.contains(&elem) && guard.cold.contains(&elem);
        if in_hot || in_cold {
            return false;
        }
        guard.pending_removes.remove(&elem);
        guard.hot.insert(elem);
        guard.record_write();
        true
    }

    /// Removes `elem`, returning `true` if it was logically present.
    ///
    /// Time: O(1) amortised.
    pub fn remove(&self, elem: &A) -> bool {
        let mut guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        let was_in_hot = guard.hot.remove(elem);
        let in_cold = !guard.pending_removes.contains(elem) && guard.cold.contains(elem);
        if was_in_hot || in_cold {
            guard.pending_removes.insert(elem.clone());
            guard.record_write();
            true
        } else {
            false
        }
    }

    /// Tests whether `elem` is logically present.
    ///
    /// Time: O(1) amortised.
    pub fn contains(&self, elem: &A) -> bool {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        if guard.hot.contains(elem) {
            return true;
        }
        if guard.pending_removes.contains(elem) {
            return false;
        }
        guard.cold.contains(elem)
    }

    /// Returns the number of elements across both tiers (approximate).
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        guard.flush();
    }

    /// Returns all elements in cross-tier insertion order: cold first, then hot.
    ///
    /// Pending-removed elements and hot-shadowed cold elements are excluded.
    ///
    /// Time: O(n).
    pub fn iter_insertion_order(&self) -> Vec<A> {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        let hot_elems = guard.hot.iter_insertion_order();
        let hot_set: HashSet<A> = hot_elems.iter().cloned().collect();

        let mut result: Vec<A> = guard
            .cold
            .iter_insertion_order()
            .into_iter()
            .filter(|a| !hot_set.contains(a) && !guard.pending_removes.contains(a))
            .collect();
        for elem in hot_elems {
            if !guard.pending_removes.contains(&elem) {
                result.push(elem);
            }
        }
        result
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self
            .state
            .lock()
            .expect("TieredInsertionOrderSet mutex poisoned");
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
            let guard = state_clone
                .lock()
                .expect("TieredInsertionOrderSet mutex poisoned");
            match &guard.policy {
                PropagationPolicy::Timed(d) => *d,
                _ => std::time::Duration::from_secs(1),
            }
        };
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || loop {
            match rx.recv_timeout(duration) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let mut guard = state_clone
                        .lock()
                        .expect("TieredInsertionOrderSet mutex poisoned");
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
