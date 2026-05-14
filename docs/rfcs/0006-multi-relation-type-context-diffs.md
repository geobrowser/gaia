# Multi-Relation-Type Context-Aware Diffs (RFC)

*Date: 2026-04-30*

**Status:** Draft

**Supersedes:** [RFC 0003 — Context-Aware Versioned Diffs](./0003-context-aware-versioned-diffs.md)

## Summary
RFC 0003 introduced grouped diff responses but limited the supported groupings to `BLOCKS_ID`, deferring other relation types to a future RFC. This RFC generalizes that: every relation type found in the diff window that meets the criteria in the [Context-Edge Grouping Algorithm](#context-edge-grouping-algorithm-concrete) becomes a grouping key. `BLOCKS` stays special-cased as a static `blocks` field (preserving the existing client contract); every other relation type appears as a dynamic UUID key at the response root, listed in `groupKeys`.

## Goals
- Keep the diff query surface area unchanged (same route + params).
- Support rendering diffs inline on an entity page (block changes shown in place, plus any other relation-typed children).
- Group changes by **relation type ID** for **every** qualifying relation type found in the diff window, not just BLOCKS.
- For non-BLOCKS types, use **dynamic keys** (UUID relation type IDs) at the response root, alongside a `groupKeys: string[]` array for discoverability.
- Preserve the existing static `blocks: BlockChange[]` field so existing clients don't break.
- Preserve existing flat `values` and `relations` diff lists for changes that don't map to a relation-type grouping.

## Non-Goals
- Adding new diff endpoints or query params.
- Changing the diff API contract beyond extending the existing hybrid shape with dynamic keys (the storage model already includes context columns from RFC 0003's implementation; this RFC reuses them).
- Removing the static `blocks` field. Clients depend on it; a switch to dynamic-only is out of scope here.

## Diff vs RFC 0003
- **All qualifying relation types, not just BLOCKS.** The grouping algorithm runs over every distinct `edges[0].type_id` it observes; non-BLOCKS types now surface under their UUID keys instead of being dropped.
- **`groupKeys` is required.** Clients always get a discoverable list of dynamic keys present (excluding the static `blocks` slot).
- **Static `blocks` preserved.** Hybrid mode is intentional — BLOCKS stays under its named field for backward compatibility; everything else is dynamic.
- **"Alternative: Named Group Keys" is dropped.** Once we accept hybrid mode for BLOCKS, the named-key alternative for other types adds mapping overhead without payoff — UUID keys plus `groupKeys` are enough.

## Current Diff Shape (Reference)
`GET /versioned/entities/:id/diff` returns:
```
{
  "entityId": "...",
  "name": "...",
  "values": [...],
  "relations": [...],
  "blocks": [...]
}
```

Blocks are already derived from `BLOCKS` relations, but the response is flat and doesn’t provide a rendering structure that mirrors the page.

## GRC-20 Edit Context — Already Available
RFC 0003 sketched a protocol extension (an edit-header `contexts` dictionary plus per-op `context_ref` indices) for carrying edit context. That specific encoding wasn't adopted; the published `grc-20` crate (v0.4.x) instead carries `context: Option<Context>` inline on each op type (`CreateEntity`, `UpdateEntity`, `CreateRelation`, `UpdateRelation`). Same semantics — the dictionary optimization didn't ship.

Concretely:
```rust
pub struct Context {
    pub root_id: Id,
    pub edges: Vec<ContextEdge>,   // each edge: { type_id, to_entity_id }
}
```

So this RFC doesn't propose any GRC-20 protocol changes. It assumes the existing inline per-op `context` field as the authoritative source of context metadata.

## Proposed Response Shape
The root node keeps the static `blocks: BlockChange[]` field for BLOCKS-typed children (unchanged from RFC 0003's implementation). Every **other** qualifying relation type appears as a dynamic key (its UUID) at the response root. A `groupKeys` array enumerates the dynamic keys (BLOCKS is **not** in `groupKeys` because it lives under `blocks`).

Example (BLOCKS in the static slot, a custom `TABLE_ROWS_ID` as a dynamic key):
```json
{
  "entityId": "Byron",
  "name": "Byron",
  "values": [],
  "relations": [],
  "blocks": [
    {
      "id": "TextBlock_9",
      "type": "textBlock",
      "diff": [
        { "value": "old ", "removed": true },
        { "value": "new ", "added": true }
      ]
    },
    {
      "id": "ImageBlock_2",
      "type": "imageBlock",
      "before": null,
      "after": "https://..."
    }
  ],
  "groupKeys": ["TABLE_ROWS_ID"],
  "TABLE_ROWS_ID": [
    {
      "id": "Row_4",
      "type": "dataBlock",
      "before": "Old row name",
      "after": "New row name"
    }
  ]
}
```

### Key Points
- The grouping key is the **relation type ID**, not the relation ID.
- BLOCKS-typed children land in the static `blocks` field (existing behavior). All other qualifying relation types land in dynamic root-level keys.
- `groupKeys: string[]` lists every **dynamic** key present (sorted, only keys with at least one change). `blocks` is **not** in `groupKeys`.
- Items inside a group are `GroupedChangeItem` payloads (`BlockChange` for known block-shaped types, `EntityDiff` otherwise).
- Ordering inside a group follows existing snapshot ordering when available (e.g. `position` for blocks); falls back to entity ID order otherwise.
- Root-level `values` and `relations` remain unchanged for changes that don't map to any grouping (BLOCKS or dynamic).

## Type Model
```ts
export type GroupedChangeItem =
  | TextBlockChange
  | ImageBlockChange
  | DataBlockChange
  | EntityDiff;

export type GroupedEntityDiff = {
  entityId: string;
  name: string | null;
  values: ValueChange[];
  relations: RelationChange[];
  // Static slot for BLOCKS-typed children (existing client contract).
  blocks: BlockChange[];
  // Lists every dynamic key present at the root (BLOCKS is excluded).
  groupKeys: string[];
} & Record<string, GroupedChangeItem[]>;
```

`blocks` is the BLOCKS-only static slot — it predates this RFC and stays for backward compatibility. Every other qualifying relation type lands under its UUID at the root and is enumerated in `groupKeys`.

## Mapping Strategy
1) Compute the current `EntityDiff` using existing snapshot + diff logic.
2) Emit the grouped root with the standard fields (`entityId`, `name`, `values`, `relations`, `blocks`, `groupKeys`).
3) For each distinct `groupKey` produced by the grouping algorithm below:
   - If `groupKey == BLOCKS_ID`, append the diffs into the static `blocks` field.
   - Otherwise, map the diffs onto `[groupKey]` at the response root and add the key to `groupKeys`.
4) Keep changes that don't match any context grouping in the flat `values` and `relations` arrays.
5) If a change is grouped (under `blocks` or any dynamic key), do not emit it as a standalone root in proposal diffs.

## Context-Edge Grouping Algorithm (Concrete)
When edit context metadata is available, use it to route changes into relation-type groupings.

Inputs:
- `rootEntityId` from the diff request
- `entityDiff` for the root (already computed)
- `contexts` for edits in the diff window (GRC-20 context extension)
- `changedEntityIds` derived from the diff (see below)

### Implementation note: persisted-context model

The runtime algorithm described above is simplified by persisting context
columns directly on each version row (see `docs/specs/versioned-diffing.md`
for the full data model). Instead of matching contexts at diff time by
walking edges, the indexer extracts `(root_id, first_edge_type_id)` at write
time and stores those two values as columns on `value_versions` and
`relation_versions`. Diff queries then locate candidate entities with a
single index seek:

```sql
-- Values
SELECT v.entity_id, v.context_edge_type_id FROM value_versions v
WHERE v.context_root_id = $rootEntityId AND v.context_edge_type_id IS NOT NULL ...

-- Relations: from_entity_id is the changed child (the relation's owner),
-- not to_entity_id (which is the relation's target — different dimension).
SELECT r.from_entity_id AS entity_id, r.context_edge_type_id FROM relation_versions r
WHERE r.context_root_id = $rootEntityId AND r.context_edge_type_id IS NOT NULL ...
```

The `groupKey = edges[0].type_id` rule applies uniformly during discovery —
every distinct `context_edge_type_id` value found in the result set is a
candidate grouping. The response shape then routes BLOCKS into the static
`blocks` slot and every other type into a dynamic root-level key.

#### Algorithm steps

1) Build a set of `changedEntityIds`:
   - Include any entity IDs surfaced by the diff (value rows, relation rows whose `from_entity_id` is in the relation graph rooted at `rootEntityId`).
2) For each `changedEntityId`, find the `Context` it belongs to (from persisted columns or, fallback, edit metadata):
   - Match if the context's `root_id` equals `rootEntityId` **and**
     the last `ContextEdge.to_entity_id` equals `changedEntityId`.
3) Use the **first** edge in `Context.edges` to choose the grouping key:
   - `groupKey = edges[0].type_id`
4) Route the change:
   - If `groupKey == BLOCKS_ID`, append into the static `blocks` field.
   - Otherwise, append under the `[groupKey]` dynamic key and ensure `groupKey` is in `groupKeys`.
5) If no matching context, leave the change in the flat `values` / `relations` lists.

Notes:
- The discovery step (steps 1–3) is type-agnostic; the routing step (4) is
  where BLOCKS gets its static slot. This keeps the algorithm uniform while
  preserving the legacy `blocks` contract.
- The first edge represents the immediate child relation from the root (e.g.
  `Byron --BLOCKS--> TextBlock_9`, or `Table_3 --TABLE_ROWS--> Row_4`),
  which is what we need to render inline.

## Inspectability
Dynamic UUID keys at the response root reduce strict typing, so `groupKeys: string[]` is **required** in every response. Clients iterate `groupKeys` to discover which dynamic keys are present and read each one off the root. The static `blocks` field is read directly — it's not in `groupKeys`.

Example response with BLOCKS in the static slot and one dynamic grouping:
```json
{
  "entityId": "Byron",
  "name": "Byron",
  "values": [],
  "relations": [],
  "blocks": [
    {
      "id": "TextBlock_9",
      "type": "textBlock",
      "diff": [
        { "value": "old ", "removed": true },
        { "value": "new ", "added": true }
      ]
    }
  ],
  "groupKeys": ["TABLE_ROWS_ID"],
  "TABLE_ROWS_ID": [
    {
      "id": "Row_4",
      "type": "dataBlock",
      "before": "Old",
      "after": "New"
    }
  ]
}
```

`groupKeys` is sorted (deterministic ordering for clients), and only includes dynamic keys with at least one change. `blocks` is always present (possibly empty).

## Future Extensions
- Specify per-type child renderers (e.g. column shape for `TABLE_ROWS_ID` items) once frontend rendering needs become concrete.
- Apply the same dynamic-key grouping to proposal diffs (currently flat `EntityDiff[]`).

## Why Context `to_entity_id` Must Be Persisted
Context-aware grouping answers two separate questions:

1. Which relation-type bucket should this change render under?
   - Answer: the first context edge's `type_id`, e.g. `BLOCKS_ID`.
2. Which child inside that bucket changed?
   - Answer: the last context edge's `to_entity_id`.

For example, if Byron has a text block:

```text
Byron
  -- BLOCKS --> TextBlock_9
```

and an edit changes text inside `TextBlock_9`, the edit context is:

```text
root_id = Byron
edges = [
  { type_id: BLOCKS_ID, to_entity_id: TextBlock_9 }
]
```

From this, the API can derive:

```text
group key = BLOCKS_ID
changed child = TextBlock_9
```

The second value is what determines where the change is shown. Today, the
implementation has no column for it; it falls back to inference from the row:

- For value rows, "changed child" is read off `value_versions.entity_id`.
- For relation rows, "changed child" is read off `relation_versions.from_entity_id`.

**Neither column is guaranteed to be the context's leaf entity.** They happen
to coincide in well-formed cases, but the storage cannot prove it. A reader
of `queries.ts:queryContextEntities` (the value/relation branches under
"Context-based discovery") sees this load-bearing assumption embedded in SQL,
not enforced anywhere upstream. When the assumption holds the answer is
correct; when it doesn't, the wrong entity gets surfaced and there's no
diagnostic to detect it.

### A case where inference is just convenient (and looks fine)

```text
Byron
  -- BLOCKS --> TextBlock_9

TextBlock_9
  -- MENTIONS --> Ada
```

An edit changes the `MENTIONS` relation inside `TextBlock_9`. The relation
row:

```text
from_entity_id = TextBlock_9
to_entity_id = Ada
type_id = MENTIONS
```

The edit context:

```text
root_id = Byron
edges[0].type_id = BLOCKS_ID
edges[last].to_entity_id = TextBlock_9
```

The changed child per the RFC is `TextBlock_9`. Inference using
`from_entity_id` produces `TextBlock_9` — same answer. No bug, but only by
coincidence: nothing in the schema says "use `from_entity_id` because that's
the leaf." The implementation reads `from_entity_id` because *in this
example* it happens to match.

### A concrete breaking case

```text
Byron
  -- BLOCKS --> TextBlock_9
```

An edit, authored from inside `TextBlock_9`, creates a reified relation
between two foreign entities — say, a `LINK` from `Source_A` to `Source_B`,
neither of which is structurally a child of Byron in the relation graph.
The relation row:

```text
from_entity_id = Source_A
to_entity_id   = Source_B
type_id        = LINK
```

The edit context still describes "this happened inside Byron's TextBlock_9":

```text
root_id            = Byron
edges[0].type_id   = BLOCKS_ID
edges[last].to_id  = TextBlock_9
```

By the RFC rule, the changed child to render under Byron's `BLOCKS_ID`
group is `TextBlock_9` — a `LINK` was added inside it. The current
implementation infers from `from_entity_id` and surfaces `Source_A` instead,
which isn't a block of Byron and isn't even reachable from Byron in this
fixture. The output is wrong and there is no signal at the storage layer
that anything is off.

### Fix

Persist the context's leaf entity alongside the existing context columns —
specifically `context_last_to_entity_id`, holding `edges.last().to_entity_id`
for every value or relation row authored under a context. The column is
named after the GRC-20 field deliberately: every row already has its own
`entity_id`, so a column called `context_entity_id` would be ambiguous;
`context_last_to_entity_id` makes clear this is a *different* entity than
the row's own.

With that column in place, the discovery rule the implementation enforces
becomes literally the rule the RFC specifies:

```text
context_root_id            = rootEntityId
context_last_to_entity_id  = changedEntityId
group key                  = context_edge_type_id
```

This closes the algorithm gap flagged in *Implementation note:
persisted-context model* above — the row-level attribution there is
currently correct only by structural coincidence with `entity_id` /
`from_entity_id`. With the leaf column it is correct by construction.
