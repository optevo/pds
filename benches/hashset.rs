use criterion::{criterion_group, criterion_main, Bencher, Criterion};
use pds::HashSet;
use std::collections::HashSet as StdHashSet;
use std::hash::Hash;
use std::hint::black_box;
use std::iter::FromIterator;
use std::sync::Arc;

mod utils;
use utils::*;

// Trait to abstract over different set implementations
trait BenchSet<A>: Clone + FromIterator<A>
where
    A: Clone + Hash + Eq,
{
    const IMMUTABLE: bool = true;

    fn new() -> Self;
    fn insert(&mut self, a: A) -> bool;
    fn insert_clone(&self, a: A) -> Self;
    fn remove(&mut self, a: &A) -> bool;
    fn remove_clone(&self, a: &A) -> Self;
    fn contains(&self, a: &A) -> bool;
    fn len(&self) -> usize;
    fn iter_count(&self) -> usize;
}

impl<A> BenchSet<A> for HashSet<A>
where
    A: Clone + Hash + Eq,
{
    fn new() -> Self {
        HashSet::new()
    }
    fn insert(&mut self, a: A) -> bool {
        let had = self.contains(&a);
        HashSet::insert(self, a);
        !had
    }
    fn insert_clone(&self, a: A) -> Self {
        self.update(a)
    }
    fn remove(&mut self, a: &A) -> bool {
        HashSet::remove(self, a).is_some()
    }
    fn remove_clone(&self, a: &A) -> Self {
        self.without(a)
    }
    fn contains(&self, a: &A) -> bool {
        HashSet::contains(self, a)
    }
    fn len(&self) -> usize {
        HashSet::len(self)
    }
    fn iter_count(&self) -> usize {
        self.iter().count()
    }
}

impl<A> BenchSet<A> for StdHashSet<A>
where
    A: Clone + Hash + Eq,
{
    const IMMUTABLE: bool = false;

    fn new() -> Self {
        StdHashSet::new()
    }
    fn insert(&mut self, a: A) -> bool {
        StdHashSet::insert(self, a)
    }
    fn insert_clone(&self, a: A) -> Self {
        let mut ret = self.clone();
        ret.insert(a);
        ret
    }
    fn remove(&mut self, a: &A) -> bool {
        StdHashSet::remove(self, a)
    }
    fn remove_clone(&self, a: &A) -> Self {
        let mut ret = self.clone();
        ret.remove(a);
        ret
    }
    fn contains(&self, a: &A) -> bool {
        StdHashSet::contains(self, a)
    }
    fn len(&self) -> usize {
        StdHashSet::len(self)
    }
    fn iter_count(&self) -> usize {
        self.iter().count()
    }
}

fn bench_lookup<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchSet<A>,
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
    S: BenchSet<A>,
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
    S: BenchSet<A>,
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
    S: BenchSet<A>,
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

fn bench_iter<S, A>(b: &mut Bencher, size: usize)
where
    S: BenchSet<A>,
    A: TestData,
{
    let values = A::generate(size);
    let s: S = values.into_iter().collect();
    b.iter(|| {
        black_box(s.iter_count());
    })
}

fn bench_union<A>(b: &mut Bencher, size: usize)
where
    A: TestData,
{
    let a_vals = A::generate(size);
    let b_vals = A::generate(size);
    let set_a: HashSet<A> = a_vals.into_iter().collect();
    let set_b: HashSet<A> = b_vals.into_iter().collect();
    b.iter(|| {
        black_box(set_a.clone().union(set_b.clone()));
    })
}

fn bench_intersection<A>(b: &mut Bencher, size: usize)
where
    A: TestData,
{
    let a_vals = A::generate(size);
    let b_vals = A::generate(size);
    let set_a: HashSet<A> = a_vals.into_iter().collect();
    let set_b: HashSet<A> = b_vals.into_iter().collect();
    b.iter(|| {
        black_box(set_a.clone().intersection(set_b.clone()));
    })
}

fn bench_difference<A>(b: &mut Bencher, size: usize)
where
    A: TestData,
{
    let a_vals = A::generate(size);
    let b_vals = A::generate(size);
    let set_a: HashSet<A> = a_vals.into_iter().collect();
    let set_b: HashSet<A> = b_vals.into_iter().collect();
    b.iter(|| {
        black_box(set_a.clone().difference(set_b.clone()));
    })
}

fn bench_group<S, A>(c: &mut Criterion, group_name: &str)
where
    S: BenchSet<A>,
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

    for size in &[1000, 10000, 100000] {
        group.bench_function(format!("iter_{}", size), |b| {
            bench_iter::<S, A>(b, *size)
        });
    }

    if S::IMMUTABLE {
        for size in &[100, 1000, 10000] {
            group.bench_function(format!("insert_{}", size), |b| {
                bench_insert::<S, A>(b, *size)
            });
        }
    }

    group.finish();
}

fn bench_set_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("hashset_ops_i64");
    for size in &[100, 1000, 10000] {
        group.bench_function(format!("union_{}", size), |b| {
            bench_union::<i64>(b, *size)
        });
        group.bench_function(format!("intersection_{}", size), |b| {
            bench_intersection::<i64>(b, *size)
        });
        group.bench_function(format!("difference_{}", size), |b| {
            bench_difference::<i64>(b, *size)
        });
    }
    group.finish();
}

fn hashset_benches(c: &mut Criterion) {
    bench_group::<HashSet<i64>, i64>(c, "hashset_i64");
    bench_group::<HashSet<Arc<String>>, Arc<String>>(c, "hashset_str");
    bench_set_ops(c);

    if std::env::var("BENCH_STD").is_ok() {
        bench_group::<StdHashSet<i64>, i64>(c, "stdhashset_i64");
        bench_group::<StdHashSet<Arc<String>>, Arc<String>>(c, "stdhashset_str");
    }
}

criterion_group!(benches, hashset_benches);
criterion_main!(benches);
