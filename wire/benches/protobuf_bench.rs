//! Benchmarks for protobuf encode/decode performance at different scales.
//!
//! These benchmarks measure throughput to inform performance budgets for
//! the indexing pipeline. Key metric: can we decode in < 250ms (block time)?

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use prost::Message;
use std::fs;
use wire::compression::decompress_bytes;
use wire::deserialize::deserialize;
use wire::pb::grc20::{Edit, Entity, Op, Relation, Value, op};

/// Generate a synthetic Edit with the specified number of ops.
/// Mix of entity updates and relation creates to simulate realistic data.
fn generate_edit(num_ops: usize) -> Edit {
    let ops: Vec<Op> = (0..num_ops)
        .map(|i| {
            if i % 3 == 0 {
                // Create relation (larger op)
                Op {
                    payload: Some(op::Payload::CreateRelation(Relation {
                        id: format!("rel-{:016x}", i).into_bytes(),
                        r#type: b"RELATES_TO".to_vec(),
                        from_entity: format!("entity-{:016x}", i).into_bytes(),
                        from_space: Some(b"space-001".to_vec()),
                        from_version: None,
                        to_entity: format!("entity-{:016x}", i + 1).into_bytes(),
                        to_space: Some(b"space-001".to_vec()),
                        to_version: None,
                        entity: format!("rel-entity-{:016x}", i).into_bytes(),
                        position: Some("a0".to_string()),
                        verified: Some(true),
                    })),
                }
            } else {
                // Update entity (smaller op)
                Op {
                    payload: Some(op::Payload::UpdateEntity(Entity {
                        id: format!("entity-{:016x}", i).into_bytes(),
                        values: vec![
                            Value {
                                property: b"name".to_vec(),
                                value: format!("Entity Name {}", i),
                                options: None,
                            },
                            Value {
                                property: b"description".to_vec(),
                                value: format!("Description for entity {} with some additional text to simulate realistic content length", i),
                                options: None,
                            },
                        ],
                    })),
                }
            }
        })
        .collect();

    Edit {
        id: b"benchmark-edit-001".to_vec(),
        name: "Benchmark Edit".to_string(),
        ops,
        authors: vec![b"author-001".to_vec()],
        language: None,
    }
}

/// Benchmark decoding at different op counts to understand scaling behavior.
fn bench_decode_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("protobuf_decode_scaling");

    // Test at different scales
    for num_ops in [1_000, 10_000, 50_000, 100_000, 250_000, 500_000] {
        let edit = generate_edit(num_ops);
        let encoded = edit.encode_to_vec();
        let size_mb = encoded.len() as f64 / 1_000_000.0;

        group.throughput(Throughput::Elements(num_ops as u64));
        group.bench_with_input(
            BenchmarkId::new("ops", format!("{num_ops} ({size_mb:.2}MB)")),
            &encoded,
            |b, data| {
                b.iter(|| {
                    let result = Edit::decode(black_box(data.as_slice()));
                    black_box(result.expect("decode should succeed"))
                })
            },
        );
    }

    group.finish();
}

/// Benchmark with the real test data file (148K ops, 19MB).
fn bench_decode_real_data(c: &mut Criterion) {
    let proto_data = fs::read("data/proto").expect("Failed to read proto file");
    let edit = deserialize(&proto_data).expect("Failed to deserialize");
    let num_ops = edit.ops.len();

    let mut group = c.benchmark_group("protobuf_decode_real_data");
    group.throughput(Throughput::Elements(num_ops as u64));
    group.throughput(Throughput::Bytes(proto_data.len() as u64));

    group.bench_function("148k_ops_19MB", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&proto_data));
            black_box(result.expect("decode should succeed"))
        })
    });

    group.finish();
}

/// Benchmark full pipeline: decompress + decode.
fn bench_decompress_and_decode(c: &mut Criterion) {
    let compressed = fs::read("data/proto.zst").expect("Failed to read compressed proto");
    let decompressed = decompress_bytes(&compressed).expect("Failed to decompress");
    let edit = deserialize(&decompressed).expect("Failed to deserialize");
    let num_ops = edit.ops.len();

    let mut group = c.benchmark_group("protobuf_full_pipeline");
    group.throughput(Throughput::Elements(num_ops as u64));

    group.bench_function("decompress_only", |b| {
        b.iter(|| {
            let result = decompress_bytes(black_box(&compressed));
            black_box(result.expect("decompress should succeed"))
        })
    });

    group.bench_function("decode_only", |b| {
        b.iter(|| {
            let result = deserialize(black_box(&decompressed));
            black_box(result.expect("decode should succeed"))
        })
    });

    group.bench_function("decompress_and_decode", |b| {
        b.iter(|| {
            let decompressed = decompress_bytes(black_box(&compressed))
                .expect("decompress should succeed");
            let result = deserialize(black_box(&decompressed));
            black_box(result.expect("decode should succeed"))
        })
    });

    group.finish();
}

/// Benchmark encode performance (for reference).
fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("protobuf_encode");

    for num_ops in [10_000, 100_000, 500_000] {
        let edit = generate_edit(num_ops);

        group.throughput(Throughput::Elements(num_ops as u64));
        group.bench_with_input(
            BenchmarkId::new("ops", num_ops),
            &edit,
            |b, edit| {
                b.iter(|| {
                    let encoded = black_box(edit).encode_to_vec();
                    black_box(encoded)
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    protobuf_benches,
    bench_decode_scaling,
    bench_decode_real_data,
    bench_decompress_and_decode,
    bench_encode,
);

criterion_main!(protobuf_benches);
