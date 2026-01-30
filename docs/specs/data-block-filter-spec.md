# Data Block Filter Spec

## Summary
Defines the JSON filter language stored in data blocks for querying the Knowledge Graph API. Filters are stringified JSON and use property UUIDs as keys. The spec also defines how relation filters and logical operators work, and how data types are selected when property values are heterogeneous.

## Motivation
Data blocks need a stable, expressive filter format for dynamic queries. The Knowledge Graph is heterogeneous across spaces and does not enforce property data types at the protocol level. A clear filter spec prevents ambiguity and enables consistent client and server behavior.

## Goals
- Provide a concise, deterministic JSON filter format for data blocks.
- Support logical grouping (AND, OR, NOT) and recursive relation filters.
- Align operators with the canonical operator set per data type.
- Support explicit data type specification in filters when needed to avoid ambiguity.

## Non-goals
- Enforce or validate property data types at the protocol level.
- Define query execution semantics beyond filtering (ordering, pagination).
- Define client SDK APIs or validation error payloads.

## 1. Spec

### 1.1 Top-level shape
Data blocks store a stringified JSON object with the following shape:

```json
{
  "spaceId": { "in": ["space-a-uuid", "space-b-uuid"] },
  "filter": { ... },
  "types": { "PROPERTY_UUID": "TEXT" }
}
```

- `spaceId` (optional): scopes results to one or more spaces.
- `filter` (required): the filter expression.
- `types` (optional): per-property data type overrides using GRC-20 DataType enum values (see Section 5).

### 1.2 Property predicates
Each predicate is keyed by a property UUID and contains an operator object.

```json
{
  "filter": {
    "NAME_UUID": { "is": "John" }
  }
}
```

### 1.3 Logical operators
At any object level, logical operators can be combined with property predicates.

- Default: sibling predicates are AND-ed.
- `OR`: array of filter objects.
- `NOT`: a single filter object.

```json
{
  "filter": {
    "AGE_UUID": { "greaterThan": 42 },
    "OR": [
      { "NOT": { "NAME_UUID": { "is": "Jane Doe" } } },
      { "NOT": { "NAME_UUID": { "is": "John Doe" } } }
    ]
  }
}
```

### 1.4 Relation filtering (recursive)
Relation-typed properties can be filtered via `_relation`, which scopes filters to:
- `entity` (the relation entity itself)
- `fromEntity` (the source entity)
- `toEntity` (the target entity)

Each of these is a full filter object (UUID keys + OR/NOT). `_relation` does not accept property UUID predicates directly.

```json
{
  "filter": {
    "ASSIGNEES_ID": {
      "_relation": {
        "entity": {
          "ASSIGNED_AT_ID": { "greaterThan": "2025-01-01T00:00:00.000Z" }
        },
        "fromEntity": {
          "NAME_ID": { "is": "Jane" }
        },
        "toEntity": {
          "NAME_ID": { "is": "John" }
        }
      }
    }
  }
}
```

Recursion is allowed at any depth.

### 1.5 Data type selection
Property data types are mutable and can vary across spaces; the protocol does not enforce correctness. Filters therefore support two modes:

1) **Implicit type selection** (default)
   - The caller or API selects a data type to interpret the filter value.
   - If multiple data types exist for a property, the API may choose a default or require an explicit type.

2) **Explicit type selection**
   - Callers may specify a data type override per property via `types` using GRC-20 DataType enum values.
   - Allowed values: `BOOL | INT64 | FLOAT64 | DECIMAL | TEXT | BYTES | DATE | TIME | DATETIME | SCHEDULE | POINT | RECT | EMBEDDING`.
   - The specified type controls which operator set is valid and how values are parsed.

```json
{
  "types": {
    "NAME_UUID": "TEXT",
    "PRICE_UUID": "DECIMAL",
    "START_DATE_UUID": "DATE",
    "ACTIVE_UUID": "BOOL",
    "CREATED_AT_UUID": "DATETIME",
    "RANK_UUID": "INT64"
  },
  "filter": {
    "NAME_UUID": { "is": "Mercury" },
    "PRICE_UUID": { "greaterThan": "12.50" },
    "START_DATE_UUID": { "greaterThan": "2025-01-01" },
    "ACTIVE_UUID": { "is": true },
    "CREATED_AT_UUID": { "greaterThan": "2025-01-01T00:00:00.000Z" },
    "RANK_UUID": { "greaterThanOrEqualTo": 3 }
  }
}
```

### 1.6 Value representations
- `DATE`, `TIME`, and `DATETIME` are filtered as ISO-8601 strings.
- `BYTES` and `POINT` are filtered as strings (encoding specified by the API).
- `SCHEDULE` and `EMBEDDING` use JSON-based operators.
- `RECT` is supported as a data type (encoding specified by the API).

### 1.7 Validation rules
- `filter` must be an object.
- Keys are either property UUIDs, or logical operators (`OR`, `NOT`), or `_relation`.
- `OR` must be an array of objects.
- `NOT` must be an object.
- `_relation` only allows `entity`, `fromEntity`, `toEntity`.
- Operator must be valid for the selected data type.
- Explicit negated operators are not supported; use `NOT` instead.

## 2. Appendix

### 2.1 Operators by data type

Operator sets are defined per data type. The spec allows only non-negated operators; all negation is expressed via the `NOT` logical operator.

#### BOOL
- `isNull`, `is`, `distinctFrom`, `in`, `lessThan`, `lessThanOrEqualTo`, `greaterThan`, `greaterThanOrEqualTo`

#### INT64
- `isNull`, `is`, `distinctFrom`, `in`, `lessThan`, `lessThanOrEqualTo`, `greaterThan`, `greaterThanOrEqualTo`

#### FLOAT64
- `isNull`, `is`, `distinctFrom`, `in`, `lessThan`, `lessThanOrEqualTo`, `greaterThan`, `greaterThanOrEqualTo`

#### DECIMAL
- `isNull`, `is`, `distinctFrom`, `in`, `lessThan`, `lessThanOrEqualTo`, `greaterThan`, `greaterThanOrEqualTo`

#### TEXT
- `isNull`, `is`, `distinctFrom`, `in`
- `lessThan`, `lessThanOrEqualTo`, `greaterThan`, `greaterThanOrEqualTo`
- `includes`, `includesInsensitive`
- `startsWith`, `startsWithInsensitive`
- `endsWith`, `endsWithInsensitive`
- `like`, `likeInsensitive`
- `isInsensitive`, `distinctFromInsensitive`
- `inInsensitive`, `lessThanInsensitive`, `lessThanOrEqualToInsensitive`
- `greaterThanInsensitive`, `greaterThanOrEqualToInsensitive`

#### BYTES
- same as TEXT

#### DATE
- same as TEXT

#### TIME
- same as TEXT

#### DATETIME
- same as TEXT

#### POINT
- same as TEXT

#### RECT
- same as TEXT

#### SCHEDULE
- `isNull`, `is`, `distinctFrom`, `in`
- `lessThan`, `lessThanOrEqualTo`, `greaterThan`, `greaterThanOrEqualTo`
- `containsKey`, `containsAllKeys`, `containsAnyKeys`, `containedBy`

#### EMBEDDING
- same as SCHEDULE
