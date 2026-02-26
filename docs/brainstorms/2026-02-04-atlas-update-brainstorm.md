---
date: 2026-02-04
topic: atlas-update
---

# Atlas Update: Live Substream + RFC Implementation

## What We're Building

Three related changes to Atlas:

1. **Live Substream Migration**: Switch from `StreamSource::Mock` to `StreamSource::Live` for real blockchain data
2. **RFC 0001 Implementation**: Add Editor/Member edges as canonical-granting, update event handling
3. **RFC 0002 Implementation**: Add incremental graph diff emission with MOVED semantics

## Why This Approach

The RFCs define the target state for canonical graph computation and diff emission. We audited both for edge cases and resolved ambiguities before implementation.

**Sequencing decision**: Get something working end-to-end first, then optimize for performance. Full BFS recomputation (~70-90ms at 100k nodes) is fast enough for current scale.

## Key Decisions

### RFC 0001: Canonical Graph Inputs

| Decision | Choice | Rationale |
|----------|--------|-----------|
| MOVED encoding | Explicit `MOVED` change type | Cleaner semantics than REMOVED+ADDED pairs; no ordering dependencies |
| Cascade handling | MOVED for every descendant | Keeps distances authoritative; diffs are self-contained |
| DAO initial membership | Separate `EDITOR_ADDED`/`MEMBER_ADDED` events | Already the pattern in `action-data-mapping.md` |
| Topic edge as parent | Yes, emit `TopicEdge` | If discovered via topic edge at shorter distance, that's the canonical parent |
| SUBSPACE_ADDED/REMOVED | Legacy/deprecated | Don't need to handle these event types |

### RFC 0002: Graph Diff Emission

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Diff algorithm | Full BFS + sorted vector merge | Provably correct; RFC benchmarks show acceptable performance |
| Bootstrap | Diff with all ADDEDs | Consistent format; consumers always process diffs |
| Schema versioning | Skip for now | Other hermes-schema protos don't have it; YAGNI |
| Incremental optimization | Note for future | Full BFS is fast enough; complexity not justified yet |

## Edge Cases Analyzed

### Correctness Concerns (Resolved)

1. **Circular references**: BFS handles naturally (first-seen wins)
2. **Self-edges**: No-op (space already exists if referenced)
3. **Edge to non-existent space**: Edge stored; node discoverable if later created
4. **Concurrent explicit + topic edges**: Log order within block is deterministic
5. **Topic membership cascades**: Full BFS recomputation handles correctly
6. **Subtree detachment with alternate paths**: Full BFS finds remaining paths
7. **Multiple paths to same node**: First-seen at shortest distance wins; if removed, MOVE to alternate

### Out of Scope

- **Root space changes**: Configuration concern, not event-driven
- **Consumer crash recovery**: Standard Kafka offset semantics apply
- **Incremental diff algorithm**: Documented as future optimization path

## Implementation Gaps

### Event Handling (RFC 0001)

- [ ] Add `EditorAdded`, `EditorRemoved`, `MemberAdded`, `MemberRemoved` to `TrustExtension` enum
- [ ] Add `EdgeType::Editor` and `EdgeType::Member` to tree representation
- [ ] Wire action conversion for `EDITOR_ADDED`, `EDITOR_REMOVED`, `MEMBER_ADDED`, `MEMBER_REMOVED`

### Diff Emission (RFC 0002)

- [ ] Add `MOVED` change type to existing ADDED/REMOVED
- [ ] Add `CanonicalGraphDiff` and `TransitiveGraphDiff` protobuf messages
- [ ] Implement sorted vector merge diff algorithm
- [ ] Add new Kafka emitter for diffs

### Live Substream

- [ ] Add environment variable configuration (`USE_MOCK`, `SUBSTREAMS_ENDPOINT`, etc.)
- [ ] Switch `main.rs` from `StreamSource::mock()` to conditional `StreamSource::live()`

## Open Questions

None remaining - all edge cases resolved during brainstorm.

## Future Optimization Path

If performance becomes a concern at scale (>100k nodes), consider:

1. **Add parent pointers and distances** to tree structure
2. **Implement alternate path detection** for edge removal
3. **Incremental diff computation** instead of full BFS

The existing `TransitiveCache` with `reverse_deps` provides foundation for this.

## Next Steps

Run `/workflows:plan` for detailed implementation steps.
