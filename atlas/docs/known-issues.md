# Known Issues

## Event Ordering and Deduplication on Producer Reprocess

**Status**: Unresolved

### Problem

When Atlas (or any derived event producer) needs to reprocess events from the beginning of the source stream, downstream consumers may receive:

1. **Duplicate events** - Events they've already processed
2. **Out-of-order events** - Older events arriving after newer ones

This is problematic because:
- Kafka doesn't deduplicate across producer restarts
- Block numbers can't be used for ordering when multiple producers emit to the same topic
- We don't want consumers to implement complex ordering/deduplication logic

### When This Happens

- New Atlas version requires full reprocess
- Bug fix requires reprocessing historical events
- Schema migration requires rebuilding derived state

### Why Standard Solutions Don't Fully Apply

| Solution | Limitation |
|----------|------------|
| Cursor persistence | Only helps for normal restarts, not intentional reprocessing |
| Kafka idempotent producer | Only dedupes within a session, PID changes on restart |
| Block number ordering | Doesn't work with multiple producers emitting to same topic |
| Producer epoch/version | Forces consumers to track producer state |

### Potential Approaches

**1. Operational procedure (current)**
- Full reprocess is a manual coordination event
- Clear topics, notify consumers, replay
- Simple but not automated

**2. New topic per major version**
- `topology.canonical.v1` → `topology.canonical.v2`
- Consumers switch to new topic
- Clean separation, requires consumer reconfiguration

**3. Reset signal message**
- Producer emits a "reset" message before replaying
- Consumers clear state when they see it
- Requires minimal consumer logic, automatable

**4. Idempotent state application**
- If events are full state snapshots (not deltas), duplicates are harmless
- Consumers overwrite with same data
- Only works if all consumers treat events as snapshots

### Current Approach

We accept that full reprocessing is an operational event requiring manual coordination. For normal operation:

1. Atlas persists its cursor (TODO: implement)
2. Normal restarts resume from cursor, no duplicate emission
3. Full reprocess is rare and planned

### Consumer Guidance

- Treat `CanonicalGraphUpdated` as a full state snapshot
- Applying the same snapshot twice should be idempotent
- If you see unexpected duplicates, Atlas may be reprocessing

### Future Considerations

If automated reprocessing becomes necessary, consider:
- Adding `producer_epoch` to messages (consumers reset on new epoch)
- Using Kafka Streams to deduplicate before delivery
- Implementing transactional outbox pattern
