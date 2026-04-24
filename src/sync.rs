// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

pub(crate) use self::lock::Lock;

// Lock wraps a Mutex for thread-safe interior mutability. In no_std mode,
// std::sync::Mutex is not available — Lock uses a spin-based approach
// with core atomics instead.
#[cfg(feature = "std")]
mod lock {
    use alloc::sync::Arc;
    use std::sync::{Mutex, MutexGuard};

    /// Thread safe lock: just wraps a `Mutex`.
    pub(crate) struct Lock<A> {
        lock: Arc<Mutex<A>>,
    }

    impl<A> Lock<A> {
        pub(crate) fn new(value: A) -> Self {
            Lock {
                lock: Arc::new(Mutex::new(value)),
            }
        }

        #[inline]
        pub(crate) fn lock(&mut self) -> Option<MutexGuard<'_, A>> {
            self.lock.lock().ok()
        }
    }

    impl<A> Clone for Lock<A> {
        fn clone(&self) -> Self {
            Lock {
                lock: self.lock.clone(),
            }
        }
    }
}

// no_std fallback: uses a spin lock via core atomics. This is used only
// by FocusMut which needs interior mutability for its tree reference.
#[cfg(not(feature = "std"))]
mod lock {
    use alloc::sync::Arc;
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicBool, Ordering};

    pub(crate) struct SpinGuard<'a, A> {
        lock: &'a AtomicBool,
        data: &'a mut A,
    }

    impl<A> core::ops::Deref for SpinGuard<'_, A> {
        type Target = A;
        fn deref(&self) -> &A {
            self.data
        }
    }

    impl<A> core::ops::DerefMut for SpinGuard<'_, A> {
        fn deref_mut(&mut self) -> &mut A {
            self.data
        }
    }

    impl<A> Drop for SpinGuard<'_, A> {
        fn drop(&mut self) {
            self.lock.store(false, Ordering::Release);
        }
    }

    struct SpinMutex<A> {
        locked: AtomicBool,
        data: UnsafeCell<A>,
    }

    // SAFETY: SpinMutex provides exclusive access via atomic lock flag,
    // same guarantees as std::sync::Mutex.
    #[allow(unsafe_code)]
    unsafe impl<A: Send> Send for SpinMutex<A> {}
    #[allow(unsafe_code)]
    unsafe impl<A: Send> Sync for SpinMutex<A> {}

    impl<A> SpinMutex<A> {
        fn new(value: A) -> Self {
            SpinMutex {
                locked: AtomicBool::new(false),
                data: UnsafeCell::new(value),
            }
        }

        #[allow(unsafe_code)]
        fn lock(&self) -> SpinGuard<'_, A> {
            while self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
            SpinGuard {
                lock: &self.locked,
                // SAFETY: We hold the atomic lock, guaranteeing exclusive access.
                data: unsafe { &mut *self.data.get() },
            }
        }
    }

    /// Thread safe lock: wraps a spin mutex for no_std environments.
    pub(crate) struct Lock<A> {
        lock: Arc<SpinMutex<A>>,
    }

    impl<A> Lock<A> {
        pub(crate) fn new(value: A) -> Self {
            Lock {
                lock: Arc::new(SpinMutex::new(value)),
            }
        }

        #[inline]
        pub(crate) fn lock(&mut self) -> Option<SpinGuard<'_, A>> {
            Some(self.lock.lock())
        }
    }

    impl<A> Clone for Lock<A> {
        fn clone(&self) -> Self {
            Lock {
                lock: self.lock.clone(),
            }
        }
    }
}
