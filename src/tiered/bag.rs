//! [`TieredBag`] — a two-tier write-behind multiset (bag) collection.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for bags (multisets). The `pending_removes` map tracks how many times each
//! element has been decremented in the hot tier but not yet applied to cold.
//! On flush, removes are applied first (count-wise), then hot elements are
//! merged into cold by adding counts.

use super::bag_backend::BagBackend;
use super::policy::PropagationPolicy;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredBag`].
struct TieredBagState<A, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Maps each element to the number of times it has been removed from hot
    /// but not yet applied to cold. Applied to cold during flush.
    pending_removes: HashMap<A, usize>,
    /// Number of writes since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
    /// Phantom: `A` is a logical part of the type but stored only inside the backends.
    _a: PhantomData<A>,
}

impl<A, Hot, Cold> TieredBagState<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash,
    Hot: BagBackend<A>,
    Cold: BagBackend<A>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// Pending removes are applied to cold first (count-wise), then each hot
    /// element is merged into cold by adding counts (insert N times = add N
    /// to cold's existing tally).
    fn flush(&mut self) {
        // Apply pending removes to cold: decrement cold's count for each
        // element that was removed from hot but not yet applied to cold.
        for (elem, count) in self.pending_removes.drain() {
            for _ in 0..count {
                self.cold.remove(&elem);
            }
        }
        // Merge hot into cold by adding counts.
        for (elem, count) in self.hot.drain() {
            for _ in 0..count {
                self.cold.insert(elem.clone());
            }
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

// --- TieredBag ---

/// A two-tier write-behind multiset (bag).
///
/// `TieredBag<A, Hot, Cold>` routes writes to the `Hot` backend and computes
/// element counts by summing hot and cold counts, minus any `pending_removes`.
/// Hot elements are propagated to `Cold` according to the
/// [`PropagationPolicy`].
///
/// # Count semantics
///
/// `count(x)` = `hot.count(x) + cold.count(x) - pending_removes[x]`. This
/// correctly tracks the logical count across both tiers even before a flush.
///
/// # Cheap `Clone`
///
/// Cloning a `TieredBag` is O(1): it clones the inner `Arc` so both handles
/// share the same state.
pub struct TieredBag<A, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredBagState<A, Hot, Cold>>>,
}

impl<A, Hot, Cold> Clone for TieredBag<A, Hot, Cold> {
    /// Clones the bag by cloning the inner `Arc` — O(1).
    fn clone(&self) -> Self {
        TieredBag {
            state: Arc::clone(&self.state),
        }
    }
}

impl<A, Hot, Cold> TieredBag<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: BagBackend<A>,
    Cold: BagBackend<A>,
{
    /// Creates a new `TieredBag` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredBag {
            state: Arc::new(Mutex::new(TieredBagState {
                hot,
                cold,
                pending_removes: HashMap::new(),
                write_count: 0,
                policy,
                _a: PhantomData,
            })),
        }
    }

    /// Creates a `TieredBag` with a [`Timed`][PropagationPolicy::Timed] policy
    /// and immediately starts the background propagation thread.
    ///
    /// Returns `(bag, handle)`. Drop `handle` to stop the background thread.
    ///
    /// Time: O(1).
    pub fn with_timed_propagation(
        hot: Hot,
        cold: Cold,
        interval: std::time::Duration,
    ) -> (Self, super::PropagationHandle) {
        let tb = Self::new(hot, cold, PropagationPolicy::Timed(interval));
        let handle = tb.start_background_propagation();
        (tb, handle)
    }

    /// Inserts one occurrence of `value` into the hot tier.
    ///
    /// If `value` is in `pending_removes`, the pending-remove count is
    /// decremented first (re-inserting a removed element un-removes one
    /// occurrence).
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn insert(&self, value: A) {
        let mut guard = self.state.lock().expect("TieredBag mutex poisoned");
        // If there is a pending remove for this element, cancel one remove
        // rather than adding to hot (the logical effect is the same).
        if let Some(pr) = guard.pending_removes.get_mut(&value) {
            if *pr > 0 {
                *pr -= 1;
                if *pr == 0 {
                    guard.pending_removes.remove(&value);
                }
                guard.record_write();
                return;
            }
        }
        guard.hot.insert(value);
        guard.record_write();
    }

    /// Removes one occurrence of `value`.
    ///
    /// Returns `false` if `value` has a logical count of zero (absent).
    ///
    /// Removes from hot first (if present there). If not in hot but present in
    /// cold (adjusted for pending removes), increments `pending_removes`.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn remove(&self, value: &A) -> bool {
        let mut guard = self.state.lock().expect("TieredBag mutex poisoned");
        // Logical count: hot + cold - pending_removes.
        let pending = *guard.pending_removes.get(value).unwrap_or(&0);
        let logical = guard
            .hot
            .count(value)
            .saturating_add(guard.cold.count(value))
            .saturating_sub(pending);
        if logical == 0 {
            return false;
        }
        // Remove from hot first if possible.
        if guard.hot.count(value) > 0 {
            guard.hot.remove(value);
        } else {
            // Element is in cold — add to pending_removes.
            *guard.pending_removes.entry(value.clone()).or_insert(0) += 1;
        }
        guard.record_write();
        true
    }

    /// Returns the logical count of `value` across both tiers.
    ///
    /// `count(x)` = `hot.count(x) + cold.count(x) - pending_removes[x]`.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn count(&self, value: &A) -> usize {
        let guard = self.state.lock().expect("TieredBag mutex poisoned");
        let pending = *guard.pending_removes.get(value).unwrap_or(&0);
        guard
            .hot
            .count(value)
            .saturating_add(guard.cold.count(value))
            .saturating_sub(pending)
    }

    /// Tests whether `value` has a non-zero logical count.
    ///
    /// Equivalent to `self.count(value) > 0`.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn contains(&self, value: &A) -> bool {
        self.count(value) > 0
    }

    /// Returns the total logical count of all elements (with multiplicity).
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredBag mutex poisoned");
        let pending_total: usize = guard.pending_removes.values().sum();
        guard
            .hot
            .len()
            .saturating_add(guard.cold.len())
            .saturating_sub(pending_total)
    }

    /// Tests whether the logical total count is zero.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Pending removes are applied to cold, then hot's elements are merged
    /// into cold by incrementing counts. The write counter is reset.
    ///
    /// Time: O(hot_distinct × log cold).
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredBag mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredBag mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredBag mutex poisoned");
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
            let guard = state_clone.lock().expect("TieredBag mutex poisoned");
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
                    let mut guard = state_clone.lock().expect("TieredBag mutex poisoned");
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

/// A [`TieredBag`] constrained to ordered backends.
///
/// `TieredOrdBag<A, Hot, Cold>` is a type alias for `TieredBag<A, Hot, Cold>`.
/// When both `Hot` and `Cold` implement
/// [`OrderedBagBackend`][super::bag_backend::OrderedBagBackend], the ordered
/// iteration and range queries are available via those trait methods.
pub type TieredOrdBag<A, Hot, Cold> = TieredBag<A, Hot, Cold>;
