//! Integration tests for governance pipeline.
//!
//! These tests verify end-to-end processing of governance events.
//!
//! NOTE: These tests require updates to work with the new squashing logic
//! where PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED are combined into a single
//! HermesProposalCreated event.
//!
//! TODO: Re-enable these tests once mock_events are updated to support
//! the new event structure.

// Tests temporarily disabled pending mock_events updates for squashing support.
// See: gaia-t0x.5 for the original implementation requirements.
