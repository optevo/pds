// Memory profiling benchmarks using dhat.
//
// Measures heap allocation counts and bytes for each collection type
// at various sizes. Run with:
//
//   cargo bench --bench memory
//
// Results are printed to stdout. The dhat profiler also writes a
// dhat-heap.json file for interactive analysis.

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::hint::black_box;

fn measure<F: FnOnce()>(label: &str, f: F) -> dhat::HeapStats {
    // Force a fresh stats snapshot, run the closure, capture delta
    let before = dhat::HeapStats::get();
    f();
    let after = dhat::HeapStats::get();

    let allocs = after.total_blocks - before.total_blocks;
    let bytes = after.total_bytes - before.total_bytes;
    let max_blocks = after.max_blocks.saturating_sub(before.max_blocks);
    let max_bytes = after.max_bytes.saturating_sub(before.max_bytes);

    println!(
        "{label:<50} allocs: {allocs:>8}  bytes: {bytes:>12}  peak_blocks: {max_blocks:>8}  peak_bytes: {max_bytes:>12}"
    );
    after
}

fn bench_hashmap(sizes: &[usize]) {
    use pds::HashMap;

    println!("\n--- HashMap<i64, i64> ---");
    for &n in sizes {
        measure(&format!("from_iter({n})"), || {
            let map: HashMap<i64, i64> = (0..n as i64).map(|i| (i, i)).collect();
            black_box(&map);
        });
    }

    let n = 10_000;
    let map: HashMap<i64, i64> = (0..n as i64).map(|i| (i, i)).collect();

    measure("single insert", || {
        let mut m = map.clone();
        m.insert(n as i64, 42);
        black_box(&m);
    });

    measure("clone + modify (structural sharing)", || {
        let mut m = map.clone();
        m.insert(0, 999);
        black_box(&m);
    });

    measure("clone (should be ~0 allocs)", || {
        let m = map.clone();
        black_box(&m);
    });
}

fn bench_hashset(sizes: &[usize]) {
    use pds::HashSet;

    println!("\n--- HashSet<i64> ---");
    for &n in sizes {
        measure(&format!("from_iter({n})"), || {
            let set: HashSet<i64> = (0..n as i64).collect();
            black_box(&set);
        });
    }
}

fn bench_ordmap(sizes: &[usize]) {
    use pds::OrdMap;

    println!("\n--- OrdMap<i64, i64> ---");
    for &n in sizes {
        measure(&format!("from_iter({n})"), || {
            let map: OrdMap<i64, i64> = (0..n as i64).map(|i| (i, i)).collect();
            black_box(&map);
        });
    }

    let n = 10_000;
    let map: OrdMap<i64, i64> = (0..n as i64).map(|i| (i, i)).collect();

    measure("single insert", || {
        let mut m = map.clone();
        m.insert(n as i64, 42);
        black_box(&m);
    });

    measure("clone + modify (structural sharing)", || {
        let mut m = map.clone();
        m.insert(0, 999);
        black_box(&m);
    });
}

fn bench_ordset(sizes: &[usize]) {
    use pds::OrdSet;

    println!("\n--- OrdSet<i64> ---");
    for &n in sizes {
        measure(&format!("from_iter({n})"), || {
            let set: OrdSet<i64> = (0..n as i64).collect();
            black_box(&set);
        });
    }
}

fn bench_vector(sizes: &[usize]) {
    use pds::Vector;

    println!("\n--- Vector<i64> ---");
    for &n in sizes {
        measure(&format!("push_back({n})"), || {
            let mut v = Vector::new();
            for i in 0..n as i64 {
                v.push_back(i);
            }
            black_box(&v);
        });
    }

    for &n in sizes {
        measure(&format!("from_iter({n})"), || {
            let v: Vector<i64> = (0..n as i64).collect();
            black_box(&v);
        });
    }

    let n = 10_000;
    let vec: Vector<i64> = (0..n as i64).collect();

    measure("clone + push_back (structural sharing)", || {
        let mut v = vec.clone();
        v.push_back(999);
        black_box(&v);
    });
}

fn bench_bag(sizes: &[usize]) {
    use pds::Bag;

    println!("\n--- Bag<i64> ---");
    for &n in sizes {
        measure(&format!("from_iter({n})"), || {
            let bag: Bag<i64> = (0..n as i64).collect();
            black_box(&bag);
        });
    }
}

fn bench_bimap_symmap() {
    use pds::BiMap;
    use pds::SymMap;

    println!("\n--- BiMap<i64, i64> ---");
    let n = 10_000;
    measure(&format!("from_iter({n})"), || {
        let bm: BiMap<i64, i64> = (0..n as i64).map(|i| (i, i + n as i64)).collect();
        black_box(&bm);
    });

    println!("\n--- SymMap<i64> ---");
    measure(&format!("from_iter({n})"), || {
        let sm: SymMap<i64> = (0..n as i64).map(|i| (i, i + n as i64)).collect();
        black_box(&sm);
    });
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    let sizes = [1_000, 10_000, 100_000];

    println!("=== pds Memory Profiling (dhat) ===");
    println!(
        "Each line: operation, total allocations, total bytes, peak live blocks, peak live bytes\n"
    );

    bench_hashmap(&sizes);
    bench_hashset(&sizes);
    bench_ordmap(&sizes);
    bench_ordset(&sizes);
    bench_vector(&sizes);
    bench_bag(&sizes);
    bench_bimap_symmap();

    println!("\n=== Done ===");
    println!("Full dhat profile written to dhat-heap.json (view at https://nnethercote.github.io/dh_view/dh_view.html)");
}
