# Values OrderBy Score

**Status:** Implemented

## Problem

The `values` connection in GraphQL supports filtering (via `ConnectionFilterPlugin`) but has no `orderBy` support. Consumers need to sort values by their associated `local_scores.score` — for example, to display entities ranked by relevance within a space.

## Current State

PostGraphile auto-generates the `values` GraphQL connection from the `public.values` table. The `local_scores` table stores per-entity-per-space scores but lives in a separate table with no direct FK from `values`:

```
values (entity_id, space_id) → local_scores (entity_id, space_id)
```

Both tables share the `(entity_id, space_id)` composite key, making correlation straightforward.

## Approach: `makeAddPgTableOrderByPlugin`

`graphile-utils` (already installed) exports `makeAddPgTableOrderByPlugin` — a factory specifically for adding custom `orderBy` enum values to PostGraphile connections. This is the idiomatic way to add sorting in PostGraphile 4.x.

### Why not `addArgDataGenerator`?

The `EntitySpaceFilterPlugin` uses `addArgDataGenerator` with `queryBuilder.where()` for custom filtering. While you *could* use `queryBuilder.orderBy()` the same way, `makeAddPgTableOrderByPlugin` handles:

- Enum type generation (the `ValuesOrderBy` GraphQL enum)
- Integration with cursor-based pagination (cursors encode sort position)
- Nulls ordering
- ASC/DESC pairing via `orderByAscDesc` helper

Rolling this by hand with `addArgDataGenerator` would require reimplementing all of the above.

### Why correlated subqueries instead of LEFT JOIN?

The initial draft proposed using `queryBuilder.leftJoin()`. However, `leftJoin` is **not a public API** on PostGraphile 4's `QueryBuilder` — the internal `join` array has no public push method. The implementation uses correlated subqueries as the ORDER BY expression instead.

This is acceptable because:
- The subqueries hit primary key indexes (O(1) per row)
- PostGraphile's pagination cap limits result sets to 1000 rows max
- The subquery only executes when the orderBy enum is actually selected

## Implementation

### Plugin File

```typescript
// api/src/kg/valueOrderByScorePlugin.ts
import {makeAddPgTableOrderByPlugin, orderByAscDesc} from "graphile-utils"

export const ValueOrderByScorePlugin = makeAddPgTableOrderByPlugin(
  "public",
  "values",
  (build) => {
    const {pgSql: sql} = build

    const localScore = orderByAscDesc(
      "LOCAL_SCORE",
      ({queryBuilder}) => {
        const t = queryBuilder.getTableAlias()
        return sql.fragment`(
          SELECT ls.score FROM public.local_scores ls
          WHERE ls.entity_id = ${t}.entity_id
            AND ls.space_id = ${t}.space_id
        )`
      },
      {unique: false, nulls: "last"},
    )

    const globalScore = orderByAscDesc(
      "GLOBAL_SCORE",
      ({queryBuilder}) => {
        const t = queryBuilder.getTableAlias()
        return sql.fragment`(
          SELECT gs.score FROM public.global_scores gs
          WHERE gs.entity_id = ${t}.entity_id
        )`
      },
      {unique: false, nulls: "last"},
    )

    return {...localScore, ...globalScore}
  },
  "Adding orderBy local_scores.score and global_scores.score to values connection",
)

export default ValueOrderByScorePlugin
```

Both `LOCAL_SCORE` and `GLOBAL_SCORE` live in the same plugin by spreading multiple `orderByAscDesc` results.

### Plugin Registration

In `api/src/kg/postgraphile.ts`, added to `appendPlugins` after `EntitySpaceFilterPlugin`:

```typescript
import ValueOrderByScorePlugin from "./valueOrderByScorePlugin"

appendPlugins: [
  UndashedUuidPlugin,
  ValueScalarsPlugin,
  ConnectionFilterPlugin,
  SimplifyInflectionPlugin,
  EntitySpaceFilterPlugin,
  ValueOrderByScorePlugin,   // orderBy LOCAL_SCORE / GLOBAL_SCORE
  PaginationCapPlugin,
],
```

### GraphQL Usage

```graphql
# Top entities in a space by local score
query ValuesByScore {
  valuesConnection(
    first: 50
    orderBy: LOCAL_SCORE_DESC
    filter: { spaceId: { is: "space-uuid" } }
  ) {
    nodes {
      id
      entityId
      propertyId
      text
    }
    pageInfo {
      hasNextPage
      endCursor
    }
  }
}

# Global ranking across all spaces
query GlobalTopValues {
  valuesConnection(orderBy: GLOBAL_SCORE_DESC, first: 100) {
    nodes { id entityId text }
  }
}

# Multi-column sort
query ValuesByScoreThenProperty {
  valuesConnection(orderBy: [LOCAL_SCORE_DESC, PROPERTY_ID_ASC], first: 50) {
    nodes { id entityId propertyId text }
  }
}
```

## SQL Generated

The plugin produces SQL roughly equivalent to:

```sql
SELECT v.*
FROM public.values v
ORDER BY (
  SELECT ls.score FROM public.local_scores ls
  WHERE ls.entity_id = v.entity_id AND ls.space_id = v.space_id
) DESC NULLS LAST
LIMIT 51  -- first: 50 + 1 for hasNextPage
```

Entities without scores get `NULL` from the subquery and always sort to the end via `nulls: "last"`.

## Index Coverage

| Query pattern | Index used |
|---|---|
| Subquery: `local_scores WHERE entity_id AND space_id` | PK `(entity_id, space_id)` |
| Subquery: `global_scores WHERE entity_id` | PK `(entity_id)` |
| Filter by space + sort by score | `idx_local_scores_space_score (space_id, score DESC)` |

The composite index `idx_local_scores_space_score` was added to optimize the common pattern of filtering values by `spaceId` while ordering by local score.

## Risks & Considerations

1. **Correlated subquery cost**: Each result row triggers a subquery. Mitigated by PK index lookups (O(1)) and pagination cap (max 1000 rows). The subquery only runs when the orderBy enum is selected — no cost on other queries.

2. **Cursor pagination**: PostGraphile encodes the sort column value into the cursor. If scores change between page fetches, rows may shift. This is acceptable for score-based ranking (scores update infrequently via the indexer pipeline).

3. **Multiple orderBy values**: PostGraphile supports `orderBy: [LOCAL_SCORE_DESC, PROPERTY_ID_ASC]` — the SQL gets multiple `ORDER BY` columns. This works out of the box.

4. **Nulls ordering**: `nulls: "last"` ensures entities without scores always sort to the end for both ASC and DESC. The alternative `"last-iff-ascending"` was considered but rejected because it causes `DESC NULLS FIRST`, placing unscored entities before scored ones in descending order.
