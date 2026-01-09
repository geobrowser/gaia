//! Benchmarks for kg-indexer edit handler performance.
//!
//! Measures the CPU time for mapping/validation of edits at different scales.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use kg_indexer::handlers::edits::handle_edit;
use wire::pb::grc20::{Entity, Op, Relation, Value, op};

/// Generate a synthetic HermesEdit with the specified number of ops.
/// Mix of entity updates and relation creates to simulate realistic data.
fn generate_hermes_edit(num_ops: usize) -> HermesEdit {
    let ops: Vec<Op> = (0..num_ops)
        .map(|i| {
            if i % 3 == 0 {
                // Create relation (larger op, more processing)
                Op {
                    payload: Some(op::Payload::CreateRelation(Relation {
                        id: format!("rel-{:016x}", i).into_bytes(),
                        r#type: b"RELATES_TO".to_vec(),
                        from_entity: format!("entity-{:016x}", i).into_bytes(),
                        from_space: Some(vec![0u8; 16]),
                        from_version: None,
                        to_entity: format!("entity-{:016x}", i + 1).into_bytes(),
                        to_space: Some(vec![0u8; 16]),
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
                                property: b"name____________".to_vec(), // 16 bytes
                                value: format!("Entity Name {}", i),
                                options: None,
                            },
                            Value {
                                property: b"desc____________".to_vec(), // 16 bytes
                                value: format!("Description for entity {} with some text", i),
                                options: None,
                            },
                        ],
                    })),
                }
            }
        })
        .collect();

    HermesEdit {
        id: vec![0u8; 16],
        name: "Benchmark Edit".to_string(),
        ops,
        authors: vec![vec![0u8; 20]],
        language: None,
        space_id: vec![1u8; 16],
        is_canonical: true,
        meta: Some(BlockchainMetadata {
            created_at: 1700000000,
            created_by: vec![0u8; 20],
            block_number: 1000000,
            cursor: "cursor".to_string(),
            sequence: 0,
            is_last: true,
        }),
    }
}

/// Benchmark handle_edit at different op counts.
fn bench_handle_edit_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("handle_edit_scaling");

    for num_ops in [1_000, 10_000, 50_000, 100_000, 250_000] {
        let edit = generate_hermes_edit(num_ops);

        group.throughput(Throughput::Elements(num_ops as u64));
        group.bench_with_input(
            BenchmarkId::new("ops", num_ops),
            &edit,
            |b, edit| {
                b.iter(|| {
                    let result = handle_edit(black_box(edit));
                    black_box(result.expect("handle_edit should succeed"))
                })
            },
        );
    }

    group.finish();
}

/// Benchmark individual extraction functions.
fn bench_extraction_components(c: &mut Criterion) {
    let mut group = c.benchmark_group("handle_edit_components");

    // Test with 100K ops to see relative cost of each component
    let edit = generate_hermes_edit(100_000);

    group.throughput(Throughput::Elements(100_000));
    group.bench_function("full_handle_edit_100k", |b| {
        b.iter(|| {
            let result = handle_edit(black_box(&edit));
            black_box(result.expect("handle_edit should succeed"))
        })
    });

    group.finish();
}

criterion_group!(
    handler_benches,
    bench_handle_edit_scaling,
    bench_extraction_components,
);

criterion_main!(handler_benches);
