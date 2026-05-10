use criterion::{criterion_group, criterion_main, Bencher, Criterion};
use pds::vector::Vector;
use rand::seq::SliceRandom;
use std::collections::VecDeque;
use std::hint::black_box;
use std::iter::FromIterator;

mod utils;

// Trait to abstract over different vector-like implementations
trait BenchVector<T>: Clone + FromIterator<T>
where
    T: Clone,
{
    type Iter<'a>: Iterator<Item = &'a T>
    where
        Self: 'a,
        T: 'a;

    fn new() -> Self;
    fn push_front(&mut self, value: T);
    fn push_back(&mut self, value: T);
    fn pop_front(&mut self) -> Option<T>;
    fn pop_back(&mut self) -> Option<T>;
    fn get(&self, index: usize) -> Option<&T>;
    fn iter(&self) -> Self::Iter<'_>;

    // Only some implementations support these
    fn split_off(&mut self, at: usize) -> Self;
    fn append(&mut self, other: Self);
    fn sort(&mut self)
    where
        T: Ord;

    // Vector-specific features
    fn supports_focus() -> bool {
        false
    }
    fn focus(&self) -> Option<VectorFocus<'_, T>> {
        None
    }
    fn focus_mut(&mut self) -> Option<VectorFocusMut<'_, T>> {
        None
    }
}

// Wrapper types for Vector's focus feature
struct VectorFocus<'a, T> {
    focus: pds::vector::Focus<'a, T, pds::shared_ptr::DefaultSharedPtr>,
}

impl<'a, T> VectorFocus<'a, T> {
    fn get(&mut self, index: usize) -> Option<&T> {
        self.focus.get(index)
    }
}

struct VectorFocusMut<'a, T> {
    focus: pds::vector::FocusMut<'a, T, pds::shared_ptr::DefaultSharedPtr>,
}

impl<'a, T: Clone> VectorFocusMut<'a, T> {
    fn get(&mut self, index: usize) -> Option<&T> {
        self.focus.get(index)
    }
}

// Implementation for pds::Vector
impl<T: Clone> BenchVector<T> for Vector<T> {
    type Iter<'a>
        = pds::vector::Iter<'a, T, pds::shared_ptr::DefaultSharedPtr>
    where
        T: 'a;

    fn new() -> Self {
        Vector::new()
    }

    fn push_front(&mut self, value: T) {
        self.push_front(value);
    }

    fn push_back(&mut self, value: T) {
        self.push_back(value);
    }

    fn pop_front(&mut self) -> Option<T> {
        self.pop_front()
    }

    fn pop_back(&mut self) -> Option<T> {
        self.pop_back()
    }

    fn get(&self, index: usize) -> Option<&T> {
        self.get(index)
    }

    fn iter(&self) -> Self::Iter<'_> {
        self.iter()
    }

    fn split_off(&mut self, at: usize) -> Self {
        self.split_off(at)
    }

    fn append(&mut self, other: Self) {
        self.append(other);
    }

    fn sort(&mut self)
    where
        T: Ord,
    {
        self.sort();
    }

    fn supports_focus() -> bool {
        true
    }

    fn focus(&self) -> Option<VectorFocus<'_, T>> {
        Some(VectorFocus {
            focus: self.focus(),
        })
    }

    fn focus_mut(&mut self) -> Option<VectorFocusMut<'_, T>> {
        Some(VectorFocusMut {
            focus: self.focus_mut(),
        })
    }
}

// Implementation for std::collections::VecDeque
impl<T: Clone> BenchVector<T> for VecDeque<T> {
    type Iter<'a>
        = std::collections::vec_deque::Iter<'a, T>
    where
        T: 'a;

    fn new() -> Self {
        VecDeque::new()
    }

    fn push_front(&mut self, value: T) {
        self.push_front(value);
    }

    fn push_back(&mut self, value: T) {
        self.push_back(value);
    }

    fn pop_front(&mut self) -> Option<T> {
        self.pop_front()
    }

    fn pop_back(&mut self) -> Option<T> {
        self.pop_back()
    }

    fn get(&self, index: usize) -> Option<&T> {
        self.get(index)
    }

    fn iter(&self) -> Self::Iter<'_> {
        self.iter()
    }

    fn split_off(&mut self, at: usize) -> Self {
        self.split_off(at)
    }

    fn append(&mut self, mut other: Self) {
        self.append(&mut other);
    }

    fn sort(&mut self)
    where
        T: Ord,
    {
        self.make_contiguous().sort();
    }
}

// Generic benchmark functions
fn bench_sort_sorted<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    b.iter(|| {
        let mut v: V = (0..size).collect();
        v.sort();
        black_box(v);
    });
}

fn bench_sort_reverse<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    b.iter(|| {
        let mut v: V = (0..size).rev().collect();
        v.sort();
        black_box(v);
    });
}

fn bench_sort_shuffled<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let mut rng = rand::rng();
    b.iter(|| {
        let mut v: Vec<_> = (0..size).collect();
        v.shuffle(&mut rng);
        let mut v: V = v.into_iter().collect();
        v.sort();
        black_box(v);
    });
}

fn bench_push_front<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    b.iter(|| {
        let mut v = V::new();
        for i in 0..size {
            v.push_front(i);
        }
        black_box(v);
    });
}

fn bench_push_back<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    b.iter(|| {
        let mut v = V::new();
        for i in 0..size {
            v.push_back(i);
        }
        black_box(v);
    });
}

fn bench_pop_front<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v: V = (0..size).collect();
    b.iter(|| {
        let mut v = v.clone();
        for _ in 0..size {
            v.pop_front();
        }
        black_box(v);
    });
}

fn bench_pop_back<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v: V = (0..size).collect();
    b.iter(|| {
        let mut v = v.clone();
        for _ in 0..size {
            v.pop_back();
        }
        black_box(v);
    });
}

fn bench_split<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v: V = (0..size).collect();
    b.iter(|| {
        let mut v = v.clone();
        black_box(v.split_off(size / 2));
    });
}

fn bench_append<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v1: V = (0..size / 2).collect();
    let v2: V = (size / 2..size).collect();
    b.iter(|| {
        let mut v = v1.clone();
        v.append(v2.clone());
        black_box(v);
    });
}

fn bench_iter<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v: V = (0..size).collect();
    b.iter(|| {
        for item in v.iter() {
            black_box(item);
        }
    });
}

fn bench_get_seq<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v: V = (0..size).collect();
    b.iter(|| {
        for i in 0..size {
            black_box(v.get(i));
        }
    });
}

fn bench_get_seq_focus<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    if !V::supports_focus() {
        return;
    }
    let v: V = (0..size).collect();
    if let Some(mut focus) = v.focus() {
        b.iter(|| {
            for i in 0..size {
                black_box(focus.get(i));
            }
        });
    }
}

fn bench_get_seq_focus_mut<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    if !V::supports_focus() {
        return;
    }
    let v: V = (0..size).collect();
    b.iter(|| {
        let mut v = v.clone();
        if let Some(mut focus) = v.focus_mut() {
            for i in 0..size {
                black_box(focus.get(i));
            }
        }
    });
}

fn bench_iter_max<V: BenchVector<usize>>(b: &mut Bencher, size: usize) {
    let v: V = (0..size).collect();
    b.iter(|| black_box(v.iter().max()));
}

// Helper function to run sort benchmarks
fn bench_sort_group<V: BenchVector<usize>>(c: &mut Criterion, group_name: &str) {
    let mut group = c.benchmark_group(format!("{}_sort", group_name));

    for size in &[500, 1000, 1500, 2000, 2500] {
        group.bench_function(format!("sorted_{}", size), |b| {
            bench_sort_sorted::<V>(b, *size)
        });

        group.bench_function(format!("reverse_{}", size), |b| {
            bench_sort_reverse::<V>(b, *size)
        });

        group.bench_function(format!("shuffled_{}", size), |b| {
            bench_sort_shuffled::<V>(b, *size)
        });
    }

    group.finish();
}

// Helper function to run vector operation benchmarks
fn bench_ops_group<V: BenchVector<usize>>(c: &mut Criterion, group_name: &str) {
    let mut group = c.benchmark_group(format!("{}_ops", group_name));

    for size in &[100, 1000, 100000] {
        group.bench_function(format!("push_front_{}", size), |b| {
            bench_push_front::<V>(b, *size)
        });

        group.bench_function(format!("push_back_{}", size), |b| {
            bench_push_back::<V>(b, *size)
        });

        group.bench_function(format!("pop_front_{}", size), |b| {
            bench_pop_front::<V>(b, *size)
        });

        group.bench_function(format!("pop_back_{}", size), |b| {
            bench_pop_back::<V>(b, *size)
        });

        group.bench_function(format!("split_{}", size), |b| bench_split::<V>(b, *size));

        group.bench_function(format!("iter_{}", size), |b| bench_iter::<V>(b, *size));

        group.bench_function(format!("get_seq_{}", size), |b| {
            bench_get_seq::<V>(b, *size)
        });

        if <V as BenchVector<usize>>::supports_focus() {
            group.bench_function(format!("get_seq_focus_{}", size), |b| {
                bench_get_seq_focus::<V>(b, *size)
            });

            group.bench_function(format!("get_seq_focus_mut_{}", size), |b| {
                bench_get_seq_focus_mut::<V>(b, *size)
            });
        }
    }

    // Append has different sizes
    for size in &[10, 100, 1000, 10000, 100000] {
        group.bench_function(format!("append_{}", size), |b| bench_append::<V>(b, *size));
    }

    // Iterator max benchmarks
    for size in &[1000, 100000, 10000000] {
        group.bench_function(format!("iter_max_{}", size), |b| {
            bench_iter_max::<V>(b, *size)
        });
    }

    group.finish();
}

// Benchmark functions for each vector type
fn bench_vector(c: &mut Criterion) {
    bench_sort_group::<Vector<usize>>(c, "vector");
    bench_ops_group::<Vector<usize>>(c, "vector");
}

fn bench_vecdeque(c: &mut Criterion) {
    bench_sort_group::<VecDeque<usize>>(c, "vecdeque");
    bench_ops_group::<VecDeque<usize>>(c, "vecdeque");
}

// ---------------------------------------------------------------------------
// Range view (VectorRange / subrange) benchmarks
//
// Key differences from OrdSetRange: VectorRange::len is trivially O(1) (end -
// start, no scan). first/last are O(log n) at construction but O(1) after.
// iter() seeks in O(log n) via Focus::narrow. get(i) is O(log n).
//
// Comparisons:
//   subrange_len vs manual (end - start): both O(1) — should be identical
//   subrange_first vs vector.get(start): O(1) vs O(log n) — view wins
//   subrange_iter vs focus narrow:        both O(log n) seek — should match
//   subrange_get_seq vs vector.get_seq:   similar O(log n) per element
//
// Sizes: 1_000 and 10_000 entries; range = middle 50%.
// ---------------------------------------------------------------------------

fn bench_subrange_vector(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_subrange");

    for &n in &[1_000usize, 10_000usize] {
        let v: Vector<usize> = (0..n).collect();
        let start = n / 4;
        let end = 3 * n / 4; // middle 50%

        // Construction: O(log n) — two get() calls for first and last
        group.bench_function(format!("subrange_construct_{n}"), |b| {
            b.iter(|| black_box(v.subrange(start..end)))
        });
        // Baseline: cloning + slicing (what to_vector does)
        group.bench_function(format!("clone_slice_{n}"), |b| {
            b.iter(|| {
                let mut c = v.clone();
                black_box(c.slice(start..end))
            })
        });

        // len: O(1) — end minus start, no tree walk
        {
            let view = v.subrange(start..end);
            group.bench_function(format!("subrange_len_{n}"), |b| {
                b.iter(|| black_box(view.len()))
            });
        }

        // first / last: O(1) from cache vs O(log n) fresh get
        {
            let view = v.subrange(start..end);
            group.bench_function(format!("subrange_first_{n}"), |b| {
                b.iter(|| black_box(view.first()))
            });
            group.bench_function(format!("subrange_last_{n}"), |b| {
                b.iter(|| black_box(view.last()))
            });
        }
        group.bench_function(format!("vector_get_start_{n}"), |b| {
            b.iter(|| black_box(v.get(start)))
        });
        group.bench_function(format!("vector_get_end_{n}"), |b| {
            b.iter(|| black_box(v.get(end - 1)))
        });

        // iter: O(log n) positioning via Focus::narrow, then O(k) scan
        {
            let view = v.subrange(start..end);
            group.bench_function(format!("subrange_iter_{n}"), |b| {
                b.iter(|| black_box(view.iter().count()))
            });
        }
        // Baseline: naive skip+take on the full iterator
        group.bench_function(format!("iter_skip_take_{n}"), |b| {
            b.iter(|| black_box(v.iter().skip(start).take(end - start).count()))
        });

        // Sequential get via view vs via underlying vector
        {
            let view = v.subrange(start..end);
            let k = end - start;
            group.bench_function(format!("subrange_get_seq_{n}"), |b| {
                b.iter(|| {
                    let mut sum = 0usize;
                    for i in 0..k {
                        sum = sum.wrapping_add(*view.get(i).unwrap());
                    }
                    black_box(sum)
                })
            });
            group.bench_function(format!("vector_get_seq_{n}"), |b| {
                b.iter(|| {
                    let mut sum = 0usize;
                    for i in start..end {
                        sum = sum.wrapping_add(*v.get(i).unwrap());
                    }
                    black_box(sum)
                })
            });
        }

        // Chained subrange: O(log n) narrowing of an existing view
        {
            let view = v.subrange(start..end);
            let start2 = (end - start) / 4;
            let end2 = 3 * (end - start) / 4;
            group.bench_function(format!("subrange_chain_{n}"), |b| {
                b.iter(|| black_box(view.subrange(start2..end2)))
            });
        }
    }

    group.finish();
}

// Main benchmark entry point
fn vector_benches(c: &mut Criterion) {
    bench_vector(c);
    bench_subrange_vector(c);

    if std::env::var("BENCH_STD").is_ok() {
        bench_vecdeque(c);
    }
}

criterion_group!(benches, vector_benches);
criterion_main!(benches);
