//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events and computes transitive and canonical graphs.

use atlas::events::{SpaceId, SpaceTopologyEvent, SpaceTopologyPayload};
use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};
use atlas::mock_substream::{MockSubstream, SPACE_P, SPACE_X};

fn main() {
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                     Atlas Topology Processor Demo                            ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();

    // Create mock substream and generate deterministic topology
    let mut substream = MockSubstream::new();
    let events = substream.generate_deterministic_topology();

    let root_space = substream.root_space_id();

    println!("Generated {} events from mock substream", events.len());
    println!("Root space: {}", format_space_id(root_space));
    println!();

    // Create graph state and transitive processor
    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();

    // Process each event
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Processing Events                                                            │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");
    for (i, event) in events.iter().enumerate() {
        print_event(i, event);

        // Update transitive cache based on event
        transitive.handle_event(event, &state);

        // Apply event to graph state
        state.apply_event(event);
    }
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Graph State Summary                                                          │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");
    println!(
        "│ Total spaces:     {:>4}                                                       │",
        state.space_count()
    );
    println!(
        "│ Explicit edges:   {:>4}                                                       │",
        state.explicit_edge_count()
    );
    println!(
        "│ Topic edges:      {:>4}                                                       │",
        state.topic_edge_count()
    );
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    // Compute canonical graph
    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Canonical Graph (from Root)                                                  │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");

    let mut canonical_processor = CanonicalProcessor::new(root_space);
    let canonical = canonical_processor.compute(&state, &mut transitive);

    if let Some(ref graph) = canonical {
        println!(
            "│ Canonical nodes:  {:>4}                                                       │",
            graph.len()
        );
        println!(
            "├──────────────────────────────────────────────────────────────────────────────┤"
        );
        println!(
            "│ Tree Structure:                                                              │"
        );
        print_tree_boxed(&graph.tree, 1);
    }
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    // Show non-canonical transitive graphs
    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Non-Canonical Islands (Transitive Graphs)                                    │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");

    // Island 1: X's transitive graph
    let x_transitive = transitive.get_full(SPACE_X, &state);
    println!(
        "│ Island 1 (from X): {} nodes                                                   │",
        x_transitive.len()
    );
    print_tree_boxed(&x_transitive.tree, 1);
    println!("│                                                                              │");

    // Island 2: P's transitive graph
    let p_transitive = transitive.get_full(SPACE_P, &state);
    println!(
        "│ Island 2 (from P): {} nodes                                                   │",
        p_transitive.len()
    );
    print_tree_boxed(&p_transitive.tree, 1);
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    // Show cache stats
    let stats = transitive.cache_stats();
    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Cache Statistics                                                             │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");
    println!(
        "│ Full graphs cached:         {:>4}                                             │",
        stats.full_count
    );
    println!(
        "│ Explicit-only graphs cached:{:>4}                                             │",
        stats.explicit_only_count
    );
    println!(
        "│ Reverse deps tracked:       {:>4}                                             │",
        stats.reverse_deps_count
    );
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    // Summary of what the demo shows
    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Demo Summary                                                                 │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");
    println!("│ This topology demonstrates:                                                  │");
    println!("│                                                                              │");
    println!("│ 1. CANONICAL GRAPH: 11 spaces reachable from Root via explicit edges         │");
    println!("│    - Root -> A, B (verified), H (related)                                    │");
    println!("│    - A -> C (verified), D (related)                                          │");
    println!("│    - B -> E (verified)                                                       │");
    println!("│    - C -> F (verified), G (related)                                          │");
    println!("│    - H -> I, J (verified)                                                    │");
    println!("│                                                                              │");
    println!("│ 2. TOPIC EDGES: Add connections between canonical nodes                      │");
    println!("│    - B -> topic[H] attaches H's subtree {{I, J}}                               │");
    println!("│    - A -> topic[SHARED] resolves to {{C, G}} (Y filtered out)                  │");
    println!("│                                                                              │");
    println!("│ 3. NON-CANONICAL ISLANDS: Spaces not reachable from Root                     │");
    println!("│    - Island 1: X -> Y -> Z, X -> W (4 nodes)                                 │");
    println!("│    - Island 2: P -> Q (2 nodes)                                              │");
    println!("│    - Island 3: S (isolated, 1 node)                                          │");
    println!("│                                                                              │");
    println!("│ 4. SHARED TOPIC: Topic 0xF0 announced by C, G (canonical) and Y (not)        │");
    println!("│    - When A resolves topic[SHARED], Y is filtered out                        │");
    println!("│                                                                              │");
    println!("│ 5. POINTING TO CANONICAL: W -> Root, X -> topic[A]                           │");
    println!("│    - Having an edge TO canonical doesn't make you canonical                  │");
    println!("└──────────────────────────────────────────────────────────────────────────────┘");
}

/// Format a space ID with a friendly name if known
fn format_space_id(id: SpaceId) -> String {
    let last_byte = id[15];
    let name = match last_byte {
        0x01 => "Root",
        0x0A => "A",
        0x0B => "B",
        0x0C => "C",
        0x0D => "D",
        0x0E => "E",
        0x0F => "F",
        0x10 => "G",
        0x11 => "H",
        0x12 => "I",
        0x13 => "J",
        0x20 => "X",
        0x21 => "Y",
        0x22 => "Z",
        0x23 => "W",
        0x30 => "P",
        0x31 => "Q",
        0x40 => "S",
        _ => return format!("{:.8}…", hex::encode(id)),
    };
    format!("{} (0x{:02x})", name, last_byte)
}

/// Format a topic ID with a friendly name if known
fn format_topic_id(id: &[u8; 16]) -> String {
    let last_byte = id[15];
    let name = match last_byte {
        0x02 => "T_Root",
        0x8A => "T_A",
        0x8B => "T_B",
        0x8C => "T_C",
        0x8D => "T_D",
        0x8E => "T_E",
        0x8F => "T_F",
        0x90 => "T_G",
        0x91 => "T_H",
        0x92 => "T_I",
        0x93 => "T_J",
        0xA0 => "T_X",
        0xA1 => "T_Y",
        0xA2 => "T_Z",
        0xA3 => "T_W",
        0xB0 => "T_P",
        0xB1 => "T_Q",
        0xC0 => "T_S",
        0xF0 => "T_SHARED",
        _ => return format!("{:.8}…", hex::encode(id)),
    };
    format!("{} (0x{:02x})", name, last_byte)
}

/// Print a single topology event
fn print_event(index: usize, event: &SpaceTopologyEvent) {
    match &event.payload {
        SpaceTopologyPayload::SpaceCreated(created) => {
            println!(
                "│ [{:2}] SpaceCreated: {} announces {}",
                index,
                format_space_id(created.space_id),
                format_topic_id(&created.topic_id),
            );
        }
        SpaceTopologyPayload::TrustExtended(extended) => {
            let extension_str = match &extended.extension {
                atlas::events::TrustExtension::Verified { target_space_id } => {
                    format!("──verified──▶ {}", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Related { target_space_id } => {
                    format!("──related──▶ {}", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Subtopic { target_topic_id } => {
                    format!("──topic──▶ {}", format_topic_id(target_topic_id))
                }
            };
            println!(
                "│ [{:2}] TrustExtended: {} {}",
                index,
                format_space_id(extended.source_space_id),
                extension_str,
            );
        }
    }
}

/// Print a tree node with box drawing characters
fn print_tree_boxed(node: &atlas::graph::TreeNode, depth: usize) {
    let indent = "│   ".repeat(depth);
    let edge_str = match node.edge_type {
        atlas::graph::EdgeType::Root => "ROOT",
        atlas::graph::EdgeType::Verified => "verified",
        atlas::graph::EdgeType::Related => "related",
        atlas::graph::EdgeType::Topic => "topic",
    };

    let topic_str = node
        .topic_id
        .map(|t| format!(" via {}", format_topic_id(&t)))
        .unwrap_or_default();

    println!(
        "│ {} {} ({}{}){}",
        indent,
        format_space_id(node.space_id),
        edge_str,
        topic_str,
        " ".repeat(40_usize.saturating_sub(
            indent.len() + format_space_id(node.space_id).len() + edge_str.len() + topic_str.len()
        ))
    );

    for child in &node.children {
        print_tree_boxed(child, depth + 1);
    }
}
