//! [`TieredUniqueVector`] — a two-tier write-behind unique sequence.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for unique vectors. The hot tier receives all writes; elements are present
//! in exactly one tier at any time (logically). Pending removes suppress cold
//! lookups until the next flush.

use super::policy::PropagationPolicy;
use super::unique_vec_backend::UniqueVecBackend;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredUniqueVector`].
struct TieredUniqueVecState<A, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Elements removed from hot not yet applied to cold.
    pending_removes: HashSet<A>,
    /// Write counter for `Batched` policy.
    write_count: usize,
    /// Propagation policy.
    policy: PropagationPolicy,
}

impl<A, Hot, Cold> TieredUniqueVecState<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash,
    Hot: UniqueVecBackend<A>,
    Cold: UniqueVecBackend<A>,
{
    fn flush(&mut self) {
        // Apply pending removes to cold first.
        for elem in self.pending_removes.drain() {
            self.cold.remove_by_value(&elem);
        }
        // Drain hot and append unique elements to cold (in hot's order).
        for elem in self.hot.drain() {
            // Cold may already contain the element from a prior flush — remove
            // first so the re-insertion preserves the latest position.
            self.cold.remove_by_value(&elem);
            self.cold.push_back(elem);
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

// --- TieredUniqueVector ---

/// A two-tier write-behind unique sequence.
///
/// `TieredUniqueVector<A, Hot, Cold>` routes writes to the `Hot` backend. All
/// elements are logically distinct across both tiers: pushing an element that
/// exists in either tier (or has a pending remove in the same flush cycle) is
/// a no-op and returns `false`.
///
/// Cross-tier iteration (`iter_all`) yields cold elements first (in cold's
/// order), followed by hot elements (in hot's order), with cold elements that
/// are shadowed by hot excluded. This preserves the append-log model: committed
/// history precedes recent writes.
///
/// # Cheap `Clone`
///
/// Cloning is O(1) — clones the inner `Arc`.
pub struct TieredUniqueVector<A, Hot, Cold> {
    state: Arc<Mutex<TieredUniqueVecState<A, Hot, Cold>>>,
}

impl<A, Hot, Cold> Clone for TieredUniqueVector<A, Hot, Cold> {
    fn clone(&self) -> Self {
        TieredUniqueVector {
            state: Arc::clone(&self.state),
        }
    }
}

impl<A, Hot, Cold> TieredUniqueVector<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: UniqueVecBackend<A>,
    Cold: UniqueVecBackend<A>,
{
    /// Creates a new `TieredUniqueVector` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredUniqueVector {
            state: Arc::new(Mutex::new(TieredUniqueVecState {
                hot,
                cold,
                pending_removes: HashSet::new(),
                write_count: 0,
                policy,
            })),
        }
    }

    /// Creates a `TieredUniqueVector` with a [`Timed`][PropagationPolicy::Timed] policy
    /// and starts the background propagation thread.
    ///
    /// Time: O(1).
    pub fn with_timed_propagation(
        hot: Hot,
        cold: Cold,
        interval: std::time::Duration,
    ) -> (Self, super::PropagationHandle) {
        let tv = Self::new(hot, cold, PropagationPolicy::Timed(interval));
        let handle = tv.start_background_propagation();
        (tv, handle)
    }

    /// Appends `elem` to the back of the hot tier.
    ///
    /// Returns `false` if `elem` already exists in either tier (or has a
    /// pending remove that has not yet been flushed).
    ///
    /// Time: O(log n).
    pub fn push_back(&self, elem: A) -> bool {
        let mut guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        // Element must be absent from hot, cold (unless pending-removed), and
        // not currently in pending_removes (which means it was just removed
        // from hot; re-adding before a flush is allowed).
        if guard.hot.contains(&elem) {
            return false;
        }
        if !guard.pending_removes.contains(&elem) && guard.cold.contains(&elem) {
            return false;
        }
        guard.pending_removes.remove(&elem);
        guard.hot.push_back(elem);
        guard.record_write();
        true
    }

    /// Prepends `elem` to the front of the hot tier.
    ///
    /// Returns `false` if `elem` already exists in either tier.
    ///
    /// Time: O(log n).
    pub fn push_front(&self, elem: A) -> bool {
        let mut guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        if guard.hot.contains(&elem) {
            return false;
        }
        if !guard.pending_removes.contains(&elem) && guard.cold.contains(&elem) {
            return false;
        }
        guard.pending_removes.remove(&elem);
        guard.hot.push_front(elem);
        guard.record_write();
        true
    }

    /// Removes and returns the last element.
    ///
    /// Checks hot first; falls back to cold unless the element is pending-removed.
    ///
    /// Time: O(log n).
    pub fn pop_back(&self) -> Option<A> {
        let mut guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        if let Some(elem) = guard.hot.pop_back() {
            guard.pending_removes.insert(elem.clone());
            guard.record_write();
            return Some(elem);
        }
        // Hot is empty; try cold (skipping pending-removed tail elements).
        let cold_elems = guard.cold.iter_all();
        for elem in cold_elems.into_iter().rev() {
            if !guard.pending_removes.contains(&elem) {
                guard.pending_removes.insert(elem.clone());
                guard.record_write();
                return Some(elem);
            }
        }
        None
    }

    /// Removes and returns the first element.
    ///
    /// Checks hot first (logically after cold in the full order); for the
    /// combined sequence the front is the front of cold (or hot if cold is
    /// empty / all pending-removed).
    ///
    /// Time: O(n).
    pub fn pop_front(&self) -> Option<A> {
        let mut guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        // The logical sequence is [cold (non-suppressed)] ++ [hot].
        // pop_front removes the first non-suppressed cold element, falling
        // back to hot if cold is exhausted.
        let cold_elems = guard.cold.iter_all();
        for elem in cold_elems {
            if !guard.pending_removes.contains(&elem) && !guard.hot.contains(&elem) {
                guard.pending_removes.insert(elem.clone());
                guard.record_write();
                return Some(elem);
            }
        }
        // All cold elements are suppressed; take from hot front.
        if let Some(elem) = guard.hot.pop_front() {
            guard.pending_removes.insert(elem.clone());
            guard.record_write();
            return Some(elem);
        }
        None
    }

    /// Returns a clone of the element at logical index `index`.
    ///
    /// The logical sequence is `[cold (non-suppressed)] ++ [hot]`.
    ///
    /// Time: O(n).
    pub fn get(&self, index: usize) -> Option<A> {
        let guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        let all = self.iter_all_locked(&guard);
        all.into_iter().nth(index)
    }

    /// Tests whether `elem` exists in either tier (and is not pending-removed).
    ///
    /// Time: O(log n).
    pub fn contains(&self, elem: &A) -> bool {
        let guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        if guard.hot.contains(elem) {
            return true;
        }
        if guard.pending_removes.contains(elem) {
            return false;
        }
        guard.cold.contains(elem)
    }

    /// Removes `elem` from the collection, returning `true` if it was present.
    ///
    /// Removes from hot if present; adds to `pending_removes` to suppress cold.
    ///
    /// Time: O(n).
    pub fn remove_by_value(&self, elem: &A) -> bool {
        let mut guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        let in_hot = guard.hot.remove_by_value(elem);
        if in_hot {
            guard.pending_removes.insert(elem.clone());
            guard.record_write();
            return true;
        }
        if guard.pending_removes.contains(elem) {
            return false;
        }
        if guard.cold.contains(elem) {
            guard.pending_removes.insert(elem.clone());
            guard.record_write();
            return true;
        }
        false
    }

    /// Returns all elements in logical order: cold (non-suppressed) then hot.
    ///
    /// Time: O(n).
    pub fn iter_all(&self) -> Vec<A> {
        let guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        self.iter_all_locked(&guard)
    }

    /// Returns the total number of logically present elements (approximate).
    ///
    /// Returns `hot.len() + cold.len()`. This may over-count when the same
    /// element exists in both tiers before a flush.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self
            .state
            .lock()
            .expect("TieredUniqueVector mutex poisoned");
        guard.flush();
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
            .expect("TieredUniqueVector mutex poisoned");
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
            .expect("TieredUniqueVector mutex poisoned");
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
                .expect("TieredUniqueVector mutex poisoned");
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
                        .expect("TieredUniqueVector mutex poisoned");
                    guard.flush();
                }
            }
        });
        super::PropagationHandle {
            stop: tx,
            thread: Some(thread),
        }
    }

    /// Returns all elements in logical order with the lock already held.
    fn iter_all_locked(&self, guard: &TieredUniqueVecState<A, Hot, Cold>) -> Vec<A> {
        let hot_set: HashSet<A> = guard.hot.iter_all().into_iter().collect();
        let mut result: Vec<A> = guard
            .cold
            .iter_all()
            .into_iter()
            .filter(|e| !hot_set.contains(e) && !guard.pending_removes.contains(e))
            .collect();
        for elem in guard.hot.iter_all() {
            if !guard.pending_removes.contains(&elem) {
                result.push(elem);
            }
        }
        result
    }
}

// --- Type alias ---

/// A [`TieredUniqueVector`] using `pds::UniqueVector` for both tiers.
pub type TieredPdsUniqueVector<A> = TieredUniqueVector<
    A,
    super::unique_vec_backends::PdsUniqueVecBackend<A>,
    super::unique_vec_backends::PdsUniqueVecBackend<A>,
>;
