# Decision Records & RFCs

Central index of architectural decisions and design proposals across the Gaia codebase.

## How We Record Decisions

Two formats are used:

- **Inline `DECISIONS.md`** — multiple ADRs in one file, used for crate-specific decisions
- **Standalone files in `decisions/`** — one file per decision, used for detailed analyses with options considered

New decisions should follow whichever format the crate already uses. For new crates, prefer inline `DECISIONS.md` unless the decision warrants a detailed options analysis.

## Service Decision Records

| Crate | ID | Title | Status | Link |
|---|---|---|---|---|
| hermes-pipeline | ADR-001 | Event sequencing for cross-topic ordering | Accepted | [link](../../hermes-pipeline/docs/DECISIONS.md#adr-001-event-sequencing-for-cross-topic-ordering) |
| kg-indexer | ADR-001 | Per-message processing instead of cross-message batching | Superseded | [link](../../kg-indexer/docs/DECISIONS.md#adr-001-per-message-processing-instead-of-cross-message-batching) |
| kg-indexer | ADR-002 | Block-level buffering for cross-topic ordering | Accepted | [link](../../kg-indexer/docs/DECISIONS.md#adr-002-block-level-buffering-for-cross-topic-ordering) |
| hermes-ipfs-cache | 0001 | Cursor persistence strategy | Accepted | [link](../../hermes-ipfs-cache/docs/decisions/0001-cursor-persistence.md) |
| hermes-relay | 0001 | Multiple substreams modules consumers | Accepted | [link](../../hermes-relay/docs/decisions/0001-multiple-substreams-modules-consumers.md) |
| hermes-schema | 0001 | Wrapper messages for multi-event topics | Accepted | [link](../../hermes-schema/docs/decisions/0001-wrapper-messages-for-multi-event-topics.md) |

## Cross-cutting RFCs

| ID | Title | Link |
|---|---|---|
| RFC-0001 | Canonical graph inputs | [link](../rfcs/0001-canonical-graph-inputs.md) |
| RFC-0002 | Graph diff emission | [link](../rfcs/0002-graph-diff-emission.md) |
| RFC-0003 | Context-aware versioned diffs | [link](../rfcs/0003-context-aware-versioned-diffs.md) |
