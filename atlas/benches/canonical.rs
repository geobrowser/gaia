//! Benchmarks for canonical graph computation
//!
//! Run with: cargo bench -p atlas --bench canonical

use atlas::events::{
    BlockMetadata, SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, SpaceType,
    TopicId, TrustExtended, TrustExtension,
};
use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::prelude::*;

// ============================================================================
// Helpers for synthetic graph generation
// ============================================================================

fn make_space_id(n: u32) -> SpaceId {
    let bytes = n.to_be_bytes();
    let mut id = [0u8; 16];
    id[12..16].copy_from_slice(&bytes);
    id
}

fn make_topic_id(n: u32) -> TopicId {
    let bytes = n.to_be_bytes();
    let mut id = [0u8; 16];
    id[12..16].copy_from_slice(&bytes);
    // Differentiate from space IDs
    id[0] = 0xFF;
    id
}

fn make_block_meta() -> BlockMetadata {
    BlockMetadata {
        block_number: 1,
        block_timestamp: 12,
        tx_hash: "0x1".to_string(),
        cursor: "cursor_1".to_string(),
    }
}

/// Create a space and add it to the graph state
fn create_space(state: &mut GraphState, n: u32) -> SpaceId {
    let space = make_space_id(n);
    let topic = make_topic_id(n);
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id: space,
            topic_id: topic,
            space_type: SpaceType::Dao {
                initial_editors: vec![],
                initial_members: vec![],
            },
        }),
    };
    state.apply_event(&event);
    space
}

/// Create a space with a specific topic ID
fn create_space_with_topic(state: &mut GraphState, space_n: u32, topic_n: u32) -> SpaceId {
    let space = make_space_id(space_n);
    let topic = make_topic_id(topic_n);
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id: space,
            topic_id: topic,
            space_type: SpaceType::Dao {
                initial_editors: vec![],
                initial_members: vec![],
            },
        }),
    };
    state.apply_event(&event);
    space
}

/// Add a verified edge between two spaces
fn add_verified_edge(state: &mut GraphState, source: SpaceId, target: SpaceId) {
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: source,
            extension: TrustExtension::Verified {
                target_space_id: target,
            },
        }),
    };
    state.apply_event(&event);
}

/// Add a topic edge from a space to a topic
fn add_topic_edge(state: &mut GraphState, source: SpaceId, topic: TopicId) {
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: source,
            extension: TrustExtension::Subtopic {
                target_topic_id: topic,
            },
        }),
    };
    state.apply_event(&event);
}

/// Generate a random graph with n nodes and e edges (all explicit)
fn generate_random_graph(n: u32, edges: u32, seed: u64) -> (GraphState, SpaceId) {
    let mut state = GraphState::new();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut spaces = Vec::with_capacity(n as usize);

    for i in 0..n {
        spaces.push(create_space(&mut state, i));
    }

    // Add random edges
    for _ in 0..edges {
        let source = rng.gen_range(0..n) as usize;
        let target = rng.gen_range(0..n) as usize;
        if source != target {
            add_verified_edge(&mut state, spaces[source], spaces[target]);
        }
    }

    (state, spaces[0])
}

/// Generate a graph with topic edges where some members are canonical and some are not
fn generate_canonical_with_topics(
    canonical_count: u32,
    non_canonical_count: u32,
    topics_per_canonical: u32,
    members_per_topic: u32,
    seed: u64,
) -> (GraphState, SpaceId) {
    let mut state = GraphState::new();
    let mut rng = StdRng::seed_from_u64(seed);

    // Create root
    let root = create_space(&mut state, 0);

    // Create canonical spaces (connected to root via explicit edges)
    let mut canonical_spaces = vec![root];
    for i in 1..canonical_count {
        let space = create_space(&mut state, i);
        // Connect to a random existing canonical space
        let parent_idx = rng.gen_range(0..canonical_spaces.len());
        add_verified_edge(&mut state, canonical_spaces[parent_idx], space);
        canonical_spaces.push(space);
    }

    // Create non-canonical spaces (not connected to root)
    let mut non_canonical_spaces = Vec::new();
    for i in 0..non_canonical_count {
        let space = create_space(&mut state, canonical_count + i);
        non_canonical_spaces.push(space);
    }

    // Create shared topics with a mix of canonical and non-canonical members
    let num_topics = (canonical_count * topics_per_canonical) / members_per_topic;
    for t in 0..num_topics {
        let topic = make_topic_id(10000 + t);

        // Create spaces that announce this topic (mix of canonical and non-canonical)
        for m in 0..members_per_topic {
            let space_id = 100000 + t * members_per_topic + m;
            if m % 2 == 0 && !canonical_spaces.is_empty() {
                // Make some members canonical by connecting to existing canonical space
                let space = create_space_with_topic(&mut state, space_id, 10000 + t);
                let parent_idx = rng.gen_range(0..canonical_spaces.len());
                add_verified_edge(&mut state, canonical_spaces[parent_idx], space);
            } else {
                // Non-canonical member (no explicit path from root)
                let _space = create_space_with_topic(&mut state, space_id, 10000 + t);
            }
        }

        // Add topic edge from a random canonical space
        let source_idx = rng.gen_range(0..canonical_spaces.len());
        add_topic_edge(&mut state, canonical_spaces[source_idx], topic);
    }

    (state, root)
}

// ============================================================================
// Benchmarks
// ============================================================================

/// Benchmark full canonical graph computation at various sizes
fn bench_canonical_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_computation");

    for size in [100, 500, 1000, 5000] {
        let (state, root) = generate_random_graph(size, size * 2, 42);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let mut transitive = TransitiveProcessor::new();
                let mut processor = CanonicalProcessor::new(root);
                let graph = processor.compute(&state, &mut transitive);
                black_box(graph)
            });
        });
    }

    group.finish();
}

/// Benchmark Phase 1 only (explicit-only transitive)
fn bench_canonical_phase1(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_phase1");

    for size in [100, 500, 1000, 5000] {
        let (state, root) = generate_random_graph(size, size * 2, 42);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let mut transitive = TransitiveProcessor::new();
                let graph = transitive.get_explicit_only(root, &state);
                black_box(graph.len())
            });
        });
    }

    group.finish();
}

/// Benchmark with topic edges (Phase 1 + Phase 2)
fn bench_canonical_with_topics(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_with_topics");

    // (canonical_count, non_canonical_count, topics_per_canonical, members_per_topic)
    let scenarios = [
        (100, 50, 2, 5, "small"),
        (500, 200, 2, 10, "medium"),
        (1000, 500, 3, 10, "large"),
    ];

    for (canonical, non_canonical, topics_per, members_per, name) in scenarios {
        let (state, root) =
            generate_canonical_with_topics(canonical, non_canonical, topics_per, members_per, 42);

        group.bench_with_input(BenchmarkId::from_parameter(name), &name, |b, _| {
            b.iter(|| {
                let mut transitive = TransitiveProcessor::new();
                let mut processor = CanonicalProcessor::new(root);
                let graph = processor.compute(&state, &mut transitive);
                black_box(graph)
            });
        });
    }

    group.finish();
}

/// Benchmark affects_canonical check
fn bench_affects_canonical(c: &mut Criterion) {
    let mut group = c.benchmark_group("affects_canonical");

    let (state, root) = generate_random_graph(1000, 2000, 42);
    let mut transitive = TransitiveProcessor::new();
    let mut processor = CanonicalProcessor::new(root);
    let graph = processor.compute(&state, &mut transitive).unwrap();
    let canonical_set = graph.flat.clone();

    // Event from canonical source
    let canonical_source = *canonical_set.iter().next().unwrap();
    let event_canonical = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: canonical_source,
            extension: TrustExtension::Verified {
                target_space_id: make_space_id(9999),
            },
        }),
    };

    // Event from non-canonical source
    let event_non_canonical = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: make_space_id(9999),
            extension: TrustExtension::Verified {
                target_space_id: make_space_id(9998),
            },
        }),
    };

    group.bench_function("canonical_source", |b| {
        b.iter(|| black_box(processor.affects_canonical(&event_canonical, &canonical_set)));
    });

    group.bench_function("non_canonical_source", |b| {
        b.iter(|| black_box(processor.affects_canonical(&event_non_canonical, &canonical_set)));
    });

    group.finish();
}

/// Benchmark change detection (compute twice, second should detect no change)
fn bench_change_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("change_detection");

    let (state, root) = generate_random_graph(1000, 2000, 42);

    // First compute (with change)
    group.bench_function("first_compute", |b| {
        b.iter(|| {
            let mut transitive = TransitiveProcessor::new();
            let mut processor = CanonicalProcessor::new(root);
            let graph = processor.compute(&state, &mut transitive);
            black_box(graph)
        });
    });

    // Second compute (no change - should return None quickly)
    let mut transitive = TransitiveProcessor::new();
    let mut processor = CanonicalProcessor::new(root);
    let _ = processor.compute(&state, &mut transitive);

    group.bench_function("second_compute_no_change", |b| {
        b.iter(|| {
            let graph = processor.compute(&state, &mut transitive);
            black_box(graph)
        });
    });

    group.finish();
}

/// Benchmark subtree filtering with various canonical set densities
fn bench_subtree_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("subtree_filtering");

    // Sparse: 10% of nodes are canonical
    // Dense: 90% of nodes are canonical
    let scenarios = [
        (1000, 100, "sparse_10pct"),
        (1000, 500, "medium_50pct"),
        (1000, 900, "dense_90pct"),
    ];

    for (total, canonical_count, name) in scenarios {
        let (state, root) =
            generate_canonical_with_topics(canonical_count, total - canonical_count, 5, 10, 42);

        group.bench_with_input(BenchmarkId::from_parameter(name), &name, |b, _| {
            b.iter(|| {
                let mut transitive = TransitiveProcessor::new();
                let mut processor = CanonicalProcessor::new(root);
                let graph = processor.compute(&state, &mut transitive);
                black_box(graph)
            });
        });
    }

    group.finish();
}

/// Benchmark end-to-end latency with realistic scenario
fn bench_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("end_to_end");

    // Realistic scenario: 1000 canonical nodes, 500 non-canonical, moderate topic usage
    let (state, root) = generate_canonical_with_topics(1000, 500, 3, 10, 42);

    group.bench_function("full_pipeline_1000_nodes", |b| {
        b.iter(|| {
            let mut transitive = TransitiveProcessor::new();
            let mut processor = CanonicalProcessor::new(root);
            let graph = processor.compute(&state, &mut transitive);
            black_box(graph)
        });
    });

    // With pre-warmed transitive cache
    let mut transitive = TransitiveProcessor::new();
    let mut processor = CanonicalProcessor::new(root);
    let _ = processor.compute(&state, &mut transitive); // Warm the cache

    group.bench_function("with_warm_cache_1000_nodes", |b| {
        b.iter(|| {
            // Reset processor state but keep transitive cache
            let mut processor = CanonicalProcessor::new(root);
            let graph = processor.compute(&state, &mut transitive);
            black_box(graph)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_canonical_computation,
    bench_canonical_phase1,
    bench_canonical_with_topics,
    bench_affects_canonical,
    bench_change_detection,
    bench_subtree_filtering,
    bench_end_to_end,
);

criterion_main!(benches);
