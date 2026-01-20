# Context-Aware Versioned Diffs (RFC)

## Summary
We want diffs to render changes in the context where they were made. The API should continue to use the same diff query interface but return a response that groups changes under renderable relation types (starting with blocks).

## Goals
- Keep the diff query surface area unchanged (same route + params).
- Support rendering diffs inline on an entity page (block changes shown in place).
- Group changes by **relation type ID** (e.g. `BLOCKS_ID`).
- Preserve existing flat `values` and `relations` diff lists for standard renderable changes.

## Non-Goals
- Adding new diff endpoints or query params.
- Changing the storage model in this RFC.

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

## GRC-20 Edit Context Extensions
To support context-aware grouping beyond relation structure (e.g. edits spanning multiple nested contexts), we propose adding context metadata to edits.

### Edit-Level Context Dictionary
Add a `contexts` dictionary to the edit header:
```
Context {
  root_id: ID
  edges: List<ContextEdge>
}

ContextEdge {
  type_id: ID
  to_entity_id: ID
}
```

Add to the Edit header:
```
context_count: varint
contexts: Context[]
```

### Per-Op Context Reference
Add an optional `context_ref` field to each Op payload:
```
context_ref: varint?   // index into contexts[] (optional)
```

- If `context_ref` is omitted, the op is treated as having no explicit context.
- Multiple ops can share a single context entry via `context_ref`.

### Rationale
- Avoids repeating full context paths on every op.
- Allows a single edit to span many contexts.
- Keeps the diff API surface unchanged while enabling richer grouping.

## Proposed Response Shape
Return a grouped diff object where the root node includes **dynamic keys** for relation-type groupings.

Example (blocks):
```json
{
  "entityId": "Byron",
  "name": "Byron",
  "values": [],
  "relations": [],
  "BLOCKS_ID": [
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
      "from": null,
      "to": "https://..."
    }
  ]
}
```

### Key Points
- The grouping key is the **relation type ID**, not the relation ID.
- Items remain `BlockChange` payloads.
- Ordering follows existing snapshot ordering for blocks.
- Root-level `values` and `relations` remain unchanged for diffs not mapped to renderable relations.

## Type Model
```ts
export type GroupedEntityDiff = {
  entityId: string;
  name: string | null;
  values: ValueChange[];
  relations: RelationChange[];
} & Record<string, BlockChange[]>;
```

## Mapping Strategy
1) Compute the current `EntityDiff` using existing snapshot + diff logic.
2) Emit the grouped root with the standard fields (`entityId`, `name`, `values`, `relations`).
3) For each supported relation type grouping (initially `BLOCKS_ID`), map the grouped diff array onto `[relationTypeId]`.
4) Keep other changes in the flat `values` and `relations` arrays.

## Context-Edge Grouping Algorithm (Concrete)
When edit context metadata is available, use it to route changes into relation-type groupings.

Inputs:
- `rootEntityId` from the diff request
- `entityDiff` for the root (already computed)
- `contexts` for edits in the diff window (GRC-20 context extension)
- `changedEntityIds` derived from the diff (see below)

Steps:
1) Build a set of `changedEntityIds`:
   - Include any entity IDs referenced in `entityDiff.blocks` (block entity IDs).
   - Include entity IDs from value/relations diffs if we later group non-block changes.
2) For each `changedEntityId`, find the `Context` it belongs to (from edit metadata):
   - Match if the context’s `root_id` equals `rootEntityId` **and**
     the last `ContextEdge.to_entity_id` equals `changedEntityId`.
3) Use the **first** edge in `Context.edges` to choose the grouping key:
   - `groupKey = edges[0].type_id`
4) Attach the change to `[groupKey]`:
   - If `groupKey` equals `BLOCKS_ID`, push the block diff item.
   - Otherwise, defer to future relation-type groupings.
5) If no matching context, leave the change in the flat `values` / `relations` lists.

Notes:
- This keeps the API response shape unchanged while using context metadata to
  pick the correct relation-type bucket.
- The first edge represents the immediate child relation from the root (e.g.
  Byron --BLOCKS--> TextBlock_9), which is what we need to render inline.

## Inspectability
Dynamic keys reduce strict typing. Two possible approaches:
- Maintain a known list of renderable relation type IDs in the UI.
- Optionally add `groupKeys: string[]` to the response in the future for discovery.

## Alternative: Named Group Keys
Instead of using raw relation type IDs as dynamic keys, we could expose a stable set of
named group keys (e.g. `blocks`) and map those to relation type IDs internally. This
improves inspectability at the cost of an additional mapping layer.

Example (named key):
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
  ]
}
```

## Future Extensions
- Support grouping for additional relation types (e.g., tables, rows, cells).
- Persist edit-context metadata from GRC-20 edits in the indexer for UI grouping.
