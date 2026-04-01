# ADR-002: Values OrderBy Score in PostGraphile API

## Status

Proposed

## Date

2026-04-01

## Context

Users need to sort the `values` GraphQL connection by entity scores. The current PostGraphile API exposes filtering on `values` (via `ConnectionFilterPlugin`) but no `orderBy` support for scores.

Score data exists in separate tables:
- `local_scores(entity_id, space_id, score)` — per-entity-per-space score
- `global_scores(entity_id, score)` — per-entity global score

Both tables are populated nightly by the scoring service via Kafka. The `values` table shares `(entity_id, space_id)` with `local_scores` and `entity_id` with `global_scores`, making correlation straightforward.

### Current Infrastructure

- **PostGraphile**: 4.14.1 with graphql-yoga
- **graphile-utils**: 4.14.1 (transitive dep, already available)
- **Existing plugins**: UndashedUuidPlugin, ValueScalarsPlugin, ConnectionFilterPlugin, SimplifyInflectionPlugin, EntitySpaceFilterPlugin, PaginationCapPlugin

### Key Constraint: No `queryBuilder.leftJoin()`

The draft research doc (`docs/research/values-order-by-score.md`) proposed using `queryBuilder.leftJoin()`. However, PostGraphile 4's `QueryBuilder` does **not** expose `leftJoin` as a public method — the internal `join` array has no public push API. The implementation must use **correlated subqueries** as the ORDER BY expression instead.

This is acceptable because:
- The subqueries hit primary key indexes (O(1) per row)
- PostGraphile's pagination cap limits result sets to 1000 rows max
- The subquery only executes when the orderBy enum is actually selected

## Decision

Use `makeAddPgTableOrderByPlugin` + `orderByAscDesc` from `graphile-utils` to add four new `ValuesOrderBy` enum values with correlated subquery sort expressions.

### 1. Plugin Implementation

**File:** `api/src/kg/valueOrderByScorePlugin.ts`

```typescript
import { makeAddPgTableOrderByPlugin, orderByAscDesc } from "graphile-utils"

export const ValueOrderByScorePlugin = makeAddPgTableOrderByPlugin(
  "public",
  "values",
  (build) => {
    const { pgSql: sql } = build

    const localScore = orderByAscDesc(
      "LOCAL_SCORE",
      ({ queryBuilder }) => {
        const t = queryBuilder.getTableAlias()
        return sql.fragment`(
          SELECT ls.score FROM public.local_scores ls
          WHERE ls.entity_id = ${t}.entity_id
            AND ls.space_id = ${t}.space_id
        )`
      },
      { unique: false, nulls: "last-iff-ascending" },
    )

    const globalScore = orderByAscDesc(
      "GLOBAL_SCORE",
      ({ queryBuilder }) => {
        const t = queryBuilder.getTableAlias()
        return sql.fragment`(
          SELECT gs.score FROM public.global_scores gs
          WHERE gs.entity_id = ${t}.entity_id
        )`
      },
      { unique: false, nulls: "last-iff-ascending" },
    )

    return { ...localScore, ...globalScore }
  },
  "Adding orderBy local_scores.score and global_scores.score to values connection",
)

export default ValueOrderByScorePlugin
```

**SQL generated** (when `orderBy: LOCAL_SCORE_DESC` is used):

```sql
SELECT v.*
FROM public.values v
ORDER BY (
  SELECT ls.score FROM public.local_scores ls
  WHERE ls.entity_id = v.entity_id AND ls.space_id = v.space_id
) DESC NULLS FIRST
LIMIT 51
```

Entities without scores get `NULL` from the subquery and sort to the end via `nulls: "last-iff-ascending"`.

### 2. Plugin Registration

**File:** `api/src/kg/postgraphile.ts`

Insert after `EntitySpaceFilterPlugin`, before `PaginationCapPlugin`:

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

### 3. Performance Index

**Migration file:** `api/drizzle/0052_values_order_by_score.sql`

```sql
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_local_scores_space_score
  ON local_scores (space_id, score DESC);
```

Also update `api/src/services/storage/schema.ts` to document the index:

```typescript
idxSpaceScore: index("idx_local_scores_space_score").on(table.spaceId, table.score),
```

**Rationale:** The existing PK indexes cover the subquery lookups. This composite index optimizes the common pattern of filtering values by `spaceId` (via ConnectionFilterPlugin) while ordering by local score — PostgreSQL can use it to avoid a sort step.

### 4. GraphQL Usage

```graphql
# Top entities in a space by local score
query TopScoredValues($spaceId: UUID!) {
  values(
    filter: { spaceId: { is: $spaceId } }
    orderBy: LOCAL_SCORE_DESC
    first: 50
  ) {
    nodes { id entityId spaceId text }
    pageInfo { hasNextPage endCursor }
  }
}

# Global ranking across all spaces
query GlobalTopValues {
  values(orderBy: GLOBAL_SCORE_DESC, first: 100) {
    nodes { id entityId text }
    pageInfo { hasNextPage endCursor }
  }
}

# Multi-column sort
query ValuesByScoreThenProperty {
  values(orderBy: [LOCAL_SCORE_DESC, PROPERTY_ID_ASC], first: 50) {
    nodes { id entityId propertyId text }
  }
}
```

### 5. Tests

**File:** `api/src/kg/__tests__/valueOrderByScorePlugin.test.ts`

Following the pattern from `paginationCapPlugin.test.ts` (uses `graphqlServer.fetch()`):

| Test | What it validates |
|------|------------------|
| Schema introspection | `ValuesOrderBy` enum includes `LOCAL_SCORE_ASC`, `LOCAL_SCORE_DESC`, `GLOBAL_SCORE_ASC`, `GLOBAL_SCORE_DESC` |
| LOCAL_SCORE_DESC query | No GraphQL errors, results returned |
| GLOBAL_SCORE_DESC query | No GraphQL errors, results returned |
| Pagination with score ordering | Cursor-based pagination works (first page + next page via endCursor) |
| Combined with spaceId filter | `filter` + `orderBy` compose correctly |

### 6. Documentation Update

Update `docs/research/values-order-by-score.md`:
- Status: Draft -> Implemented
- Replace LEFT JOIN approach with correlated subquery approach
- Document why `queryBuilder.leftJoin()` is not available

## Index Coverage

| Query pattern | Index used |
|---|---|
| Subquery: `local_scores WHERE entity_id AND space_id` | PK `(entity_id, space_id)` |
| Subquery: `global_scores WHERE entity_id` | PK `(entity_id)` |
| Filter by space + sort by score | `idx_local_scores_space_score (space_id, score DESC)` |

## Risks & Mitigations

1. **Correlated subquery cost**: Each result row triggers a subquery. Mitigated by PK index lookups (O(1)) and pagination cap (max 1000 rows). Monitor with `EXPLAIN ANALYZE`.

2. **Cursor stability**: Scores change between page fetches (nightly scoring pipeline). Rows may shift between pages. Acceptable for ranking use cases — same behavior as any mutable sort column.

3. **`orderByAscDesc` callback form**: If the callback `({ queryBuilder }) => sql.fragment` form doesn't work as expected, fallback to a manual plugin using `builder.hook("GraphQLEnumType:values", ...)` directly.

4. **Enum naming**: SimplifyInflectionPlugin may transform names. Verify via introspection test. SCREAMING_SNAKE_CASE should pass through unchanged.

## Consequences

- The `values` connection gains four new `orderBy` enum values
- No impact on existing queries — the subquery only runs when the enum is selected
- Cursor-based pagination works with score ordering out of the box
- Future score types can be added by spreading additional `orderByAscDesc` calls in the same plugin
