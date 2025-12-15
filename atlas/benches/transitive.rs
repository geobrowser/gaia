//! Benchmarks for transitive graph computation
//!
//! Run with: cargo bench -p atlas

use atlas::events::{
    BlockMetadata, SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, SpaceType,
    TopicId, TrustExtended, TrustExtension,
};
use atlas::graph::{hash_tree, memory, GraphState, TransitiveProcessor};
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

/// Generate a linear chain: 0 -> 1 -> 2 -> ... -> n-1
fn generate_linear_chain(n: u32) -> (GraphState, SpaceId) {
    let mut state = GraphState::new();
    let mut spaces = Vec::with_capacity(n as usize);

    for i in 0..n {
        spaces.push(create_space(&mut state, i));
    }

    for i in 0..(n - 1) {
        add_verified_edge(&mut state, spaces[i as usize], spaces[(i + 1) as usize]);
    }

    (state, spaces[0])
}

/// Generate a wide graph: root -> [1, 2, 3, ..., n-1]
fn generate_wide_graph(n: u32) -> (GraphState, SpaceId) {
    let mut state = GraphState::new();
    let root = create_space(&mut state, 0);

    for i in 1..n {
        let space = create_space(&mut state, i);
        add_verified_edge(&mut state, root, space);
    }

    (state, root)
}

/// Generate a binary tree of depth d (2^(d+1) - 1 nodes)
fn generate_binary_tree(depth: u32) -> (GraphState, SpaceId) {
    let mut state = GraphState::new();
    let mut counter = 0u32;

    fn build_tree(
        state: &mut GraphState,
        counter: &mut u32,
        depth: u32,
        current_depth: u32,
    ) -> SpaceId {
        let space = create_space(state, *counter);
        *counter += 1;

        if current_depth < depth {
            let left = build_tree(state, counter, depth, current_depth + 1);
            let right = build_tree(state, counter, depth, current_depth + 1);
            add_verified_edge(state, space, left);
            add_verified_edge(state, space, right);
        }

        space
    }

    let root = build_tree(&mut state, &mut counter, depth, 0);
    (state, root)
}

/// Generate a random graph with n nodes and e edges
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

/// Generate a graph with topic edges
fn generate_graph_with_topics(n: u32, topic_edges: u32, seed: u64) -> (GraphState, SpaceId) {
    let mut state = GraphState::new();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut spaces = Vec::with_capacity(n as usize);

    for i in 0..n {
        spaces.push(create_space(&mut state, i));
    }

    // Add some explicit edges to form a base structure
    for i in 0..(n / 2) {
        let target = rng.gen_range(0..n) as usize;
        if i as usize != target {
            add_verified_edge(&mut state, spaces[i as usize], spaces[target]);
        }
    }

    // Add topic edges
    for _ in 0..topic_edges {
        let source = rng.gen_range(0..n) as usize;
        let target_space = rng.gen_range(0..n) as usize;
        // Get the topic announced by target_space
        let topic = make_topic_id(target_space as u32);
        add_topic_edge(&mut state, spaces[source], topic);
    }

    (state, spaces[0])
}

// ============================================================================
// Benchmarks
// ============================================================================

fn bench_bfs_linear_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("bfs_linear_chain");

    for size in [100, 500, 1000, 5000] {
        let (state, root) = generate_linear_chain(size);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let mut processor = TransitiveProcessor::new();
                let graph = processor.get_full(root, &state);
                black_box(graph.len())
            });
        });
    }

    group.finish();
}

fn bench_bfs_wide_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("bfs_wide_graph");

    for size in [100, 500, 1000, 5000] {
        let (state, root) = generate_wide_graph(size);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let mut processor = TransitiveProcessor::new();
                let graph = processor.get_full(root, &state);
                black_box(graph.len())
            });
        });
    }

    group.finish();
}

fn bench_bfs_binary_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("bfs_binary_tree");

    // depth 6 = 127 nodes, depth 8 = 511 nodes, depth 10 = 2047 nodes, depth 12 = 8191 nodes
    for depth in [6, 8, 10, 12] {
        let (state, root) = generate_binary_tree(depth);
        let node_count = (1 << (depth + 1)) - 1;

        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(node_count), &depth, |b, _| {
            b.iter(|| {
                let mut processor = TransitiveProcessor::new();
                let graph = processor.get_full(root, &state);
                black_box(graph.len())
            });
        });
    }

    group.finish();
}

fn bench_bfs_random_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("bfs_random_graph");

    // (nodes, edges)
    for (nodes, edges) in [(100, 200), (500, 1500), (1000, 5000), (5000, 20000)] {
        let (state, root) = generate_random_graph(nodes, edges, 42);

        group.throughput(Throughput::Elements(nodes as u64));
        group.bench_with_input(
            BenchmarkId::new("nodes", format!("{}_edges_{}", nodes, edges)),
            &nodes,
            |b, _| {
                b.iter(|| {
                    let mut processor = TransitiveProcessor::new();
                    let graph = processor.get_full(root, &state);
                    black_box(graph.len())
                });
            },
        );
    }

    group.finish();
}

fn bench_full_vs_explicit_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_vs_explicit_only");

    let (state, root) = generate_graph_with_topics(1000, 200, 42);

    group.bench_function("full_transitive", |b| {
        b.iter(|| {
            let mut processor = TransitiveProcessor::new();
            let graph = processor.get_full(root, &state);
            black_box(graph.len())
        });
    });

    group.bench_function("explicit_only_transitive", |b| {
        b.iter(|| {
            let mut processor = TransitiveProcessor::new();
            let graph = processor.get_explicit_only(root, &state);
            black_box(graph.len())
        });
    });

    group.finish();
}

fn bench_cache_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache");

    let (state, root) = generate_random_graph(1000, 5000, 42);

    // Benchmark cache miss (first computation)
    group.bench_function("miss_1000_nodes", |b| {
        b.iter(|| {
            let mut processor = TransitiveProcessor::new();
            let graph = processor.get_full(root, &state);
            black_box(graph.len())
        });
    });

    // Benchmark cache hit
    let mut processor = TransitiveProcessor::new();
    let _ = processor.get_full(root, &state); // Prime the cache

    group.bench_function("hit_1000_nodes", |b| {
        b.iter(|| {
            let graph = processor.get_full(root, &state);
            black_box(graph.len())
        });
    });

    group.finish();
}

fn bench_tree_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("tree_hashing");

    for size in [100, 500, 1000, 5000] {
        let (state, root) = generate_wide_graph(size);
        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(root, &state);

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| black_box(hash_tree(&graph.tree)));
        });
    }

    group.finish();
}

fn bench_graph_state_event_application(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_state_events");

    // Benchmark space creation
    group.bench_function("space_created", |b| {
        b.iter_batched(
            GraphState::new,
            |mut state| {
                for i in 0..100 {
                    create_space(&mut state, i);
                }
                black_box(state)
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // Benchmark edge addition
    group.bench_function("edge_addition", |b| {
        b.iter_batched(
            || {
                let mut state = GraphState::new();
                for i in 0..100 {
                    create_space(&mut state, i);
                }
                state
            },
            |mut state| {
                for i in 0..99 {
                    add_verified_edge(&mut state, make_space_id(i), make_space_id(i + 1));
                }
                black_box(state)
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_invalidation(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_invalidation");

    let (state, root) = generate_random_graph(1000, 5000, 42);

    // Prime the cache with multiple transitive graphs
    let mut processor = TransitiveProcessor::new();
    let _ = processor.get_full(root, &state);

    // Also compute transitive graphs for other nodes
    for i in 1..10 {
        let _ = processor.get_full(make_space_id(i), &state);
    }

    group.bench_function("invalidate_on_edge_add", |b| {
        b.iter_batched(
            || processor.clone(),
            |mut proc| {
                // Simulate adding an edge event
                let event = SpaceTopologyEvent {
                    meta: make_block_meta(),
                    payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                        source_space_id: make_space_id(5),
                        extension: TrustExtension::Verified {
                            target_space_id: make_space_id(500),
                        },
                    }),
                };
                proc.handle_event(&event, &state);
                black_box(proc)
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ============================================================================
// Memory size benchmarks (not timing benchmarks - just measurements)
// ============================================================================

/// Print memory usage statistics for various graph sizes
/// This is not a timing benchmark - it measures and reports memory usage
fn bench_memory_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_sizes");

    // We use a trivial benchmark that just measures memory, not time
    // The actual memory values are printed to stdout

    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                         MEMORY SIZE MEASUREMENTS                             ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");

    // GraphState memory at different scales
    println!("║                                                                              ║");
    println!("║  GraphState Memory                                                           ║");
    println!("║  ─────────────────                                                           ║");
    println!(
        "║  {:>8} {:>12} {:>12} {:>12} {:>12}                   ║",
        "Nodes", "Edges", "Total", "Per Node", "Per Edge"
    );

    for (nodes, edges_per_node) in [(100, 2), (1_000, 4), (5_000, 4), (10_000, 4), (50_000, 4)] {
        let (state, _root) = generate_random_graph(nodes, nodes * edges_per_node, 42);
        let mem = memory::graph_state_size(&state);
        let total_edges = state.explicit_edge_count();
        let per_node = if nodes > 0 {
            mem.total_bytes / nodes as usize
        } else {
            0
        };
        let per_edge = if total_edges > 0 {
            mem.total_bytes / total_edges
        } else {
            0
        };

        println!(
            "║  {:>8} {:>12} {:>12} {:>12} {:>12}                   ║",
            nodes,
            total_edges,
            memory::format_bytes(mem.total_bytes),
            memory::format_bytes(per_node),
            memory::format_bytes(per_edge)
        );
    }

    // TransitiveGraph memory at different scales
    println!("║                                                                              ║");
    println!("║  TransitiveGraph Memory (single graph)                                       ║");
    println!("║  ────────────────────────────────────────                                    ║");
    println!(
        "║  {:>8} {:>12} {:>12} {:>12}                             ║",
        "Nodes", "Total", "Tree", "FlatSet"
    );

    for nodes in [100, 1_000, 5_000, 10_000] {
        let (state, root) = generate_linear_chain(nodes);
        let mut processor = TransitiveProcessor::new();
        let graph = processor.get_full(root, &state);
        let mem = memory::transitive_graph_size(graph);

        println!(
            "║  {:>8} {:>12} {:>12} {:>12}                             ║",
            graph.len(),
            memory::format_bytes(mem.total_bytes),
            memory::format_bytes(mem.tree_bytes),
            memory::format_bytes(mem.flat_set_bytes)
        );
    }

    // Cache memory with multiple cached graphs
    println!("║                                                                              ║");
    println!("║  Cache Memory (multiple cached graphs)                                       ║");
    println!("║  ──────────────────────────────────────                                      ║");
    println!(
        "║  {:>8} {:>12} {:>14} {:>12}                           ║",
        "Graphs", "Nodes/Graph", "Cache Total", "Per Graph"
    );

    for (graph_count, nodes_per_graph) in [(10, 100), (100, 100), (10, 1_000), (100, 1_000)] {
        let (state, _) = generate_random_graph(
            graph_count * nodes_per_graph,
            graph_count * nodes_per_graph * 2,
            42,
        );
        let mut processor = TransitiveProcessor::new();

        // Cache multiple graphs
        for i in 0..graph_count {
            let space = make_space_id(i);
            let _ = processor.get_full(space, &state);
        }

        let cache_bytes = processor.cache_memory_bytes();
        let per_graph = if graph_count > 0 {
            cache_bytes / graph_count as usize
        } else {
            0
        };

        println!(
            "║  {:>8} {:>12} {:>14} {:>12}                           ║",
            graph_count,
            nodes_per_graph,
            memory::format_bytes(cache_bytes),
            memory::format_bytes(per_graph)
        );
    }

    println!("║                                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();

    // Run a trivial benchmark so criterion doesn't complain
    group.bench_function("memory_measurement_overhead", |b| {
        let state = GraphState::new();
        b.iter(|| black_box(memory::graph_state_size(&state)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_bfs_linear_chain,
    bench_bfs_wide_graph,
    bench_bfs_binary_tree,
    bench_bfs_random_graph,
    bench_full_vs_explicit_only,
    bench_cache_hit,
    bench_tree_hashing,
    bench_graph_state_event_application,
    bench_invalidation,
    bench_memory_sizes,
);

criterion_main!(benches);
