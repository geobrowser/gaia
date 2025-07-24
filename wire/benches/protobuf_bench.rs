use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::fs;
use wire::compression::decompress_bytes;
use wire::deserialize::deserialize;

fn bench_deserialize_proto_basic(c: &mut Criterion) {
    // Load and decompress the proto data once for reuse
    let proto_data =
        fs::read("data/proto").expect("Failed to read proto.zst file for benchmarking");

    c.bench_function("protobuf_deserialize_basic", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&proto_data));
            black_box(result.expect("Deserialization should succeed"))
        })
    });
}

fn bench_full_pipeline_decompress_deserialize(c: &mut Criterion) {
    let compressed_proto_data =
        fs::read("data/proto.zst").expect("Failed to read proto.zst file for benchmarking");

    c.bench_function("protobuf_full_pipeline", |b| {
        b.iter(|| {
            // Decompress first
            let decompressed = decompress_bytes(black_box(&compressed_proto_data))
                .expect("Decompression should succeed");

            // Then deserialize
            let result = deserialize(black_box(&decompressed));
            black_box(result.expect("Deserialization should succeed"))
        })
    });
}

fn bench_deserialize_repeated_operations(c: &mut Criterion) {
    let proto_data =
        fs::read("data/proto").expect("Failed to read proto.zst file for benchmarking");

    let mut group = c.benchmark_group("protobuf_repeated_operations");

    group.bench_function("deserialize_1x", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&proto_data));
            black_box(result.expect("Deserialization should succeed"))
        })
    });

    group.bench_function("deserialize_10x", |b| {
        b.iter(|| {
            for _ in 0..10 {
                let result = deserialize(black_box(&proto_data));
                black_box(result.expect("Deserialization should succeed"));
            }
        })
    });

    group.finish();
}

fn bench_deserialize_different_data_sizes(c: &mut Criterion) {
    let proto_data =
        fs::read("data/proto").expect("Failed to read proto.zst file for benchmarking");

    let mut group = c.benchmark_group("protobuf_different_sizes");

    // Full data
    group.bench_function("full_proto_data", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&proto_data));
            black_box(result)
        })
    });

    // Test with truncated data (this will likely fail, but we can measure error handling)
    let quarter_size = proto_data.len() / 4;
    let half_size = proto_data.len() / 2;
    let three_quarter_size = (proto_data.len() * 3) / 4;

    if quarter_size > 0 {
        group.bench_function("quarter_size_data", |b| {
            b.iter(|| {
                let truncated = &proto_data[..quarter_size];
                let result = deserialize(black_box(truncated));
                black_box(result) // This will likely be an error
            })
        });
    }

    if half_size > 0 {
        group.bench_function("half_size_data", |b| {
            b.iter(|| {
                let truncated = &proto_data[..half_size];
                let result = deserialize(black_box(truncated));
                black_box(result) // This will likely be an error
            })
        });
    }

    if three_quarter_size > 0 {
        group.bench_function("three_quarter_size_data", |b| {
            b.iter(|| {
                let truncated = &proto_data[..three_quarter_size];
                let result = deserialize(black_box(truncated));
                black_box(result) // This will likely be an error
            })
        });
    }

    group.finish();
}

fn bench_deserialize_error_cases(c: &mut Criterion) {
    let mut group = c.benchmark_group("protobuf_error_handling");

    // Empty data
    group.bench_function("deserialize_empty_data", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&[]));
            black_box(result) // This should be an error
        })
    });

    // Invalid protobuf data
    let invalid_data = vec![0xFF, 0xFE, 0xFD, 0xFC, 0xFB, 0xFA, 0xF9, 0xF8];
    group.bench_function("deserialize_invalid_data", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&invalid_data));
            black_box(result) // This should be an error
        })
    });

    // Single byte
    let single_byte = vec![0x01];
    group.bench_function("deserialize_single_byte", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&single_byte));
            black_box(result) // This will likely be an error
        })
    });

    group.finish();
}

fn bench_memory_allocation_patterns(c: &mut Criterion) {
    let proto_data =
        fs::read("data/proto.zst").expect("Failed to read proto.zst file for benchmarking");

    let mut group = c.benchmark_group("protobuf_memory_patterns");

    // Reuse existing data (minimal allocation)
    group.bench_function("reuse_existing_data", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&proto_data));
            black_box(result.expect("Deserialization should succeed"))
        })
    });

    // Clone data each time (fresh allocation)
    group.bench_function("fresh_allocation", |b| {
        b.iter(|| {
            let data_copy = proto_data.clone();
            let result = deserialize(black_box(&data_copy));
            black_box(result.expect("Deserialization should succeed"))
        })
    });

    group.finish();
}

criterion_group!(
    protobuf_benches,
    bench_deserialize_proto_basic,
    bench_full_pipeline_decompress_deserialize,
    bench_deserialize_repeated_operations,
    bench_deserialize_different_data_sizes,
    bench_deserialize_error_cases,
    bench_memory_allocation_patterns
);

criterion_main!(protobuf_benches);
