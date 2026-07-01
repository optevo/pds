//! [`TieredSequence`] — a two-tier write-behind sequence collection.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for indexed sequences rather than keyed maps. The cold tier acts as a
//! committed append log: flushing appends hot's elements to cold (in order) and
//! then clears hot. Index `i` addresses cold for `0..cold.len()` and hot for
//! indices beyond that.
//!
//! # Append-only constraint
//!
//! `TieredSequence` does not expose `push_front` or `pop_front`. The cold tier
//! is an append-only committed log — prepending to hot would produce indices that
//! are inconsistent with cold's committed prefix, so these operations are
//! intentionally omitted from the public API and from `SequenceBackend`.

use super::policy::PropagationPolicy;
use super::sequence_backend::SequenceBackend;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredSequence`].
struct TieredSequenceState<A, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all new elements.
    hot: Hot,
    /// Committed log — receives elements appended from hot on flush.
    cold: Cold,
    /// Number of push operations since the last flush. Used by `Batched` policy.
    write_count: usize,
    /// The propagation policy for this tier boundary.
    policy: PropagationPolicy,
    /// Phantom: `A` is a logical part of the collection's type but is only
    /// stored inside `Hot` and `Cold`, not in this struct directly.
    _a: PhantomData<A>,
}

impl<A, Hot, Cold> TieredSequenceState<A, Hot, Cold>
where
    A: Clone,
    Hot: SequenceBackend<A>,
    Cold: SequenceBackend<A>,
{
    /// Flushes the hot tier into the cold tier.
    ///
    /// All hot elements are appended to cold in order, then hot is cleared.
    /// The write counter is reset.
    ///
    /// Unlike map flush, this is append-only: cold retains all previously
    /// committed elements.
    fn flush(&mut self) {
        let hot_elems = self.hot.drain();
        self.cold.load_from(hot_elems.into_iter());
        self.write_count = 0;
    }

    /// Records a write (push) and auto-flushes if the policy demands it.
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

// --- TieredSequence ---

/// A two-tier write-behind sequence.
///
/// `TieredSequence<A, Hot, Cold>` writes new elements to `Hot` and reads by
/// index across both tiers: indices `0..cold.len()` resolve to cold (the
/// committed log), while indices `cold.len()..total_len` resolve to hot
/// (uncommitted).
///
/// Flushing appends all hot elements to cold in order, then clears hot. This
/// gives cold append-only (log) semantics, in contrast to `TieredCollection`
/// where cold is a full key-value mirror.
///
/// # Cheap `Clone`
///
/// Cloning a `TieredSequence` is O(1): it increments the inner `Arc` so both
/// handles share state. Mutations via either handle are visible to the other.
///
/// # `len`
///
/// [`TieredSequence::len`] returns `cold.len() + hot.len()` — an exact count
/// because there are no duplicate-key issues in sequences.
pub struct TieredSequence<A, Hot, Cold> {
    /// Shared, mutex-protected state.
    state: Arc<Mutex<TieredSequenceState<A, Hot, Cold>>>,
}

impl<A, Hot, Cold> Clone for TieredSequence<A, Hot, Cold> {
    /// Clones the sequence by cloning the inner `Arc` — O(1).
    ///
    /// The clone shares state with the original: mutations through either
    /// handle are visible to the other.
    fn clone(&self) -> Self {
        TieredSequence {
            state: Arc::clone(&self.state),
        }
    }
}

impl<A, Hot, Cold> TieredSequence<A, Hot, Cold>
where
    A: Clone + Send + 'static,
    Hot: SequenceBackend<A>,
    Cold: SequenceBackend<A>,
{
    /// Creates a new `TieredSequence` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredSequence {
            state: Arc::new(Mutex::new(TieredSequenceState {
                hot,
                cold,
                write_count: 0,
                policy,
                _a: PhantomData,
            })),
        }
    }

    /// Appends `value` to the back of the hot tier.
    ///
    /// After the push, the write counter is incremented and, depending on the
    /// [`PropagationPolicy`], the hot tier may be flushed to cold immediately.
    ///
    /// Time: O(1) amortised for both concrete backends.
    pub fn push_back(&self, value: A) {
        let mut guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.hot.push_back(value);
        guard.record_write();
    }

    /// Removes and returns the last element.
    ///
    /// Pops from hot first. If hot is empty, pops from cold.
    ///
    /// Time: O(1) amortised.
    pub fn pop_back(&self) -> Option<A> {
        let mut guard = self.state.lock().expect("TieredSequence mutex poisoned");
        if let Some(v) = guard.hot.pop_back() {
            return Some(v);
        }
        guard.cold.pop_back()
    }

    /// Returns the element at `index`, or `None` if out of bounds.
    ///
    /// Indices `0..cold.len()` resolve to cold; indices `cold.len()..total_len`
    /// resolve to hot at offset `index - cold.len()`.
    ///
    /// Time: O(log n) for RRB-tree backends; O(1) for `StdVecBackend`.
    pub fn get(&self, index: usize) -> Option<A> {
        let guard = self.state.lock().expect("TieredSequence mutex poisoned");
        let cold_len = guard.cold.len();
        if index < cold_len {
            guard.cold.get(index)
        } else {
            guard.hot.get(index - cold_len)
        }
    }

    /// Returns the total number of elements in both tiers.
    ///
    /// Time: O(1).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.cold.len() + guard.hot.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes hot elements into cold synchronously.
    ///
    /// All hot elements are appended to cold in order and hot is cleared. The
    /// write counter is reset.
    ///
    /// This is always safe to call regardless of the [`PropagationPolicy`].
    ///
    /// Time: O(n) where n is the number of elements in hot.
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// The snapshot is independent: subsequent mutations to `self` do not affect
    /// it. For `PdsVectorBackend` the clone is O(1) via structural sharing; for
    /// `StdVecBackend` it is O(n).
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.hot.clone()
    }

    /// Spawns a background thread that flushes on the given interval.
    ///
    /// Only meaningful with the [`Timed`][PropagationPolicy::Timed] policy, but
    /// safe to call with any policy. The thread wakes every `duration` and calls
    /// `flush`.
    ///
    /// Drop the returned handle to stop the thread.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is poisoned when the background thread runs.
    pub fn start_background_propagation(&self) -> super::PropagationHandle {
        let state_clone = Arc::clone(&self.state);

        let duration = {
            let guard = state_clone.lock().expect("TieredSequence mutex poisoned");
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
                    let mut guard = state_clone.lock().expect("TieredSequence mutex poisoned");
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

// --- SequenceBackend impl for TieredSequence ---
//
// This makes TieredSequence itself usable as a tier inside another
// TieredSequence, enabling three-tier (and deeper) compositions.

impl<A, Hot, Cold> SequenceBackend<A> for TieredSequence<A, Hot, Cold>
where
    A: Clone + Send + 'static,
    Hot: SequenceBackend<A>,
    Cold: SequenceBackend<A>,
{
    fn get(&self, index: usize) -> Option<A> {
        TieredSequence::get(self, index)
    }

    fn push_back(&mut self, value: A) {
        TieredSequence::push_back(self, value);
    }

    fn pop_back(&mut self) -> Option<A> {
        TieredSequence::pop_back(self)
    }

    fn len(&self) -> usize {
        TieredSequence::len(self)
    }

    fn is_empty(&self) -> bool {
        TieredSequence::is_empty(self)
    }

    /// Appends elements from `iter` to the hot tier.
    ///
    /// Elements will propagate to cold according to the policy on the next flush.
    ///
    /// Time: O(k) where k is the number of elements in `iter`.
    fn load_from(&mut self, iter: impl Iterator<Item = A>) {
        let mut guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.hot.load_from(iter);
    }

    /// Flushes hot to cold, then drains cold.
    ///
    /// After this call both tiers are empty. Returns all elements in committed
    /// order (cold first, then what was in hot, in push order).
    ///
    /// Time: O(n).
    fn drain(&mut self) -> Vec<A> {
        let mut guard = self.state.lock().expect("TieredSequence mutex poisoned");
        guard.flush();
        guard.cold.drain()
    }
}

// --- Type alias ---

/// A [`TieredSequence`] — a two-tier write-behind vector.
///
/// `TieredVector<A, Hot, Cold>` is a type alias for `TieredSequence<A, Hot, Cold>`.
/// Both names are interchangeable; prefer `TieredVector` in application code for
/// clarity.
///
/// # Example
///
/// ```
/// # use pds::tiered::{TieredVector, PropagationPolicy};
/// # use pds::tiered::sequence_backends::{StdVecBackend, PdsVectorBackend};
/// let tv: TieredVector<i32, StdVecBackend<i32>, PdsVectorBackend<i32>> =
///     TieredVector::new(StdVecBackend::new(), PdsVectorBackend::new(), PropagationPolicy::Manual);
/// tv.push_back(1);
/// tv.push_back(2);
/// assert_eq!(tv.get(0), Some(1));
/// ```
pub type TieredVector<A, Hot, Cold> = TieredSequence<A, Hot, Cold>;
