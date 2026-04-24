#![no_main]

use std::collections::BTreeMap as NatMap;
use std::fmt::Debug;
use std::ops::Bound;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use pds::OrdMap;

#[derive(Arbitrary, Debug)]
enum NextAction {
    Fwd,
    Bwd,
    BwdFwd,
    FwdBwd,
}

#[derive(Arbitrary, Debug)]
enum Action<K: Clone + PartialOrd, V> {
    Insert(K, V),
    Remove(K),
    Get(K),
    Range((Bound<K>, Bound<K>), NextAction),
}

fuzz_target!(|actions: Vec<Action<u32, u32>>| {
    let mut map = OrdMap::new();
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
            Action::Range(range, na) => {
                assert_eq!(
                    map.get_min().map(|(k, v)| (*k, *v)),
                    nat.first_key_value().map(|(k, v)| (*k, *v))
                );
                assert_eq!(
                    map.get_max().map(|(k, v)| (*k, *v)),
                    nat.last_key_value().map(|(k, v)| (*k, *v))
                );
                match (range.0, range.1) {
                    (Bound::Included(v) | Bound::Excluded(v), ..)
                    | (.., Bound::Included(v) | Bound::Excluded(v)) => {
                        assert_eq!(
                            map.get_next(&v).map(|(k, v)| (*k, *v)),
                            nat.range(v..).next().map(|(k, v)| (*k, *v))
                        );
                        assert_eq!(
                            map.get_prev(&v).map(|(k, v)| (*k, *v)),
                            nat.range(..=v).last().map(|(k, v)| (*k, *v))
                        );
                        assert_eq!(map.get(&v), nat.get(&v));
                    }
                    _ => {}
                }
                // std BTreeMap panics if the range end isn't >= range start
                // but OrdMap returns an empty iterator
                let valid_std = match (range.0, range.1) {
                    (Bound::Included(v), Bound::Included(w)) => v <= w,
                    (Bound::Excluded(v), Bound::Excluded(w))
                    | (Bound::Included(v), Bound::Excluded(w))
                    | (Bound::Excluded(v), Bound::Included(w)) => v < w,
                    _ => true,
                };
                if !valid_std {
                    assert_eq!(map.range(range).count(), 0);
                    assert_eq!(map.range(range).rev().count(), 0);
                    continue;
                }

                let mut map_it = map.range(range.clone());
                let mut nat_it = nat.range(range);
                loop {
                    let (a, b) = match na {
                        NextAction::Fwd => (map_it.next(), nat_it.next()),
                        NextAction::Bwd => (map_it.next_back(), nat_it.next_back()),
                        NextAction::BwdFwd => {
                            let ma = map_it.next_back();
                            let na = nat_it.next_back();
                            assert_eq!(
                                ma.map(|(k, v)| (*k, *v)),
                                na.map(|(k, v)| (*k, *v))
                            );
                            (map_it.next(), nat_it.next())
                        }
                        NextAction::FwdBwd => {
                            let ma = map_it.next();
                            let na = nat_it.next();
                            assert_eq!(
                                ma.map(|(k, v)| (*k, *v)),
                                na.map(|(k, v)| (*k, *v))
                            );
                            (map_it.next_back(), nat_it.next_back())
                        }
                    };
                    assert_eq!(
                        a.map(|(k, v)| (*k, *v)),
                        b.map(|(k, v)| (*k, *v))
                    );
                    if a.is_none() {
                        assert!(map_it.next().is_none());
                        assert!(map_it.next_back().is_none());
                        break;
                    }
                }
            }
        }
        assert_eq!(nat.len(), map.len());
    }
    assert_eq!(
        OrdMap::<_, _>::from(nat.clone()),
        map
    );
    for ((ak, av), (bk, bv)) in map.iter().zip(&nat) {
        assert_eq!(ak, bk);
        assert_eq!(av, bv);
    }
    for ((ak, av), (bk, bv)) in map.iter().rev().zip(nat.iter().rev()) {
        assert_eq!(ak, bk);
        assert_eq!(av, bv);
    }
    for ((ak, av), (bk, bv)) in map.into_iter().zip(nat) {
        assert_eq!(ak, bk);
        assert_eq!(av, bv);
    }
});
