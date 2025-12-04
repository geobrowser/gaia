//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events and computes transitive and canonical graphs.

use atlas::events::{SpaceTopologyEvent, SpaceTopologyPayload};
use atlas::graph::{GraphState, TransitiveProcessor};
use atlas::mock_substream::MockSubstream;

fn main() {
    println!("Starting Atlas topology processor...");
    println!();

    // Create mock substream and generate initial topology
    let mut substream = MockSubstream::new();
    let events = substream.generate_topology(10);

    let root_space = substream.root_space_id();

    println!("Generated {} events from mock substream", events.len());
    println!("Root space: {}", hex::encode(root_space));
    println!("Root topic: {}", hex::encode(substream.root_topic_id()));
    println!();

    // Create graph state and transitive processor
    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();

    // Process each event
    println!("=== Processing Events ===");
    for (i, event) in events.iter().enumerate() {
        print_event(i, event);

        // Update transitive cache based on event
        transitive.handle_event(event, &state);

        // Apply event to graph state
        state.apply_event(event);
    }

    println!();
    println!("=== Graph State ===");
    println!("Spaces: {}", state.space_count());
    println!("Explicit edges: {}", state.explicit_edge_count());
    println!("Topic edges: {}", state.topic_edge_count());

    // Compute transitive graphs
    println!();
    println!("=== Transitive Graphs ===");

    // Compute full transitive from root
    let full = transitive.get_full(root_space, &state).clone();
    println!(
        "Root full transitive: {} nodes (hash: {:016x})",
        full.len(),
        full.hash
    );

    // Compute explicit-only transitive from root
    let explicit = transitive.get_explicit_only(root_space, &state).clone();
    println!(
        "Root explicit-only transitive: {} nodes (hash: {:016x})",
        explicit.len(),
        explicit.hash
    );

    // Show cache stats
    let stats = transitive.cache_stats();
    println!();
    println!("=== Cache Stats ===");
    println!("Full graphs cached: {}", stats.full_count);
    println!("Explicit-only graphs cached: {}", stats.explicit_only_count);
    println!("Reverse deps tracked: {}", stats.reverse_deps_count);

    // Print tree structure for root's full transitive
    println!();
    println!("=== Root Full Transitive Tree ===");
    print_tree(&full.tree, 0);
}

/// Print a single topology event
fn print_event(index: usize, event: &SpaceTopologyEvent) {
    let block = event.meta.block_number;

    match &event.payload {
        SpaceTopologyPayload::SpaceCreated(created) => {
            println!(
                "[{:2}] Block {}: SpaceCreated {{ space: {:.8}…, topic: {:.8}… }}",
                index,
                block,
                hex::encode(created.space_id),
                hex::encode(created.topic_id),
            );
        }
        SpaceTopologyPayload::TrustExtended(extended) => {
            let extension_type = match &extended.extension {
                atlas::events::TrustExtension::Verified { target_space_id } => {
                    format!("Verified -> {:.8}…", hex::encode(target_space_id))
                }
                atlas::events::TrustExtension::Related { target_space_id } => {
                    format!("Related -> {:.8}…", hex::encode(target_space_id))
                }
                atlas::events::TrustExtension::Subtopic { target_topic_id } => {
                    format!("Subtopic -> {:.8}…", hex::encode(target_topic_id))
                }
            };
            println!(
                "[{:2}] Block {}: TrustExtended {{ source: {:.8}…, {} }}",
                index,
                block,
                hex::encode(extended.source_space_id),
                extension_type,
            );
        }
    }
}

/// Print a tree node with indentation
fn print_tree(node: &atlas::graph::TreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    let edge_str = match node.edge_type {
        atlas::graph::EdgeType::Root => "ROOT",
        atlas::graph::EdgeType::Verified => "verified",
        atlas::graph::EdgeType::Related => "related",
        atlas::graph::EdgeType::Topic => "topic",
    };

    let topic_str = node
        .topic_id
        .map(|t| format!(" via {:.8}…", hex::encode(t)))
        .unwrap_or_default();

    println!(
        "{}{:.8}… ({}{})",
        indent,
        hex::encode(node.space_id),
        edge_str,
        topic_str
    );

    for child in &node.children {
        print_tree(child, depth + 1);
    }
}
