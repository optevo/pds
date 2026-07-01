//! [`TieredTrie`] — a two-tier write-behind prefix tree.
//!
//! Mirrors the semantics of [`TieredCollection`][super::TieredCollection] but
//! for tries. Pending deletes track exact paths (`Vec<K>`); prefix queries
//! merge results from both tiers with hot winning on duplicate exact keys.

use super::policy::PropagationPolicy;
use super::trie_backend::TrieBackend;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

// --- Internal state ---

/// Internal mutable state for a [`TieredTrie`].
struct TieredTrieState<K, V, Hot, Cold> {
    /// Fast, mutable hot tier. Receives all writes.
    hot: Hot,
    /// Slower, richer cold tier. Updated only during flush.
    cold: Cold,
    /// Exact paths removed from hot not yet applied to cold.
    pending_deletes: HashSet<Vec<K>>,
    /// Write counter for `Batched` policy.
    write_count: usize,
    /// Propagation policy.
    policy: PropagationPolicy,
    /// Phantom: `V` is a logical part of the trie's type but is stored only
    /// inside `Hot` and `Cold`, not directly in this struct.
    _v: PhantomData<V>,
}

impl<K, V, Hot, Cold> TieredTrieState<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
    Hot: TrieBackend<K, V>,
    Cold: TrieBackend<K, V>,
{
    fn flush(&mut self) {
        for path in self.pending_deletes.drain() {
            self.cold.remove(&path);
        }
        for (path, v) in self.hot.drain() {
            self.cold.insert(path, v);
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

// --- TieredTrie ---

/// A two-tier write-behind prefix tree.
///
/// `TieredTrie<K, V, Hot, Cold>` routes writes to the `Hot` backend. Reads
/// check hot first, falling back to cold for paths not in hot and not
/// pending-deleted. Prefix queries merge results from both tiers (hot wins on
/// exact-key conflicts).
///
/// # Cheap `Clone`
///
/// Cloning is O(1) — clones the inner `Arc`.
pub struct TieredTrie<K, V, Hot, Cold> {
    state: Arc<Mutex<TieredTrieState<K, V, Hot, Cold>>>,
}

impl<K, V, Hot, Cold> Clone for TieredTrie<K, V, Hot, Cold> {
    fn clone(&self) -> Self {
        TieredTrie {
            state: Arc::clone(&self.state),
        }
    }
}

impl<K, V, Hot, Cold> TieredTrie<K, V, Hot, Cold>
where
    K: Clone + Eq + std::hash::Hash + Send + 'static,
    V: Clone + Send + 'static,
    Hot: TrieBackend<K, V>,
    Cold: TrieBackend<K, V>,
{
    /// Creates a new `TieredTrie` with the given backends and policy.
    ///
    /// Time: O(1).
    pub fn new(hot: Hot, cold: Cold, policy: PropagationPolicy) -> Self {
        TieredTrie {
            state: Arc::new(Mutex::new(TieredTrieState {
                hot,
                cold,
                pending_deletes: HashSet::new(),
                write_count: 0,
                policy,
                _v: PhantomData,
            })),
        }
    }

    /// Creates a `TieredTrie` with a [`Timed`][PropagationPolicy::Timed] policy
    /// and starts the background propagation thread.
    ///
    /// Time: O(1).
    pub fn with_timed_propagation(
        hot: Hot,
        cold: Cold,
        interval: std::time::Duration,
    ) -> (Self, super::PropagationHandle) {
        let tt = Self::new(hot, cold, PropagationPolicy::Timed(interval));
        let handle = tt.start_background_propagation();
        (tt, handle)
    }

    /// Inserts `(path, value)` into the hot tier, returning the previous value.
    ///
    /// Cancels any pending delete for this exact path.
    ///
    /// Time: O(d) where d is the depth.
    pub fn insert(&self, path: Vec<K>, value: V) -> Option<V> {
        let mut guard = self.state.lock().expect("TieredTrie mutex poisoned");
        let prev = if let Some(v) = guard.hot.get(&path) {
            Some(v)
        } else if guard.pending_deletes.contains(&path) {
            None
        } else {
            guard.cold.get(&path)
        };
        guard.pending_deletes.remove(&path);
        guard.hot.insert(path, value);
        guard.record_write();
        prev
    }

    /// Returns the value at `path`.
    ///
    /// Checks hot first; falls back to cold unless the path is pending-deleted.
    ///
    /// Time: O(d).
    pub fn get(&self, path: &[K]) -> Option<V> {
        let guard = self.state.lock().expect("TieredTrie mutex poisoned");
        if let Some(v) = guard.hot.get(path) {
            return Some(v);
        }
        if guard.pending_deletes.contains(path) {
            return None;
        }
        guard.cold.get(path)
    }

    /// Removes the entry at `path`, returning the previous value.
    ///
    /// Adds the path to `pending_deletes` to suppress the cold tier.
    ///
    /// Time: O(d).
    pub fn remove(&self, path: &[K]) -> Option<V> {
        let mut guard = self.state.lock().expect("TieredTrie mutex poisoned");
        let prev = if let Some(v) = guard.hot.remove(path) {
            Some(v)
        } else if guard.pending_deletes.contains(path) {
            None
        } else {
            guard.cold.get(path)
        };
        if prev.is_some() {
            guard.pending_deletes.insert(path.to_vec());
            guard.record_write();
        }
        prev
    }

    /// Tests whether `path` is logically present.
    ///
    /// Time: O(d).
    pub fn contains_path(&self, path: &[K]) -> bool {
        self.get(path).is_some()
    }

    /// Returns all `(path, value)` pairs whose path starts with `prefix`,
    /// merging results from hot and cold.
    ///
    /// Hot wins on exact-key conflicts. Pending-deleted paths are excluded.
    ///
    /// Time: O(d + m) where m is the total matching entries.
    pub fn prefix_get(&self, prefix: &[K]) -> Vec<(Vec<K>, V)> {
        let guard = self.state.lock().expect("TieredTrie mutex poisoned");
        let hot_results = guard.hot.prefix_get(prefix);
        let hot_paths: std::collections::HashSet<Vec<K>> =
            hot_results.iter().map(|(p, _)| p.clone()).collect();

        let mut result: Vec<(Vec<K>, V)> = guard
            .cold
            .prefix_get(prefix)
            .into_iter()
            .filter(|(p, _)| !hot_paths.contains(p) && !guard.pending_deletes.contains(p))
            .collect();
        for (p, v) in hot_results {
            if !guard.pending_deletes.contains(&p) {
                result.push((p, v));
            }
        }
        result
    }

    /// Returns the number of values across both tiers (approximate).
    ///
    /// Time: O(n).
    pub fn len(&self) -> usize {
        let guard = self.state.lock().expect("TieredTrie mutex poisoned");
        guard.hot.len() + guard.cold.len()
    }

    /// Tests whether both tiers are empty.
    ///
    /// Time: O(1).
    pub fn is_empty(&self) -> bool {
        let guard = self.state.lock().expect("TieredTrie mutex poisoned");
        guard.hot.is_empty() && guard.cold.is_empty()
    }

    /// Flushes the hot tier into the cold tier synchronously.
    ///
    /// Time: O(n).
    pub fn flush(&self) {
        let mut guard = self.state.lock().expect("TieredTrie mutex poisoned");
        guard.flush();
    }

    /// Returns a clone of the current cold tier.
    ///
    /// Time: depends on `Cold::clone`.
    pub fn cold_snapshot(&self) -> Cold
    where
        Cold: Clone,
    {
        let guard = self.state.lock().expect("TieredTrie mutex poisoned");
        guard.cold.clone()
    }

    /// Returns a clone of the current hot tier.
    ///
    /// Time: depends on `Hot::clone`.
    pub fn hot_snapshot(&self) -> Hot
    where
        Hot: Clone,
    {
        let guard = self.state.lock().expect("TieredTrie mutex poisoned");
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
            let guard = state_clone.lock().expect("TieredTrie mutex poisoned");
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
                    let mut guard = state_clone.lock().expect("TieredTrie mutex poisoned");
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

/// A [`TieredTrie`] using pds ordered trie backends for both tiers.
pub type TieredOrdTrie<K, V> = TieredTrie<
    K,
    V,
    super::trie_backends::PdsOrdTrieBackend<K, V>,
    super::trie_backends::PdsOrdTrieBackend<K, V>,
>;
