// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use archery::SharedPointerKind;
use serde_core::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_core::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use core::fmt;
use core::hash::{BuildHasher, Hash};
use core::marker::PhantomData;

use crate::bag::GenericBag;
use crate::bimap::GenericBiMap;
use crate::hash_multimap::GenericHashMultiMap;
use crate::hash_width::HashWidth;
use crate::hashmap::GenericHashMap;
use crate::hashset::GenericHashSet;
use crate::insertion_order_map::GenericInsertionOrderMap;
use crate::ordmap::GenericOrdMap;
use crate::ordset::GenericOrdSet;
use crate::symmap::GenericSymMap;
use crate::trie::GenericTrie;
use crate::vector::GenericVector;

struct SeqVisitor<'de, S, A> {
    phantom_s: PhantomData<S>,
    phantom_a: PhantomData<A>,
    phantom_lifetime: PhantomData<&'de ()>,
}

impl<'de, S, A> SeqVisitor<'de, S, A> {
    pub(crate) fn new() -> SeqVisitor<'de, S, A> {
        SeqVisitor {
            phantom_s: PhantomData,
            phantom_a: PhantomData,
            phantom_lifetime: PhantomData,
        }
    }
}

impl<'de, S, A> Visitor<'de> for SeqVisitor<'de, S, A>
where
    S: From<Vec<A>>,
    A: Deserialize<'de>,
{
    type Value = S;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence")
    }

    fn visit_seq<Access>(self, mut access: Access) -> Result<Self::Value, Access::Error>
    where
        Access: SeqAccess<'de>,
    {
        let mut v: Vec<A> = match access.size_hint() {
            None => Vec::new(),
            Some(l) => Vec::with_capacity(l),
        };
        while let Some(i) = access.next_element()? {
            v.push(i)
        }
        Ok(From::from(v))
    }
}

struct MapVisitor<'de, S, K, V> {
    phantom_s: PhantomData<S>,
    phantom_k: PhantomData<K>,
    phantom_v: PhantomData<V>,
    phantom_lifetime: PhantomData<&'de ()>,
}

impl<'de, S, K, V> MapVisitor<'de, S, K, V> {
    pub(crate) fn new() -> MapVisitor<'de, S, K, V> {
        MapVisitor {
            phantom_s: PhantomData,
            phantom_k: PhantomData,
            phantom_v: PhantomData,
            phantom_lifetime: PhantomData,
        }
    }
}

impl<'de, S, K, V> Visitor<'de> for MapVisitor<'de, S, K, V>
where
    S: From<Vec<(K, V)>>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    type Value = S;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence")
    }

    fn visit_map<Access>(self, mut access: Access) -> Result<Self::Value, Access::Error>
    where
        Access: MapAccess<'de>,
    {
        let mut v: Vec<(K, V)> = match access.size_hint() {
            None => Vec::new(),
            Some(l) => Vec::with_capacity(l),
        };
        while let Some(i) = access.next_entry()? {
            v.push(i)
        }
        Ok(From::from(v))
    }
}

// Set

impl<'de, A: Deserialize<'de> + Ord + Clone, P: SharedPointerKind> Deserialize<'de>
    for GenericOrdSet<A, P>
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(SeqVisitor::new())
    }
}

impl<A: Ord + Serialize, P: SharedPointerKind> Serialize for GenericOrdSet<A, P> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for i in self.iter() {
            s.serialize_element(i)?;
        }
        s.end()
    }
}

// Map

impl<'de, K: Deserialize<'de> + Ord + Clone, V: Deserialize<'de> + Clone, P: SharedPointerKind>
    Deserialize<'de> for GenericOrdMap<K, V, P>
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_map(MapVisitor::<'de, GenericOrdMap<K, V, P>, K, V>::new())
    }
}

impl<K: Serialize + Ord, V: Serialize, P: SharedPointerKind> Serialize for GenericOrdMap<K, V, P> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = ser.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            s.serialize_entry(k, v)?;
        }
        s.end()
    }
}

// HashMap

impl<'de, K, V, S, P: SharedPointerKind, H: HashWidth> Deserialize<'de> for GenericHashMap<K, V, S, P, H>
where
    K: Deserialize<'de> + Hash + Eq + Clone,
    V: Deserialize<'de> + Clone + Hash,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_map(MapVisitor::<'de, GenericHashMap<K, V, S, P, H>, K, V>::new())
    }
}

impl<K, V, S, P, H: HashWidth> Serialize for GenericHashMap<K, V, S, P, H>
where
    K: Serialize + Hash + Eq,
    V: Serialize,
    S: BuildHasher + Default,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            s.serialize_entry(k, v)?;
        }
        s.end()
    }
}

// HashSet

impl<
        'de,
        A: Deserialize<'de> + Hash + Eq + Clone,
        S: BuildHasher + Default + Clone,
        P: SharedPointerKind,
        H: HashWidth,
    > Deserialize<'de> for GenericHashSet<A, S, P, H>
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(SeqVisitor::new())
    }
}

impl<A: Serialize + Hash + Eq, S: BuildHasher + Default, P: SharedPointerKind, H: HashWidth> Serialize
    for GenericHashSet<A, S, P, H>
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for i in self.iter() {
            s.serialize_element(i)?;
        }
        s.end()
    }
}

// Vector

impl<'de, A: Clone + Deserialize<'de>, P: SharedPointerKind> Deserialize<'de>
    for GenericVector<A, P>
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(SeqVisitor::<'de, GenericVector<A, P>, A>::new())
    }
}

impl<A: Serialize, P: SharedPointerKind> Serialize for GenericVector<A, P> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for i in self.iter() {
            s.serialize_element(i)?;
        }
        s.end()
    }
}

// Bag — serialises as a flat sequence of elements (each appearing count times).

impl<'de, A, S, P> Deserialize<'de> for GenericBag<A, S, P>
where
    A: Deserialize<'de> + Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(SeqVisitor::new())
    }
}

impl<A, S, P> Serialize for GenericBag<A, S, P>
where
    A: Serialize + Hash + Eq + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for (elem, count) in self.iter() {
            for _ in 0..count {
                s.serialize_element(elem)?;
            }
        }
        s.end()
    }
}

// HashMultiMap — serialises as a sequence of (key, value) pairs.

impl<'de, K, V, S, P, H: HashWidth> Deserialize<'de> for GenericHashMultiMap<K, V, S, P, H>
where
    K: Deserialize<'de> + Hash + Eq + Clone,
    V: Deserialize<'de> + Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(SeqVisitor::<'de, GenericHashMultiMap<K, V, S, P, H>, (K, V)>::new())
    }
}

impl<K, V, S, P, H: HashWidth> Serialize for GenericHashMultiMap<K, V, S, P, H>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for (k, v) in self.iter() {
            s.serialize_element(&(k, v))?;
        }
        s.end()
    }
}

// InsertionOrderMap — serialises as a sequence of (key, value) pairs to preserve order.

impl<'de, K, V, S, P, H: HashWidth> Deserialize<'de> for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Deserialize<'de> + Hash + Eq + Clone,
    V: Deserialize<'de> + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(
            SeqVisitor::<'de, GenericInsertionOrderMap<K, V, S, P, H>, (K, V)>::new(),
        )
    }
}

impl<K, V, S, P, H: HashWidth> Serialize for GenericInsertionOrderMap<K, V, S, P, H>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + Clone,
    S: BuildHasher + Clone,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for (k, v) in self.iter() {
            s.serialize_element(&(k, v))?;
        }
        s.end()
    }
}

// BiMap — serialises as a sequence of (key, value) pairs.

impl<'de, K, V, S, P, H: HashWidth> Deserialize<'de> for GenericBiMap<K, V, S, P, H>
where
    K: Deserialize<'de> + Hash + Eq + Clone,
    V: Deserialize<'de> + Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(
            SeqVisitor::<'de, GenericBiMap<K, V, S, P, H>, (K, V)>::new(),
        )
    }
}

impl<K, V, S, P, H: HashWidth> Serialize for GenericBiMap<K, V, S, P, H>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for (k, v) in self.iter() {
            s.serialize_element(&(k, v))?;
        }
        s.end()
    }
}

// SymMap — serialises as a sequence of (A, A) pairs.

impl<'de, A, S, P, H: HashWidth> Deserialize<'de> for GenericSymMap<A, S, P, H>
where
    A: Deserialize<'de> + Hash + Eq + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(
            SeqVisitor::<'de, GenericSymMap<A, S, P, H>, (A, A)>::new(),
        )
    }
}

impl<A, S, P, H: HashWidth> Serialize for GenericSymMap<A, S, P, H>
where
    A: Serialize + Hash + Eq + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for (a, b) in self.iter() {
            s.serialize_element(&(a, b))?;
        }
        s.end()
    }
}

// Trie — serialises as a sequence of (path, value) pairs.

impl<'de, K, V, S, P> Deserialize<'de> for GenericTrie<K, V, S, P>
where
    K: Deserialize<'de> + Hash + Eq + Clone,
    V: Deserialize<'de> + Clone,
    S: BuildHasher + Default + Clone,
    P: SharedPointerKind,
{
    fn deserialize<D>(des: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        des.deserialize_seq(
            SeqVisitor::<'de, GenericTrie<K, V, S, P>, (Vec<K>, V)>::new(),
        )
    }
}

impl<K, V, S, P> Serialize for GenericTrie<K, V, S, P>
where
    K: Serialize + Hash + Eq + Clone,
    V: Serialize + Clone,
    S: BuildHasher + Clone + Default,
    P: SharedPointerKind,
{
    fn serialize<Ser>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: Serializer,
    {
        let mut s = ser.serialize_seq(Some(self.len()))?;
        for (path, v) in self.iter() {
            s.serialize_element(&(path, v))?;
        }
        s.end()
    }
}

// Tests

#[cfg(test)]
mod test {
    use crate::{
        proptest::{hash_map, hash_set, ord_map, ord_set, vector},
        Bag, BiMap, Direction, HashMap, HashMultiMap, HashSet, InsertionOrderMap,
        OrdMap, OrdSet, SymMap, Trie, Vector,
    };
    use proptest::num::i32;
    use proptest::proptest;
    use serde_json::{from_str, to_string};

    proptest! {
        #[cfg_attr(miri, ignore)]
        #[test]
        fn ser_ordset(ref v in ord_set(i32::ANY, 0..100)) {
            assert_eq!(v, &from_str::<OrdSet<i32>>(&to_string(&v).unwrap()).unwrap());
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn ser_ordmap(ref v in ord_map(i32::ANY, i32::ANY, 0..100)) {
            assert_eq!(v, &from_str::<OrdMap<i32, i32>>(&to_string(&v).unwrap()).unwrap());
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn ser_hashmap(ref v in hash_map(i32::ANY, i32::ANY, 0..100)) {
            assert_eq!(v, &from_str::<HashMap<i32, i32>>(&to_string(&v).unwrap()).unwrap());
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn ser_hashset(ref v in hash_set(i32::ANY, 0..100)) {
            assert_eq!(v, &from_str::<HashSet<i32>>(&to_string(&v).unwrap()).unwrap());
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn ser_vector(ref v in vector(i32::ANY, 0..100)) {
            assert_eq!(v, &from_str::<Vector<i32>>(&to_string(&v).unwrap()).unwrap());
        }
    }

    // Serde round-trip tests for the 6 types not covered by proptest strategies.

    #[test]
    fn ser_bag() {
        let mut b = Bag::new();
        b.insert(1i32); b.insert(1); b.insert(2); b.insert(3);
        let rt = from_str::<Bag<i32>>(&to_string(&b).unwrap()).unwrap();
        assert_eq!(b, rt);
    }

    #[test]
    fn ser_hash_multimap() {
        let mut mm = HashMultiMap::new();
        mm.insert(1i32, 10i32); mm.insert(1, 11); mm.insert(2, 20);
        let rt = from_str::<HashMultiMap<i32, i32>>(&to_string(&mm).unwrap()).unwrap();
        assert_eq!(mm, rt);
    }

    #[test]
    fn ser_insertion_order_map() {
        let mut m = InsertionOrderMap::new();
        m.insert(1i32, 100i32); m.insert(2, 200); m.insert(3, 300);
        let json = to_string(&m).unwrap();
        let rt = from_str::<InsertionOrderMap<i32, i32>>(&json).unwrap();
        assert_eq!(m, rt);
        // Insertion order must survive the round-trip.
        let orig_keys: Vec<_> = m.keys().copied().collect();
        let rt_keys: Vec<_> = rt.keys().copied().collect();
        assert_eq!(orig_keys, rt_keys);
    }

    #[test]
    fn ser_bimap() {
        let mut bm = BiMap::new();
        bm.insert(1i32, 10i32); bm.insert(2, 20);
        let rt = from_str::<BiMap<i32, i32>>(&to_string(&bm).unwrap()).unwrap();
        assert_eq!(bm, rt);
        // Reverse direction must also work.
        assert_eq!(rt.get_by_value(&10), Some(&1));
    }

    #[test]
    fn ser_symmap() {
        let mut sm = SymMap::new();
        sm.insert(1i32, 10i32); sm.insert(2, 20);
        let rt = from_str::<SymMap<i32>>(&to_string(&sm).unwrap()).unwrap();
        assert_eq!(sm, rt);
        assert_eq!(rt.get(Direction::Backward, &10), Some(&1));
    }

    #[test]
    fn ser_trie() {
        let mut t: Trie<String, i32> = Trie::new();
        t.insert(&["a".to_owned(), "b".to_owned()], 1i32);
        t.insert(&["a".to_owned(), "c".to_owned()], 2);
        t.insert(&["x".to_owned()], 3);
        let json = to_string(&t).unwrap();
        let rt = from_str::<Trie<String, i32>>(&json).unwrap();
        assert_eq!(t, rt);
        assert_eq!(rt.get(&["a".to_owned(), "b".to_owned()]), Some(&1));
        assert_eq!(rt.get(&["x".to_owned()]), Some(&3));
    }
}
