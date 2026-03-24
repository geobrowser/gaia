# RFC: Relation Data Block Schema Spec

**Status:** Draft  
**Version:** 1.0

## Summary
Defines the persisted JSON schema for relation data blocks. A relation block is identified by the presence of `sourceEntityId` and stores a filter using the RFC 0004 filter grammar.

This RFC is schema-only. It does not define execution semantics, API response shapes, pagination, authorization behavior, or UI rendering.

## Motivation
Relation data blocks need a deterministic, shared schema so clients can store and interpret relation-oriented filter configs consistently.

## Goals
- Define a canonical persisted JSON shape for relation data blocks.
- Reuse RFC 0004 operators and filter grammar.
- Keep relation block anchoring explicit and unambiguous.
- Provide concrete examples for common and recursive relation filtering patterns.

## Non-goals
- Define query execution behavior.
- Define result payload contracts.
- Define pagination, ordering, or auth semantics.
- Introduce a new operator set.

## 1. Spec

### 1.1 Block discriminator
A data block is a relation block when `sourceEntityId` exists at the top level of the stored JSON payload.

### 1.2 Top-level shape
Relation block payloads are stringified JSON objects with this shape:

```json
{
  "version": 1,
  "spaceId": { "in": ["space-a-uuid"] },
  "sourceEntityId": "source-entity-uuid",
  "properties": {
    "RELATION_TYPE_UUID": "TEXT",
    "NAME_UUID": "TEXT"
  },
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" }
      }
    }
  }
}
```

- `sourceEntityId` (required): canonical source anchor for relation block interpretation.
- `filter` (required): filter expression using RFC 0004 grammar.
- `version` (optional): defaults to `1` when omitted.
- `spaceId` (optional): same semantics as RFC 0004.
- `properties` (optional): same semantics as RFC 0004.
- Unknown top-level keys: ignored by clients.

### 1.3 Grammar and operators
Relation block filters inherit RFC 0004 filter grammar and operator semantics unchanged:
- property UUID predicates
- logical operators `OR` and `NOT`
- `_relation` scopes (`entity`, `fromEntity`, `toEntity`)
- data type selection via `properties`

For relation blocks, canonical examples in this RFC scope predicates through `_relation` so relation-vs-node intent stays explicit.

RFC 0004 examples place `_relation` under relation-typed property predicates. Relation blocks in this RFC also allow `_relation` directly under the root `filter` object, interpreted as the relation scope anchored by `sourceEntityId`.

### 1.4 Anchor conflict rule
If `sourceEntityId` exists and `_relation.fromEntity` also imposes source constraints, `sourceEntityId` is canonical.

Clients must ignore conflicting `_relation.fromEntity` constraints for relation-block anchoring.

### 1.5 Recursive relation filtering
Recursive `_relation` filtering is valid in relation block schema. This RFC defines only schema validity, not required traversal depth during execution.

### 1.6 JSON key uniqueness and relation composition
Because filter payloads are JSON objects, duplicate keys are not deterministic across parsers. Authors must not rely on duplicate top-level `_relation` keys.

To express multiple relation constraints with AND semantics, compose them in a single `_relation` object.

Example (canonical AND composition):

```json
{
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" }
      },
      "toEntity": {
        "ID_UUID": { "is": "company-acme-uuid" }
      }
    }
  }
}
```

### 1.7 Conformance examples

#### Example A: Anchor + simple relation type filtering

```json
{
  "version": 1,
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" }
      }
    }
  }
}
```

#### Example B: Anchor + specific `toEntity` id

```json
{
  "version": 1,
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "toEntity": {
        "ID_UUID": { "is": "company-acme-uuid" }
      }
    }
  }
}
```

#### Example C: Combined relation + TO predicates (AND)

```json
{
  "version": 1,
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" },
        "STATUS_UUID": { "is": "Active" }
      },
      "toEntity": {
        "NAME_UUID": { "includesInsensitive": "labs" }
      }
    }
  }
}
```

#### Example D: Top-level OR across relation scopes

```json
{
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "OR": [
      {
        "_relation": {
          "entity": {
            "RELATION_TYPE_UUID": { "is": "Works at" }
          }
        }
      },
      {
        "_relation": {
          "entity": {
            "RELATION_TYPE_UUID": { "is": "Advises" }
          }
        }
      }
    ]
  }
}
```

#### Example E: NOT wrapping relation predicates

```json
{
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "NOT": {
      "_relation": {
        "entity": {
          "STATUS_UUID": { "is": "Archived" }
        }
      }
    }
  }
}
```

#### Example F: Relation + TO with `in` operators

```json
{
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": {
          "in": ["Works at", "Advises"]
        }
      },
      "toEntity": {
        "ID_UUID": {
          "in": ["company-acme-uuid", "company-orion-uuid"]
        }
      }
    }
  }
}
```

#### Example G: Recursive relation filtering (nested relation entity)

This example expresses: relations of type `Works at` where the relation entity has a nested relation of type `Role` whose `toEntity` name is `Founder`.

```json
{
  "version": 1,
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" },
        "COMPANY_RELATIONS_UUID": {
          "_relation": {
            "entity": {
              "RELATION_TYPE_UUID": { "is": "Role" },
              "ROLE_RELATIONS_UUID": {
                "_relation": {
                  "toEntity": {
                    "NAME_UUID": { "is": "Founder" }
                  }
                }
              }
            }
          }
        }
      },
      "toEntity": {
        "IS_ACTIVE_UUID": { "is": true }
      }
    }
  }
}
```

#### Example H: Recursive filtering with nested OR on nested `toEntity`

```json
{
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" },
        "ROLE_RELATIONS_UUID": {
          "_relation": {
            "toEntity": {
              "OR": [
                { "NAME_UUID": { "is": "Founder" } },
                { "NAME_UUID": { "is": "CEO" } }
              ]
            }
          }
        }
      }
    }
  }
}
```

#### Example I: With `properties` type disambiguation

```json
{
  "sourceEntityId": "person-jane-uuid",
  "properties": {
    "START_DATE_UUID": "DATE",
    "NAME_UUID": "TEXT"
  },
  "filter": {
    "_relation": {
      "entity": {
        "START_DATE_UUID": { "greaterThan": "2025-01-01" }
      },
      "toEntity": {
        "NAME_UUID": { "startsWithInsensitive": "acme" }
      }
    }
  }
}
```

#### Example J: With `spaceId` and unknown extension key

```json
{
  "sourceEntityId": "person-jane-uuid",
  "spaceId": {
    "in": ["space-a-uuid", "space-b-uuid"]
  },
  "x-client-hint": "ignored-by-clients",
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" }
      }
    }
  }
}
```

#### Example K: Conflicting `_relation.fromEntity` is ignored for anchoring

```json
{
  "version": 1,
  "sourceEntityId": "person-jane-uuid",
  "filter": {
    "_relation": {
      "fromEntity": {
        "ID_UUID": { "is": "person-john-uuid" }
      },
      "toEntity": {
        "NAME_UUID": { "is": "Acme Inc" }
      }
    }
  }
}
```

In Example K, clients treat `sourceEntityId = person-jane-uuid` as canonical relation-block anchor.

#### Example L: Non-conformant relation block (missing `sourceEntityId`)

```json
{
  "version": 1,
  "filter": {
    "_relation": {
      "entity": {
        "RELATION_TYPE_UUID": { "is": "Works at" }
      }
    }
  }
}
```

Example L is not a conformant relation block payload because `sourceEntityId` is required.

### 1.8 Versioning
- If `version` is omitted, interpret as version `1`.
- Future schema changes must increment `version` and document migration/compatibility behavior.

## 2. Relationship to RFC 0004
This RFC is a relation-block schema profile that reuses RFC 0004 filter grammar and operators.

When this RFC and RFC 0004 overlap, this RFC only adds relation-block-specific constraints:
- relation-block discriminator (`sourceEntityId` exists)
- required top-level `sourceEntityId`
- anchor conflict resolution rule (`sourceEntityId` canonical)

All other filter language behavior remains defined by RFC 0004.
