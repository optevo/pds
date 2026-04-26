#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::hash_width::HashWidth;
use crate::{
    shared_ptr::SharedPointerKind, GenericHashMap, GenericHashSet, GenericOrdMap, GenericOrdSet,
    GenericVector,
};
#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::{
    GenericBag, GenericBiMap, GenericHashMultiMap, GenericInsertionOrderMap,
    GenericInsertionOrderSet, GenericSymMap, GenericTrie,
};
use ::quickcheck::{Arbitrary, Gen};
use core::hash::{BuildHasher, Hash};
use core::iter::FromIterator;

impl<A: Arbitrary + Sync + Clone, P: SharedPointerKind + 'static> Arbitrary
    for GenericVector<A, P>
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericVector::from_iter(Vec::<A>::arbitrary(g))
    }
}

impl<
        K: Ord + Clone + Arbitrary + Sync,
        V: Clone + Arbitrary + Sync,
        P: SharedPointerKind + 'static,
    > Arbitrary for GenericOrdMap<K, V, P>
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericOrdMap::from_iter(Vec::<(K, V)>::arbitrary(g))
    }
}

impl<A: Ord + Clone + Arbitrary + Sync, P: SharedPointerKind + 'static> Arbitrary
    for GenericOrdSet<A, P>
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericOrdSet::from_iter(Vec::<A>::arbitrary(g))
    }
}

impl<A, S, P> Arbitrary for GenericHashSet<A, S, P>
where
    A: Hash + Eq + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericHashSet::from_iter(Vec::<A>::arbitrary(g))
    }
}

impl<K, V, S, P> Arbitrary for GenericHashMap<K, V, S, P>
where
    K: Hash + Eq + Arbitrary + Sync,
    V: Arbitrary + Sync + Hash,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericHashMap::from(Vec::<(K, V)>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<A, S, P> Arbitrary for GenericBag<A, S, P>
where
    A: Hash + Eq + Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericBag::from_iter(Vec::<A>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<K, V, S, P, H: HashWidth> Arbitrary for GenericHashMultiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone + Arbitrary + Sync,
    // HashMultiMap stores values in a per-key set, so V must also be Hash + Eq.
    V: Hash + Eq + Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericHashMultiMap::from_iter(Vec::<(K, V)>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<K, V, S, P, H: HashWidth> Arbitrary for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone + Arbitrary + Sync,
    V: Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericInsertionOrderMap::from_iter(Vec::<(K, V)>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<A, S, P, H: HashWidth> Arbitrary for GenericInsertionOrderSet<A, S, P, H>
where
    A: Hash + Eq + Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericInsertionOrderSet::from_iter(Vec::<A>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<K, V, S, P, H: HashWidth> Arbitrary for GenericBiMap<K, V, S, P, H>
where
    K: Hash + Eq + Clone + Arbitrary + Sync,
    V: Hash + Eq + Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericBiMap::from_iter(Vec::<(K, V)>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<A, S, P, H: HashWidth> Arbitrary for GenericSymMap<A, S, P, H>
where
    A: Hash + Eq + Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericSymMap::from_iter(Vec::<(A, A)>::arbitrary(g))
    }
}

#[cfg(any(feature = "std", feature = "foldhash"))]
impl<K, V, S, P> Arbitrary for GenericTrie<K, V, S, P>
where
    K: Hash + Eq + Clone + Arbitrary + Sync,
    V: Clone + Arbitrary + Sync,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
    P: SharedPointerKind + 'static,
{
    fn arbitrary(g: &mut Gen) -> Self {
        GenericTrie::from_iter(Vec::<(Vec<K>, V)>::arbitrary(g))
    }
}
