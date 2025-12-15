# 0001: Complete Action Support for Hermes Spaces

## Status

Proposed

## Context

The `hermes-pipeline` transformer currently handles only a subset of the actions emitted by `hermes-substream`. The original implementation focused on space registration and trust relationships (subspaces), which were the immediate requirements for the initial deployment.

However, `hermes-substream` defines 20 distinct action types that represent the full spectrum of on-chain governance and membership events. To provide a complete picture of space activity for downstream consumers, `hermes-pipeline` should be expanded to handle all relevant actions.

### Current State

**Supported actions (4):**
| Action | Kafka Topic | Description |
|--------|-------------|-------------|
| `SPACE_REGISTERED` | `space.creations` | New space registrations |
| `SUBSPACE_ADDED` | `space.trust.extensions` | Trust extensions (verified/related/subtopic) |
| `SUBSPACE_REMOVED` | `space.trust.extensions` | Trust revocations |
| `EDITS_PUBLISHED` | `knowledge.edits` | Published edits (fetched from IPFS cache) |

**Unsupported actions (16):**
| Category | Action | Description |
|----------|--------|-------------|
| **Space Lifecycle** | `SPACE_MIGRATED` | Space contract address updates |
| **Governance** | `PROPOSAL_CREATED` | New governance proposals |
| | `PROPOSAL_VOTED` | Votes cast on proposals |
| | `PROPOSAL_EXECUTED` | Executed proposals |
| **Membership** | `EDITOR_ADDED` | New editor permissions |
| | `EDITOR_REMOVED` | Revoked editor permissions |
| | `MEMBER_ADDED` | New member additions |
| | `MEMBER_REMOVED` | Member removals |
| | `SPACE_LEFT` | Voluntary departures |
| **Moderation** | `EDITOR_FLAGGED` | Flagged editors |
| | `EDITOR_UNFLAGGED` | Unflagged editors |
| | `CONTENT_FLAGGED` | Flagged content |
| **Content** | `TOPIC_DECLARED` | Topic declarations |
| **Voting** | `OBJECT_UPVOTED` | Upvotes on objects |
| | `OBJECT_DOWNVOTED` | Downvotes on objects |
| | `OBJECT_UNVOTED` | Vote removals |

### Scope Consideration

`EDITS_PUBLISHED` is handled within `hermes-pipeline` via the `pipelines/edits.rs` module. Unlike other pipelines, this requires an external cache lookup to resolve IPFS hash → Edit content. The pipeline fetches cached edits from `hermes-ipfs-cache` (which resolves and caches IPFS content) and emits them to the `knowledge.edits` Kafka topic.

## Decision

Expand `hermes-pipeline` to handle all remaining space-related actions. Group related actions into logical Kafka topics to simplify downstream consumption.

### Kafka Topic Design

| Topic | Actions | Message Type | Status |
|-------|---------|--------------|--------|
| `space.creations` | `SPACE_REGISTERED` | `HermesCreateSpace` | ✅ Implemented |
| `space.trust.extensions` | `SUBSPACE_ADDED`, `SUBSPACE_REMOVED` | `HermesSpaceTrustExtension` | ✅ Implemented |
| `knowledge.edits` | `EDITS_PUBLISHED` | `HermesEdit` | ✅ Implemented |
| `space.migrations` | `SPACE_MIGRATED` | `HermesSpaceMigration` (new) | ❌ Not started |
| `space.governance` | `PROPOSAL_CREATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED` | `HermesGovernanceEvent` (new) | ❌ Not started |
| `space.membership` | `EDITOR_ADDED`, `EDITOR_REMOVED`, `MEMBER_ADDED`, `MEMBER_REMOVED`, `SPACE_LEFT` | `HermesMembershipEvent` (new) | ❌ Not started |
| `space.moderation` | `EDITOR_FLAGGED`, `EDITOR_UNFLAGGED`, `CONTENT_FLAGGED` | `HermesModerationEvent` (new) | ❌ Not started |
| `space.topics` | `TOPIC_DECLARED` | `HermesTopicDeclared` (new) | ❌ Not started |
| `space.votes` | `OBJECT_UPVOTED`, `OBJECT_DOWNVOTED`, `OBJECT_UNVOTED` | `HermesObjectVote` (new) | ❌ Not started |

## Implementation Plan

### Phase 1: Schema Updates (hermes-schema)

Priority: High

Add new protobuf message types to `hermes-schema/proto/`:

1. **`HermesSpaceMigration`**
   ```protobuf
   message HermesSpaceMigration {
     bytes space_id = 1;
     bytes old_address = 2;
     bytes new_address = 3;
     BlockchainMetadata metadata = 4;
   }
   ```

2. **`HermesGovernanceEvent`**
   ```protobuf
   message HermesGovernanceEvent {
     bytes space_id = 1;
     bytes proposal_id = 2;
     oneof event {
       ProposalCreated created = 3;
       ProposalVoted voted = 4;
       ProposalExecuted executed = 5;
     }
     BlockchainMetadata metadata = 6;
   }
   ```

3. **`HermesMembershipEvent`**
   ```protobuf
   message HermesMembershipEvent {
     bytes space_id = 1;
     bytes account = 2;  // editor or member address
     MembershipEventType event_type = 3;
     BlockchainMetadata metadata = 4;
   }
   
   enum MembershipEventType {
     EDITOR_ADDED = 0;
     EDITOR_REMOVED = 1;
     MEMBER_ADDED = 2;
     MEMBER_REMOVED = 3;
     SPACE_LEFT = 4;
   }
   ```

4. **`HermesModerationEvent`**
   ```protobuf
   message HermesModerationEvent {
     bytes space_id = 1;
     bytes subject = 2;  // editor address or content ID
     ModerationEventType event_type = 3;
     bytes data = 4;  // additional context
     BlockchainMetadata metadata = 5;
   }
   
   enum ModerationEventType {
     EDITOR_FLAGGED = 0;
     EDITOR_UNFLAGGED = 1;
     CONTENT_FLAGGED = 2;
   }
   ```

5. **`HermesTopicDeclared`**
   ```protobuf
   message HermesTopicDeclared {
     bytes space_id = 1;
     bytes topic_id = 2;
     bytes data = 3;
     BlockchainMetadata metadata = 4;
   }
   ```

6. **`HermesObjectVote`**
   ```protobuf
   message HermesObjectVote {
     bytes voter_id = 1;
     bytes object_type = 2;
     bytes object_id = 3;
     VoteType vote_type = 4;
     bytes data = 5;
     BlockchainMetadata metadata = 6;
   }
   
   enum VoteType {
     UPVOTE = 0;
     DOWNVOTE = 1;
     UNVOTE = 2;
   }
   ```

### Phase 2: Space Lifecycle Events

Priority: High

**Action:** `SPACE_MIGRATED`

1. Add conversion function in `conversion.rs`:
   ```rust
   pub fn convert_space_migrated(
       action: &Action,
       block_meta: &BlockMetadata,
   ) -> Result<HermesSpaceMigration>
   ```

2. Add Kafka send function in `kafka.rs`:
   ```rust
   pub fn send_space_migration(
       producer: &BaseProducer,
       migration: &HermesSpaceMigration,
   ) -> Result<()>
   ```

3. Update `transformer.rs` to handle `SPACE_MIGRATED`

**Files changed:**
- `hermes-pipeline/src/conversion.rs`
- `hermes-pipeline/src/kafka.rs`
- `hermes-pipeline/src/transformer.rs`

### Phase 3: Governance Events

Priority: Medium

**Actions:** `PROPOSAL_CREATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED`

1. Add conversion functions for each governance action type
2. Add `send_governance_event` Kafka function
3. Update transformer to handle governance actions

**Files changed:**
- `hermes-pipeline/src/conversion.rs`
- `hermes-pipeline/src/kafka.rs`
- `hermes-pipeline/src/transformer.rs`

### Phase 4: Membership Events

Priority: Medium

**Actions:** `EDITOR_ADDED`, `EDITOR_REMOVED`, `MEMBER_ADDED`, `MEMBER_REMOVED`, `SPACE_LEFT`

1. Add unified conversion function that maps action type to `MembershipEventType`
2. Add `send_membership_event` Kafka function
3. Update transformer to handle membership actions

**Files changed:**
- `hermes-pipeline/src/conversion.rs`
- `hermes-pipeline/src/kafka.rs`
- `hermes-pipeline/src/transformer.rs`

### Phase 5: Moderation Events

Priority: Low

**Actions:** `EDITOR_FLAGGED`, `EDITOR_UNFLAGGED`, `CONTENT_FLAGGED`

1. Add conversion functions for moderation events
2. Add `send_moderation_event` Kafka function
3. Update transformer to handle moderation actions

**Files changed:**
- `hermes-pipeline/src/conversion.rs`
- `hermes-pipeline/src/kafka.rs`
- `hermes-pipeline/src/transformer.rs`

### Phase 6: Topic & Voting Events

Priority: Low

**Actions:** `TOPIC_DECLARED`, `OBJECT_UPVOTED`, `OBJECT_DOWNVOTED`, `OBJECT_UNVOTED`

1. Add conversion functions for topic and voting events
2. Add `send_topic_declared` and `send_object_vote` Kafka functions
3. Update transformer to handle these actions

**Files changed:**
- `hermes-pipeline/src/conversion.rs`
- `hermes-pipeline/src/kafka.rs`
- `hermes-pipeline/src/transformer.rs`

### Phase 7: Testing & Documentation

Priority: High (alongside each phase)

1. Add unit tests for each new conversion function
2. Add integration tests using `mock-substream`
3. Update `README.md` with new event types and Kafka topics
4. Update `hermes/k8s/` manifests if new environment variables are needed

## File Changes Summary

| File | Action | Phase |
|------|--------|-------|
| `hermes-schema/proto/space.proto` | Modify | 1 |
| `hermes-pipeline/src/pipelines/*.rs` | Add new pipeline modules | 2-6 |
| `hermes-pipeline/src/pipelines/mod.rs` | Modify to export new pipelines | 2-6 |
| `hermes-pipeline/src/emit.rs` | Modify to add new topics and `KafkaEvent` impls | 2-6 |
| `hermes-pipeline/src/main.rs` | Modify to handle new action types | 2-6 |
| `hermes-pipeline/README.md` | Modify | 7 |

## Consequences

### Positive

- **Complete visibility**: Downstream consumers get all space-related events
- **Unified streaming**: Single transformer for all space events (except edits)
- **Logical grouping**: Related events share Kafka topics for simpler consumption
- **Incremental delivery**: Phased approach allows shipping value early

### Negative

- **Increased complexity**: More action types mean more code to maintain
- **Higher throughput**: More messages through Kafka (though filtering is unchanged)
- **Schema additions**: New proto types require coordination with consumers

### Neutral

- **No breaking changes**: Existing topics and message types unchanged
- **Same architecture**: Follows established patterns from initial implementation

## Open Questions

1. **Should governance events be a separate transformer?**
   - Governance has complex state (proposals, voting periods)
   - May warrant dedicated `hermes-governance` transformer
   - Decision: Start in `hermes-pipeline`, extract if complexity grows

2. **Should we add filtering configuration?**
   - Allow operators to enable/disable specific event types
   - Reduces Kafka traffic if some events aren't needed
   - Decision: Defer until there's concrete demand

3. **What about historical backfill?**
   - New events won't have historical data in Kafka
   - May need backfill job to replay from genesis
   - Decision: Document as future work, not blocking initial implementation

## References

- `docs/hermes-architecture.md` - Overall Hermes system design
- `hermes-substream/proto/schema.proto` - Source event definitions
- `hermes-relay/src/actions.rs` - Action type constants
