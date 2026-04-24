#![no_main]

use std::collections::HashMap as NatMap;
use std::fmt::Debug;
use std::iter::FromIterator;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use pds::HashMap;

#[derive(Arbitrary, Debug)]
enum Action<K, V> {
    Insert(K, V),
    Remove(K),
    Get(K),
    Union(Vec<(K, V)>),
    SymmetricDifference(Vec<(K, V)>),
    Intersection(Vec<(K, V)>),
}

fuzz_target!(|actions: Vec<Action<u32, u32>>| {
    let mut map = HashMap::new();
    let mut nat = NatMap::new();
    for action in actions {
        match action {
            Action::Insert(key, value) => {
                nat.insert(key, value);
                map.insert(key, value);
            }
            Action::Remove(key) => {
                nat.remove(&key);
                map.remove(&key);
            }
            Action::Get(key) => {
                assert_eq!(nat.get(&key), map.get(&key));
            }
            Action::Union(pairs) => {
                let other: HashMap<_, _> = pairs.iter().cloned().collect();
                let other_nat: NatMap<_, _> = pairs.into_iter().collect();
                let merged = map.clone().union(other);
                let mut merged_nat = nat.clone();
                // pds union: right side wins on conflict
                for (k, v) in &other_nat {
                    merged_nat.insert(*k, *v);
                }
                assert_eq!(merged.len(), merged_nat.len());
                for (k, v) in &merged_nat {
                    assert_eq!(merged.get(k), Some(v));
                }
            }
            Action::SymmetricDifference(pairs) => {
                let other: HashMap<_, _> = pairs.iter().cloned().collect();
                let other_nat: NatMap<_, _> = pairs.into_iter().collect();
                let diff = map.clone().symmetric_difference(other);
                // Symmetric difference: keys in either but not both
                let mut diff_nat = NatMap::new();
                for (k, v) in &nat {
                    if !other_nat.contains_key(k) {
                        diff_nat.insert(*k, *v);
                    }
                }
                for (k, v) in &other_nat {
                    if !nat.contains_key(k) {
                        diff_nat.insert(*k, *v);
                    }
                }
                assert_eq!(diff.len(), diff_nat.len());
                for (k, v) in &diff_nat {
                    assert_eq!(diff.get(k), Some(v));
                }
            }
            Action::Intersection(pairs) => {
                let other: HashMap<_, _> = pairs.iter().cloned().collect();
                let other_nat: NatMap<_, _> = pairs.into_iter().collect();
                let inter = map.clone().intersection(other);
                let inter_nat: NatMap<_, _> = nat
                    .iter()
                    .filter(|(k, _)| other_nat.contains_key(k))
                    .map(|(k, v)| (*k, *v))
                    .collect();
                assert_eq!(inter.len(), inter_nat.len());
                for (k, v) in &inter_nat {
                    assert_eq!(inter.get(k), Some(v));
                }
            }
        }
        assert_eq!(nat.len(), map.len());
    }
    assert_eq!(HashMap::<_, _>::from(nat.clone()), map);
    assert_eq!(NatMap::from_iter(map.iter().map(|(k, v)| (*k, *v))), nat);
    assert_eq!(map.iter().count(), nat.len());
    assert_eq!(map.into_iter().count(), nat.len());
});
