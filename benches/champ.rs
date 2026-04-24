use criterion::{criterion_group, criterion_main, Bencher, Criterion};
use imbl::champ::ChampMap;
use imbl::hashmap::HashMap;
use std::borrow::Borrow;
use std::hash::Hash;
use std::hint::black_box;
use std::iter::FromIterator;
use std::sync::Arc;

mod utils;
use utils::*;

// Trait to abstract over ChampMap and imbl HashMap for side-by-side comparison.
trait BenchMap<K, V>: Clone + FromIterator<(K, V)>
where
    K: Clone + Hash + Eq,
    V: Clone,
{
    type Iter<'a>: Iterator<Item = (&'a K, &'a V)>
    where
        Self: 'a,
        K: 'a,
        V: 'a;

    fn new() -> Self;
    fn insert_mut(&mut self, k: K, v: V);
    fn insert_clone(&self, k: K, v: V) -> Self;
    fn remove_mut(&mut self, k: &K);
    fn remove_clone(&self, k: &K) -> Self;
    fn get<Q>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized;
    fn iter(&self) -> Self::Iter<'_>;
}

// --- ChampMap implementation ---

impl<K, V> BenchMap<K, V> for ChampMap<K, V>
where
    K: Clone + Hash + Eq,
    V: Clone,
{
    type Iter<'a>
        = imbl::champ::Iter<'a, K, V>
    where
        K: 'a,
        V: 'a;

    fn new() -> Self {
        ChampMap::new()
    }

    fn insert_mut(&mut self, k: K, v: V) {
        self.insert_mut_hashed(k, v);
    }

    fn insert_clone(&self, k: K, v: V) -> Self {
        self.update(k, v)
    }

    fn remove_mut(&mut self, k: &K) {
        self.remove_mut(k);
    }

    fn remove_clone(&self, k: &K) -> Self {
        self.remove_persistent(k)
    }

    fn get<Q>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get(k)
    }

    fn iter(&self) -> Self::Iter<'_> {
        self.iter()
    }
}

// --- imbl HashMap implementation ---

impl<K, V> BenchMap<K, V> for HashMap<K, V>
where
    K: Clone + Hash + Eq,
    V: Clone,
{
    type Iter<'a>
        = imbl::hashmap::Iter<'a, K, V, imbl::shared_ptr::DefaultSharedPtr>
    where
        K: 'a,
        V: 'a;

    fn new() -> Self {
        HashMap::new()
    }

    fn insert_mut(&mut self, k: K, v: V) {
        self.insert(k, v);
    }

    fn insert_clone(&self, k: K, v: V) -> Self {
        self.update(k, v)
    }

    fn remove_mut(&mut self, k: &K) {
        self.remove(k);
    }

    fn remove_clone(&self, k: &K) -> Self {
        self.without(k)
    }

    fn get<Q>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get(k)
    }

    fn iter(&self) -> Self::Iter<'_> {
        self.iter()
    }
}

// ---------------------------------------------------------------------------
// Generic benchmark functions
// ---------------------------------------------------------------------------

fn bench_lookup<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    let order = reorder(&keys);
    let m: M = keys.into_iter().zip(values).collect();
    b.iter(|| {
        for k in &order {
            black_box(m.get(k));
        }
    })
}

fn bench_lookup_ne<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size * 2);
    let values = V::generate(size);
    let order = reorder(&keys[size..]);
    let m: M = keys.into_iter().zip(values).collect();
    b.iter(|| {
        for k in &order {
            black_box(m.get(k));
        }
    })
}

fn bench_insert<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    b.iter(|| {
        let mut m = M::new();
        for (k, v) in keys.clone().into_iter().zip(values.clone()) {
            m = m.insert_clone(k, v);
        }
        m
    })
}

fn bench_insert_mut<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    b.iter(|| {
        let mut m = M::new();
        for (k, v) in keys.clone().into_iter().zip(values.clone()) {
            m.insert_mut(k, v);
        }
        m
    })
}

fn bench_remove<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    let order = reorder(&keys);
    let map: M = keys.into_iter().zip(values).collect();
    b.iter(|| {
        let mut m = map.clone();
        for k in &order {
            m = m.remove_clone(k);
        }
        m
    })
}

fn bench_remove_mut<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    let order = reorder(&keys);
    let map: M = keys.into_iter().zip(values).collect();
    b.iter(|| {
        let mut m = map.clone();
        for k in &order {
            m.remove_mut(k);
        }
        m
    })
}

fn bench_insert_once<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    let korder = reorder(&keys);
    let vorder = reorder(&values);
    let m: M = keys.clone().into_iter().zip(values).collect();
    b.iter(|| {
        for (k, v) in korder.iter().zip(vorder.iter()).take(100) {
            black_box(m.insert_clone(k.clone(), v.clone()));
        }
    })
}

fn bench_remove_once<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    let order = reorder(&keys);
    let map: M = keys.clone().into_iter().zip(values).collect();
    b.iter(|| {
        for k in order.iter().take(100) {
            black_box(map.remove_clone(k));
        }
    })
}

fn bench_iter<M, K, V>(b: &mut Bencher, size: usize)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let keys = K::generate(size);
    let values = V::generate(size);
    let m: M = keys.into_iter().zip(values).collect();
    b.iter(|| {
        for p in m.iter() {
            black_box(p);
        }
    })
}

// ---------------------------------------------------------------------------
// Benchmark groups — side-by-side CHAMP vs SIMD HAMT
// ---------------------------------------------------------------------------

fn bench_group<M, K, V>(c: &mut Criterion, group_name: &str)
where
    M: BenchMap<K, V>,
    K: TestData,
    V: TestData,
{
    let mut group = c.benchmark_group(group_name);

    for size in &[100, 1000, 10000, 100000] {
        group.bench_function(format!("lookup_{size}"), |b| {
            bench_lookup::<M, K, V>(b, *size)
        });
    }

    for size in &[10000, 100000] {
        group.bench_function(format!("lookup_ne_{size}"), |b| {
            bench_lookup_ne::<M, K, V>(b, *size)
        });
    }

    for size in &[100, 1000, 10000, 100000] {
        group.bench_function(format!("insert_mut_{size}"), |b| {
            bench_insert_mut::<M, K, V>(b, *size)
        });
    }

    for size in &[100, 1000, 10000] {
        group.bench_function(format!("remove_mut_{size}"), |b| {
            bench_remove_mut::<M, K, V>(b, *size)
        });
    }

    for size in &[1000, 10000, 100000] {
        group.bench_function(format!("iter_{size}"), |b| {
            bench_iter::<M, K, V>(b, *size)
        });
    }

    for size in &[100, 1000, 10000] {
        group.bench_function(format!("insert_{size}"), |b| {
            bench_insert::<M, K, V>(b, *size)
        });
        group.bench_function(format!("remove_{size}"), |b| {
            bench_remove::<M, K, V>(b, *size)
        });
    }

    for size in &[100, 1000, 10000, 100000] {
        group.bench_function(format!("insert_once_{size}"), |b| {
            bench_insert_once::<M, K, V>(b, *size)
        });
        group.bench_function(format!("remove_once_{size}"), |b| {
            bench_remove_once::<M, K, V>(b, *size)
        });
    }

    group.finish();
}

fn champ_benches(c: &mut Criterion) {
    // CHAMP
    bench_group::<ChampMap<i64, i64>, i64, i64>(c, "champ_i64");
    bench_group::<ChampMap<Arc<String>, Arc<String>>, Arc<String>, Arc<String>>(c, "champ_str");
    // SIMD HAMT (imbl HashMap) for direct comparison
    bench_group::<HashMap<i64, i64>, i64, i64>(c, "hamt_i64");
    bench_group::<HashMap<Arc<String>, Arc<String>>, Arc<String>, Arc<String>>(c, "hamt_str");
}

criterion_group!(benches, champ_benches);
criterion_main!(benches);
