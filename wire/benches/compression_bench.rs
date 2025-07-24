use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::fs;
use wire::compression::decompress_bytes;

fn bench_decompress_ops_json(c: &mut Criterion) {
    // Load the compressed data once
    let compressed_data =
        fs::read("data/ops.json.zst").expect("Failed to read ops.json.zst file for benchmarking");

    c.bench_function("decompress_ops_json_zst", |b| {
        b.iter(|| {
            let result = decompress_bytes(black_box(&compressed_data));
            black_box(result.expect("Decompression should succeed"))
        })
    });
}

fn bench_decompress_ops_json_multiple_sizes(c: &mut Criterion) {
    let compressed_data =
        fs::read("data/ops.json.zst").expect("Failed to read ops.json.zst file for benchmarking");

    let mut group = c.benchmark_group("decompress_different_sizes");

    // Test with full file
    group.bench_function("full_file", |b| {
        b.iter(|| {
            let result = decompress_bytes(black_box(&compressed_data));
            black_box(result.expect("Decompression should succeed"))
        })
    });

    // Test with truncated versions to simulate different input sizes
    let quarter_size = compressed_data.len() / 4;
    let half_size = compressed_data.len() / 2;
    let three_quarter_size = (compressed_data.len() * 3) / 4;

    // Note: These truncated tests might fail since they're not valid zstd data
    // but we can benchmark the error path too
    if quarter_size > 0 {
        group.bench_function("quarter_size_data", |b| {
            b.iter(|| {
                let truncated = &compressed_data[..quarter_size];
                let result = decompress_bytes(black_box(truncated));
                black_box(result) // This will likely be an error, but we benchmark it anyway
            })
        });
    }

    if half_size > 0 {
        group.bench_function("half_size_data", |b| {
            b.iter(|| {
                let truncated = &compressed_data[..half_size];
                let result = decompress_bytes(black_box(truncated));
                black_box(result)
            })
        });
    }

    if three_quarter_size > 0 {
        group.bench_function("three_quarter_size_data", |b| {
            b.iter(|| {
                let truncated = &compressed_data[..three_quarter_size];
                let result = decompress_bytes(black_box(truncated));
                black_box(result)
            })
        });
    }

    group.finish();
}

fn bench_decompress_empty_data(c: &mut Criterion) {
    c.bench_function("decompress_empty_data", |b| {
        b.iter(|| {
            let result = decompress_bytes(black_box(&[]));
            black_box(result) // This should be an error
        })
    });
}

fn bench_decompress_repeated_calls(c: &mut Criterion) {
    let compressed_data =
        fs::read("data/ops.json.zst").expect("Failed to read ops.json.zst file for benchmarking");

    c.bench_function("decompress_10_iterations", |b| {
        b.iter(|| {
            for _ in 0..10 {
                let result = decompress_bytes(black_box(&compressed_data));
                black_box(result.expect("Decompression should succeed"));
            }
        })
    });
}

fn bench_with_memory_allocation(c: &mut Criterion) {
    let compressed_data =
        fs::read("data/ops.json.zst").expect("Failed to read ops.json.zst file for benchmarking");

    c.bench_function("decompress_with_fresh_allocation", |b| {
        b.iter(|| {
            // Clone the data to test allocation overhead
            let data_copy = compressed_data.clone();
            let result = decompress_bytes(black_box(&data_copy));
            black_box(result.expect("Decompression should succeed"))
        })
    });
}

criterion_group!(
    benches,
    bench_decompress_ops_json,
    bench_decompress_ops_json_multiple_sizes,
    bench_decompress_empty_data,
    bench_decompress_repeated_calls,
    bench_with_memory_allocation
);

criterion_main!(benches);
