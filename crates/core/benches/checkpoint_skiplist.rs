use bdk_core::CheckPoint;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use criterion::{black_box, criterion_group, criterion_main, Bencher, Criterion};

/// Create a checkpoint chain with the given length
fn create_checkpoint_chain(length: u32) -> CheckPoint<BlockHash> {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());
    for height in 1..=length {
        let hash = BlockHash::from_byte_array([(height % 256) as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }
    cp
}

/// Benchmark get() operations at various depths
fn bench_checkpoint_get(c: &mut Criterion) {
    // Small chain - get near start
    c.bench_function("get_100_near_start", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(100);
        let target = 10;
        b.iter(|| {
            black_box(cp.get(target));
        });
    });

    // Medium chain - get middle
    c.bench_function("get_1000_middle", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(1000);
        let target = 500;
        b.iter(|| {
            black_box(cp.get(target));
        });
    });

    // Large chain - get near end
    c.bench_function("get_10000_near_end", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let target = 9000;
        b.iter(|| {
            black_box(cp.get(target));
        });
    });

    // Large chain - get near start (best case for skiplist)
    c.bench_function("get_10000_near_start", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let target = 100;
        b.iter(|| {
            black_box(cp.get(target));
        });
    });
}

/// Benchmark floor_at() operations
fn bench_checkpoint_floor_at(c: &mut Criterion) {
    c.bench_function("floor_at_1000", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(1000);
        let target = 750; // Target that might not exist exactly
        b.iter(|| {
            black_box(cp.floor_at(target));
        });
    });

    c.bench_function("floor_at_10000", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let target = 7500;
        b.iter(|| {
            black_box(cp.floor_at(target));
        });
    });
}

/// Benchmark range() iteration
fn bench_checkpoint_range(c: &mut Criterion) {
    c.bench_function("range_1000_20pct", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(1000);
        let start = 400;
        let end = 600;
        b.iter(|| {
            let range: Vec<_> = cp.range(start..=end).collect();
            black_box(range);
        });
    });

    c.bench_function("range_10000_to_end", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let from = 5000;
        b.iter(|| {
            let range: Vec<_> = cp.range(from..).collect();
            black_box(range);
        });
    });
}

/// Benchmark insert() operations
fn bench_checkpoint_insert(c: &mut Criterion) {
    c.bench_function("insert_sparse_1000", |b: &mut Bencher| {
        // Create a sparse chain
        let mut cp = CheckPoint::new(0, BlockHash::all_zeros());
        for i in 1..=100 {
            let height = i * 10;
            let hash = BlockHash::from_byte_array([(height % 256) as u8; 32]);
            cp = cp.push(height, hash).unwrap();
        }

        let insert_height = 505;
        let insert_hash = BlockHash::from_byte_array([255; 32]);

        b.iter(|| {
            let result = cp.clone().insert(insert_height, insert_hash);
            black_box(result);
        });
    });
}

/// Compare linear traversal vs skiplist-enhanced get()
fn bench_traversal_comparison(c: &mut Criterion) {
    // Linear traversal benchmark
    c.bench_function("linear_traversal_10000", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let target = 100; // Near the beginning

        b.iter(|| {
            let mut current = cp.clone();
            while current.height() > target {
                if let Some(prev) = current.prev() {
                    current = prev;
                } else {
                    break;
                }
            }
            black_box(current);
        });
    });

    // Skiplist-enhanced get() for comparison
    c.bench_function("skiplist_get_10000", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let target = 100; // Same target

        b.iter(|| {
            black_box(cp.get(target));
        });
    });
}

/// Analyze skip pointer distribution and usage
fn bench_skip_pointer_analysis(c: &mut Criterion) {
    c.bench_function("count_skip_pointers_10000", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);

        b.iter(|| {
            let mut count = 0;
            let mut current = cp.clone();
            loop {
                if current.skip().is_some() {
                    count += 1;
                }
                if let Some(prev) = current.prev() {
                    current = prev;
                } else {
                    break;
                }
            }
            black_box(count);
        });
    });

    // Measure actual skip pointer usage during traversal
    c.bench_function("skip_usage_in_traversal", |b: &mut Bencher| {
        let cp = create_checkpoint_chain(10000);
        let target = 100;

        b.iter(|| {
            let mut current = cp.clone();
            let mut skips_used = 0;

            while current.height() > target {
                if let Some(skip_cp) = current.skip() {
                    if skip_cp.height() >= target {
                        current = skip_cp;
                        skips_used += 1;
                        continue;
                    }
                }

                if let Some(prev) = current.prev() {
                    current = prev;
                } else {
                    break;
                }
            }
            black_box((current, skips_used));
        });
    });
}

criterion_group!(
    benches,
    bench_checkpoint_get,
    bench_checkpoint_floor_at,
    bench_checkpoint_range,
    bench_checkpoint_insert,
    bench_traversal_comparison,
    bench_skip_pointer_analysis
);

criterion_main!(benches);
