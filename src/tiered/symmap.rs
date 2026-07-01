//! [`TieredSymMap`] — a two-tier write-behind symmetric map.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for symmetric maps. Pending removes (tracked per-direction) suppress cold-tier
//! lookups until the next flush.

use super::policy::PropagationPolicy;
use super::symmap_backend::{SymMapBackend, SymMapDirection};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredSymMap`].
struct TieredSymMapState<A, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Keys removed from hot in the forward direction, not yet applied to cold.
    pending_forward_removes: HashSet<A>,
    /// Keys removed from hot in the backward direction, not yet applied to cold.
    pending_backward_removes: HashSet<A>,
    /// Number of writes since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
}

impl<A, Hot, Cold> TieredSymMapState<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash,
    Hot: SymMapBackend<A>,
    Cold: SymMapBackend<A>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// Order: forward removes → backward removes → hot merge.
    fn flush(&mut self) {
        for key in self.pending_forward_removes.drain() {
            self.cold.remove(SymMapDirection::Forward, &key);
        }
        for key in self.pending_backward_removes.drain() {
            self.cold.remove(SymMapDirection::Backward, &key);
        }
        for (a, b) in self.hot.drain() {
            self.cold.insert(a, b);
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

// --- TieredSymMap ---

/// A two-tier write-behind symmetric map.
///
/// `TieredSymMap<A, Hot, Cold>` routes writes to the `Hot` backend. Reads
/// check hot first, then fall back to cold (suppressing cold when the looked-up
/// key is covered by a pending remove in the appropriate direction).
///
/// # Cheap `Clone`
///
/// Cloning a `TieredSymMap` is O(1): it clones the inner `Arc` so both handles
/// share the same state.
pub struct TieredSymMap<A, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredSymMapState<A, Hot, Cold>>>,
}

impl<A, Hot, Cold> Clone for TieredSymMap<A, Hot, Cold> {
    /// Clones the symmap by cloning the inner `Arc` — O(1).
    fn clone(&self) -> Self {
        TieredSymMap {
            state: Arc::clone(&self.state),
        }
    }
}

impl<A, Hot, Cold> TieredSymMap<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: SymMapBackend<A>,
    Cold: SymMapBackend<A>,
{
    /// Creates a new `TieredSymMap` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredSymMap {
            state: Arc::new(Mutex::new(TieredSymMapState {
                hot,
                cold,
                pending_forward_removes: HashSet::new(),
                pending_backward_removes: HashSet::new(),
                write_count: 0,
                policy,
            })),
        }
    }

    /// Creates a `TieredSymMap` with a [`Timed`][PropagationPolicy::Timed]
    /// policy and immediately starts the background propagation thread.
    ///
    /// Returns `(symmap, handle)`. Drop `handle` to stop the background thread.
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

    /// Inserts the pair `(a, b)` into the hot tier.
    ///
    /// Cancels any pending removes that would shadow this pair in either
    /// direction.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn insert(&self, a: A, b: A) {
        let mut guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        guard.pending_forward_removes.remove(&a);
        guard.pending_backward_removes.remove(&b);
        guard.hot.insert(a, b);
        guard.record_write();
    }

    /// Returns the partner of `key` in `dir`.
    ///
    /// Checks hot first; falls back to cold unless the key is covered by a
    /// pending remove in the corresponding direction.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn get(&self, dir: SymMapDirection, key: &A) -> Option<A> {
        let guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        if let Some(v) = guard.hot.get(dir, key) {
            return Some(v);
        }
        let pending = match dir {
            SymMapDirection::Forward => &guard.pending_forward_removes,
            SymMapDirection::Backward => &guard.pending_backward_removes,
        };
        if pending.contains(key) {
            return None;
        }
        guard.cold.get(dir, key)
    }

    /// Tests whether `key` is present in the given direction.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn contains(&self, dir: SymMapDirection, key: &A) -> bool {
        self.get(dir, key).is_some()
    }

    /// Removes the pair associated with `key` in `dir`, returning the partner.
    ///
    /// Adds the removed key to the appropriate pending-removes set so the cold
    /// tier is suppressed until the next flush.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for ordered backends.
    pub fn remove(&self, dir: SymMapDirection, key: &A) -> Option<A> {
        let mut guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        // Try removing from hot first.
        let prev = if let Some(partner) = guard.hot.remove(dir, key) {
            // Also suppress the partner in the opposite direction.
            match dir {
                SymMapDirection::Forward => {
                    guard.pending_backward_removes.insert(partner.clone());
                }
                SymMapDirection::Backward => {
                    guard.pending_forward_removes.insert(partner.clone());
                }
            }
            Some(partner)
        } else {
            // Check if already pending-removed.
            let already_removed = match dir {
                SymMapDirection::Forward => guard.pending_forward_removes.contains(key),
                SymMapDirection::Backward => guard.pending_backward_removes.contains(key),
            };
            if already_removed {
                None
            } else if let Some(partner) = guard.cold.get(dir, key) {
                // Present in cold — mark both directions as pending-removed.
                match dir {
                    SymMapDirection::Forward => {
                        guard.pending_backward_removes.insert(partner.clone());
                    }
                    SymMapDirection::Backward => {
                        guard.pending_forward_removes.insert(partner.clone());
                    }
                }
                Some(partner)
            } else {
                None
            }
        };
        if prev.is_some() {
            match dir {
                SymMapDirection::Forward => {
                    guard.pending_forward_removes.insert(key.clone());
                }
                SymMapDirection::Backward => {
                    guard.pending_backward_removes.insert(key.clone());
                }
            }
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
        let guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Pending removes, then hot inserts, are applied to cold in that order.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredSymMap mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredSymMap mutex poisoned");
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
            let guard = state_clone.lock().expect("TieredSymMap mutex poisoned");
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
                    let mut guard = state_clone.lock().expect("TieredSymMap mutex poisoned");
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

/// A [`TieredSymMap`] using pds ordered backends for both tiers.
///
/// `TieredOrdSymMap<A>` is a convenience alias — use it when the pds
/// `OrdSymMap` backend is the right choice for both hot and cold tiers.
pub type TieredOrdSymMap<A> = TieredSymMap<
    A,
    super::symmap_backends::PdsOrdSymMapBackend<A>,
    super::symmap_backends::PdsOrdSymMapBackend<A>,
>;
