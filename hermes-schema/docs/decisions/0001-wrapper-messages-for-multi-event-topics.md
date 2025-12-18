# ADR-0001: Wrapper Messages for Multi-Event Topics

## Status

Proposed

## Context

The hermes-pipeline produces events to Kafka topics. Some topics contain multiple event types:

| Topic | Event Types |
|-------|-------------|
| `space.governance` | `HermesProposalCreated`, `HermesProposalVoted`, `HermesProposalExecuted` |
| `space.membership` | `HermesRoleGranted`, `HermesRoleRevoked`, `HermesSpaceLeft` |
| `space.moderation` | `HermesEditorFlagged`, `HermesEditorUnflagged`, `HermesContentFlagged`, `HermesContentUnflagged` |

Currently, we distinguish event types using Kafka message headers (e.g., `event-type: PROPOSAL_CREATED`). This approach has several limitations:

1. **Kafka-UI incompatibility**: The `ProtobufFileSerde` requires a 1:1 mapping between topic and message type. It cannot dynamically select a proto schema based on headers, so it fails to decode messages that don't match the configured schema.

2. **Schema Registry incompatibility**: Tools like Confluent Schema Registry enforce one schema per topic. Multiple unrelated schemas on the same topic breaks compatibility checking.

3. **Consumer complexity**: Consumers must read headers, switch on event type, and decode with the correct proto. Header handling varies across Kafka client libraries.

4. **Not self-describing**: Without reading headers, a consumer cannot determine the message type from the payload alone.

## Decision

Wrap related events in a single protobuf message using `oneof` for topics with multiple event types.

### Proposed Schema Changes

```protobuf
// governance.proto
message GovernanceEvent {
  oneof event {
    HermesProposalCreated proposal_created = 1;
    HermesProposalVoted proposal_voted = 2;
    HermesProposalExecuted proposal_executed = 3;
  }
}

// membership.proto
message MembershipEvent {
  oneof event {
    HermesRoleGranted role_granted = 1;
    HermesRoleRevoked role_revoked = 2;
    HermesSpaceLeft space_left = 3;
  }
}

// moderation.proto
message ModerationEvent {
  oneof event {
    HermesEditorFlagged editor_flagged = 1;
    HermesEditorUnflagged editor_unflagged = 2;
    HermesContentFlagged content_flagged = 3;
    HermesContentUnflagged content_unflagged = 4;
  }
}
```

### Producer Changes

The pipeline wraps events before sending:

```rust
// Before
emitter.emit(&proposal_created)?;

// After
emitter.emit(&GovernanceEvent {
    event: Some(governance_event::Event::ProposalCreated(proposal_created)),
})?;
```

### Consumer Changes

Consumers decode the wrapper and match on the inner event:

```rust
let event = GovernanceEvent::decode(payload)?;
match event.event {
    Some(Event::ProposalCreated(e)) => handle_created(e),
    Some(Event::ProposalVoted(e)) => handle_voted(e),
    Some(Event::ProposalExecuted(e)) => handle_executed(e),
    None => return Err("Empty event"),
}
```

## Consequences

### Benefits

1. **Kafka-UI works**: One schema per topic means ProtobufFileSerde can decode all messages.

2. **Schema Registry compatible**: Each topic has exactly one schema that evolves together.

3. **Self-describing messages**: The payload itself indicates the event type via the `oneof` field tag.

4. **Language-agnostic**: Works identically in all languages since it's pure protobuf, no header parsing required.

5. **Simpler consumer code**: No header inspection or type-switching logic needed outside of protobuf's native `oneof` matching.

### Trade-offs

1. **Extra serialization layer**: 1-2 bytes overhead per message for the `oneof` field tag. Negligible in practice.

2. **Schema coupling**: All event types in a wrapper evolve together. Adding a field to `HermesProposalCreated` technically changes `GovernanceEvent`. In practice this is acceptable since they're already logically coupled.

3. **Breaking change**: Existing consumers must update to decode wrappers instead of inner messages.

4. **Migration effort**: Producer and consumer code changes required.

## Alternatives Considered

### Alternative 1: One Topic Per Event Type

Split into separate topics:
```
space.governance.proposal-created
space.governance.proposal-voted
space.governance.proposal-executed
```

**Rejected because:**
- Topic proliferation (16 topics instead of 9)
- Loses ordering guarantees for related events
- More operational overhead

### Alternative 2: Keep Using Headers

Continue using `event-type` headers to distinguish message types.

**Rejected because:**
- Kafka-UI cannot decode all messages
- Schema Registry incompatible
- Consumer complexity across languages

## Implementation Notes

- The `event-type` header can be retained for debugging/routing purposes even after wrappers are implemented
- Topics with single event types (`space.creations`, `knowledge.edits`, etc.) do not need wrappers
- Consider adding a `metadata` field to wrappers for future cross-cutting concerns
