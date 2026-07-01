//! [`TieredSet`] — a two-tier write-behind set collection.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for sets rather than keyed maps. Pending removes are tracked until the next
//! flush, at which point they are applied to the cold tier before hot elements
//! are inserted.

use super::policy::PropagationPolicy;
use super::set_backend::{OrderedSetBackend, SetBackend};
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredSet`].
struct TieredSetState<A, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Elements removed from the hot tier that have not yet been removed from
    /// cold. Checked during `contains` to suppress cold-tier lookups for
    /// recently removed elements, and applied to cold during `flush`.
    pending_removes: HashSet<A>,
    /// Number of writes since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
    /// Phantom: `A` is a logical part of the collection's type but is only
    /// stored inside `Hot` and `Cold`, not in this struct directly.
    _a: PhantomData<A>,
}

impl<A, Hot, Cold> TieredSetState<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash,
    Hot: SetBackend<A>,
    Cold: SetBackend<A>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// Uses per-entry operations — O(hot × log cold). Pending removes are
    /// applied first, then each hot element is inserted individually into cold.
    fn flush(&mut self) {
        for elem in self.pending_removes.drain() {
            self.cold.remove(&elem);
        }
        for elem in self.hot.drain() {
            self.cold.insert(elem);
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

// --- TieredSet ---

/// A two-tier write-behind set.
///
/// `TieredSet<A, Hot, Cold>` routes writes to the `Hot` backend and reads from
/// `Hot` first, falling back to `Cold` when the element is not in the hot tier.
/// A `pending_removes` mask suppresses cold-tier lookups for recently removed
/// elements. Hot elements are propagated to `Cold` according to the
/// [`PropagationPolicy`].
///
/// # Cheap `Clone`
///
/// Cloning a `TieredSet` is O(1): it clones the inner `Arc` so both handles
/// share the same state. Mutations through either handle are visible to the
/// other.
///
/// # `len` approximation
///
/// [`TieredSet::len`] returns `hot.len() + cold.len()`. This **over-counts**
/// when the same element exists in both tiers simultaneously (before the hot
/// entry is flushed into cold). An exact deduplicated count would require an
/// O(n) scan.
pub struct TieredSet<A, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredSetState<A, Hot, Cold>>>,
}

impl<A, Hot, Cold> Clone for TieredSet<A, Hot, Cold> {
    /// Clones the set by cloning the inner `Arc` — O(1).
    ///
    /// The clone shares state with the original: mutations through either
    /// handle are visible to the other.
    fn clone(&self) -> Self {
        TieredSet {
            state: Arc::clone(&self.state),
        }
    }
}

impl<A, Hot, Cold> TieredSet<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: SetBackend<A>,
    Cold: SetBackend<A>,
{
    /// Creates a new `TieredSet` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredSet {
            state: Arc::new(Mutex::new(TieredSetState {
                hot,
                cold,
                pending_removes: HashSet::new(),
                write_count: 0,
                policy,
                _a: PhantomData,
            })),
        }
    }

    /// Creates a `TieredSet` with a [`Timed`][PropagationPolicy::Timed] policy
    /// and immediately starts the background propagation thread.
    ///
    /// Returns `(set, handle)`. Drop `handle` to stop the background thread.
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

    /// Inserts `value` into the hot tier.
    ///
    /// Returns `true` if the element was newly inserted (not present in either
    /// tier). Removes `value` from `pending_removes` if it was there.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn insert(&self, value: A) -> bool {
        let mut guard = self.state.lock().expect("TieredSet mutex poisoned");
        // If the value exists in hot or cold (and is not pending-removed), it
        // is already present — insertion returns false.
        let already_present = guard.hot.contains(&value)
            || (!guard.pending_removes.contains(&value) && guard.cold.contains(&value));
        guard.pending_removes.remove(&value);
        guard.hot.insert(value);
        guard.record_write();
        !already_present
    }

    /// Tests whether `value` is present in the set.
    ///
    /// Checks the hot tier first. If not in hot and not in `pending_removes`,
    /// falls back to the cold tier.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn contains(&self, value: &A) -> bool {
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        if guard.hot.contains(value) {
            return true;
        }
        if guard.pending_removes.contains(value) {
            return false;
        }
        guard.cold.contains(value)
    }

    /// Removes `value`, returning `true` if it was present.
    ///
    /// Removes from hot if present, then adds to `pending_removes` so that the
    /// cold-tier fallback is suppressed until the next flush.
    ///
    /// Time: O(1) amortised for hash backends; O(log n) for tree backends.
    pub fn remove(&self, value: &A) -> bool {
        let mut guard = self.state.lock().expect("TieredSet mutex poisoned");
        let in_hot = guard.hot.remove(value);
        let in_cold = !guard.pending_removes.contains(value) && guard.cold.contains(value);
        let was_present = in_hot || in_cold;
        if was_present {
            guard.pending_removes.insert(value.clone());
            guard.record_write();
        }
        was_present
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Pending removes are applied first, then all hot elements are inserted
    /// into cold. The write counter is reset.
    ///
    /// Time: O(hot × log cold).
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredSet mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// The snapshot is independent: subsequent mutations to `self` do not
    /// affect it.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        guard.hot.clone()
    }

    /// Returns an approximation of the total number of elements.
    ///
    /// Returns `hot.len() + cold.len()`. This **over-counts** when the same
    /// element exists in both tiers simultaneously. An exact deduplicated count
    /// would require an O(n) scan.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both the hot and cold tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Spawns a background thread that flushes on the given interval.
    ///
    /// Only meaningful with the [`Timed`][PropagationPolicy::Timed] policy,
    /// but safe to call with any policy. The thread wakes every `duration` and
    /// calls `flush`.
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
            let guard = state_clone.lock().expect("TieredSet mutex poisoned");
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
                    let mut guard = state_clone.lock().expect("TieredSet mutex poisoned");
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

// --- SetBackend impl for TieredSet ---
//
// This makes TieredSet itself usable as a tier inside another TieredSet,
// enabling three-tier (and deeper) compositions.

impl<A, Hot, Cold> SetBackend<A> for TieredSet<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Send + 'static,
    Hot: SetBackend<A>,
    Cold: SetBackend<A>,
{
    fn contains(&self, value: &A) -> bool {
        TieredSet::contains(self, value)
    }

    fn insert(&mut self, value: A) -> bool {
        TieredSet::insert(self, value)
    }

    fn remove(&mut self, value: &A) -> bool {
        TieredSet::remove(self, value)
    }

    fn len(&self) -> usize {
        TieredSet::len(self)
    }

    fn is_empty(&self) -> bool {
        TieredSet::is_empty(self)
    }

    /// Loads elements into the hot tier.
    ///
    /// Elements will propagate to cold according to the policy on the next flush.
    ///
    /// Time: O(n).
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        let mut guard = self.state.lock().expect("TieredSet mutex poisoned");
        let elems: Vec<A> = iter.collect();
        guard.hot.load_from(elems.into_iter());
    }

    /// Flushes, then drains the cold tier.
    ///
    /// After this call both tiers are empty.
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        let mut guard = self.state.lock().expect("TieredSet mutex poisoned");
        guard.flush();
        guard.cold.drain()
    }
}

// --- Type alias ---

/// A [`TieredSet`] constrained to ordered backends.
///
/// `TieredOrdSet<A, Hot, Cold>` is a type alias for `TieredSet<A, Hot, Cold>`.
/// When both `Hot` and `Cold` implement [`OrderedSetBackend`], the
/// [`TieredSetOrdExt`] trait provides ordered-iteration and range-query methods.
pub type TieredOrdSet<A, Hot, Cold> = TieredSet<A, Hot, Cold>;

// --- Extension trait: TieredSetOrdExt ---

/// Extension methods for [`TieredSet`] when both tiers implement
/// [`OrderedSetBackend`].
///
/// These methods merge hot and cold tier results in element order, with hot
/// entries winning on duplicates. Pending removes are excluded from results.
pub trait TieredSetOrdExt<A> {
    /// Returns all elements in ascending order, merging both tiers.
    ///
    /// Hot and cold tiers are merged: hot wins on duplicates. Pending removes
    /// are excluded.
    ///
    /// Time: O(n) where n is the total number of elements in both tiers.
    fn iter_ordered(&self) -> Vec<A>;

    /// Returns all elements whose values lie within `range`, in ascending
    /// element order.
    ///
    /// Hot and cold tiers are merged. Pending removes are excluded.
    ///
    /// Time: O(n) where n is the total number of elements in both tiers.
    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<A>;
}

impl<A, Hot, Cold> TieredSetOrdExt<A> for TieredSet<A, Hot, Cold>
where
    A: Clone + Eq + std::hash::Hash + Ord + Send + 'static,
    Hot: OrderedSetBackend<A> + Clone,
    Cold: OrderedSetBackend<A> + Clone,
{
    fn iter_ordered(&self) -> Vec<A> {
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        merge_ordered_set_results(
            guard.hot.iter_ordered(),
            guard.cold.iter_ordered(),
            &guard.pending_removes,
        )
    }

    fn range(&self, range: impl std::ops::RangeBounds<A>) -> Vec<A> {
        use std::ops::Bound;
        let start = match range.start_bound() {
            Bound::Included(a) => Bound::Included(a.clone()),
            Bound::Excluded(a) => Bound::Excluded(a.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match range.end_bound() {
            Bound::Included(a) => Bound::Included(a.clone()),
            Bound::Excluded(a) => Bound::Excluded(a.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let guard = self.state.lock().expect("TieredSet mutex poisoned");
        let hot_results = guard.hot.range((start.clone(), end.clone()));
        let cold_results = guard.cold.range((start, end));
        merge_ordered_set_results(hot_results, cold_results, &guard.pending_removes)
    }
}

/// Merges hot and cold ordered set results, with hot winning on duplicates and
/// pending removes excluded from the output.
///
/// Both slices must be in ascending element order. The output is in ascending
/// order.
fn merge_ordered_set_results<A>(hot: Vec<A>, cold: Vec<A>, pending_removes: &HashSet<A>) -> Vec<A>
where
    A: Clone + Ord + Eq + std::hash::Hash,
{
    let hot_set: HashSet<A> = hot.iter().cloned().collect();

    let mut result: Vec<A> = cold
        .into_iter()
        .filter(|a| !hot_set.contains(a) && !pending_removes.contains(a))
        .chain(hot.into_iter().filter(|a| !pending_removes.contains(a)))
        .collect();

    result.sort_unstable();
    result
}
