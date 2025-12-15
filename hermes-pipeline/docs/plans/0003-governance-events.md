# 0003: Governance Events Support

## Status

In Progress

## Context

This plan implements Phase 3 (Governance Events) from [0001-complete-action-support.md](./0001-complete-action-support.md). The original plan proposed a single `HermesGovernanceEvent` message with a `oneof` for the three governance action types. After review, we've decided to use separate message types for each event to provide clearer semantics and simpler consumption patterns.

### Governance Actions

| Action | Description | Source |
|--------|-------------|--------|
| `PROPOSAL_CREATED` | New governance proposal created in a space | `hermes-relay/src/actions.rs:36` |
| `PROPOSAL_VOTED` | Vote cast on an existing proposal | `hermes-relay/src/actions.rs:42` |
| `PROPOSAL_EXECUTED` | Proposal executed after passing | `hermes-relay/src/actions.rs:48` |

### Action Field Mappings

Based on `hermes-substream/proto/schema.proto`:

| Action | from_id | to_id | topic | data |
|--------|---------|-------|-------|------|
| `PROPOSAL_CREATED` | space_id (16 bytes) | unused | proposal_id (32 bytes) | proposal metadata |
| `PROPOSAL_VOTED` | voter_id (16 bytes) | space_id (16 bytes) | proposal_id (32 bytes) | vote choice |
| `PROPOSAL_EXECUTED` | space_id (16 bytes) | unused | proposal_id (32 bytes) | execution result |

## Decision

Implement three separate protobuf message types for governance events, all emitted to a single `space.governance` Kafka topic with event-type headers for filtering.

### Protobuf Schema

Create `hermes-schema/proto/governance.proto`:

```protobuf
syntax = "proto3";

package governance;

import "blockchain_metadata.proto";

// HermesProposalCreated - emitted when a new proposal is created in a space
message HermesProposalCreated {
  bytes space_id = 1;       // 16 bytes - space creating the proposal
  bytes proposal_id = 2;    // 32 bytes - unique proposal identifier (from topic field)
  bytes data = 3;           // proposal metadata (title, description, voting period, etc.)
  blockchain_metadata.BlockchainMetadata meta = 4;
}

// HermesProposalVoted - emitted when a vote is cast on a proposal
message HermesProposalVoted {
  bytes voter_id = 1;       // 16 bytes - space casting the vote (from from_id)
  bytes space_id = 2;       // 16 bytes - space that owns the proposal (from to_id)
  bytes proposal_id = 3;    // 32 bytes - proposal being voted on (from topic field)
  bytes data = 4;           // vote choice and any additional vote data
  blockchain_metadata.BlockchainMetadata meta = 5;
}

// HermesProposalExecuted - emitted when a passed proposal is executed
message HermesProposalExecuted {
  bytes space_id = 1;       // 16 bytes - space executing the proposal
  bytes proposal_id = 2;    // 32 bytes - executed proposal identifier (from topic field)
  bytes data = 3;           // execution result/details
  blockchain_metadata.BlockchainMetadata meta = 4;
}
```

### Kafka Topic Design

| Topic | Events | Key | Headers |
|-------|--------|-----|---------|
| `space.governance` | `HermesProposalCreated`, `HermesProposalVoted`, `HermesProposalExecuted` | `space_id` | `event-type: PROPOSAL_CREATED\|PROPOSAL_VOTED\|PROPOSAL_EXECUTED` |

Using a single topic with headers allows:
- Consumers interested in all governance events to subscribe once
- Consumers to filter by event type using Kafka headers
- Ordered processing of related governance events within a space (same partition key)

## Implementation Plan

### Step 1: Protobuf Schema

**File:** `hermes-schema/proto/governance.proto`

Create new proto file with the three message types as shown above.

**File:** `hermes-schema/build.rs`

Add `governance.proto` to the protobuf compilation list if not auto-discovered.

**File:** `hermes-schema/src/lib.rs`

Export the generated governance module:
```rust
pub mod pb {
    pub mod governance {
        include!(concat!(env!("OUT_DIR"), "/governance.rs"));
    }
    // ... existing modules
}
```

### Step 2: Pipeline Module

**File:** `hermes-pipeline/src/pipelines/governance.rs`

```rust
//! Pipeline: PROPOSAL_CREATED, PROPOSAL_VOTED, PROPOSAL_EXECUTED → space.governance
//!
//! Converts governance actions to typed Hermes events.

use anyhow::Result;

use hermes_relay::{actions, Action};
use hermes_schema::pb::governance::{
    HermesProposalCreated, HermesProposalExecuted, HermesProposalVoted,
};

use super::BlockMetadata;

/// Result of transforming governance actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub proposals_created: Vec<HermesProposalCreated>,
    pub proposals_voted: Vec<HermesProposalVoted>,
    pub proposals_executed: Vec<HermesProposalExecuted>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.proposals_created.len() 
            + self.proposals_voted.len() 
            + self.proposals_executed.len()
    }
}

/// Transform all governance actions in a block.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for action in actions {
        if actions::matches(&action.action, &actions::PROPOSAL_CREATED) {
            result.proposals_created.push(convert_proposal_created(action, meta)?);
        } else if actions::matches(&action.action, &actions::PROPOSAL_VOTED) {
            result.proposals_voted.push(convert_proposal_voted(action, meta)?);
        } else if actions::matches(&action.action, &actions::PROPOSAL_EXECUTED) {
            result.proposals_executed.push(convert_proposal_executed(action, meta)?);
        }
    }

    Ok(result)
}

fn convert_proposal_created(action: &Action, meta: &BlockMetadata) -> Result<HermesProposalCreated> {
    Ok(HermesProposalCreated {
        space_id: action.from_id.clone(),
        proposal_id: action.topic.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

fn convert_proposal_voted(action: &Action, meta: &BlockMetadata) -> Result<HermesProposalVoted> {
    Ok(HermesProposalVoted {
        voter_id: action.from_id.clone(),
        space_id: action.to_id.clone(),
        proposal_id: action.topic.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

fn convert_proposal_executed(action: &Action, meta: &BlockMetadata) -> Result<HermesProposalExecuted> {
    Ok(HermesProposalExecuted {
        space_id: action.from_id.clone(),
        proposal_id: action.topic.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}
```

**File:** `hermes-pipeline/src/pipelines/mod.rs`

Add export:
```rust
pub mod governance;
```

### Step 3: Kafka Emission

**File:** `hermes-pipeline/src/emit.rs`

Add topic constant:
```rust
pub mod topics {
    // ... existing topics
    pub const GOVERNANCE: &str = "space.governance";
}
```

Add `KafkaEvent` implementations:
```rust
impl KafkaEvent for HermesProposalCreated {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_CREATED"),
        })
    }
}

impl KafkaEvent for HermesProposalVoted {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()  // Key by proposal's space for ordering
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_VOTED"),
        })
    }
}

impl KafkaEvent for HermesProposalExecuted {
    const TOPIC: &'static str = topics::GOVERNANCE;

    fn key(&self) -> Vec<u8> {
        self.space_id.clone()
    }

    fn headers(&self) -> OwnedHeaders {
        OwnedHeaders::new().insert(Header {
            key: "event-type",
            value: Some("PROPOSAL_EXECUTED"),
        })
    }
}
```

### Step 4: Main Integration

**File:** `hermes-pipeline/src/main.rs`

Add governance pipeline to parallel transform phase:
```rust
// Spawn governance transform (sync)
let governance_handle = tokio::task::spawn_blocking(move || {
    pipelines::governance::transform(&actions_clone, &meta_clone)
});
```

Add to emit phase (after trust, before edits):
```rust
// Emit governance events
for event in &governance.proposals_created {
    self.emitter.emit(event)?;
    println!(
        "Block {}: Proposal created: {} in space {}",
        meta.block_number,
        hex::encode(&event.proposal_id),
        hex::encode(&event.space_id)
    );
}

for event in &governance.proposals_voted {
    self.emitter.emit(event)?;
    println!(
        "Block {}: Proposal voted: {} by {}",
        meta.block_number,
        hex::encode(&event.proposal_id),
        hex::encode(&event.voter_id)
    );
}

for event in &governance.proposals_executed {
    self.emitter.emit(event)?;
    println!(
        "Block {}: Proposal executed: {} in space {}",
        meta.block_number,
        hex::encode(&event.proposal_id),
        hex::encode(&event.space_id)
    );
}
```

Update block summary logging to include governance counts.

### Step 5: Testing

**File:** `hermes-pipeline/src/pipelines/governance.rs` (tests module)

Add unit tests for:
- `convert_proposal_created` - verify field mapping
- `convert_proposal_voted` - verify voter_id vs space_id mapping
- `convert_proposal_executed` - verify field mapping
- `transform` - verify filtering of non-governance actions

## File Changes Summary

| File | Action |
|------|--------|
| `hermes-schema/proto/governance.proto` | Create |
| `hermes-schema/src/lib.rs` | Modify (add governance export) |
| `hermes-pipeline/src/pipelines/governance.rs` | Create |
| `hermes-pipeline/src/pipelines/mod.rs` | Modify (add export) |
| `hermes-pipeline/src/emit.rs` | Modify (add topic + impls) |
| `hermes-pipeline/src/main.rs` | Modify (integrate pipeline) |

## Consequences

### Positive

- **Clear semantics**: Each event type has its own message with appropriate fields
- **Type safety**: Consumers know exactly what fields to expect for each event
- **Flexible consumption**: Single topic with headers allows filtering without multiple subscriptions
- **Consistent ordering**: All governance events for a space go to the same partition

### Negative

- **More message types**: Three types instead of one (but simpler than `oneof`)
- **Schema coordination**: Consumers need to handle three message types

### Neutral

- **Same topic**: All governance events share `space.governance` topic
- **Follows existing patterns**: Consistent with spaces and trust pipeline structure

## References

- [0001-complete-action-support.md](./0001-complete-action-support.md) - Parent plan
- `hermes-substream/proto/schema.proto` - Source event definitions (lines 45-74)
- `hermes-relay/src/actions.rs` - Action type constants (lines 35-51)
- `hermes-pipeline/src/pipelines/spaces.rs` - Reference implementation pattern
