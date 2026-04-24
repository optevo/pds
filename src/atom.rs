// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! A thread-safe atomic state holder for persistent collections.
//!
//! `Atom<T>` wraps [`arc_swap::ArcSwap`] to provide lock-free reads
//! and CAS-loop writes. Readers get consistent snapshots via
//! [`load()`][Atom::load]; writers apply pure functions via
//! [`update()`][Atom::update] with automatic retry on contention.
//!
//! This is the canonical way to share persistent data structures
//! across threads: the combination of structural sharing (cheap
//! clones) and atomic swapping means readers never block writers
//! and vice versa.
//!
//! # Examples
//!
//! ```rust,ignore
//! use imbl::{atom::Atom, HashMap};
//!
//! let state = Atom::new(HashMap::new());
//!
//! // Writer: atomically insert a key
//! state.update(|map| {
//!     let mut m = map.clone();
//!     m.insert("key", 42);
//!     m
//! });
//!
//! // Reader: get a consistent snapshot
//! let snapshot = state.load();
//! assert_eq!(snapshot.get("key"), Some(&42));
//! ```

use core::fmt::{self, Debug};
use alloc::sync::Arc;

use arc_swap::ArcSwap;

/// A thread-safe atomic state holder.
///
/// Provides lock-free reads and compare-and-swap writes for any
/// `Send + Sync` type. Designed for persistent collections where
/// cloning is O(1) and updates produce new values from old ones.
pub struct Atom<T> {
    inner: ArcSwap<T>,
}

impl<T> Atom<T> {
    /// Create a new `Atom` holding the given value.
    pub fn new(value: T) -> Self {
        Atom {
            inner: ArcSwap::from_pointee(value),
        }
    }

    /// Load the current value.
    ///
    /// Returns an `Arc<T>` pointing to the current state. This is
    /// lock-free and wait-free. The returned `Arc` is a consistent
    /// snapshot — it will not be affected by subsequent updates.
    pub fn load(&self) -> Arc<T> {
        self.inner.load_full()
    }

    /// Store a new value, replacing whatever was there.
    pub fn store(&self, value: T) {
        self.inner.store(Arc::new(value));
    }

    /// Atomically update the value using a pure function.
    ///
    /// The function `f` receives the current value and returns the new
    /// value. If another thread updates the value between the read and
    /// the swap, `f` is retried with the newer value. This is a
    /// compare-and-swap loop — `f` should be cheap and side-effect-free.
    pub fn update<F>(&self, mut f: F)
    where
        F: FnMut(&T) -> T,
    {
        // rcu (read-copy-update) handles the CAS retry loop: it loads
        // the current value, calls f, and atomically swaps the result
        // in — retrying f if another thread updated in between.
        self.inner.rcu(|old| f(old));
    }

    /// Swap the current value with a new one, returning the old value.
    pub fn swap(&self, value: T) -> Arc<T> {
        self.inner.swap(Arc::new(value))
    }
}

impl<T: Default> Default for Atom<T> {
    fn default() -> Self {
        Atom::new(T::default())
    }
}

impl<T: Debug> Debug for Atom<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let val = self.inner.load();
        f.debug_tuple("Atom").field(&**val).finish()
    }
}

// Atom is Send+Sync if T is Send+Sync (inherited from ArcSwap).
// ArcSwap<T> requires T: Send + Sync for its own Send + Sync impls,
// which is correct — the value must be safely shareable across threads.

impl<T> Clone for Atom<T> {
    /// Clone an `Atom` by loading the current value and wrapping it
    /// in a new `Atom`. The new atom starts with the same value but
    /// is independently updatable.
    fn clone(&self) -> Self {
        let current = self.inner.load_full();
        Atom {
            inner: ArcSwap::new(current),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::HashMap;

    #[test]
    fn basic_load_store() {
        let atom = Atom::new(42);
        assert_eq!(*atom.load(), 42);
        atom.store(99);
        assert_eq!(*atom.load(), 99);
    }

    #[test]
    fn update_applies_function() {
        let atom = Atom::new(10);
        atom.update(|v| v + 5);
        assert_eq!(*atom.load(), 15);
    }

    #[test]
    fn swap_returns_old_value() {
        let atom = Atom::new("hello");
        let old = atom.swap("world");
        assert_eq!(*old, "hello");
        assert_eq!(*atom.load(), "world");
    }

    #[test]
    fn snapshot_isolation() {
        let atom = Atom::new(1);
        let snapshot = atom.load();
        atom.store(2);
        // Snapshot should still see old value
        assert_eq!(*snapshot, 1);
        assert_eq!(*atom.load(), 2);
    }

    #[test]
    fn with_hashmap() {
        let atom = Atom::new(HashMap::new());
        atom.update(|map| {
            let mut m = map.clone();
            m.insert("key", 42);
            m
        });
        let snap = atom.load();
        assert_eq!(snap.get("key"), Some(&42));
    }

    #[test]
    fn clone_independence() {
        let a = Atom::new(1);
        let b = a.clone();
        a.store(10);
        // b should still see the value at clone time
        assert_eq!(*b.load(), 1);
        assert_eq!(*a.load(), 10);
    }

    #[test]
    fn default_atom() {
        let atom: Atom<i32> = Atom::default();
        assert_eq!(*atom.load(), 0);
    }

    #[test]
    fn concurrent_updates() {
        use alloc::sync::Arc as StdArc;
        use std::thread;

        let atom = StdArc::new(Atom::new(0i64));
        let n = 1000;
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let atom = StdArc::clone(&atom);
                thread::spawn(move || {
                    for _ in 0..n {
                        atom.update(|v| v + 1);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(*atom.load(), 4 * n);
    }
}
