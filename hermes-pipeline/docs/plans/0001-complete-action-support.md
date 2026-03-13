# 0001: Complete Action Support for Hermes Spaces

## Status

Implemented

## Context

The `hermes-pipeline` transformer currently handles only a subset of the actions emitted by `hermes-substream`. The original implementation focused on space registration and trust relationships (subspaces), which were the immediate requirements for the initial deployment.

However, `hermes-substream` defines 20 distinct action types that represent the full spectrum of on-chain governance and membership events. To provide a complete picture of space activity for downstream consumers, `hermes-pipeline` should be expanded to handle all relevant actions.

### Final State

**Supported actions (20):**
| Category | Action | Kafka Topic | Description |
|----------|--------|-------------|-------------|
| **Space Lifecycle** | `SPACE_REGISTERED` | `space.creations` | New space registrations |
| **Trust** | `SUBSPACE_VERIFIED` | `space.trust.extensions` | Verified trust extensions |
| | `SUBSPACE_RELATED` | `space.trust.extensions` | Related trust extensions |
| | `SUBSPACE_TOPIC_DECLARED` | `space.trust.extensions` | Topic-based trust extensions |
| | `SUBSPACE_REMOVED` | `space.trust.extensions` | Trust revocations |
| **Membership** | `EDITOR_ADDED` | `space.membership` | New editor permissions |
| | `EDITOR_REMOVED` | `space.membership` | Revoked editor permissions |
| | `MEMBER_ADDED` | `space.membership` | New member additions |
| | `MEMBER_REMOVED` | `space.membership` | Member removals |
| | `SPACE_LEFT` | `space.membership` | Voluntary departures |
| **Moderation** | `EDITOR_FLAGGED` | `space.moderation` | Flagged editors |
| | `EDITOR_UNFLAGGED` | `space.moderation` | Unflagged editors |
| | `FLAGGED` | `space.moderation` | Flagged content |
| | `UNFLAGGED` | `space.moderation` | Unflagged content |
| **Topics** | `TOPIC_DECLARED` | `space.topics` | Topic declarations |
| **Governance** | `PROPOSAL_CREATED` | `space.governance` | New governance proposals |
| | `PROPOSAL_VOTED` | `space.governance` | Votes cast on proposals |
| | `PROPOSAL_EXECUTED` | `space.governance` | Executed proposals |
| **Voting** | `UPVOTED` | `curation.votes` | Upvotes on objects |
| | `DOWNVOTED` | `curation.votes` | Downvotes on objects |
| | `UNVOTED` | `curation.votes` | Vote removals |
| **Knowledge** | `EDITS_PUBLISHED` | `knowledge.edits` | Published edits (fetched from IPFS cache) |

**Not implemented:**
| Category | Action | Description | Reason |
|----------|--------|-------------|--------|
| **Space Lifecycle** | `SPACE_MIGRATED` | Space contract address updates | Not yet needed by consumers |

### Scope Consideration

`EDITS_PUBLISHED` is handled within `hermes-pipeline` via the `pipelines/edits.rs` module. Unlike other pipelines, this requires an external cache lookup to resolve IPFS hash → Edit content. The pipeline fetches cached edits from `hermes-ipfs-cache` (which resolves and caches IPFS content) and emits them to the `knowledge.edits` Kafka topic.

## Decision

Expand `hermes-pipeline` to handle all remaining space-related actions. Group related actions into logical Kafka topics to simplify downstream consumption.

### Kafka Topic Design

| Topic | Actions | Message Type | Status |
|-------|---------|--------------|--------|
| `space.creations` | `SPACE_REGISTERED` | `HermesCreateSpace` | ✅ Implemented |
| `space.trust.extensions` | `SUBSPACE_VERIFIED`, `SUBSPACE_RELATED`, `SUBSPACE_TOPIC_DECLARED`, `SUBSPACE_REMOVED` | `HermesSpaceTrustExtension` | ✅ Implemented |
| `space.membership` | `EDITOR_ADDED`, `EDITOR_REMOVED`, `MEMBER_ADDED`, `MEMBER_REMOVED`, `SPACE_LEFT` | `HermesRoleGranted`, `HermesRoleRevoked`, `HermesSpaceLeft` | ✅ Implemented |
| `space.moderation` | `EDITOR_FLAGGED`, `EDITOR_UNFLAGGED`, `FLAGGED`, `UNFLAGGED` | `HermesEditorFlagged`, `HermesEditorUnflagged`, `HermesContentFlagged`, `HermesContentUnflagged` | ✅ Implemented |
| `space.topics` | `TOPIC_DECLARED` | `HermesTopicDeclared` | ✅ Implemented |
| `space.governance` | `PROPOSAL_CREATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED` | `HermesGovernanceEvent` | ✅ Implemented |
| `curation.votes` | `UPVOTED`, `DOWNVOTED`, `UNVOTED` | `HermesVoteCast` | ✅ Implemented |
| `knowledge.edits` | `EDITS_PUBLISHED` | `HermesEdit` | ✅ Implemented |
| `space.migrations` | `SPACE_MIGRATED` | - | ❌ Not started |

## Implementation Plan

All phases below have been completed except Phase 2 (Space Migrations).

### Phase 1: Schema Updates (hermes-schema) ✅

New protobuf files added to `hermes-schema/proto/`:

- `membership.proto` - `HermesRoleGranted`, `HermesRoleRevoked`, `HermesSpaceLeft`
- `moderation.proto` - `HermesEditorFlagged`, `HermesEditorUnflagged`, `HermesContentFlagged`, `HermesContentUnflagged`
- `topics.proto` - `HermesTopicDeclared`
- `voting.proto` - `HermesVoteCast` with `VoteDirection` enum

### Phase 2: Space Lifecycle Events ❌

**Action:** `SPACE_MIGRATED` - Not yet implemented, awaiting consumer demand.

### Phase 3: Governance Events ✅

**Actions:** `PROPOSAL_CREATED`, `PROPOSAL_VOTED`, `PROPOSAL_EXECUTED`

Implemented in `hermes-pipeline/src/pipelines/governance.rs`.

### Phase 4: Membership Events ✅

**Actions:** `EDITOR_ADDED`, `EDITOR_REMOVED`, `MEMBER_ADDED`, `MEMBER_REMOVED`, `SPACE_LEFT`

Implemented in `hermes-pipeline/src/pipelines/membership.rs`.

### Phase 5: Moderation Events ✅

**Actions:** `EDITOR_FLAGGED`, `EDITOR_UNFLAGGED`, `FLAGGED`, `UNFLAGGED`

Implemented in `hermes-pipeline/src/pipelines/moderation.rs`.

### Phase 6: Topic & Voting Events ✅

**Actions:** `TOPIC_DECLARED`, `UPVOTED`, `DOWNVOTED`, `UNVOTED`

Implemented in:
- `hermes-pipeline/src/pipelines/topics.rs`
- `hermes-pipeline/src/pipelines/voting.rs`

### Phase 7: Testing & Documentation ✅

- Unit tests added for all pipeline modules (48 tests pass)
- `README.md` updated with all event types and Kafka topics
- Pipeline architecture documented

## Files Changed

| File | Change |
|------|--------|
| `hermes-schema/proto/membership.proto` | New - membership event messages |
| `hermes-schema/proto/moderation.proto` | New - moderation event messages |
| `hermes-schema/proto/topics.proto` | New - topic declaration message |
| `hermes-schema/proto/voting.proto` | New - vote cast message |
| `hermes-pipeline/src/pipelines/membership.rs` | New - membership pipeline |
| `hermes-pipeline/src/pipelines/moderation.rs` | New - moderation pipeline |
| `hermes-pipeline/src/pipelines/topics.rs` | New - topics pipeline |
| `hermes-pipeline/src/pipelines/voting.rs` | New - voting pipeline |
| `hermes-pipeline/src/pipelines/mod.rs` | Modified - export new pipelines |
| `hermes-pipeline/src/emit.rs` | Modified - new topics and `KafkaEvent` impls |
| `hermes-pipeline/src/main.rs` | Modified - integrate new pipelines |
| `hermes-pipeline/README.md` | Modified - document all event types |

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

- `docs/architecture.md` - Overall Hermes system design
- `hermes-substream/proto/schema.proto` - Source event definitions
- `hermes-relay/src/actions.rs` - Action type constants
