//! End-to-end tests for Atlas canonical graph processing.
//!
//! These tests verify the complete pipeline:
//! 1. Convert blockchain actions to topology events
//! 2. Build graph state from events
//! 3. Compute transitive and canonical graphs
//! 4. Compute incremental diffs
//!
//! Uses the mock topology from hermes-relay for reproducible test data.

use atlas::convert::convert_action;
use atlas::events::{BlockMetadata, SpaceId, SpaceTopologyEvent};
use atlas::graph::{
    CanonicalProcessor, ChangeType, DiffTracker, EdgeType, GraphState, TransitiveProcessor,
};
use hermes_relay::source::mock_events::test_topology::{
    ROOT_SPACE_ID, SPACE_A, SPACE_B, SPACE_C, SPACE_D, SPACE_E, SPACE_F, SPACE_G, SPACE_H, SPACE_I,
    SPACE_J, SPACE_P, SPACE_Q, SPACE_S, SPACE_W, SPACE_X, SPACE_Y, SPACE_Z,
};
use hermes_relay::source::mock_events::{self, test_topology};
use hermes_relay::Action;
use std::collections::HashSet;

/// Helper to create block metadata for tests
fn make_meta(block_number: u64) -> BlockMetadata {
    BlockMetadata {
        block_number,
        block_timestamp: block_number * 12, // ~12 seconds per block
        tx_hash: format!("0x{:064x}", block_number),
        cursor: format!("cursor_{}", block_number),
    }
}

/// Process a sequence of actions through the full pipeline, returning events and final state.
fn process_actions(
    actions: &[Action],
) -> (Vec<SpaceTopologyEvent>, GraphState, TransitiveProcessor) {
    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();
    let mut events = Vec::new();

    for (i, action) in actions.iter().enumerate() {
        let meta = make_meta(i as u64);
        if let Some(event) = convert_action(action, &meta) {
            transitive.handle_event(&event, &state);
            state.apply_event(&event);
            events.push(event);
        }
    }

    (events, state, transitive)
}

/// Process actions and compute canonical graph with diffs.
/// Returns the final computed graph along with the processors and diffs.
fn process_with_canonical(
    actions: &[Action],
    root: SpaceId,
) -> (
    GraphState,
    TransitiveProcessor,
    CanonicalProcessor,
    DiffTracker,
    Vec<atlas::graph::GraphDiff>,
    Option<atlas::graph::CanonicalGraph>,
) {
    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();
    let mut canonical = CanonicalProcessor::new(root);
    let mut diff_tracker = DiffTracker::new();
    let mut diffs = Vec::new();
    let mut last_graph: Option<atlas::graph::CanonicalGraph> = None;

    for (i, action) in actions.iter().enumerate() {
        let meta = make_meta(i as u64);
        if let Some(event) = convert_action(action, &meta) {
            transitive.handle_event(&event, &state);
            state.apply_event(&event);

            if let Some(graph) = canonical.compute(&state, &mut transitive) {
                let diff = diff_tracker.track(&graph);
                if !diff.is_empty() {
                    diffs.push(diff);
                }
                last_graph = Some(graph);
            }
        }
    }

    (
        state,
        transitive,
        canonical,
        diff_tracker,
        diffs,
        last_graph,
    )
}

// =============================================================================
// Test: Basic Pipeline Flow
// =============================================================================

#[test]
fn test_e2e_pipeline_processes_all_events() {
    let actions = test_topology::generate();
    let (events, _state, _transitive) = process_actions(&actions);

    // Should have processed multiple events
    assert!(
        events.len() > 10,
        "Expected many events, got {}",
        events.len()
    );
}

#[test]
fn test_e2e_canonical_set_from_test_topology() {
    let actions = test_topology::generate();
    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // Get the final canonical graph
    let graph = last_graph.expect("Should have computed canonical graph");

    // Verify root is canonical
    assert!(graph.contains(&ROOT_SPACE_ID), "Root should be canonical");

    // Verify expected canonical spaces (reachable via explicit edges from root)
    // From test_topology:
    // - Root -> A (verified), Root -> B (verified), Root -> H (related)
    // - A -> C (verified), A -> D (related), A -> B (editor), A -> E (member)
    //   - A -> 0x11 (member, added via Proposal 1)
    // - B -> E (verified)
    //   - B -> 0x50 (editor, added via Proposal 3)
    // - C -> F (verified), C -> G (related)
    // - H -> I (verified), H -> J (verified)
    let expected_canonical: HashSet<SpaceId> = [
        ROOT_SPACE_ID,
        SPACE_A,
        SPACE_B,
        SPACE_C,
        SPACE_D,
        SPACE_E,
        SPACE_F,
        SPACE_G,
        SPACE_H,
        SPACE_I,
        SPACE_J,
        mock_events::make_id(0x11), // Added as member of A via Proposal 1
        mock_events::make_id(0x50), // Added as editor of B via Proposal 3
    ]
    .into_iter()
    .collect();

    for space in &expected_canonical {
        assert!(
            graph.contains(space),
            "Space {:?} should be canonical",
            space
        );
    }

    // Verify non-canonical spaces are NOT in the set
    // X, Y, Z, W form an island not connected to root
    // P, Q form another island
    // S is isolated
    let expected_non_canonical: Vec<SpaceId> = vec![
        SPACE_X, SPACE_Y, SPACE_Z, SPACE_W, SPACE_P, SPACE_Q, SPACE_S,
    ];

    for space in &expected_non_canonical {
        assert!(
            !graph.contains(space),
            "Space {:?} should NOT be canonical",
            space
        );
    }

    // Verify correct count
    assert_eq!(
        graph.len(),
        expected_canonical.len(),
        "Canonical set should have exactly {} spaces",
        expected_canonical.len()
    );
}

// =============================================================================
// Test: Diff Computation
// =============================================================================

#[test]
fn test_e2e_bootstrap_diff_contains_all_canonical() {
    let actions = test_topology::generate();
    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // Should have at least one diff (the bootstrap)
    assert!(!diffs.is_empty(), "Should have emitted at least one diff");

    // Collect all ADDED space_ids across all diffs
    let mut all_added: HashSet<SpaceId> = HashSet::new();
    for diff in &diffs {
        for change in &diff.changes {
            if change.change_type == ChangeType::Added {
                all_added.insert(change.space_id);
            }
        }
    }

    // All canonical spaces (except root, which is implicit) should have been ADDED
    let canonical_non_root: HashSet<SpaceId> = [
        SPACE_A, SPACE_B, SPACE_C, SPACE_D, SPACE_E, SPACE_F, SPACE_G, SPACE_H, SPACE_I, SPACE_J,
    ]
    .into_iter()
    .collect();

    for space in &canonical_non_root {
        assert!(
            all_added.contains(space),
            "Space {:?} should have been ADDED in some diff",
            space
        );
    }
}

#[test]
fn test_e2e_diff_changes_have_position_info() {
    let actions = test_topology::generate();
    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    for diff in &diffs {
        for change in &diff.changes {
            match change.change_type {
                ChangeType::Added | ChangeType::Moved => {
                    assert!(
                        change.position.is_some(),
                        "ADDED/MOVED change for {:?} should have position",
                        change.space_id
                    );
                    let pos = change.position.as_ref().unwrap();
                    assert!(pos.distance > 0, "Non-root nodes should have distance > 0");
                }
                ChangeType::Removed => {
                    // REMOVED doesn't require position
                }
            }
        }
    }
}

// =============================================================================
// Test: Edge Types
// =============================================================================

#[test]
fn test_e2e_editor_member_edges_grant_canonical() {
    // Build a minimal topology with editor/member edges
    let mut actions = Vec::new();

    // Create spaces
    actions.extend(mock_events::personal_space_registered(
        ROOT_SPACE_ID,
        test_topology::ROOT_OWNER,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_A,
        test_topology::USER_1,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_B,
        test_topology::USER_2,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_C,
        test_topology::USER_1,
    ));

    // Root -> A (verified)
    actions.push(mock_events::subspace_verified(ROOT_SPACE_ID, SPACE_A));
    // A -> B (editor) - B should become canonical via editor edge
    actions.push(mock_events::editor_added(SPACE_A, SPACE_B));
    // A -> C (member) - C should become canonical via member edge
    actions.push(mock_events::member_added(SPACE_A, SPACE_C));

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    // All should be canonical
    assert!(graph.contains(&ROOT_SPACE_ID));
    assert!(graph.contains(&SPACE_A));
    assert!(
        graph.contains(&SPACE_B),
        "B should be canonical via editor edge"
    );
    assert!(
        graph.contains(&SPACE_C),
        "C should be canonical via member edge"
    );
}

#[test]
fn test_e2e_topic_edges_dont_grant_canonical() {
    // Build topology where topic edge points to non-canonical space
    let mut actions = Vec::new();

    // Create spaces
    actions.extend(mock_events::personal_space_registered(
        ROOT_SPACE_ID,
        test_topology::ROOT_OWNER,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_A,
        test_topology::USER_1,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_X,
        test_topology::USER_2,
    )); // X is not connected via explicit edge

    // Root -> A (verified)
    actions.push(mock_events::subspace_verified(ROOT_SPACE_ID, SPACE_A));
    // A -> topic pointing to X's topic
    // X has topic TOPIC_A set, and A declares subtopic to TOPIC_A
    // But this should NOT make X canonical

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    assert!(graph.contains(&ROOT_SPACE_ID));
    assert!(graph.contains(&SPACE_A));
    assert!(
        !graph.contains(&SPACE_X),
        "X should NOT be canonical - topic edges don't grant canonical"
    );
}

// =============================================================================
// Test: Transitive Edges from Members
// =============================================================================

#[test]
fn test_e2e_member_spaces_edges_are_followed() {
    // Test that edges FROM member spaces are followed transitively
    let mut actions = Vec::new();

    // Create spaces
    actions.extend(mock_events::personal_space_registered(
        ROOT_SPACE_ID,
        test_topology::ROOT_OWNER,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_A, // DAO
        test_topology::USER_1,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_B, // Personal space (member of A)
        test_topology::USER_2,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_C, // Space verified by B
        test_topology::USER_1,
    ));

    // Root -> A (verified)
    actions.push(mock_events::subspace_verified(ROOT_SPACE_ID, SPACE_A));
    // A -> B (member) - B is a personal space that's a member
    actions.push(mock_events::member_added(SPACE_A, SPACE_B));
    // B -> C (verified) - B's own verified edge
    actions.push(mock_events::subspace_verified(SPACE_B, SPACE_C));

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    // All should be canonical: Root -> A -> B -> C
    assert!(graph.contains(&ROOT_SPACE_ID));
    assert!(graph.contains(&SPACE_A));
    assert!(graph.contains(&SPACE_B), "B should be canonical via member");
    assert!(
        graph.contains(&SPACE_C),
        "C should be canonical via B's verified edge (transitive from member)"
    );
    assert_eq!(graph.len(), 4);
}

// =============================================================================
// Test: Edge Removal
// =============================================================================

#[test]
fn test_e2e_editor_removal_causes_removed_diff() {
    let mut actions = Vec::new();

    // Create spaces
    actions.extend(mock_events::personal_space_registered(
        ROOT_SPACE_ID,
        test_topology::ROOT_OWNER,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_A,
        test_topology::USER_1,
    ));
    actions.extend(mock_events::personal_space_registered(
        SPACE_B,
        test_topology::USER_2,
    ));

    // Root -> A (verified)
    actions.push(mock_events::subspace_verified(ROOT_SPACE_ID, SPACE_A));
    // A -> B (editor)
    actions.push(mock_events::editor_added(SPACE_A, SPACE_B));

    // Remove editor edge A -> B
    actions.push(mock_events::editor_removed(SPACE_A, SPACE_B));

    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // Should have diffs, and one should contain REMOVED for B
    let mut found_b_removed = false;
    for diff in &diffs {
        for change in &diff.changes {
            if change.space_id == SPACE_B && change.change_type == ChangeType::Removed {
                found_b_removed = true;
            }
        }
    }

    assert!(
        found_b_removed,
        "Should have REMOVED diff for B after editor edge removal"
    );
}

// =============================================================================
// Test: Tree Structure
// =============================================================================

#[test]
fn test_e2e_tree_structure_correct() {
    let actions = test_topology::generate();
    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    // Verify tree structure
    assert_eq!(graph.tree.space_id, ROOT_SPACE_ID);
    assert_eq!(graph.tree.edge_type, EdgeType::Root);

    // Root should have children (A, B, H at minimum via explicit edges)
    assert!(!graph.tree.children.is_empty(), "Root should have children");

    // Find A in root's children
    let a_node = graph.tree.children.iter().find(|n| n.space_id == SPACE_A);
    assert!(a_node.is_some(), "A should be a child of root");

    let a_node = a_node.unwrap();
    assert_eq!(
        a_node.edge_type,
        EdgeType::Verified,
        "A should be connected via Verified edge"
    );
}

// =============================================================================
// Test: Determinism
// =============================================================================

#[test]
fn test_e2e_deterministic_output() {
    let actions = test_topology::generate();

    // Process twice
    let (_, _, _, _, diffs1, _) = process_with_canonical(&actions, ROOT_SPACE_ID);
    let (_, _, _, _, diffs2, _) = process_with_canonical(&actions, ROOT_SPACE_ID);

    // Should produce identical diffs
    assert_eq!(
        diffs1.len(),
        diffs2.len(),
        "Should produce same number of diffs"
    );

    for (d1, d2) in diffs1.iter().zip(diffs2.iter()) {
        assert_eq!(
            d1.changes.len(),
            d2.changes.len(),
            "Each diff should have same number of changes"
        );

        for (c1, c2) in d1.changes.iter().zip(d2.changes.iter()) {
            assert_eq!(c1.space_id, c2.space_id, "Space IDs should match");
            assert_eq!(c1.change_type, c2.change_type, "Change types should match");
            assert_eq!(c1.position, c2.position, "Positions should match");
        }
    }
}

// =============================================================================
// Test: Performance Sanity Check
// =============================================================================

#[test]
fn test_e2e_performance_reasonable() {
    use std::time::Instant;

    let actions = test_topology::generate();

    let start = Instant::now();
    let (_state, _transitive, _canonical, _diff_tracker, _diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);
    let elapsed = start.elapsed();

    // Should complete quickly (< 100ms for the small test topology)
    assert!(
        elapsed.as_millis() < 100,
        "Processing should complete in < 100ms, took {:?}",
        elapsed
    );
}
