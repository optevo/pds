// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use arbitrary::{size_hint, Arbitrary, MaxRecursionReached, Result, Unstructured};
use core::hash::{BuildHasher, Hash};

#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::hash_width::HashWidth;
use crate::{
    shared_ptr::SharedPointerKind, GenericHashMap, GenericHashSet, GenericOrdBag, GenericOrdBiMap,
    GenericOrdInsertionOrderMap, GenericOrdInsertionOrderSet, GenericOrdMap, GenericOrdMultiMap,
    GenericOrdSet, GenericOrdSymMap, GenericOrdTrie, GenericVector,
};
#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::{
    GenericBag, GenericBiMap, GenericHashMultiMap, GenericInsertionOrderMap,
    GenericInsertionOrderSet, GenericSymMap, GenericTrie, GenericUniqueVector,
};

impl<'a, A: Arbitrary<'a> + Clone, P: SharedPointerKind + 'static> Arbitrary<'a>
    for GenericVector<A, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<
        'a,
        K: Arbitrary<'a> + Ord + Clone,
        V: Arbitrary<'a> + Clone,
        P: SharedPointerKind + 'static,
    > Arbitrary<'a> for GenericOrdMap<K, V, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<'a, A: Arbitrary<'a> + Ord + Clone, P: SharedPointerKind + 'static> Arbitrary<'a>
    for GenericOrdSet<A, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<'a, K, V, S, P> Arbitrary<'a> for GenericHashMap<K, V, S, P>
where
    K: Arbitrary<'a> + Hash + Eq + Clone,
    V: Arbitrary<'a> + Clone + Hash,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<'a, A, S, P> Arbitrary<'a> for GenericHashSet<A, S, P>
where
    A: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, A, S, P> Arbitrary<'a> for GenericBag<A, S, P>
where
    A: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, K, V, S, P, H: HashWidth> Arbitrary<'a> for GenericHashMultiMap<K, V, S, P, H>
where
    K: Arbitrary<'a> + Hash + Eq + Clone,
    // HashMultiMap stores values in a per-key set, so V must also be Hash + Eq.
    V: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, K, V, S, P, H: HashWidth> Arbitrary<'a> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Arbitrary<'a> + Hash + Eq + Clone,
    V: Arbitrary<'a> + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, A, S, P, H: HashWidth> Arbitrary<'a> for GenericInsertionOrderSet<A, S, P, H>
where
    A: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, K, V, S, P, H: HashWidth> Arbitrary<'a> for GenericBiMap<K, V, S, P, H>
where
    K: Arbitrary<'a> + Hash + Eq + Clone,
    V: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, A, S, P, H: HashWidth> Arbitrary<'a> for GenericSymMap<A, S, P, H>
where
    A: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, K, V, S, P> Arbitrary<'a> for GenericTrie<K, V, S, P>
where
    K: Arbitrary<'a> + Hash + Eq + Clone,
    V: Arbitrary<'a> + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<'a, A: Arbitrary<'a> + Ord + Clone, P: SharedPointerKind + 'static> Arbitrary<'a>
    for GenericOrdBag<A, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<
        'a,
        K: Arbitrary<'a> + Ord + Clone,
        V: Arbitrary<'a> + Ord + Clone,
        P: SharedPointerKind + 'static,
    > Arbitrary<'a> for GenericOrdMultiMap<K, V, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<
        'a,
        K: Arbitrary<'a> + Ord + Clone,
        V: Arbitrary<'a> + Ord + Clone,
        P: SharedPointerKind + 'static,
    > Arbitrary<'a> for GenericOrdBiMap<K, V, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<'a, A: Arbitrary<'a> + Ord + Clone, P: SharedPointerKind + 'static> Arbitrary<'a>
    for GenericOrdSymMap<A, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<
        'a,
        K: Arbitrary<'a> + Ord + Clone,
        V: Arbitrary<'a> + Clone,
        P: SharedPointerKind + 'static,
    > Arbitrary<'a> for GenericOrdTrie<K, V, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<
        'a,
        K: Arbitrary<'a> + Ord + Clone,
        V: Arbitrary<'a> + Clone,
        P: SharedPointerKind + 'static,
    > Arbitrary<'a> for GenericOrdInsertionOrderMap<K, V, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

impl<'a, A: Arbitrary<'a> + Ord + Clone, P: SharedPointerKind + 'static> Arbitrary<'a>
    for GenericOrdInsertionOrderSet<A, P>
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<'a, A, S, P, H: HashWidth> Arbitrary<'a> for GenericUniqueVector<A, S, P, H>
where
    A: Arbitrary<'a> + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        u.arbitrary_iter()?.collect()
    }

    fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
        u.arbitrary_take_rest_iter()?.collect()
    }

    fn try_size_hint(
        depth: usize,
    ) -> core::result::Result<(usize, Option<usize>), MaxRecursionReached> {
        size_hint::try_recursion_guard(depth, |depth| {
            Ok(size_hint::and(
                <usize as Arbitrary>::try_size_hint(depth)?,
                (0, None),
            ))
        })
    }
}
