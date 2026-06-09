# `/v2/versioned` — server-side diff enrichment

This module moves the geogenesis client-side diff post-processing onto the backend,
so the frontend can stop transforming raw diffs and eventually delete most of
`postProcessDiffs`. v2 is **purely additive** over v1: same response shape plus
optional enriched fields. `/versioned` (v1) is never changed.

## What this replaces (geogenesis)

- **`apps/web/core/utils/diff/diff.ts` → `postProcessDiffs`** (~L217–714; the ticket cites L203–680).
  A 9-step pipeline turning a flat, ID-only entity diff into a hierarchical, display-ready one.
- Helper `groupBlocksUnderParents` (~L154).
- GitHub: https://github.com/geobrowser/geogenesis/blob/master/apps/web/core/utils/diff/diff.ts#L203-L680

When v2 is adopted, the entity-history path can skip `postProcessDiffs` entirely
(verified locally). The proposal path still needs it until the v2 proposal
endpoints exist (see "Remaining").

## Linear ticket — "Server-Side Diff Enrichment" (Sophia)

Move six client transforms to the backend (each independent, additive to the diff response):

1. **Block folding** — nest block entities (text/image/video/data) under the parent's `blocks[]`
   instead of returning them as separate top-level entity diffs.
2. **Media-property entity filtering** — IMAGE_TYPE/VIDEO_TYPE property entities (avatar/cover)
   look like image/video blocks but aren't; drop them as top-level entries and inline their URL.
3. **Name resolution** — resolve human names for property IDs, relation type IDs, relation
   source/target IDs, and block entity IDs (no raw UUIDs in the UI).
4. **Media URL inlining** — inline the image/video URL on relation before/after so the UI can
   render previews (API returns only the target entity ID today).
5. **Block-diff synthesis for the entity-history endpoint** — history `blocks: []` is empty even
   when blocks changed; synthesize/return the block diffs. ("most painful" per Sophia.)
6. **Data-block config merging** — a data block's config (view/columns/sort) lives on a separate
   "blocks relation entity"; merge it into the parent data block so it's one change.

Affected endpoints (each gets a `/v2/...` enriched variant):
- `GET /v2/versioned/entities/:id/diff` (#1, #3–#6 apply; #2 media-property filtering is proposal-only since it needs multiple top-level entities; #5 = the no-`fromEditId` snapshot path)
- `GET /v2/versioned/entities/:id` (snapshot)
- `GET /v2/versioned/proposals/:id/diff` (flat `EntityDiff[]` across many roots)
- `GET /v2/versioned/proposal-groups/diff`

### Sophia's review fixes to the examples (must hold)
- Example 1 must show the **most common data-block change — view/columns/sort — which lives on the
  separate "blocks relation entity"** and must be folded into the parent block (not just the block's
  own values/relations).
- `block.relations[]` must carry **`spaceId`** (matches the frontend shape).
- Cover **removal + replacement (swap)** must be covered: a swapped cover needs **both
  `before.imageUrl` and `after.imageUrl`**.

## Status — the 6 asks vs the v2 entity-diff endpoint

| # | Ask | Entity-diff `/v2/versioned/entities/:id/diff` | Where |
|---|-----|------------------------------------------------|-------|
| 3 | Name resolution | ✅ done | `enrich-names.ts` (`enrichNames`) — `42b3af76` |
| 4 | Media URL inlining | ✅ done (versioned before/after + group-nested) | `enrich.ts` (`enrichWithMediaUrls`), `queries.ts` (`batchGetMediaUrlsAtVersion`) — `42b3af76` |
| 5 | Block synthesis + snapshot mode | ✅ done | snapshot mode in `router.ts` (`6d6a944b`); blocks come from the grouped snapshot's versioned discovery |
| 1 | Block folding | ✅ for the single-entity path (rich shape) | `enrich-blocks.ts` (`enrichBlocks`) — `5f5ebf8a`. Cross-parent folding = proposal-only (TODO) |
| 6 | Data-block config merge | ✅ done | `enrich-block-config.ts` (`enrichBlockConfig`), `queries.ts` (`getBlockConfigEntityIds`) — `51903d3f` |
| 2 | Media-property filtering | n/a for single-entity; proposal-only | TODO (proposal endpoints) |

## The 9 `postProcessDiffs` steps vs v2 (entity-diff)

| Step (geogenesis) | v2 entity-diff |
|-------------------|----------------|
| 1. Classify entities (block/media-property/config; ADD/REMOVE; collect name IDs) | ✅ block typing from the grouped snapshot; name-ID collection in `enrichNames`. Media-property classification only matters with multiple top-level entities → proposal-only |
| 2. Resolve orphan block parents via backlinks | ✅ native — the parent IS the queried entity; blocks discovered via its BLOCKS relations. Cross-parent = proposal-only |
| 3. Resolve block-relation-entity (config) parents | ✅ `getBlockConfigEntityIds` (parent's BLOCKS relation → relation_id) |
| 4. Batch-fetch names | ✅ `enrichNames` → `batchGetEntityNames` |
| 5. Merge config into data block | ✅ `enrichBlockConfig` |
| 6. Inject orphan BLOCKS + resolve media URLs | ✅ media URLs `enrichWithMediaUrls`. Orphan-BLOCKS injection / media-property filtering = proposal-only |
| 7. Synthesize missing block diffs | ✅ grouped snapshot discovers + diffs blocks (add/update/remove) from `relation_versions` |
| 8. Group blocks under parents | ✅ native (single parent). Cross-parent = proposal-only |
| 9. Apply resolved names | ✅ `enrichNames` stamps names on values + relations (top-level, grouped, and block values/relations) |

**Bottom line:** for `/v2/versioned/entities/:id/diff`, the 9-step pipeline is fully covered. The
steps not explicitly implemented (orphan-parent backlinks, media-property filtering, cross-parent
grouping) only apply when a response has multiple top-level entities — i.e. the **proposal /
proposal-groups** endpoints, which don't exist in v2 yet.

## Pipeline (router.ts)

```
diffGroupedEntitySnapshots(before, after)   // shared v1 diff (Byron)
  → enrichBlocks(before, after)             // A1: blockName + block.values/relations
  → enrichBlockConfig(parent, from, to)     // A2/#6: fold config from the BLOCKS relation entity
  → enrichNames(db)                          // #3/#9: propertyName/typeName/toEntityName (incl. blocks)
  → enrichWithMediaUrls(db, from, to)        // #4: imageUrl/videoUrl, versioned before/after
```
Snapshot mode: omit `fromEditId` → empty "before" → all-added diff (then same enrichment).

## Tests

`api/src/versioned/__tests__/v2-entity-diff.test.ts` (vitest, real DB, `DATABASE_URL`-gated).
Covers: names, versioned media before/after via a **cover swap**, `videoUrl`, rich data-block
(blockName + values + relations w/ `spaceId`), **config merge from the relation entity (A2)**,
and snapshot mode. Run in CI by `.github/workflows/api-integration-tests.yml` (now includes
`src/versioned/__tests__/`).

## Remaining (proposal-centric → Track B)

- **#2 media-property filtering** + **#1 cross-parent block folding** — only bite with multiple
  top-level entities.
- **Missing v2 endpoints:** `/v2/versioned/entities/:id` (snapshot), `/proposals/:id/diff`,
  `/proposal-groups/diff`. The enrichers above are reusable; proposal folding additionally needs
  to fold each block under the *right* parent (or expose a `parentId` per top-level diff).

---

# v2 response format (reference)

Same top-level shape as v1 `DiffResponse` (entity-diff endpoint), with **optional, additive**
fields. Anything v2 adds is absent on v1. Authoritative types: `src/versioned/v2/types.ts`
(`DiffResponseV2`, `RelationChangeV2`) + the optional fields on `BlockChange` in `src/versioned/types.ts`.

**Top level** (unchanged from v1): `entityId`, `name`, `values[]`, `relations[]`, `blocks[]`,
`groupKeys[]`, dynamic group keys spread at root, and edit metadata
`fromEditName` / `fromCreatedById` / `fromCreatedBy` / `toEditName` / `toCreatedById` / `toCreatedBy`.
In **snapshot mode** (no `fromEditId`) the `from*` fields are `null` and every change is all-added.

**ValueChange** — v1: `propertyId`, `spaceId`, `type`, `before`, `after`, `diff?`.
v2 adds:
| field | type | when |
|---|---|---|
| `propertyName` | `string \| null` | always (resolved name; null if none) |

**RelationChange** — v1: `relationId`, `typeId`, `spaceId`, `changeType`, `before`, `after`.
v2 adds:
| field | type | when |
|---|---|---|
| `typeName` | `string \| null` | always |

**RelationChange.before / .after** (endpoint) — v1: `toEntityId`, `toSpaceId`, `position`.
v2 adds:
| field | type | when |
|---|---|---|
| `toEntityName` | `string \| null` | always |
| `imageUrl` | `string \| null` | target is an `IMAGE_TYPE` entity (mutually exclusive with `videoUrl`); resolved at that side's version |
| `videoUrl` | `string \| null` | target is a `VIDEO_TYPE` entity |

**BlockChange** (`textBlock` / `imageBlock` / `dataBlock`) — v1: `id`, `type`, `before`, `after`, `diff?` (text).
v2 adds:
| field | type | when |
|---|---|---|
| `blockName` | `string \| null` | always (block entity's NAME) |
| `values` | `ValueChange[]` | present when the block has non-headline value changes (NAME/MARKDOWN/IMAGE_URL excluded) |
| `relations` | `RelationChange[]` | present when the block has relation changes (TYPES stripped, BLOCKS excluded); for data blocks, includes the merged config (VIEW_PROPERTY/SHOWN_COLUMNS/PROPERTIES) from the reified BLOCKS relation entity |

Notes: `imageUrl`/`videoUrl` use exactly those field names (not `mediaUrl`+`mediaType`); image
width/height are not inlined. `block.relations[]` carry `spaceId` at the relation level.

---

# v2 response examples

Real captures + the v2 target shape. The plan: `/v2/...` emits the AFTER directly.
**Add new examples here as endpoints/enrichments land.**

## Example 1 — block folding + data-block config merge (proposal diff)

`GET /versioned/proposals/148a0917fe1a47cc82868d6a877e194c/diff?spaceId=41e851610e13a19441c4d980f2f2ce6b&limit=50`

Proposal "Edited Benchmark ontology": page "Benchmark" gains data block "Benchmarks".
**BEFORE:** two separate top-level entity diffs (the page + the "Benchmarks" data block), `blocks: []` on both.

**AFTER (v2 target)** — the data block folds into `Benchmark.blocks[]`, with its own values/relations
**and** the view/columns/sort config (from the separate blocks-relation entity) merged in. Note
`spaceId` is kept on `block.relations[]` (Sophia fix), and the config relations (`View`, `Shown columns`)
come from the reified BLOCKS relation entity, not the data block:

```json
[
  {
    "entityId": "05a8f182dccf4ee5b81bcaf07bae976d",
    "name": "Benchmark",
    "values": [ /* Name, "Is type property" */ ],
    "relations": [ /* Types, Data type, To entity types */ ],
    "blocks": [
      {
        "id": "34a55c289f1d4e798fdbe0804e1c802f",
        "type": "dataBlock",
        "before": null,
        "after": "Benchmarks",
        "blockName": "Benchmarks",
        "values": [
          { "propertyId": "14a46854bfd14b1882152785c2dab9f3", "spaceId": "41e851610e13a19441c4d980f2f2ce6b",
            "type": "TEXT", "before": null, "after": "{\"filter\":{…}}",
            "diff": [{ "value": "{\"filter\":…}", "added": true }], "propertyName": "Filter" }
        ],
        "relations": [
          { "relationId": "e9c57ffde3cc4851b84a9ad4c071a7bf", "typeId": "1f69cc9880d444abad493df6a7b15ee4",
            "spaceId": "41e851610e13a19441c4d980f2f2ce6b", "changeType": "ADD",
            "after": { "toEntityId": "1295037a5d9c4d09b27c5502654b9177", "toSpaceId": null, "position": "a03SY", "toEntityName": "Collection data source" },
            "typeName": "Data source type" },
          { "relationId": "ca192dcbc63542a594c7fe3d813b0843", "typeId": "a99f9ce12ffa4dac8c61f6310d46064a",
            "spaceId": "41e851610e13a19441c4d980f2f2ce6b", "changeType": "ADD",
            "after": { "toEntityId": "7cf1cbe8097c4a0f94b52b4716b65433", "toSpaceId": null, "position": "a034i", "toEntityName": "ChatGPT 5.4 BrowseComp" },
            "typeName": "Collection item" },
          { "relationId": "aaaee4da1d3f4f94a491d5b4e64239ef", "typeId": "a99f9ce12ffa4dac8c61f6310d46064a",
            "spaceId": "41e851610e13a19441c4d980f2f2ce6b", "changeType": "ADD",
            "after": { "toEntityId": "bcb77a795e8f4861aff7e7d9b3e6a697", "toSpaceId": null, "position": "a0AIv", "toEntityName": "GDPval (wins or ties)" },
            "typeName": "Collection item" },

          /* ── Sophia fix: config from the separate "blocks relation entity", folded in ── */
          { "relationId": "<view-config-relation>", "typeId": "1907fd1c80d4...VIEW_PROPERTY",
            "spaceId": "41e851610e13a19441c4d980f2f2ce6b", "changeType": "ADD",
            "after": { "toEntityId": "<gallery-view>", "toSpaceId": null, "position": "v0", "toEntityName": "Gallery" },
            "typeName": "View" },
          { "relationId": "<columns-config-relation>", "typeId": "4221fb36...SHOWN_COLUMNS",
            "spaceId": "41e851610e13a19441c4d980f2f2ce6b", "changeType": "ADD",
            "after": { "toEntityId": "<name-column>", "toSpaceId": null, "position": "c0", "toEntityName": "Name" },
            "typeName": "Shown columns" }
        ]
      }
    ]
  }
]
```

Notes:
- Block `Types → Data block` relation is stripped (redundant with `type: "dataBlock"`).
- The view/columns/sort relations live on the BLOCKS **relation entity** (`relation_id` of the
  Page→Block BLOCKS relation); v2 discovers it (`getBlockConfigEntityIds`) and folds its config
  relations/values into the block (`enrichBlockConfig`).
- Maps to asks **#1** + **#6**. Block folding across *which* parent is the proposal-only piece.

## Example 2 — media-property filtering + imageUrl inlining (proposal diff)

`GET /versioned/proposals/c99929131f6b4a29887d652bb406d598/diff?spaceId=89bd89bf28ff8a0963faf92a8c905e20&limit=50`

Proposal "Add cover and avatar to: Truth Social".
**BEFORE:** 3 top-level entities — two anonymous IMAGE_TYPE entities + the parent "Truth Social".

**AFTER (v2 target):** the two IMAGE_TYPE entities are dropped from the top level and their IPFS URL
is inlined onto the parent's Avatar/Cover relations:

```json
[
  {
    "entityId": "b2121f1f2239437d901bb9896f6bf49d", "name": "Truth Social", "values": [],
    "relations": [
      { "relationId": "0f500d41c71a49c9bea591428001abb1", "typeId": "1155befffad549b7a2e0da4777b8792c",
        "spaceId": "89bd89bf28ff8a0963faf92a8c905e20", "changeType": "ADD",
        "after": { "toEntityId": "8b3667cf78ab429d8e2b5f8a1da909b3", "toSpaceId": null, "position": "a07pW",
                   "imageUrl": "ipfs://bafkreifpxe…", "toEntityName": null }, "typeName": "Avatar" },
      { "relationId": "db0770ab18a1414ca30bd8cc90c0659d", "typeId": "34f535072e6b42c5a84443981a77cfa2",
        "spaceId": "89bd89bf28ff8a0963faf92a8c905e20", "changeType": "ADD",
        "after": { "toEntityId": "ab91813636ab46fc903807691d02846c", "toSpaceId": null, "position": "a073L",
                   "imageUrl": "ipfs://bafkreiet5a…", "toEntityName": null }, "typeName": "Cover" }
    ],
    "blocks": []
  }
]
```

Notes: field name is `imageUrl` / `videoUrl` (mutually exclusive per side, decided by the target's
type). Image width/height values are dropped (UI consumes only the URL). Maps to **#2** + **#4**.
The URL inlining (#4) is done on the entity-diff endpoint; the top-level filtering (#2) is proposal-only.

## Example 2b — cover SWAP (Sophia fix: before.imageUrl + after.imageUrl)

A page swaps its cover from image A → image B (same `Cover` relation, UPDATE). v2 resolves each side
at its own version (before @ `from`, after @ `to`), so both URLs are present:

```json
{
  "relationId": "…", "typeId": "34f535072e6b42c5a84443981a77cfa2", "spaceId": "…",
  "changeType": "UPDATE",
  "before": { "toEntityId": "<imageA>", "imageUrl": "ipfs://<A>", "position": "a", "toEntityName": null },
  "after":  { "toEntityId": "<imageB>", "imageUrl": "ipfs://<B>", "position": "a", "toEntityName": null },
  "typeName": "Cover"
}
```
Removal (cover removed) → `changeType: "REMOVE"`, `before.imageUrl` set, `after: null`.
Covered by `enrichWithMediaUrls` (versioned before/after) — see the swap test in `v2-entity-diff.test.ts`.

## Example 3 — single-entity history, UPDATE block

`GET /versioned/entities/8ad783f5b7af4fdb886693e92573ca11/diff?spaceId=41e851610e13a19441c4d980f2f2ce6b&fromEditId=fa64cbe3f26243979e14f33c028d9fa8&toEditId=02df1662b64248bdad7c19d617171fe9`

Adjacent ChatGPT edits rename a data block "Growth" → "Adoption".
**BEFORE:** v1 entity-history skips enrichment — no `propertyName`/`typeName`/`toEntityName`.
**AFTER (v2 target):** same structure, plus resolved names on relations and `imageUrl` inlined on any
IMAGE_TYPE/VIDEO_TYPE relation targets. The block diff is already emitted inline by the grouped
snapshot, so no folding is needed here. The entity-history surface needs far less v2 work than proposal-diff.

## Frontend questions (from the example doc) — answers as implemented

1. `block.values` + `block.relations` use the nested `ValueChange`/`RelationChange` shape (not flattened).
2. Field name is `imageUrl` / `videoUrl` on `relation.before/after` (not `mediaUrl` + `mediaType`).
3. Only the URL is inlined; image width/height are dropped.
4. v2 emits `typeName`/`toEntityName`/`propertyName` directly (the frontend mappers can pass them through
   instead of re-fetching).
