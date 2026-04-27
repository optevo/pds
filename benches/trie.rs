/// Head-to-head benchmarks: old (flatten-and-rebuild) vs new (merge-walk / ptr_eq)
/// set-operation implementations for OrdTrie and Trie.
///
/// "Old" implementations are re-implemented inline using only the public API
/// (IntoIterator + insert / contains_path), exactly as the pre-DEC-038 code worked.
/// "New" implementations call the current library methods directly.
///
/// Three scenarios per operation:
///   overlapping — two tries share ~50% of paths (typical production case)
///   identical   — b is a clone of a; ptr_eq fast-path fires in new impl
///   disjoint    — no shared paths (worst case for merge-walk advantage)
///
/// Two sizes: 200 paths (depth 3) and 2 000 paths (depth 4).
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pds::ord_trie::OrdTrie;
use pds::trie::Trie;
use std::hint::black_box;
use std::time::Duration;

// ── test-data helpers ─────────────────────────────────────────────────────────

/// Generate `n` depth-`depth` paths by treating the index as a base-8 number.
/// Indices 0..8^depth give distinct paths; larger indices wrap (non-unique).
fn paths(n: usize, depth: usize) -> Vec<Vec<u32>> {
    (0..n)
        .map(|i| {
            let mut p = vec![0u32; depth];
            let mut x = i;
            for slot in p.iter_mut().rev() {
                *slot = (x % 8) as u32;
                x /= 8;
            }
            p
        })
        .collect()
}

fn build_ord_trie(ps: &[Vec<u32>]) -> OrdTrie<u32, u32> {
    let mut t = OrdTrie::new();
    for (i, p) in ps.iter().enumerate() {
        t.insert(p.as_slice(), i as u32);
    }
    t
}

fn build_trie(ps: &[Vec<u32>]) -> Trie<u32, u32> {
    let mut t = Trie::new();
    for (i, p) in ps.iter().enumerate() {
        t.insert(p.as_slice(), i as u32);
    }
    t
}

// ── old OrdTrie set-op implementations (pre-DEC-038 flatten-and-rebuild) ──────

fn old_ord_union(a: OrdTrie<u32, u32>, b: OrdTrie<u32, u32>) -> OrdTrie<u32, u32> {
    let mut result = a;
    for (path, value) in b {
        result.insert(&path, value);
    }
    result
}

fn old_ord_difference(a: OrdTrie<u32, u32>, b: &OrdTrie<u32, u32>) -> OrdTrie<u32, u32> {
    a.into_iter()
        .filter(|(path, _)| !b.contains_path(path.as_slice()))
        .collect()
}

fn old_ord_intersection(a: OrdTrie<u32, u32>, b: &OrdTrie<u32, u32>) -> OrdTrie<u32, u32> {
    a.into_iter()
        .filter(|(path, _)| b.contains_path(path.as_slice()))
        .collect()
}

fn old_ord_sym_diff(a: OrdTrie<u32, u32>, b: &OrdTrie<u32, u32>) -> OrdTrie<u32, u32> {
    let a_clone = a.clone();
    let a_diff: OrdTrie<u32, u32> = a
        .into_iter()
        .filter(|(path, _)| !b.contains_path(path.as_slice()))
        .collect();
    let b_diff: OrdTrie<u32, u32> = b
        .clone()
        .into_iter()
        .filter(|(path, _)| !a_clone.contains_path(path.as_slice()))
        .collect();
    old_ord_union(a_diff, b_diff)
}

// ── old Trie set-op implementations (pre-DEC-038, no ptr_eq check) ────────────

fn old_trie_union(mut a: Trie<u32, u32>, b: Trie<u32, u32>) -> Trie<u32, u32> {
    for (path, value) in b {
        a.insert(&path, value);
    }
    a
}

fn old_trie_difference(a: Trie<u32, u32>, b: &Trie<u32, u32>) -> Trie<u32, u32> {
    a.into_iter()
        .filter(|(path, _)| !b.contains_path(path.as_slice()))
        .collect()
}

#[allow(dead_code)]
fn old_trie_intersection(a: Trie<u32, u32>, b: &Trie<u32, u32>) -> Trie<u32, u32> {
    a.into_iter()
        .filter(|(path, _)| b.contains_path(path.as_slice()))
        .collect()
}

// ── OrdTrie benchmarks ────────────────────────────────────────────────────────

fn bench_ord_union(c: &mut Criterion) {
    let mut group = c.benchmark_group("ord_trie/union");
    group.measurement_time(Duration::from_secs(4));
    group.warm_up_time(Duration::from_secs(1));

    for &(n, depth) in &[(200usize, 3usize), (2_000, 4)] {
        let all = paths(n * 2, depth);
        let ps_a = &all[..n];
        let ps_b_over = &all[n / 2..n + n / 2]; // 50% overlap with a
        let ps_b_disj = &all[n..];

        let a_over = build_ord_trie(ps_a);
        let b_over = build_ord_trie(ps_b_over);
        let b_disj = build_ord_trie(ps_b_disj);

        // overlapping
        group.bench_with_input(BenchmarkId::new("old/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_union(a_over.clone(), b_over.clone())))
        });
        group.bench_with_input(BenchmarkId::new("new/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(a_over.clone().union(b_over.clone())))
        });

        // disjoint
        group.bench_with_input(BenchmarkId::new("old/disjoint", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_union(a_over.clone(), b_disj.clone())))
        });
        group.bench_with_input(BenchmarkId::new("new/disjoint", n), &n, |b, _| {
            b.iter(|| black_box(a_over.clone().union(b_disj.clone())))
        });

        // identical (ptr_eq fast-path fires in new)
        group.bench_with_input(BenchmarkId::new("old/identical", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_union(a_over.clone(), a_over.clone())))
        });
        group.bench_with_input(BenchmarkId::new("new/identical", n), &n, |b, _| {
            b.iter(|| black_box(a_over.clone().union(a_over.clone())))
        });
    }
    group.finish();
}

fn bench_ord_difference(c: &mut Criterion) {
    let mut group = c.benchmark_group("ord_trie/difference");
    group.measurement_time(Duration::from_secs(4));
    group.warm_up_time(Duration::from_secs(1));

    for &(n, depth) in &[(200usize, 3usize), (2_000, 4)] {
        let all = paths(n * 2, depth);
        let ps_a = &all[..n];
        let ps_b_over = &all[n / 2..n + n / 2];
        let ps_b_disj = &all[n..];

        let a = build_ord_trie(ps_a);
        let b_over = build_ord_trie(ps_b_over);
        let b_disj = build_ord_trie(ps_b_disj);

        group.bench_with_input(BenchmarkId::new("old/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_difference(a.clone(), &b_over)))
        });
        group.bench_with_input(BenchmarkId::new("new/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().difference(&b_over)))
        });

        group.bench_with_input(BenchmarkId::new("old/disjoint", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_difference(a.clone(), &b_disj)))
        });
        group.bench_with_input(BenchmarkId::new("new/disjoint", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().difference(&b_disj)))
        });

        group.bench_with_input(BenchmarkId::new("old/identical", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_difference(a.clone(), &a)))
        });
        group.bench_with_input(BenchmarkId::new("new/identical", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().difference(&a)))
        });
    }
    group.finish();
}

fn bench_ord_intersection(c: &mut Criterion) {
    let mut group = c.benchmark_group("ord_trie/intersection");
    group.measurement_time(Duration::from_secs(4));
    group.warm_up_time(Duration::from_secs(1));

    for &(n, depth) in &[(200usize, 3usize), (2_000, 4)] {
        let all = paths(n * 2, depth);
        let ps_a = &all[..n];
        let ps_b_over = &all[n / 2..n + n / 2];
        let ps_b_disj = &all[n..];

        let a = build_ord_trie(ps_a);
        let b_over = build_ord_trie(ps_b_over);
        let b_disj = build_ord_trie(ps_b_disj);

        group.bench_with_input(BenchmarkId::new("old/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_intersection(a.clone(), &b_over)))
        });
        group.bench_with_input(BenchmarkId::new("new/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().intersection(&b_over)))
        });

        group.bench_with_input(BenchmarkId::new("old/disjoint", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_intersection(a.clone(), &b_disj)))
        });
        group.bench_with_input(BenchmarkId::new("new/disjoint", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().intersection(&b_disj)))
        });

        group.bench_with_input(BenchmarkId::new("old/identical", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_intersection(a.clone(), &a)))
        });
        group.bench_with_input(BenchmarkId::new("new/identical", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().intersection(&a)))
        });
    }
    group.finish();
}

fn bench_ord_sym_diff(c: &mut Criterion) {
    let mut group = c.benchmark_group("ord_trie/symmetric_difference");
    group.measurement_time(Duration::from_secs(4));
    group.warm_up_time(Duration::from_secs(1));

    for &(n, depth) in &[(200usize, 3usize), (2_000, 4)] {
        let all = paths(n * 2, depth);
        let ps_a = &all[..n];
        let ps_b_over = &all[n / 2..n + n / 2];

        let a = build_ord_trie(ps_a);
        let b_over = build_ord_trie(ps_b_over);

        group.bench_with_input(BenchmarkId::new("old/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_sym_diff(a.clone(), &b_over)))
        });
        group.bench_with_input(BenchmarkId::new("new/overlapping", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().symmetric_difference(&b_over)))
        });

        group.bench_with_input(BenchmarkId::new("old/identical", n), &n, |b, _| {
            b.iter(|| black_box(old_ord_sym_diff(a.clone(), &a)))
        });
        group.bench_with_input(BenchmarkId::new("new/identical", n), &n, |b, _| {
            b.iter(|| black_box(a.clone().symmetric_difference(&a)))
        });
    }
    group.finish();
}

// ── Trie benchmarks ───────────────────────────────────────────────────────────

fn bench_trie_union(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie/union");
    group.measurement_time(Duration::from_secs(4));
    group.warm_up_time(Duration::from_secs(1));

    for &(n, depth) in &[(200usize, 3usize), (2_000, 4)] {
        let all = paths(n * 2, depth);
        let ps_a = &all[..n];
        let ps_b = &all[n / 2..n + n / 2]; // 50% overlap

        let a = build_trie(ps_a);
        let b = build_trie(ps_b);

        // different tries: ptr_eq does not fire — is the check overhead measurable?
        group.bench_with_input(BenchmarkId::new("old/different", n), &n, |bch, _| {
            bch.iter(|| black_box(old_trie_union(a.clone(), b.clone())))
        });
        group.bench_with_input(BenchmarkId::new("new/different", n), &n, |bch, _| {
            bch.iter(|| black_box(a.clone().union(b.clone())))
        });

        // identical (clone): ptr_eq fires in new, saves full flatten-and-rebuild
        group.bench_with_input(BenchmarkId::new("old/identical", n), &n, |bch, _| {
            bch.iter(|| black_box(old_trie_union(a.clone(), a.clone())))
        });
        group.bench_with_input(BenchmarkId::new("new/identical", n), &n, |bch, _| {
            bch.iter(|| black_box(a.clone().union(a.clone())))
        });
    }
    group.finish();
}

fn bench_trie_difference(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie/difference");
    group.measurement_time(Duration::from_secs(4));
    group.warm_up_time(Duration::from_secs(1));

    for &(n, depth) in &[(200usize, 3usize), (2_000, 4)] {
        let all = paths(n * 2, depth);
        let ps_a = &all[..n];
        let ps_b = &all[n / 2..n + n / 2];

        let a = build_trie(ps_a);
        let b = build_trie(ps_b);

        group.bench_with_input(BenchmarkId::new("old/different", n), &n, |bch, _| {
            bch.iter(|| black_box(old_trie_difference(a.clone(), &b)))
        });
        group.bench_with_input(BenchmarkId::new("new/different", n), &n, |bch, _| {
            bch.iter(|| black_box(a.clone().difference(&b)))
        });

        group.bench_with_input(BenchmarkId::new("old/identical", n), &n, |bch, _| {
            bch.iter(|| black_box(old_trie_difference(a.clone(), &a)))
        });
        group.bench_with_input(BenchmarkId::new("new/identical", n), &n, |bch, _| {
            bch.iter(|| black_box(a.clone().difference(&a)))
        });
    }
    group.finish();
}

criterion_group!(
    ord_trie_benches,
    bench_ord_union,
    bench_ord_difference,
    bench_ord_intersection,
    bench_ord_sym_diff,
);
criterion_group!(trie_benches, bench_trie_union, bench_trie_difference,);
criterion_main!(ord_trie_benches, trie_benches);
