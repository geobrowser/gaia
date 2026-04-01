# Values OrderBy Score

**Status:** Draft

## Problem

The `values` connection in GraphQL supports filtering (via `ConnectionFilterPlugin`) but has no `orderBy` support. Consumers need to sort values by their associated `local_scores.score` — for example, to display entities ranked by relevance within a space.

## Current State

PostGraphile auto-generates the `values` GraphQL connection from the `public.values` table. The `local_scores` table stores per-entity-per-space scores but lives in a separate table with no direct FK from `values`:

```
values (entity_id, space_id) → local_scores (entity_id, space_id)
```

Both tables share the `(entity_id, space_id)` composite key, making a direct JOIN possible.

## Approach: `makeAddPgTableOrderByPlugin`

`graphile-utils` (already installed) exports `makeAddPgTableOrderByPlugin` — a factory specifically for adding custom `orderBy` enum values to PostGraphile connections. This is the idiomatic way to add sorting in PostGraphile 4.x.

### Why not `addArgDataGenerator`?

The `EntitySpaceFilterPlugin` uses `addArgDataGenerator` with `queryBuilder.where()` for custom filtering. While you *could* use `queryBuilder.orderBy()` the same way, `makeAddPgTableOrderByPlugin` handles:

- Enum type generation (the `ValuesOrderBy` GraphQL enum)
- Integration with cursor-based pagination (cursors encode sort position)
- Nulls ordering
- ASC/DESC pairing via `orderByAscDesc` helper

Rolling this by hand with `addArgDataGenerator` would require reimplementing all of the above.

## Implementation

### 1. New Plugin File

```typescript
// api/src/kg/valueOrderByScorePlugin.ts
import {makeAddPgTableOrderByPlugin, orderByAscDesc} from "graphile-utils"

/**
 * Adds orderBy support to the values connection for sorting by local_scores.score.
 *
 * Joins local_scores on (entity_id, space_id) and orders by score.
 * Uses a LEFT JOIN so values without scores sort to the end (nulls last when ascending).
 *
 * Usage:
 *   values(orderBy: [LOCAL_SCORE_ASC], first: 100) { ... }
 *   values(orderBy: [LOCAL_SCORE_DESC], first: 100) { ... }
 */
export const ValueOrderByScorePlugin = makeAddPgTableOrderByPlugin(
  "public",
  "values",
  (build) => {
    const {pgSql: sql} = build

    return orderByAscDesc(
      "LOCAL_SCORE",
      ({queryBuilder}) => {
        const tableAlias = queryBuilder.getTableAlias()
        // Unique alias to avoid conflicts if multiple orderBy expressions join
        const scoreAlias = sql.identifier(Symbol("local_scores"))

        queryBuilder.leftJoin(
          sql.fragment`public.local_scores`,
          scoreAlias,
          sql.fragment`${scoreAlias}.entity_id = ${tableAlias}.entity_id
            AND ${scoreAlias}.space_id = ${tableAlias}.space_id`,
        )

        return sql.fragment`${scoreAlias}.score`
      },
      {unique: false, nulls: "last-iff-ascending"},
    )
  },
  "Adding orderBy local_scores.score to values connection",
)

export default ValueOrderByScorePlugin
```

### 2. Register the Plugin

In `api/src/kg/postgraphile.ts`, add to `appendPlugins` after `EntitySpaceFilterPlugin`:

```typescript
import ValueOrderByScorePlugin from "./valueOrderByScorePlugin"

appendPlugins: [
  UndashedUuidPlugin,
  ValueScalarsPlugin,
  ConnectionFilterPlugin,
  SimplifyInflectionPlugin,
  EntitySpaceFilterPlugin,
  ValueOrderByScorePlugin,   // NEW
  PaginationCapPlugin,
],
```

### 3. GraphQL Usage

```graphql
query ValuesByScore {
  values(
    first: 50
    orderBy: [LOCAL_SCORE_DESC]
    filter: { spaceId: { is: "space-uuid" } }
  ) {
    edges {
      node {
        id
        entityId
        propertyId
        text
      }
    }
    pageInfo {
      hasNextPage
      endCursor
    }
  }
}
```

## QueryBuilder API

`makeAddPgTableOrderByPlugin`'s `OrderBySpecIdentity` accepts a function `({queryBuilder}) => SQL`. The `queryBuilder` exposes:

| Method | Purpose |
|---|---|
| `getTableAlias()` | Current table alias in the generated SQL |
| `leftJoin(table, alias, condition)` | Add a LEFT JOIN clause |
| `where(fragment)` | Add a WHERE condition |
| `orderBy(fragment, ascending)` | Add ORDER BY (handled automatically by the plugin) |

The plugin handles `ORDER BY` itself — the callback only needs to return the SQL expression to sort on. The framework wraps it with `ASC`/`DESC` and nulls ordering.

## SQL Generated

The plugin will produce SQL roughly equivalent to:

```sql
SELECT v.*
FROM public.values v
LEFT JOIN public.local_scores ls
  ON ls.entity_id = v.entity_id
  AND ls.space_id = v.space_id
ORDER BY ls.score DESC NULLS LAST
LIMIT 51  -- first: 50 + 1 for hasNextPage
```

## Index Coverage

Existing indexes already cover this JOIN:

- `idx_local_scores_entity_id` on `local_scores(entity_id)`
- `idx_local_scores_space_id` on `local_scores(space_id)`
- `values_entity_space_idx` on `values(entity_id, space_id)`

For optimal performance on large result sets with `ORDER BY score`, consider adding a composite index:

```sql
CREATE INDEX idx_local_scores_space_score
  ON local_scores (space_id, score DESC);
```

This would help when filtering values by `spaceId` and ordering by score (the most common query pattern).

## Extension: Global Score OrderBy

The same pattern works for `global_scores` (one score per entity, no space dimension):

```typescript
...orderByAscDesc(
  "GLOBAL_SCORE",
  ({queryBuilder}) => {
    const tableAlias = queryBuilder.getTableAlias()
    const scoreAlias = sql.identifier(Symbol("global_scores"))

    queryBuilder.leftJoin(
      sql.fragment`public.global_scores`,
      scoreAlias,
      sql.fragment`${scoreAlias}.entity_id = ${tableAlias}.entity_id`,
    )

    return sql.fragment`${scoreAlias}.score`
  },
  {unique: false, nulls: "last-iff-ascending"},
)
```

Both can live in the same plugin by spreading multiple `orderByAscDesc` results:

```typescript
return {
  ...orderByAscDesc("LOCAL_SCORE", localScoreSql, opts),
  ...orderByAscDesc("GLOBAL_SCORE", globalScoreSql, opts),
}
```

## Risks & Considerations

1. **LEFT JOIN cost**: The JOIN runs on every values query that uses this orderBy. Without the orderBy argument, the JOIN is not added — `makeAddPgTableOrderByPlugin` only activates when the enum value is selected.

2. **Cursor pagination**: PostGraphile encodes the sort column value into the cursor. If scores change between page fetches, rows may shift. This is acceptable for score-based ranking (scores update infrequently via the indexer pipeline).

3. **Multiple orderBy values**: PostGraphile supports `orderBy: [LOCAL_SCORE_DESC, PROPERTY_ID_ASC]` — the SQL gets multiple `ORDER BY` columns. This works out of the box.

4. **`queryBuilder.leftJoin` availability**: This method exists on PostGraphile 4.x's `QueryBuilder`. Verified in the graphile-build-pg source. If it's not available at runtime, the fallback is a raw SQL subquery expression instead of a JOIN (less efficient but guaranteed to work):
   ```typescript
   // Fallback: correlated subquery instead of JOIN
   return sql.fragment`(
     SELECT ls.score FROM public.local_scores ls
     WHERE ls.entity_id = ${tableAlias}.entity_id
       AND ls.space_id = ${tableAlias}.space_id
   )`
   ```
