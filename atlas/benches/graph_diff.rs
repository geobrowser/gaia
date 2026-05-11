use atlas::events::SpaceId;
use atlas::graph::{CanonicalGraph, DiffTracker, EdgeType, TreeNode};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;

/// Build a random tree with the given number of nodes
fn build_random_tree(node_count: usize, seed: u64) -> TreeNode {
    let mut rng = StdRng::seed_from_u64(seed);

    // Generate unique space IDs
    let mut ids: Vec<SpaceId> = Vec::with_capacity(node_count);
    let mut seen: HashSet<SpaceId> = HashSet::with_capacity(node_count);
    while ids.len() < node_count {
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes);
        if seen.insert(bytes) {
            ids.push(bytes);
        }
    }

    // Build tree structure: each node (except root) gets a random parent
    // from nodes already in the tree
    // Track all nodes for random parent selection
    // We'll build a flat list and construct tree at the end
    let mut parents: Vec<usize> = vec![0]; // Index of parent for each node
    for i in 1..node_count {
        let parent_idx = rng.gen_range(0..i);
        parents.push(parent_idx);
    }

    // Build nodes with random edge types
    let edge_types = [
        EdgeType::Verified,
        EdgeType::Related,
        EdgeType::Editor,
    ];

    // Build tree recursively from parent indices
    fn add_children(
        node_idx: usize,
        ids: &[SpaceId],
        parents: &[usize],
        edge_types: &[EdgeType],
        rng: &mut StdRng,
    ) -> TreeNode {
        let mut node = if node_idx == 0 {
            TreeNode::new_root(ids[0])
        } else {
            TreeNode::new(
                ids[node_idx],
                edge_types[rng.gen_range(0..edge_types.len())],
            )
        };

        // Find all children of this node
        for (child_idx, &parent_idx) in parents.iter().enumerate() {
            if parent_idx == node_idx && child_idx != node_idx {
                node.add_child(add_children(child_idx, ids, parents, edge_types, rng));
            }
        }

        node
    }

    add_children(0, &ids, &parents, &edge_types, &mut rng)
}

/// Create a CanonicalGraph from a tree
fn tree_to_graph(tree: TreeNode) -> CanonicalGraph {
    // Collect all space IDs from tree
    fn collect_ids(node: &TreeNode, ids: &mut HashSet<SpaceId>) {
        ids.insert(node.space_id);
        for child in &node.children {
            collect_ids(child, ids);
        }
    }

    let mut members = HashSet::new();
    collect_ids(&tree, &mut members);
    let root = tree.space_id;

    CanonicalGraph::new(root, tree, members)
}

/// Mutate a tree by adding/removing/moving nodes
fn mutate_tree(tree: &TreeNode, change_rate: f64, seed: u64) -> TreeNode {
    let mut rng = StdRng::seed_from_u64(seed);

    // Collect all nodes as (space_id, edge_type, children_space_ids)
    fn collect_nodes(node: &TreeNode) -> Vec<(SpaceId, EdgeType, Vec<SpaceId>)> {
        let mut nodes = vec![(
            node.space_id,
            node.edge_type,
            node.children.iter().map(|c| c.space_id).collect(),
        )];
        for child in &node.children {
            nodes.extend(collect_nodes(child));
        }
        nodes
    }

    let mut nodes = collect_nodes(tree);
    let node_count = nodes.len();

    if node_count <= 1 {
        return tree.clone();
    }

    let change_count = ((node_count as f64) * change_rate).max(1.0) as usize;

    // Remove some nodes (not root)
    let remove_count = change_count.min(node_count - 1);
    for _ in 0..remove_count {
        if nodes.len() <= 1 {
            break;
        }
        let idx = rng.gen_range(1..nodes.len());
        let removed_id = nodes[idx].0;
        nodes.remove(idx);

        // Remove from parent's children
        for (_, _, children) in &mut nodes {
            children.retain(|&id| id != removed_id);
        }
    }

    // Add some new nodes
    let add_count = change_count;
    for _ in 0..add_count {
        let mut new_id = [0u8; 16];
        rng.fill(&mut new_id);

        let parent_idx = rng.gen_range(0..nodes.len());
        let parent_id = nodes[parent_idx].0;
        nodes[parent_idx].2.push(new_id);

        let edge_types = [
            EdgeType::Verified,
            EdgeType::Related,
            EdgeType::Editor,
        ];
        nodes.push((new_id, edge_types[rng.gen_range(0..edge_types.len())], vec![]));

        // Ensure parent exists (it should, we just selected it)
        let _ = parent_id;
    }

    // Move some nodes (change parent)
    let move_count = change_count.min(nodes.len() - 1);
    for _ in 0..move_count {
        if nodes.len() <= 1 {
            break;
        }
        let node_idx = rng.gen_range(1..nodes.len());
        let node_id = nodes[node_idx].0;

        // Remove from old parent
        for (_, _, children) in &mut nodes {
            children.retain(|&id| id != node_id);
        }

        // Add to new parent
        let new_parent_idx = rng.gen_range(0..nodes.len());
        if new_parent_idx != node_idx {
            nodes[new_parent_idx].2.push(node_id);
        }
    }

    // Rebuild tree from nodes
    fn rebuild_tree(
        node_id: SpaceId,
        nodes: &[(SpaceId, EdgeType, Vec<SpaceId>)],
        is_root: bool,
    ) -> Option<TreeNode> {
        let (_, edge_type, children_ids) = nodes.iter().find(|(id, _, _)| *id == node_id)?;

        let mut node = if is_root {
            TreeNode::new_root(node_id)
        } else {
            TreeNode::new(node_id, *edge_type)
        };

        for child_id in children_ids {
            if let Some(child) = rebuild_tree(*child_id, nodes, false) {
                node.add_child(child);
            }
        }

        Some(node)
    }

    rebuild_tree(nodes[0].0, &nodes, true).unwrap_or_else(|| tree.clone())
}

/// Benchmark the actual DiffTracker implementation
fn bench_diff_tracker(c: &mut Criterion) {
    let sizes = [1_000, 10_000, 50_000, 100_000];

    // Benchmark: DiffTracker.track() with varying graph sizes
    {
        let mut group = c.benchmark_group("diff_tracker_track");
        group.sample_size(50);

        for size in sizes {
            group.throughput(Throughput::Elements(size as u64));

            let tree = build_random_tree(size, 42);
            let graph = tree_to_graph(tree);
            assert_eq!(graph.tree.node_count(), size, "tree has wrong node count");

            // Benchmark first call (bootstrap - all nodes ADDED)
            group.bench_with_input(BenchmarkId::new("bootstrap", size), &graph, |b, graph| {
                b.iter(|| {
                    let mut tracker = DiffTracker::new();
                    let diff = tracker.track(black_box(graph));
                    black_box(diff)
                })
            });

            // Benchmark subsequent call with no changes (empty diff)
            group.bench_with_input(BenchmarkId::new("no_change", size), &graph, |b, graph| {
                b.iter_batched(
                    || {
                        let mut tracker = DiffTracker::new();
                        tracker.track(graph);
                        tracker
                    },
                    |mut tracker| {
                        let diff = tracker.track(black_box(graph));
                        black_box(diff)
                    },
                    criterion::BatchSize::SmallInput,
                )
            });
        }
        group.finish();
    }

    // Benchmark: DiffTracker with different change rates
    {
        let mut group = c.benchmark_group("diff_tracker_change_rates");
        group.sample_size(50);

        let size = 100_000;
        let rates = [0.001, 0.01, 0.05, 0.1];

        let base_tree = build_random_tree(size, 42);
        let base_graph = tree_to_graph(base_tree.clone());

        for rate in rates {
            let mutated_tree = mutate_tree(&base_tree, rate, 123);
            let mutated_graph = tree_to_graph(mutated_tree);

            group.bench_with_input(
                BenchmarkId::new("rate", format!("{:.1}%", rate * 100.0)),
                &(base_graph.clone(), mutated_graph),
                |b, (base, mutated)| {
                    b.iter_batched(
                        || {
                            let mut tracker = DiffTracker::new();
                            tracker.track(base);
                            tracker
                        },
                        |mut tracker| {
                            let diff = tracker.track(black_box(mutated));
                            black_box(diff)
                        },
                        criterion::BatchSize::SmallInput,
                    )
                },
            );
        }
        group.finish();
    }

    // Benchmark: Allocation reuse effectiveness
    {
        let mut group = c.benchmark_group("diff_tracker_allocation_reuse");
        group.sample_size(50);

        let size = 100_000;
        let base_tree = build_random_tree(size, 42);
        let base_graph = tree_to_graph(base_tree.clone());

        // Compare: new tracker each time vs reused tracker
        group.bench_function("new_tracker_each_call", |b| {
            b.iter(|| {
                let mut tracker = DiffTracker::new();
                let diff = tracker.track(black_box(&base_graph));
                black_box(diff)
            })
        });

        group.bench_function("with_capacity", |b| {
            b.iter(|| {
                let mut tracker = DiffTracker::with_capacity(size);
                let diff = tracker.track(black_box(&base_graph));
                black_box(diff)
            })
        });

        // Measure multiple sequential calls (should get faster after warmup)
        let graphs: Vec<_> = (0..5)
            .map(|i| {
                let tree = mutate_tree(&base_tree, 0.01, 100 + i);
                tree_to_graph(tree)
            })
            .collect();

        group.bench_function("sequential_5_calls", |b| {
            b.iter(|| {
                let mut tracker = DiffTracker::new();
                for graph in &graphs {
                    let diff = tracker.track(black_box(graph));
                    black_box(&diff);
                }
            })
        });

        group.finish();
    }
}

criterion_group!(benches, bench_diff_tracker);
criterion_main!(benches);
