//! Proptest strategies.
//!
//! These are only available when using the `proptest` feature flag.

#[cfg(any(feature = "std", feature = "foldhash"))]
use crate::{
    Bag, BiMap, HashMap, HashMultiMap, HashSet, InsertionOrderMap, InsertionOrderSet, SymMap, Trie,
};
use crate::{OrdMap, OrdSet, Vector};
use ::proptest::collection::vec;
use ::proptest::strategy::{BoxedStrategy, Strategy, ValueTree};
#[cfg(any(feature = "std", feature = "foldhash"))]
use core::hash::Hash;
use core::iter::FromIterator;
use core::ops::Range;

/// A strategy for generating a [`Vector`][Vector] of a certain size.
///
/// # Examples
///
/// ```rust,no_run
/// # use ::proptest::proptest;
/// proptest! {
///     #[test]
///     fn proptest_a_vector(ref l in vector(".*", 10..100)) {
///         assert!(l.len() < 100);
///         assert!(l.len() >= 10);
///     }
/// }
/// ```
///
/// [Vector]: ../type.Vector.html
pub fn vector<A: Strategy + 'static>(
    element: A,
    size: Range<usize>,
) -> BoxedStrategy<Vector<<A::Tree as ValueTree>::Value>>
where
    <A::Tree as ValueTree>::Value: Clone,
{
    vec(element, size).prop_map(Vector::from_iter).boxed()
}

/// A strategy for an [`OrdMap`][OrdMap] of a given size.
///
/// # Examples
///
/// ```rust,no_run
/// # use ::proptest::proptest;
/// proptest! {
///     #[test]
///     fn proptest_works(ref m in ord_map(0..9999, ".*", 10..100)) {
///         assert!(m.len() < 100);
///         assert!(m.len() >= 10);
///     }
/// }
/// ```
///
/// [OrdMap]: ../type.OrdMap.html
pub fn ord_map<K: Strategy + 'static, V: Strategy + 'static>(
    key: K,
    value: V,
    size: Range<usize>,
) -> BoxedStrategy<OrdMap<<K::Tree as ValueTree>::Value, <V::Tree as ValueTree>::Value>>
where
    <K::Tree as ValueTree>::Value: Ord + Clone,
    <V::Tree as ValueTree>::Value: Clone,
{
    ::proptest::collection::vec((key, value), size.clone())
        .prop_map(OrdMap::from)
        .prop_filter("OrdMap minimum size".to_owned(), move |m| {
            m.len() >= size.start
        })
        .boxed()
}

/// A strategy for an [`OrdSet`][OrdSet] of a given size.
///
/// # Examples
///
/// ```rust,no_run
/// # use ::proptest::proptest;
/// proptest! {
///     #[test]
///     fn proptest_a_set(ref s in ord_set(".*", 10..100)) {
///         assert!(s.len() < 100);
///         assert!(s.len() >= 10);
///     }
/// }
/// ```
///
/// [OrdSet]: ../type.OrdSet.html
pub fn ord_set<A: Strategy + 'static>(
    element: A,
    size: Range<usize>,
) -> BoxedStrategy<OrdSet<<A::Tree as ValueTree>::Value>>
where
    <A::Tree as ValueTree>::Value: Ord + Clone,
{
    ::proptest::collection::vec(element, size.clone())
        .prop_map(OrdSet::from)
        .prop_filter("OrdSet minimum size".to_owned(), move |s| {
            s.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`HashMap`][HashMap] of a given size.
///
/// # Examples
///
/// ```rust,no_run
/// # use ::proptest::proptest;
/// proptest! {
///     #[test]
///     fn proptest_works(ref m in hash_map(0..9999, ".*", 10..100)) {
///         assert!(m.len() < 100);
///         assert!(m.len() >= 10);
///     }
/// }
/// ```
///
/// [HashMap]: ../type.HashMap.html
pub fn hash_map<K: Strategy + 'static, V: Strategy + 'static>(
    key: K,
    value: V,
    size: Range<usize>,
) -> BoxedStrategy<HashMap<<K::Tree as ValueTree>::Value, <V::Tree as ValueTree>::Value>>
where
    <K::Tree as ValueTree>::Value: Hash + Eq + Clone,
    <V::Tree as ValueTree>::Value: Clone + Hash,
{
    ::proptest::collection::vec((key, value), size.clone())
        .prop_map(HashMap::from)
        .prop_filter("Map minimum size".to_owned(), move |m| {
            m.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`HashSet`][HashSet] of a given size.
///
/// # Examples
///
/// ```rust,no_run
/// # use ::proptest::proptest;
/// proptest! {
///     #[test]
///     fn proptest_a_set(ref s in hash_set(".*", 10..100)) {
///         assert!(s.len() < 100);
///         assert!(s.len() >= 10);
///     }
/// }
/// ```
///
/// [HashSet]: ../type.HashSet.html
pub fn hash_set<A: Strategy + 'static>(
    element: A,
    size: Range<usize>,
) -> BoxedStrategy<HashSet<<A::Tree as ValueTree>::Value>>
where
    <A::Tree as ValueTree>::Value: Hash + Eq + Clone,
{
    ::proptest::collection::vec(element, size.clone())
        .prop_map(HashSet::from)
        .prop_filter("HashSet minimum size".to_owned(), move |s| {
            s.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`Bag`][Bag] of a given size.
///
/// The `size` range controls the total element count (including duplicates).
///
/// [Bag]: ../type.Bag.html
pub fn bag<A: Strategy + 'static>(
    element: A,
    size: Range<usize>,
) -> BoxedStrategy<Bag<<A::Tree as ValueTree>::Value>>
where
    <A::Tree as ValueTree>::Value: Hash + Eq + Clone,
{
    vec(element, size.clone())
        .prop_map(Bag::from_iter)
        .prop_filter("Bag minimum size".to_owned(), move |b| {
            b.total_count() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`HashMultiMap`][HashMultiMap] of a given size.
///
/// The `size` range controls the total entry count (including duplicate keys).
///
/// [HashMultiMap]: ../type.HashMultiMap.html
pub fn hash_multi_map<K: Strategy + 'static, V: Strategy + 'static>(
    key: K,
    value: V,
    size: Range<usize>,
) -> BoxedStrategy<HashMultiMap<<K::Tree as ValueTree>::Value, <V::Tree as ValueTree>::Value>>
where
    <K::Tree as ValueTree>::Value: Hash + Eq + Clone,
    // HashMultiMap stores values in a per-key set, so V must also be Hash + Eq.
    <V::Tree as ValueTree>::Value: Hash + Eq + Clone,
{
    vec((key, value), size.clone())
        .prop_map(HashMultiMap::from_iter)
        .prop_filter("HashMultiMap minimum size".to_owned(), move |m| {
            m.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for an [`InsertionOrderMap`][InsertionOrderMap] of a given size.
///
/// [InsertionOrderMap]: ../type.InsertionOrderMap.html
pub fn insertion_order_map<K: Strategy + 'static, V: Strategy + 'static>(
    key: K,
    value: V,
    size: Range<usize>,
) -> BoxedStrategy<InsertionOrderMap<<K::Tree as ValueTree>::Value, <V::Tree as ValueTree>::Value>>
where
    <K::Tree as ValueTree>::Value: Hash + Eq + Clone,
    <V::Tree as ValueTree>::Value: Clone,
{
    vec((key, value), size.clone())
        .prop_map(InsertionOrderMap::from_iter)
        .prop_filter("InsertionOrderMap minimum size".to_owned(), move |m| {
            m.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for an [`InsertionOrderSet`][InsertionOrderSet] of a given size.
///
/// [InsertionOrderSet]: ../type.InsertionOrderSet.html
pub fn insertion_order_set<A: Strategy + 'static>(
    element: A,
    size: Range<usize>,
) -> BoxedStrategy<InsertionOrderSet<<A::Tree as ValueTree>::Value>>
where
    <A::Tree as ValueTree>::Value: Hash + Eq + Clone,
{
    vec(element, size.clone())
        .prop_map(InsertionOrderSet::from_iter)
        .prop_filter("InsertionOrderSet minimum size".to_owned(), move |s| {
            s.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`BiMap`][BiMap] of a given size.
///
/// Both keys and values must be unique (BiMap is a bijection). The `size`
/// range applies to distinct entries; duplicate keys or values are silently
/// deduplicated by the collection.
///
/// [BiMap]: ../type.BiMap.html
pub fn bimap<K: Strategy + 'static, V: Strategy + 'static>(
    key: K,
    value: V,
    size: Range<usize>,
) -> BoxedStrategy<BiMap<<K::Tree as ValueTree>::Value, <V::Tree as ValueTree>::Value>>
where
    <K::Tree as ValueTree>::Value: Hash + Eq + Clone,
    <V::Tree as ValueTree>::Value: Hash + Eq + Clone,
{
    vec((key, value), size.clone())
        .prop_map(BiMap::from_iter)
        .prop_filter("BiMap minimum size".to_owned(), move |m| {
            m.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`SymMap`][SymMap] of a given size.
///
/// Pairs `(a, b)` are generated by drawing independently from `first` and
/// `second`. Both must be strategies over the same element type. For
/// reproducible pair generation from a single distribution, pass the same
/// strategy expression twice (e.g. `symmap(0i32..100, 0i32..100, 1..10)`).
///
/// [SymMap]: ../type.SymMap.html
pub fn symmap<A: Strategy + 'static>(
    first: A,
    second: A,
    size: Range<usize>,
) -> BoxedStrategy<SymMap<<A::Tree as ValueTree>::Value>>
where
    <A::Tree as ValueTree>::Value: Hash + Eq + Clone,
{
    vec((first, second), size.clone())
        .prop_map(SymMap::from_iter)
        .prop_filter("SymMap minimum size".to_owned(), move |m| {
            m.len() >= size.start
        })
        .boxed()
}

#[cfg(any(feature = "std", feature = "foldhash"))]
/// A strategy for a [`Trie`][Trie] of a given size.
///
/// Each entry is a `(Vec<K>, V)` pair where the `Vec<K>` is the path. The
/// `path_len` range controls the length of each generated path; `size`
/// controls the number of entries.
///
/// [Trie]: ../type.Trie.html
pub fn trie<K: Strategy + 'static, V: Strategy + 'static>(
    key: K,
    value: V,
    path_len: Range<usize>,
    size: Range<usize>,
) -> BoxedStrategy<Trie<<K::Tree as ValueTree>::Value, <V::Tree as ValueTree>::Value>>
where
    <K::Tree as ValueTree>::Value: Hash + Eq + Clone,
    <V::Tree as ValueTree>::Value: Clone,
{
    vec((vec(key, path_len), value), size.clone())
        .prop_map(Trie::from_iter)
        .prop_filter("Trie minimum size".to_owned(), move |t| {
            t.len() >= size.start
        })
        .boxed()
}
