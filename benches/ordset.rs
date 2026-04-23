use criterion::{criterion_group, criterion_main, Bencher, Criterion};
use imbl::OrdSet;
use std::collections::BTreeSet;
use std::hint::black_box;
use std::iter::FromIterator;
use std::sync::Arc;

mod utils;
use utils::*;

// Trait to abstract over different ordered set implementations
trait BenchOrdSet<A>: Clone + FromIterator<A>
where
    A: Clone + Ord,
{
    const IMMUTABLE: bool = true;

    fn new() -> Self;
    fn insert(&mut self, a: A);
    fn insert_clone(&self, a: A) -> Self;
    fn remove(&mut self, a: &A);
    fn remove_clone(&self, a: &A) -> Self;
    fn contains(&self, a: &A) -> bool;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn iter_count(&self) -> usize;
    fn without_min(&self) -> Self;
    fn without_max(&self) -> Self;
}

impl<A> BenchOrdSet<A> for OrdSet<A>
where
    A: Clone + Ord,
{
    fn new() -> Self {
        OrdSet::new()
    }
    fn insert(&mut self, a: A) {
        OrdSet::insert(self, a);
    }
    fn insert_clone(&self, a: A) -> Self {
        self.update(a)
    }
    fn remove(&mut self, a: &A) {
        OrdSet::remove(self, a);
    }
    fn remove_clone(&self, a: &A) -> Self {
        self.without(a)
    }
    fn contains(&self, a: &A) -> bool {
        OrdSet::contains(self, a)
    }
    fn len(&self) -> usize {
        OrdSet::len(self)
    }
    fn is_empty(&self) -> bool {
        OrdSet::is_empty(self)
    }
    fn iter_count(&self) -> usize {
        self.iter().count()
    }
    fn without_min(&self) -> Self {
        OrdSet::without_min(self).1
    }
    fn without_max(&self) -> Self {
        OrdSet::without_max(self).1
    }
}

impl<A> BenchOrdSet<A> for BTreeSet<A>
where
    A: Clone + Ord,
{
    const IMMUTABLE: bool = false;

    fn new() -> Self {
        BTreeSet::new()
    }
    fn insert(&mut self, a: A) {
        BTreeSet::insert(self, a);
    }
    fn insert_clone(&self, a: A) -> Self {
        let mut ret = self.clone();
        ret.insert(a);
        ret
    }
    fn remove(&mut self, a: &A) {
        BTreeSet::remove(self, a);
    }
    fn remove_clone(&self, a: &A) -> Self {
        let mut ret = self.clone();
        ret.remove(a);
        ret
    }
    fn contains(&self, a: &A) -> bool {
        BTreeSet::contains(self, a)
    }
    fn len(&self) -> usize {
        BTreeSet::len(self)
    }
    fn is_empty(&self) -> bool {
        BTreeSet::is_empty(self)
    }
    fn iter_count(&self) -> usize {
        self.iter().count()
    }
    fn without_min(&self) -> Self {
        let mut ret = self.clone();
        if let Some(min) = ret.iter().next().cloned() {
            ret.remove(&min);
        }
        ret
    }
    fn without_max(&self) -> Self {
        let mut ret = self.clone();
        if let Some(max) = ret.iter().next_back().cloned() {
            ret.remove(&max);
        }
        ret
    }
}

fn bench_lookup<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    let order = reorder(&values);
    let s: S = values.into_iter().collect();
    b.iter(|| {
        for v in &order {
            black_box(s.contains(v));
        }
    })
}

fn bench_insert<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    b.iter(|| {
        let mut s = S::new();
        for v in values.clone() {
            s = s.insert_clone(v);
        }
        s
    })
}

fn bench_insert_mut<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    b.iter(|| {
        let mut s = S::new();
        for v in values.clone() {
            s.insert(v);
        }
        s
    })
}

fn bench_remove_mut<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    let order = reorder(&values);
    let set: S = values.into_iter().collect();
    b.iter(|| {
        let mut s = set.clone();
        for v in &order {
            s.remove(v);
        }
        s
    })
}

fn bench_remove_min<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    let set: S = values.into_iter().collect();
    b.iter(|| {
        let mut s = set.clone();
        assert!(!s.is_empty());
        for _ in 0..size {
            s = s.without_min();
        }
        assert!(s.is_empty());
        s
    })
}

fn bench_remove_max<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    let set: S = values.into_iter().collect();
    b.iter(|| {
        let mut s = set.clone();
        assert!(!s.is_empty());
        for _ in 0..size {
            s = s.without_max();
        }
        assert!(s.is_empty());
        s
    })
}

fn bench_iter<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    let s: S = values.into_iter().collect();
    b.iter(|| {
        black_box(s.iter_count());
    })
}

fn bench_group<S, A>(c: &mut Criterion, group_name: &str)
where
    S: BenchOrdSet<A>,
    A: TestData,
{
    let mut group = c.benchmark_group(group_name);

    for size in &[100, 1000, 10000, 100000] {
        group.bench_function(format!("lookup_{}", size), |b| {
            bench_lookup::<S, A>(b, *size)
        });
    }

    for size in &[100, 1000, 10000, 100000] {
        group.bench_function(format!("insert_mut_{}", size), |b| {
            bench_insert_mut::<S, A>(b, *size)
        });
    }

    for size in &[100, 1000, 10000] {
        group.bench_function(format!("remove_mut_{}", size), |b| {
            bench_remove_mut::<S, A>(b, *size)
        });
    }

    for size in &[1000, 10000] {
        group.bench_function(format!("iter_{}", size), |b| {
            bench_iter::<S, A>(b, *size)
        });
    }

    if S::IMMUTABLE {
        for size in &[100, 1000, 10000, 100000] {
            group.bench_function(format!("insert_{}", size), |b| {
                bench_insert::<S, A>(b, *size)
            });
        }

        for size in &[1000] {
            group.bench_function(format!("remove_min_{}", size), |b| {
                bench_remove_min::<S, A>(b, *size)
            });
            group.bench_function(format!("remove_max_{}", size), |b| {
                bench_remove_max::<S, A>(b, *size)
            });
        }
    }

    group.finish();
}

fn ordset_benches(c: &mut Criterion) {
    bench_group::<OrdSet<i64>, i64>(c, "ordset_i64");
    bench_group::<OrdSet<Arc<String>>, Arc<String>>(c, "ordset_str");

    if std::env::var("BENCH_STD").is_ok() {
        bench_group::<BTreeSet<i64>, i64>(c, "btreeset_i64");
        bench_group::<BTreeSet<Arc<String>>, Arc<String>>(c, "btreeset_str");
    }
}

criterion_group!(benches, ordset_benches);
criterion_main!(benches);
