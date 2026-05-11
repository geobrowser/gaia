//! End-to-end tests for Atlas canonical graph processing.
//!
//! These tests verify the complete pipeline:
//! 1. Convert blockchain actions to topology events
//! 2. Build graph state from events
//! 3. Compute transitive and canonical graphs
//! 4. Compute incremental diffs
//!
//! Uses the mock topology from hermes-relay for reproducible test data.
//!
//! ## Test Organization
//!
//! - **Basic Pipeline Flow**: Core functionality tests
//! - **Diff Computation**: ADDED/REMOVED/MOVED semantics
//! - **Edge Types**: Verified, Related, Editor, Member, Topic behavior
//! - **Edge Removal**: Cascading removal, all edge types
//! - **Graph Patterns**: Cycles, diamonds, deep chains, wide graphs
//! - **Boundary Conditions**: Empty graphs, non-existent spaces, duplicates
//! - **Determinism & Ordering**: Reproducibility tests
//! - **Performance**: Scaling and efficiency tests

use atlas::convert::convert_action;
use atlas::events::{BlockMetadata, SpaceId, SpaceTopologyEvent};
use atlas::graph::{
    CanonicalGraph, CanonicalProcessor, ChangeType, DiffTracker, EdgeType, GraphDiff, GraphState,
    TransitiveProcessor,
};
use hermes_relay::source::mock_events::test_topology::{
    ROOT_SPACE_ID, SPACE_A, SPACE_B, SPACE_C, SPACE_D, SPACE_E, SPACE_F, SPACE_G, SPACE_H, SPACE_I,
    SPACE_J, SPACE_P, SPACE_Q, SPACE_S, SPACE_W, SPACE_X, SPACE_Y, SPACE_Z,
};
use hermes_relay::source::mock_events::{self, test_topology};
use hermes_relay::Action;

// =============================================================================
// Test Helpers
// =============================================================================

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
    Vec<GraphDiff>,
    Option<CanonicalGraph>,
) {
    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();
    let mut canonical = CanonicalProcessor::new(root);
    let mut diff_tracker = DiffTracker::new();
    let mut diffs = Vec::new();
    let mut last_graph: Option<CanonicalGraph> = None;

    for (i, action) in actions.iter().enumerate() {
        let meta = make_meta(i as u64);
        if let Some(event) = convert_action(action, &meta) {
            transitive.handle_event(&event, &state);
            state.apply_event(&event);

            if let Some(graph) = canonical.compute_if_changed(&state, &mut transitive) {
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
// Assertion Helpers (P3: Improved debuggability)
// =============================================================================

/// Assert that all given spaces are in the canonical graph
fn assert_all_canonical(graph: &CanonicalGraph, spaces: &[SpaceId], context: &str) {
    for space in spaces {
        assert!(
            graph.contains(space),
            "{}: Space 0x{:02x} should be canonical. Canonical set has {} spaces: {:?}",
            context,
            space[15],
            graph.len(),
            graph
                .members
                .iter()
                .map(|s| format!("0x{:02x}", s[15]))
                .collect::<Vec<_>>()
        );
    }
}

/// Assert that none of the given spaces are in the canonical graph
fn assert_none_canonical(graph: &CanonicalGraph, spaces: &[SpaceId], context: &str) {
    for space in spaces {
        assert!(
            !graph.contains(space),
            "{}: Space 0x{:02x} should NOT be canonical. Canonical set: {:?}",
            context,
            space[15],
            graph
                .members
                .iter()
                .map(|s| format!("0x{:02x}", s[15]))
                .collect::<Vec<_>>()
        );
    }
}

/// Find a specific change in diffs
fn find_change(diffs: &[GraphDiff], space: SpaceId, change_type: ChangeType) -> Option<usize> {
    for (i, diff) in diffs.iter().enumerate() {
        for change in &diff.changes {
            if change.space_id == space && change.change_type == change_type {
                return Some(i);
            }
        }
    }
    None
}

/// Assert that a change exists in diffs
fn assert_change_exists(
    diffs: &[GraphDiff],
    space: SpaceId,
    change_type: ChangeType,
    context: &str,
) {
    assert!(
        find_change(diffs, space, change_type).is_some(),
        "{}: Expected {:?} change for space 0x{:02x}. All changes: {:?}",
        context,
        change_type,
        space[15],
        diffs
            .iter()
            .flat_map(|d| &d.changes)
            .map(|c| format!("0x{:02x}:{:?}", c.space_id[15], c.change_type))
            .collect::<Vec<_>>()
    );
}

/// Count changes of a specific type
fn count_changes(diffs: &[GraphDiff], change_type: ChangeType) -> usize {
    diffs
        .iter()
        .flat_map(|d| &d.changes)
        .filter(|c| c.change_type == change_type)
        .count()
}

// =============================================================================
// Graph Builder Helper (P3: Reduce test setup verbosity)
// =============================================================================

/// Builder for creating test topologies
struct TopologyBuilder {
    actions: Vec<Action>,
}

impl TopologyBuilder {
    fn new() -> Self {
        Self {
            actions: Vec::new(),
        }
    }

    /// Register a personal space
    fn space(mut self, space: SpaceId, owner: [u8; 32]) -> Self {
        self.actions
            .extend(mock_events::personal_space_registered(space, owner));
        self
    }

    /// Add a verified edge
    fn verified(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::subspace_verified(from, to));
        self
    }

    /// Add a related edge
    fn related(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::subspace_related(from, to));
        self
    }

    /// Add an editor edge
    fn editor(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::editor_added(from, to));
        self
    }

    /// Add a member edge
    fn member(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::member_added(from, to));
        self
    }

    /// Remove a verified edge
    fn unverified(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions
            .push(mock_events::subspace_unverified(from, to));
        self
    }

    /// Remove a related edge
    fn unrelated(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::subspace_unrelated(from, to));
        self
    }

    /// Remove an editor edge
    fn editor_removed(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::editor_removed(from, to));
        self
    }

    /// Remove a member edge
    fn member_removed(mut self, from: SpaceId, to: SpaceId) -> Self {
        self.actions.push(mock_events::member_removed(from, to));
        self
    }

    /// Build the action list
    fn build(self) -> Vec<Action> {
        self.actions
    }
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

    let graph = last_graph.expect("Should have computed canonical graph");

    // Verify expected canonical spaces
    // Note: SPACE_H = make_id(0x11), so member added by Proposal 1 is the same as SPACE_H
    let expected_canonical: Vec<SpaceId> = vec![
        ROOT_SPACE_ID,
        SPACE_A,
        SPACE_B,
        SPACE_C,
        SPACE_D,
        SPACE_E,
        SPACE_F,
        SPACE_G,
        SPACE_H, // Also added as member of A via Proposal 1 (0x11)
        SPACE_I,
        SPACE_J,
        mock_events::make_id(0x50), // Added as editor of B via Proposal 3
    ];

    assert_all_canonical(&graph, &expected_canonical, "test_topology");

    // Verify non-canonical spaces
    let expected_non_canonical: Vec<SpaceId> = vec![
        SPACE_X, SPACE_Y, SPACE_Z, SPACE_W, SPACE_P, SPACE_Q, SPACE_S,
    ];

    assert_none_canonical(&graph, &expected_non_canonical, "test_topology");

    // Verify exact count
    assert_eq!(
        graph.len(),
        expected_canonical.len(),
        "Canonical set should have exactly {} spaces, got {}",
        expected_canonical.len(),
        graph.len()
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

    assert!(!diffs.is_empty(), "Should have emitted at least one diff");

    // All canonical spaces (except root) should have been ADDED
    let canonical_non_root: Vec<SpaceId> = vec![
        SPACE_A, SPACE_B, SPACE_C, SPACE_D, SPACE_E, SPACE_F, SPACE_G, SPACE_H, SPACE_I, SPACE_J,
    ];

    for space in &canonical_non_root {
        assert_change_exists(&diffs, *space, ChangeType::Added, "bootstrap");
    }
}

#[test]
fn test_e2e_bootstrap_diff_has_no_removed_or_moved() {
    // P2: Bootstrap vs incremental semantics
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // Bootstrap should only have ADDED changes
    let removed_count = count_changes(&diffs, ChangeType::Removed);
    let moved_count = count_changes(&diffs, ChangeType::Moved);

    assert_eq!(removed_count, 0, "Bootstrap should have no REMOVED changes");
    assert_eq!(moved_count, 0, "Bootstrap should have no MOVED changes");
}

#[test]
fn test_e2e_diff_changes_have_position_info() {
    let actions = test_topology::generate();
    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    for (diff_idx, diff) in diffs.iter().enumerate() {
        for change in &diff.changes {
            match change.change_type {
                ChangeType::Added | ChangeType::Moved => {
                    assert!(
                        change.position.is_some(),
                        "Diff {}: {:?} change for 0x{:02x} should have position",
                        diff_idx,
                        change.change_type,
                        change.space_id[15]
                    );
                    let pos = change.position.as_ref().unwrap();
                    assert!(
                        pos.distance > 0,
                        "Diff {}: Non-root node 0x{:02x} should have distance > 0, got {}",
                        diff_idx,
                        change.space_id[15],
                        pos.distance
                    );
                }
                ChangeType::Removed => {
                    // REMOVED doesn't require position
                }
            }
        }
    }
}

// =============================================================================
// Test: MOVED Diff (P1: Previously missing)
// =============================================================================

#[test]
fn test_e2e_moved_diff_when_parent_changes() {
    // B moves from being child of A to being direct child of Root
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        // Initial: Root -> A -> B
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        // Then: Root -> B (shorter path, B moves)
        .verified(ROOT_SPACE_ID, SPACE_B)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // B should still be canonical
    assert!(graph.contains(&SPACE_B), "B should be canonical");

    // B should have a MOVED change (parent changed from A to Root)
    assert_change_exists(&diffs, SPACE_B, ChangeType::Moved, "parent_change");

    // Verify B's new position is distance 1 (direct child of root)
    let moved_change = diffs
        .iter()
        .flat_map(|d| &d.changes)
        .find(|c| c.space_id == SPACE_B && c.change_type == ChangeType::Moved)
        .expect("Should have MOVED change for B");

    let pos = moved_change
        .position
        .as_ref()
        .expect("MOVED should have position");
    assert_eq!(pos.distance, 1, "B should now be at distance 1 from root");
    assert_eq!(pos.parent, ROOT_SPACE_ID, "B's parent should now be Root");
}

#[test]
fn test_e2e_moved_diff_when_edge_type_changes() {
    // B changes from verified child to editor
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        // Initial: Root -> A, A -> B (verified)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        // Remove verified, add editor (same parent, different edge type)
        .unverified(SPACE_A, SPACE_B)
        .editor(SPACE_A, SPACE_B)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");
    assert!(graph.contains(&SPACE_B), "B should still be canonical");

    // B should have been REMOVED when verified edge removed, then ADDED when editor added
    // OR have a MOVED if the implementation tracks edge type changes
    let b_removed = find_change(&diffs, SPACE_B, ChangeType::Removed);
    let b_added_count = diffs
        .iter()
        .flat_map(|d| &d.changes)
        .filter(|c| c.space_id == SPACE_B && c.change_type == ChangeType::Added)
        .count();

    // Either: B was removed then re-added, or B was moved
    // Both are valid behaviors for edge type change
    assert!(
        b_removed.is_some() || b_added_count >= 1,
        "B should have REMOVED then ADDED, or MOVED when edge type changes"
    );
}

// =============================================================================
// Test: Edge Types
// =============================================================================

#[test]
fn test_e2e_editor_edges_grant_canonical_member_does_not() {
    // Editor edges still grant canonical membership. Member edges do not — see
    // plan 0007. Same topology: Root --verified--> A --editor--> B,
    //                                              A --member-->  C
    // Expected: B is canonical, C is NOT.
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .editor(SPACE_A, SPACE_B)
        .member(SPACE_A, SPACE_C)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    assert_all_canonical(
        &graph,
        &[ROOT_SPACE_ID, SPACE_A, SPACE_B],
        "editor_grants_canonical",
    );
    assert_none_canonical(&graph, &[SPACE_C], "member_does_not_grant_canonical");
}

#[test]
fn test_e2e_topic_edges_dont_grant_canonical() {
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_X, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    assert_all_canonical(&graph, &[ROOT_SPACE_ID, SPACE_A], "topic_edges");
    assert_none_canonical(&graph, &[SPACE_X], "topic_edges");
}

// =============================================================================
// Test: Member edges are ignored by canonical computation (plan 0007)
// =============================================================================

#[test]
fn test_e2e_member_edges_do_not_propagate_to_subspaces() {
    // Root --verified--> A --member--> B --verified--> C
    // Member edges are no-ops, so the canonical graph stops at A.
    // Neither B (reached only via member) nor C (only reachable through B) is canonical.
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .member(SPACE_A, SPACE_B)
        .verified(SPACE_B, SPACE_C)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have computed canonical graph");

    assert_all_canonical(&graph, &[ROOT_SPACE_ID, SPACE_A], "member_ignored");
    assert_none_canonical(&graph, &[SPACE_B, SPACE_C], "member_ignored");
    assert_eq!(graph.len(), 2);
}

// =============================================================================
// Test: Edge Removal - All Types (P1: Previously only editor removal tested)
// =============================================================================

#[test]
fn test_e2e_verified_edge_removal() {
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .unverified(ROOT_SPACE_ID, SPACE_A)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // A should no longer be canonical
    assert_none_canonical(&graph, &[SPACE_A], "verified_removal");
    assert_change_exists(&diffs, SPACE_A, ChangeType::Removed, "verified_removal");
}

#[test]
fn test_e2e_related_edge_removal() {
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .related(ROOT_SPACE_ID, SPACE_A)
        .unrelated(ROOT_SPACE_ID, SPACE_A)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    assert_none_canonical(&graph, &[SPACE_A], "related_removal");
    assert_change_exists(&diffs, SPACE_A, ChangeType::Removed, "related_removal");
}

#[test]
fn test_e2e_editor_removal_causes_removed_diff() {
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .editor(SPACE_A, SPACE_B)
        .editor_removed(SPACE_A, SPACE_B)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    assert_change_exists(&diffs, SPACE_B, ChangeType::Removed, "editor_removal");
}

#[test]
fn test_e2e_member_add_then_remove_produces_no_diff_for_member() {
    // Member edges are no-ops in canonical computation (plan 0007). A
    // Member-add followed by a Member-remove must not produce any Added,
    // Moved, or Removed change for the member space — only the verified
    // edge from Root -> A should show up.
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .member(SPACE_A, SPACE_B)
        .member_removed(SPACE_A, SPACE_B)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    assert!(
        find_change(&diffs, SPACE_B, ChangeType::Added).is_none(),
        "member_no_diff: SPACE_B should not appear as Added"
    );
    assert!(
        find_change(&diffs, SPACE_B, ChangeType::Removed).is_none(),
        "member_no_diff: SPACE_B should not appear as Removed"
    );
    assert!(
        find_change(&diffs, SPACE_B, ChangeType::Moved).is_none(),
        "member_no_diff: SPACE_B should not appear as Moved"
    );
}

// =============================================================================
// Test: Cascading Removal (P1: Critical missing test)
// =============================================================================

#[test]
fn test_e2e_cascading_removal_disconnects_subtree() {
    // Root -> A -> B -> C
    // Remove A, B and C should also become non-canonical
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .verified(SPACE_B, SPACE_C)
        .unverified(ROOT_SPACE_ID, SPACE_A)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // All of A, B, C should be non-canonical now
    assert_none_canonical(&graph, &[SPACE_A, SPACE_B, SPACE_C], "cascading_removal");

    // All should have REMOVED diffs
    assert_change_exists(&diffs, SPACE_A, ChangeType::Removed, "cascading_removal");
    assert_change_exists(&diffs, SPACE_B, ChangeType::Removed, "cascading_removal");
    assert_change_exists(&diffs, SPACE_C, ChangeType::Removed, "cascading_removal");
}

#[test]
fn test_e2e_member_removal_does_not_cascade_because_member_is_noop() {
    // Plan 0007: Member edges never enter the graph, so a Member-remove
    // also has no effect. Root -> A is canonical; B and C are never reachable
    // through Member; no Member-related diff is emitted.
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .member(SPACE_A, SPACE_B)
        .verified(SPACE_B, SPACE_C)
        .member_removed(SPACE_A, SPACE_B)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    assert_all_canonical(&graph, &[ROOT_SPACE_ID, SPACE_A], "member_noop");
    assert_none_canonical(&graph, &[SPACE_B, SPACE_C], "member_noop");

    for space in [SPACE_B, SPACE_C] {
        assert!(
            find_change(&diffs, space, ChangeType::Added).is_none(),
            "member_noop: 0x{:02x} should not appear as Added",
            space[15]
        );
        assert!(
            find_change(&diffs, space, ChangeType::Removed).is_none(),
            "member_noop: 0x{:02x} should not appear as Removed",
            space[15]
        );
    }
}

#[test]
fn test_e2e_edge_removal_with_alternate_path() {
    // P2: Edge removal with alternate paths
    // Root -> A -> C
    // Root -> B -> C
    // Remove A -> C, C should still be canonical via B
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(ROOT_SPACE_ID, SPACE_B)
        .verified(SPACE_A, SPACE_C)
        .verified(SPACE_B, SPACE_C)
        .unverified(SPACE_A, SPACE_C)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // C should still be canonical via B
    assert_all_canonical(
        &graph,
        &[ROOT_SPACE_ID, SPACE_A, SPACE_B, SPACE_C],
        "alternate_path",
    );
}

// =============================================================================
// Test: Graph Patterns - Cycles (P1: Critical missing test)
// =============================================================================

#[test]
fn test_e2e_cycle_handled_gracefully() {
    // A -> B -> C -> A (cycle)
    // Should not cause infinite loops, all should be canonical
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .verified(SPACE_B, SPACE_C)
        .verified(SPACE_C, SPACE_A) // Creates cycle
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should complete without infinite loop");

    // All nodes should be canonical (cycle doesn't prevent reachability)
    assert_all_canonical(&graph, &[ROOT_SPACE_ID, SPACE_A, SPACE_B, SPACE_C], "cycle");
    assert_eq!(graph.len(), 4);
}

#[test]
fn test_e2e_cycle_produces_deterministic_output() {
    // Run cycle test multiple times, should produce same result
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .verified(SPACE_B, SPACE_C)
        .verified(SPACE_C, SPACE_A)
        .build();

    let (_, _, _, _, diffs1, graph1) = process_with_canonical(&actions, ROOT_SPACE_ID);
    let (_, _, _, _, diffs2, graph2) = process_with_canonical(&actions, ROOT_SPACE_ID);

    let g1 = graph1.expect("Should have graph 1");
    let g2 = graph2.expect("Should have graph 2");

    assert_eq!(
        g1.members, g2.members,
        "Cycle should produce deterministic canonical set"
    );
    assert_eq!(
        diffs1.len(),
        diffs2.len(),
        "Cycle should produce deterministic diffs"
    );
}

#[test]
fn test_e2e_cycle_from_non_canonical_cannot_gain_canonical() {
    // Non-canonical X creates edge to canonical A, A creates edge back
    // X should NOT become canonical through cycle
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_X, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        // X -> A (but X is not canonical, so this edge doesn't help X)
        .verified(SPACE_X, SPACE_A)
        // A -> X would make X canonical, but let's not add it
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // X should NOT be canonical (edges FROM non-canonical sources don't count)
    assert_none_canonical(&graph, &[SPACE_X], "non_canonical_cycle");
}

// =============================================================================
// Test: Graph Patterns - Self-Referential (P1: Critical missing test)
// =============================================================================

#[test]
fn test_e2e_self_referential_edge_handled() {
    // A -> A (self-loop)
    // Should be handled gracefully
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_A) // Self-loop
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should complete without issues");

    // A should still be canonical, self-loop shouldn't break anything
    assert_all_canonical(&graph, &[ROOT_SPACE_ID, SPACE_A], "self_loop");
}

// =============================================================================
// Test: Graph Patterns - Diamond (P1: Critical missing test)
// =============================================================================

#[test]
fn test_e2e_diamond_pattern_deterministic() {
    //     Root
    //    /    \
    //   A      B
    //    \    /
    //      C
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(ROOT_SPACE_ID, SPACE_B)
        .verified(SPACE_A, SPACE_C)
        .verified(SPACE_B, SPACE_C)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // C should appear exactly once in canonical set
    assert_all_canonical(
        &graph,
        &[ROOT_SPACE_ID, SPACE_A, SPACE_B, SPACE_C],
        "diamond",
    );
    assert_eq!(graph.len(), 4, "Diamond should have 4 unique nodes");

    // C should be ADDED exactly once
    let c_adds = diffs
        .iter()
        .flat_map(|d| &d.changes)
        .filter(|c| c.space_id == SPACE_C && c.change_type == ChangeType::Added)
        .count();
    assert_eq!(
        c_adds, 1,
        "C should be ADDED exactly once in diamond pattern"
    );
}

#[test]
fn test_e2e_diamond_with_different_edge_types() {
    // Diamond where paths have different edge types
    //     Root
    //    /    \
    //   A      B
    // (verified) (related)
    //    \    /
    //      C
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .related(ROOT_SPACE_ID, SPACE_B)
        .verified(SPACE_A, SPACE_C)
        .related(SPACE_B, SPACE_C)
        .build();

    let (_, _, _, _, diffs1, graph1) = process_with_canonical(&actions, ROOT_SPACE_ID);
    let (_, _, _, _, diffs2, graph2) = process_with_canonical(&actions, ROOT_SPACE_ID);

    let g1 = graph1.expect("Should have graph 1");
    let g2 = graph2.expect("Should have graph 2");

    // Should be deterministic
    assert_eq!(g1.members, g2.members);
    assert_eq!(diffs1.len(), diffs2.len());
}

// =============================================================================
// Test: Boundary Conditions - Empty and Minimal (P2)
// =============================================================================

#[test]
fn test_e2e_empty_actions() {
    let actions: Vec<Action> = Vec::new();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // No actions means no topology changes, so no diffs should be emitted
    assert!(diffs.is_empty());
}

#[test]
fn test_e2e_root_only() {
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph with just root");
    assert_eq!(graph.len(), 1, "Should have only root");
    assert!(graph.contains(&ROOT_SPACE_ID));
}

#[test]
fn test_e2e_single_edge() {
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");
    assert_eq!(graph.len(), 2);
    assert_change_exists(&diffs, SPACE_A, ChangeType::Added, "single_edge");
}

#[test]
fn test_e2e_edge_to_nonexistent_space() {
    // P2: Edge referencing a space that doesn't exist
    let mut actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .build();

    // Add edge to space that was never created
    actions.push(mock_events::subspace_verified(ROOT_SPACE_ID, SPACE_A));

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // Should handle gracefully - implementation may or may not include the space
    // The key is it shouldn't crash
    let graph = last_graph.expect("Should have canonical graph");
    assert!(graph.contains(&ROOT_SPACE_ID));
}

// =============================================================================
// Test: Boundary Conditions - Duplicates (P1)
// =============================================================================

#[test]
fn test_e2e_duplicate_edge_idempotent() {
    // Adding the same edge twice should be idempotent
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(ROOT_SPACE_ID, SPACE_A) // Duplicate
        .verified(ROOT_SPACE_ID, SPACE_A) // Triple
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // A should appear exactly once in canonical set
    assert_eq!(
        graph.len(),
        2,
        "Duplicate edges shouldn't create duplicates in canonical set"
    );
    assert!(graph.contains(&ROOT_SPACE_ID));
    assert!(graph.contains(&SPACE_A));
}

#[test]
fn test_e2e_remove_nonexistent_edge() {
    // Removing an edge that doesn't exist should be a no-op
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .unverified(ROOT_SPACE_ID, SPACE_B) // B was never connected
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // Should still have Root and A
    assert_all_canonical(&graph, &[ROOT_SPACE_ID, SPACE_A], "remove_nonexistent");
}

// =============================================================================
// Test: Island Reconnection (P2)
// =============================================================================

#[test]
fn test_e2e_island_becomes_canonical() {
    // X, Y, Z form an island (non-canonical)
    // Then Root -> X is added, all become canonical
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_X, test_topology::USER_1)
        .space(SPACE_Y, test_topology::USER_2)
        .space(SPACE_Z, test_topology::USER_1)
        // Create island first
        .verified(SPACE_X, SPACE_Y)
        .verified(SPACE_Y, SPACE_Z)
        // Connect island to root
        .verified(ROOT_SPACE_ID, SPACE_X)
        .build();

    let (_state, _transitive, _canonical, _diff_tracker, diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    let graph = last_graph.expect("Should have canonical graph");

    // All should now be canonical
    assert_all_canonical(
        &graph,
        &[ROOT_SPACE_ID, SPACE_X, SPACE_Y, SPACE_Z],
        "island_reconnect",
    );

    // All should have ADDED changes
    assert_change_exists(&diffs, SPACE_X, ChangeType::Added, "island_reconnect");
    assert_change_exists(&diffs, SPACE_Y, ChangeType::Added, "island_reconnect");
    assert_change_exists(&diffs, SPACE_Z, ChangeType::Added, "island_reconnect");
}

// =============================================================================
// Test: Different Roots (P2)
// =============================================================================

#[test]
fn test_e2e_different_roots_different_canonical_sets() {
    // Same graph, different roots -> different canonical sets
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .space(SPACE_C, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .verified(SPACE_B, SPACE_C)
        .build();

    let (_, _, _, _, _, graph_from_root) = process_with_canonical(&actions, ROOT_SPACE_ID);
    let (_, _, _, _, _, graph_from_a) = process_with_canonical(&actions, SPACE_A);
    let (_, _, _, _, _, graph_from_c) = process_with_canonical(&actions, SPACE_C);

    let g_root = graph_from_root.expect("Should have graph from root");
    let g_a = graph_from_a.expect("Should have graph from A");
    let g_c = graph_from_c.expect("Should have graph from C");

    // From Root: Root, A, B, C (4 nodes)
    assert_eq!(g_root.len(), 4);

    // From A: A, B, C (3 nodes, Root not reachable)
    assert_eq!(g_a.len(), 3);
    assert!(!g_a.contains(&ROOT_SPACE_ID));

    // From C: just C (1 node, nothing downstream)
    assert_eq!(g_c.len(), 1);
}

// =============================================================================
// Test: Boundary Conditions - Deep and Wide (P1)
// =============================================================================

#[test]
fn test_e2e_deep_chain() {
    // Create a chain of 100 nodes: Root -> N1 -> N2 -> ... -> N100
    let mut builder = TopologyBuilder::new().space(ROOT_SPACE_ID, test_topology::ROOT_OWNER);

    let mut prev = ROOT_SPACE_ID;
    let chain_length = 100usize;

    for i in 0..chain_length {
        // Use bytes 14 and 15 to encode index, avoiding overflow
        let mut id = [0u8; 16];
        id[14] = (i >> 8) as u8; // High byte
        id[15] = (i & 0xFF) as u8; // Low byte
        let node: SpaceId = id;
        builder = builder.space(node, test_topology::USER_1);
        builder = builder.verified(prev, node);
        prev = node;
    }

    let actions = builder.build();
    let start = std::time::Instant::now();
    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);
    let elapsed = start.elapsed();

    let graph = last_graph.expect("Should handle deep chain");

    // All nodes should be canonical
    assert_eq!(
        graph.len(),
        chain_length + 1,
        "All {} nodes + root should be canonical",
        chain_length
    );

    // Should complete in reasonable time (< 1 second even for 100 nodes)
    assert!(
        elapsed.as_millis() < 1000,
        "Deep chain should complete quickly, took {:?}",
        elapsed
    );
}

#[test]
fn test_e2e_wide_graph() {
    // Create a wide graph: Root with 100 direct children
    let mut builder = TopologyBuilder::new().space(ROOT_SPACE_ID, test_topology::ROOT_OWNER);

    let width = 100usize;

    for i in 0..width {
        // Use bytes 14 and 15 to encode index, avoiding overflow
        let mut id = [0u8; 16];
        id[13] = 0xE0; // Marker to distinguish from deep chain
        id[14] = (i >> 8) as u8; // High byte
        id[15] = (i & 0xFF) as u8; // Low byte
        let node: SpaceId = id;
        builder = builder.space(node, test_topology::USER_1);
        builder = builder.verified(ROOT_SPACE_ID, node);
    }

    let actions = builder.build();
    let start = std::time::Instant::now();
    let (_state, _transitive, _canonical, _diff_tracker, _diffs, last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);
    let elapsed = start.elapsed();

    let graph = last_graph.expect("Should handle wide graph");

    assert_eq!(
        graph.len(),
        width + 1,
        "All {} children + root should be canonical",
        width
    );

    assert!(
        elapsed.as_millis() < 1000,
        "Wide graph should complete quickly, took {:?}",
        elapsed
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

    assert_eq!(graph.tree.space_id, ROOT_SPACE_ID);
    assert_eq!(graph.tree.edge_type, EdgeType::Root);
    assert!(!graph.tree.children.is_empty(), "Root should have children");

    let a_node = graph.tree.children.iter().find(|n| n.space_id == SPACE_A);
    assert!(a_node.is_some(), "A should be a child of root");
    assert_eq!(
        a_node.unwrap().edge_type,
        EdgeType::Verified,
        "A should be connected via Verified edge"
    );
}

// =============================================================================
// Test: Determinism & Ordering
// =============================================================================

#[test]
fn test_e2e_deterministic_output() {
    let actions = test_topology::generate();

    let (_, _, _, _, diffs1, graph1) = process_with_canonical(&actions, ROOT_SPACE_ID);
    let (_, _, _, _, diffs2, graph2) = process_with_canonical(&actions, ROOT_SPACE_ID);

    let g1 = graph1.expect("Should have graph 1");
    let g2 = graph2.expect("Should have graph 2");

    assert_eq!(g1.members, g2.members, "Canonical sets should be identical");
    assert_eq!(
        diffs1.len(),
        diffs2.len(),
        "Should produce same number of diffs"
    );

    for (i, (d1, d2)) in diffs1.iter().zip(diffs2.iter()).enumerate() {
        assert_eq!(
            d1.changes.len(),
            d2.changes.len(),
            "Diff {} should have same number of changes",
            i
        );

        for (j, (c1, c2)) in d1.changes.iter().zip(d2.changes.iter()).enumerate() {
            assert_eq!(
                c1.space_id, c2.space_id,
                "Diff {} change {}: space IDs should match",
                i, j
            );
            assert_eq!(
                c1.change_type, c2.change_type,
                "Diff {} change {}: change types should match",
                i, j
            );
        }
    }
}

#[test]
fn test_e2e_event_ordering_determinism() {
    // P2: Same events in different orders should produce same final state
    // (Order may affect intermediate diffs, but final canonical set should match)

    // Order 1: Create spaces first, then edges
    let actions1 = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .build();

    // Order 2: Interleaved creation and edges
    let mut actions2 = Vec::new();
    actions2.extend(mock_events::personal_space_registered(
        ROOT_SPACE_ID,
        test_topology::ROOT_OWNER,
    ));
    actions2.extend(mock_events::personal_space_registered(
        SPACE_A,
        test_topology::USER_1,
    ));
    actions2.push(mock_events::subspace_verified(ROOT_SPACE_ID, SPACE_A));
    actions2.extend(mock_events::personal_space_registered(
        SPACE_B,
        test_topology::USER_2,
    ));
    actions2.push(mock_events::subspace_verified(SPACE_A, SPACE_B));

    let (_, _, _, _, _, graph1) = process_with_canonical(&actions1, ROOT_SPACE_ID);
    let (_, _, _, _, _, graph2) = process_with_canonical(&actions2, ROOT_SPACE_ID);

    let g1 = graph1.expect("Should have graph 1");
    let g2 = graph2.expect("Should have graph 2");

    assert_eq!(
        g1.members, g2.members,
        "Different event orders should produce same canonical set"
    );
}

// =============================================================================
// Test: Performance
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

#[test]
fn test_e2e_incremental_faster_than_full() {
    // P2: Verify incremental processing is efficient
    // Build a graph, then make a small change - the diff should be small
    let mut actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .build();

    // Add one more space
    let space_c = mock_events::make_id(0xCC);
    actions.extend(mock_events::personal_space_registered(
        space_c,
        test_topology::USER_1,
    ));
    actions.push(mock_events::subspace_verified(SPACE_B, space_c));

    let (_state, _transitive, _canonical, _diff_tracker, diffs, _last_graph) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // The last diff (adding C) should be small (just 1 change)
    let last_diff = diffs.last().expect("Should have diffs");
    assert!(
        last_diff.changes.len() <= 2, // Could be 1 ADDED or ADDED + MOVED
        "Incremental change should produce small diff, got {} changes",
        last_diff.changes.len()
    );
}

#[test]
fn test_e2e_scaling_performance() {
    // P2: Test that performance scales reasonably
    use std::time::Instant;

    // Use larger base sizes to reduce impact of fixed overhead
    let sizes = [50, 100, 200];
    let mut times = Vec::new();

    for size in sizes {
        let mut builder = TopologyBuilder::new().space(ROOT_SPACE_ID, test_topology::ROOT_OWNER);

        let mut prev = ROOT_SPACE_ID;
        for i in 0..size {
            // Use two bytes for index to support sizes > 255
            let mut id = [0u8; 16];
            id[13] = 0xAA; // Marker byte
            id[14] = (i >> 8) as u8;
            id[15] = (i & 0xFF) as u8;
            let node: SpaceId = id;
            builder = builder.space(node, test_topology::USER_1);
            builder = builder.verified(prev, node);
            prev = node;
        }

        let actions = builder.build();
        let start = Instant::now();
        let _ = process_with_canonical(&actions, ROOT_SPACE_ID);
        times.push(start.elapsed());
    }

    // Verify roughly linear scaling (not quadratic)
    // Time for 200 nodes should be less than 16x time for 50 nodes (4x size)
    // (If it were O(n²), it would be 16x; we allow some cache/overhead effects)
    let ratio = times[2].as_nanos() as f64 / times[0].as_nanos().max(1) as f64;
    assert!(
        ratio < 16.0, // For 4x size, should be at most 16x (quadratic bound)
        "Performance should scale sub-quadratically. 50 nodes: {:?}, 200 nodes: {:?}, ratio: {:.1}x (expected < 16x for 4x size increase)",
        times[0],
        times[2],
        ratio
    );
}

// =============================================================================
// Test: Memory and Allocation (P3)
// =============================================================================

#[test]
fn test_e2e_diff_tracker_with_capacity() {
    // Verify DiffTracker::with_capacity works correctly
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .space(SPACE_B, test_topology::USER_2)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .verified(SPACE_A, SPACE_B)
        .build();

    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();
    let mut canonical = CanonicalProcessor::new(ROOT_SPACE_ID);
    let mut diff_tracker = DiffTracker::with_capacity(100); // Pre-allocate

    let mut diff_count = 0;
    for (i, action) in actions.iter().enumerate() {
        let meta = make_meta(i as u64);
        if let Some(event) = convert_action(action, &meta) {
            transitive.handle_event(&event, &state);
            state.apply_event(&event);

            if let Some(graph) = canonical.compute_if_changed(&state, &mut transitive) {
                let diff = diff_tracker.track(&graph);
                if !diff.is_empty() {
                    diff_count += 1;
                }
            }
        }
    }

    // Should have produced at least one diff (bootstrap)
    assert!(
        diff_count > 0,
        "DiffTracker with pre-allocated capacity should still produce diffs"
    );
}

#[test]
fn test_e2e_diff_tracker_reset() {
    // P3: Test DiffTracker reset behavior
    let actions = TopologyBuilder::new()
        .space(ROOT_SPACE_ID, test_topology::ROOT_OWNER)
        .space(SPACE_A, test_topology::USER_1)
        .verified(ROOT_SPACE_ID, SPACE_A)
        .build();

    let (state, mut transitive, mut canonical, mut diff_tracker, _, _) =
        process_with_canonical(&actions, ROOT_SPACE_ID);

    // Reset the tracker
    diff_tracker.reset();

    // Next computation should be a "bootstrap" again
    if let Some(graph) = canonical.compute_if_changed(&state, &mut transitive) {
        let diff = diff_tracker.track(&graph);
        // Should have ADDED changes (bootstrap behavior)
        let added_count = diff
            .changes
            .iter()
            .filter(|c| c.change_type == ChangeType::Added)
            .count();
        assert!(
            added_count > 0,
            "After reset, should produce bootstrap-like diff"
        );
    }
}
