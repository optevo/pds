// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Every codebase needs a `util` module.

use core::hash::Hasher;
use core::ops::{Bound, Range, RangeBounds};

use archery::{SharedPointer, SharedPointerKind};

pub(crate) fn clone_ref<A, P>(r: SharedPointer<A, P>) -> A
where
    A: Clone,
    P: SharedPointerKind,
{
    SharedPointer::try_unwrap(r).unwrap_or_else(|r| (*r).clone())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Side {
    Left,
    Right,
}

pub(crate) fn to_range<R>(range: &R, right_unbounded: usize) -> Range<usize>
where
    R: RangeBounds<usize>,
{
    let start_index = match range.start_bound() {
        Bound::Included(i) => *i,
        Bound::Excluded(i) => *i + 1,
        Bound::Unbounded => 0,
    };
    let end_index = match range.end_bound() {
        Bound::Included(i) => *i + 1,
        Bound::Excluded(i) => *i,
        Bound::Unbounded => right_unbounded,
    };
    start_index..end_index
}

/// A minimal hasher for computing per-entry hashes that are then combined
/// with an order-independent operation (wrapping_add). Uses FNV-1a.
pub(crate) struct FnvHasher(u64);

impl FnvHasher {
    pub(crate) fn new() -> Self {
        FnvHasher(0xcbf29ce484222325)
    }
}

impl Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ops::Bound;

    use super::to_range;

    #[test]
    fn to_range_excluded_start() {
        // (Excluded(2), Unbounded) → start = 2 + 1 = 3
        let r = to_range(&(Bound::Excluded(2usize), Bound::Unbounded), 10);
        assert_eq!(r, 3..10);
    }

    #[test]
    fn to_range_included_end() {
        // ..=5 has Unbounded start and Included(5) end → 0..6
        let r = to_range(&(..=5usize), 10);
        assert_eq!(r, 0..6);
    }
}

#[cfg(test)]
macro_rules! assert_covariant {
    ($name:ident<$($gen:tt),*> in $param:ident) => {
        #[allow(dead_code, unused_assignments, unused_variables)] // The variance proof function is never called; its body uses assignments to convince the compiler.
        const _: () = {
            type Tmp<$param> = $name<$($gen),*>;
            fn assign<'a, 'b: 'a>(src: Tmp<&'b i32>, mut dst: Tmp<&'a i32>) {
                dst = src;
            }
        };
    }
}
