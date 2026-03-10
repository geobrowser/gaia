# Soft Delete Filtering Implementation Plan

**Status:** Draft
**Related:** `docs/archive/plans/grc20-v2-gaps.md` (Deferred Schema Changes section)

## Problem

The GRC-20 v2 spec defines soft deletes via `DeleteEntity`/`RestoreEntity` and `DeleteRelation`/`RestoreRelation` operations. Currently:

1. **Relations** are hard-deleted (`DELETE FROM relations`)
2. **Entities** are never deleted
3. **No `deleted_at` column** exists in the schema
4. **Tombstone dominance** is not enforced - updates to deleted entities should be ignored

Per the spec:
> Once DELETED, subsequent UpdateEntity ops for this entity are ignored.
> Once DELETED, subsequent CreateEntity ops for this entity are ignored (tombstone absorbs upserts).

## Design Constraints

- **No in-memory state**: Avoid maintaining tombstone sets in the indexer
- **No read-before-write**: Avoid explicit SELECT queries before writes
- **Database-level enforcement**: All filtering happens in SQL statements

## Solution: Two-Layer Tombstone Enforcement

### Summary

Tombstone dominance is enforced at two layers:

1. **Intra-edit (edit handler)**: Squash operations within an edit before writing. If an edit contains `DeleteEntity(A)` followed by `CreateEntity(A)`, the squash logic keeps only the delete. Values targeting deleted entities are filtered out.

2. **Cross-edit (database)**: SQL WHERE clauses reject writes to previously-deleted entities. `WHERE entities.deleted_at IS NULL` on entity upserts, and `NOT EXISTS` subqueries on value inserts.

This avoids in-memory tombstone sets and explicit read-before-write queries while correctly handling both intra-edit and cross-edit tombstone dominance.

### Implementation Details

Use SQL WHERE clauses and subqueries to atomically filter writes against deleted entities. The "read" happens within the write statement, not as a separate operation.

### 1. Schema Changes

Add `deleted_at` column to `entities` and `relations` tables:

```typescript
// api/src/services/storage/schema.ts

export const entities = pgTable(
  "entities",
  {
    id: uuid().primaryKey(),
    createdAt: text().notNull(),
    createdAtBlock: text().notNull(),
    updatedAt: text().notNull(),
    updatedAtBlock: text().notNull(),
    deletedAt: text(),  // null = active, ISO timestamp = deleted
  },
  (table) => [
    index("entities_updated_at_idx").on(table.updatedAt),
    index("entities_updated_at_id_idx").on(table.updatedAt, table.id),
    // Partial index for efficient tombstone checks
    index("entities_deleted_at_idx")
      .on(table.id)
      .where(sql`${table.deletedAt} IS NOT NULL`),
  ],
);

export const relations = pgTable(
  "relations",
  {
    // ... existing columns ...
    deletedAt: text(),  // null = active, ISO timestamp = deleted
  },
  (table) => [
    // ... existing indexes ...
    // Partial index for efficient tombstone checks
    index("relations_deleted_at_idx")
      .on(table.id)
      .where(sql`${table.deletedAt} IS NOT NULL`),
  ],
);
```

### 2. Indexer Storage Changes

#### 2.1 Entity Operations

```rust
// kg-indexer/src/storage.rs

/// Insert entities, respecting tombstone dominance.
/// - New entities: inserted normally
/// - Existing active entities: updated
/// - Existing deleted entities: ignored (tombstone absorbs)
pub async fn insert_entities(
    &self,
    entities: &[EntityItem],
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    if entities.is_empty() {
        return Ok(());
    }

    // ... prepare arrays ...

    sqlx::query!(
        r#"
        INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
        SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::text[], $5::text[])
        ON CONFLICT (id) DO UPDATE SET
            updated_at = EXCLUDED.updated_at,
            updated_at_block = EXCLUDED.updated_at_block
        WHERE entities.deleted_at IS NULL
        "#,
        &ids,
        &created_ats,
        &created_at_blocks,
        &updated_ats,
        &updated_at_blocks
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Soft-delete entities by setting deleted_at timestamp.
pub async fn delete_entities(
    &self,
    deletes: &[(Uuid, String)], // (entity_id, timestamp)
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    if deletes.is_empty() {
        return Ok(());
    }

    let ids: Vec<Uuid> = deletes.iter().map(|(id, _)| *id).collect();
    let timestamps: Vec<&str> = deletes.iter().map(|(_, ts)| ts.as_str()).collect();

    sqlx::query(
        r#"
        UPDATE entities SET deleted_at = input.deleted_at
        FROM (SELECT id, deleted_at FROM UNNEST($1::uuid[], $2::text[]) AS t(id, deleted_at)) AS input
        WHERE entities.id = input.id AND entities.deleted_at IS NULL
        "#,
    )
    .bind(&ids)
    .bind(&timestamps)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Restore soft-deleted entities by clearing deleted_at.
pub async fn restore_entities(
    &self,
    entity_ids: &[Uuid],
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    if entity_ids.is_empty() {
        return Ok(());
    }

    sqlx::query(
        r#"
        UPDATE entities SET deleted_at = NULL
        WHERE id = ANY($1::uuid[]) AND deleted_at IS NOT NULL
        "#,
    )
    .bind(entity_ids)
    .execute(&mut **tx)
    .await?;

    Ok(())
}
```

#### 2.2 Value Operations

```rust
/// Insert values, filtering out those targeting deleted entities.
pub async fn insert_values(
    &self,
    values: &[ValueOp],
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    if values.is_empty() {
        return Ok(());
    }

    // ... prepare arrays ...

    let query = r#"
        INSERT INTO values (
            id, entity_id, property_id, space_id, language, unit,
            text, decimal, boolean, time, point,
            integer, float, bytes, date, datetime, schedule, embedding
        )
        SELECT id, entity_id, property_id, space_id, language, unit,
               text, decimal, boolean, time, point,
               integer, float, bytes, date, datetime, schedule, embedding
        FROM UNNEST(
            $1::text[], $2::uuid[], $3::uuid[], $4::uuid[], $5::text[], $6::text[],
            $7::text[], $8::numeric[], $9::boolean[], $10::text[], $11::text[],
            $12::bigint[], $13::double precision[], $14::bytea[], $15::text[],
            $16::text[], $17::jsonb[], $18::jsonb[]
        ) AS input(id, entity_id, property_id, space_id, language, unit,
                   text, decimal, boolean, time, point,
                   integer, float, bytes, date, datetime, schedule, embedding)
        WHERE NOT EXISTS (
            SELECT 1 FROM entities e
            WHERE e.id = input.entity_id AND e.deleted_at IS NOT NULL
        )
        ON CONFLICT (id) DO UPDATE SET
            language = EXCLUDED.language,
            unit = EXCLUDED.unit,
            text = EXCLUDED.text,
            -- ... other columns ...
        WHERE NOT EXISTS (
            SELECT 1 FROM entities e
            WHERE e.id = values.entity_id AND e.deleted_at IS NOT NULL
        )
    "#;

    // ... execute query ...
}

/// Insert value versions, filtering out those targeting deleted entities.
pub async fn insert_value_versions(
    &self,
    values: &[ValueOp],
    version_key: i64,
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    // Same pattern: add WHERE NOT EXISTS check against entities.deleted_at
}
```

#### 2.3 Relation Operations

```rust
/// Soft-delete relations by setting deleted_at timestamp.
pub async fn delete_relations(
    &self,
    deletes: &[(Uuid, Uuid, String)], // (relation_id, space_id, timestamp)
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    if deletes.is_empty() {
        return Ok(());
    }

    let ids: Vec<Uuid> = deletes.iter().map(|(id, _, _)| *id).collect();
    let space_ids: Vec<Uuid> = deletes.iter().map(|(_, sid, _)| *sid).collect();
    let timestamps: Vec<&str> = deletes.iter().map(|(_, _, ts)| ts.as_str()).collect();

    sqlx::query(
        r#"
        UPDATE relations SET deleted_at = input.deleted_at
        FROM (SELECT id, space_id, deleted_at
              FROM UNNEST($1::uuid[], $2::uuid[], $3::text[]) AS t(id, space_id, deleted_at)) AS input
        WHERE relations.id = input.id
          AND relations.space_id = input.space_id
          AND relations.deleted_at IS NULL
        "#,
    )
    .bind(&ids)
    .bind(&space_ids)
    .bind(&timestamps)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Restore soft-deleted relations by clearing deleted_at.
pub async fn restore_relations(
    &self,
    restores: &[(Uuid, Uuid)], // (relation_id, space_id)
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), IndexerError> {
    if restores.is_empty() {
        return Ok(());
    }

    let ids: Vec<Uuid> = restores.iter().map(|(id, _)| *id).collect();
    let space_ids: Vec<Uuid> = restores.iter().map(|(_, sid)| *sid).collect();

    sqlx::query(
        r#"
        UPDATE relations SET deleted_at = NULL
        WHERE (id, space_id) IN (
            SELECT id, space_id FROM UNNEST($1::uuid[], $2::uuid[]) AS t(id, space_id)
        ) AND deleted_at IS NOT NULL
        "#,
    )
    .bind(&ids)
    .bind(&space_ids)
    .execute(&mut **tx)
    .await?;

    Ok(())
}
```

### 3. Edit Handler Changes

Handle the missing op types in `kg-indexer/src/handlers/edits.rs`:

```rust
/// Extract all entity operations from an edit, maintaining order for squashing.
fn extract_entity_ops(edit: &Grc20Edit, meta: &EditMetadata) -> Vec<EntityOp> {
    let mut ops = Vec::new();

    for op in &edit.ops {
        match op {
            Grc20Op::CreateEntity(entity) => {
                ops.push(EntityOp::Create(EntityItem {
                    id: id_to_uuid(&entity.id),
                    created_at: meta.timestamp.clone(),
                    created_at_block: meta.block_number.clone(),
                    updated_at: meta.timestamp.clone(),
                    updated_at_block: meta.block_number.clone(),
                }));
                // Also add property IDs as entities (existing behavior)
                for pv in &entity.values {
                    ops.push(EntityOp::Create(EntityItem {
                        id: id_to_uuid(&pv.property),
                        created_at: meta.timestamp.clone(),
                        created_at_block: meta.block_number.clone(),
                        updated_at: meta.timestamp.clone(),
                        updated_at_block: meta.block_number.clone(),
                    }));
                }
            }
            Grc20Op::UpdateEntity(entity) => {
                // UpdateEntity also creates the entity if it doesn't exist
                ops.push(EntityOp::Create(EntityItem {
                    id: id_to_uuid(&entity.id),
                    created_at: meta.timestamp.clone(),
                    created_at_block: meta.block_number.clone(),
                    updated_at: meta.timestamp.clone(),
                    updated_at_block: meta.block_number.clone(),
                }));
            }
            Grc20Op::DeleteEntity(del) => {
                ops.push(EntityOp::Delete {
                    id: id_to_uuid(&del.id),
                    timestamp: meta.timestamp.clone(),
                });
            }
            Grc20Op::RestoreEntity(restore) => {
                ops.push(EntityOp::Restore {
                    id: id_to_uuid(&restore.id),
                });
            }
            Grc20Op::CreateRelation(relation) => {
                // Relations create their reified entity
                ops.push(EntityOp::Create(EntityItem {
                    id: id_to_uuid(&relation.entity_id()),
                    created_at: meta.timestamp.clone(),
                    created_at_block: meta.block_number.clone(),
                    updated_at: meta.timestamp.clone(),
                    updated_at_block: meta.block_number.clone(),
                }));
            }
            _ => {}
        }
    }

    ops
}

/// Process an edit and return squashed operations ready for storage.
pub fn handle_edit(edit: &HermesEdit) -> Result<EditResult, HandlerError> {
    let edit_id = parse_edit_id(&edit.id)?;
    let space_id = parse_space_id(edit.space_id.as_slice())?;
    let meta = EditMetadata::from_edit(edit);
    let grc20_edit = decode_payload(&edit.payload)?;

    // Extract and squash entity operations
    let entity_ops = extract_entity_ops(&grc20_edit, &meta);
    let squashed_entity_ops = squash_entity_ops(&entity_ops);

    // Partition into creates, deletes, restores
    let mut entity_creates = Vec::new();
    let mut entity_deletes = Vec::new();
    let mut entity_restores = Vec::new();

    for op in squashed_entity_ops {
        match op {
            EntityOp::Create(e) => entity_creates.push(e),
            EntityOp::Delete { id, timestamp } => entity_deletes.push((id, timestamp)),
            EntityOp::Restore { id } => entity_restores.push(id),
        }
    }

    // Extract values and relations (existing logic)
    let value_ops = extract_values(&grc20_edit, &space_id);
    let relation_ops = extract_relations(&grc20_edit, &space_id, &meta);

    // Filter values targeting deleted entities (intra-edit)
    let deleted_entity_ids: HashSet<Uuid> = entity_deletes.iter().map(|(id, _)| *id).collect();
    let filtered_values: Vec<ValueOp> = squash_values(&value_ops)
        .into_iter()
        .filter(|v| !deleted_entity_ids.contains(&v.entity_id))
        .collect();

    // Squash relations (existing) + add RestoreRelation handling
    let squashed_relations = squash_relations(&relation_ops);

    Ok(EditResult {
        edit_id,
        entities: entity_creates,
        entity_deletes,
        entity_restores,
        values: filtered_values,
        relations: squashed_relations,
    })
}
```

The `EditResult` struct needs to be extended:

```rust
pub struct EditResult {
    pub edit_id: Uuid,
    pub entities: Vec<EntityItem>,
    pub entity_deletes: Vec<(Uuid, String)>,  // (id, timestamp)
    pub entity_restores: Vec<Uuid>,
    pub values: Vec<ValueOp>,
    pub relations: Vec<RelationOp>,
}
```

Similarly for relations, add `RestoreRelation` handling to `extract_relations` and update `RelationOp` enum:

```rust
pub enum RelationOp {
    Create(SetRelationItem),
    Update(UpdateRelationItem),
    Unset(UnsetRelationItem),
    Delete(DeleteRelationItem),
    Restore(RestoreRelationItem),  // New
}

pub struct RestoreRelationItem {
    pub id: Uuid,
    pub space_id: Uuid,
}
```

### 4. Intra-Edit Squashing

Within a single edit, operations must respect tombstone dominance. If an edit contains:
1. `DeleteEntity(A)`
2. `CreateEntity(A)`

The create should be ignored (tombstone absorbs).

**Solution**: Squash entity operations in the edit handler before writing, similar to existing `squash_values` and `squash_relations` functions:

```rust
// kg-indexer/src/handlers/edits.rs

#[derive(Clone, Debug)]
pub enum EntityOp {
    Create(EntityItem),
    Delete { id: Uuid, timestamp: String },
    Restore { id: Uuid },
}

impl EntityOp {
    pub fn id(&self) -> Uuid {
        match self {
            EntityOp::Create(e) => e.id,
            EntityOp::Delete { id, .. } => *id,
            EntityOp::Restore { id } => *id,
        }
    }
}

/// Squash entity operations within an edit.
/// Handles tombstone dominance per GRC-20 spec:
/// - Create then Delete → Delete only (entity created then immediately deleted)
/// - Delete then Create → Delete only (tombstone absorbs create)
/// - Delete then Restore → Restore only (explicit undo)
/// - Create then Restore → Create only (restore is no-op on active entity)
fn squash_entity_ops(ops: &[EntityOp]) -> Vec<EntityOp> {
    let mut state: HashMap<Uuid, EntityOp> = HashMap::new();

    for op in ops {
        let id = op.id();
        match (state.get(&id), op) {
            // Delete after Create → Delete wins (final state is deleted)
            (Some(EntityOp::Create(_)), EntityOp::Delete { .. }) => {
                state.insert(id, op.clone());
            }
            // Create after Delete → Delete wins (tombstone absorbs)
            (Some(EntityOp::Delete { .. }), EntityOp::Create(_)) => {
                // Keep existing delete, ignore create
            }
            // Restore after Delete → Restore wins
            (Some(EntityOp::Delete { .. }), EntityOp::Restore { .. }) => {
                state.insert(id, op.clone());
            }
            // Restore after Create → Keep create (restore is no-op)
            (Some(EntityOp::Create(_)), EntityOp::Restore { .. }) => {
                // Keep existing create
            }
            // Delete after Restore → Delete wins
            (Some(EntityOp::Restore { .. }), EntityOp::Delete { .. }) => {
                state.insert(id, op.clone());
            }
            // Create after Restore → Create wins (updates the restored entity)
            (Some(EntityOp::Restore { .. }), EntityOp::Create(_)) => {
                state.insert(id, op.clone());
            }
            // First occurrence
            (None, _) => {
                state.insert(id, op.clone());
            }
            // Same op type twice → last one wins
            _ => {
                state.insert(id, op.clone());
            }
        }
    }

    state.into_values().collect()
}
```

This keeps the SQL simple - just WHERE clauses for cross-edit tombstone dominance, no CTEs needed. The edit handler filters out invalid states before any database writes occur.

### 5. Main Loop Integration

Update `kg-indexer/src/main.rs` to call the new storage functions:

```rust
KgMessage::Edit(edit) => {
    let result = handlers::edits::handle_edit(edit)?;

    // Keep copies for versioned writes before partitioning
    let values_for_versioning = result.values.clone();
    let relations_for_versioning = result.relations.clone();

    // Partition values into sets and deletes
    let (set_values, delete_values): (Vec<_>, Vec<_>) = result
        .values
        .into_iter()
        .partition(|v| matches!(v.change_type, ValueChangeType::Set));

    let delete_value_ids: Vec<_> = delete_values
        .into_iter()
        .map(|v| (v.id, v.space_id))
        .collect();

    // Partition relations by operation type
    let mut set_relations = Vec::new();
    let mut update_relations = Vec::new();
    let mut unset_relations = Vec::new();
    let mut delete_relations = Vec::new();
    let mut restore_relations = Vec::new();  // New

    for op in result.relations {
        match op {
            RelationOp::Create(r) => set_relations.push(r),
            RelationOp::Update(r) => update_relations.push(r),
            RelationOp::Unset(r) => unset_relations.push(r),
            RelationOp::Delete(r) => delete_relations.push((r.id, r.space_id, meta.timestamp.clone())),
            RelationOp::Restore(r) => restore_relations.push((r.id, r.space_id)),  // New
        }
    }

    // Entity operations (live tables)
    storage.insert_entities(&result.entities, &mut tx).await?;
    storage.delete_entities(&result.entity_deletes, &mut tx).await?;  // New
    storage.restore_entities(&result.entity_restores, &mut tx).await?;  // New

    // Value operations (live tables)
    storage.insert_values(&set_values, &mut tx).await?;
    storage.delete_values(&delete_value_ids, &mut tx).await?;

    // Relation operations (live tables)
    storage.insert_relations(&set_relations, &mut tx).await?;
    storage.update_relations(&update_relations, &mut tx).await?;
    storage.unset_relation_fields(&unset_relations, &mut tx).await?;
    storage.delete_relations(&delete_relations, &mut tx).await?;  // Changed signature
    storage.restore_relations(&restore_relations, &mut tx).await?;  // New

    // Versioned writes (temporal tables)
    if let Some(meta) = edit.meta.as_ref() {
        let version_key = storage
            .insert_edit_version(...)
            .await?;

        storage
            .insert_value_versions(&values_for_versioning, version_key, &mut tx)
            .await?;
        storage
            .insert_relation_versions(&relations_for_versioning, version_key, &mut tx)
            .await?;
    }

    ops
}
```

### 7. PostGraphile Plugin

Create a plugin to automatically filter deleted records from queries:

```typescript
// api/src/kg/softDeleteFilterPlugin.ts

export const SoftDeleteFilterPlugin = (builder: any) => {
  builder.hook("GraphQLObjectType:fields:field:args", (args: any, build: any, context: any) => {
    const {
      scope: { isPgFieldConnection, isPgFieldSimpleCollection, pgFieldIntrospection },
      addArgDataGenerator,
    } = context

    if (!isPgFieldConnection && !isPgFieldSimpleCollection) {
      return args
    }

    const tableName = pgFieldIntrospection?.name
    if (tableName !== "entities" && tableName !== "relations") {
      return args
    }

    const { pgSql: sql } = build

    // Filter out deleted records by default
    addArgDataGenerator(({ includeDeleted }: { includeDeleted?: boolean }) => {
      if (includeDeleted === true) return {}

      return {
        pgQuery: (queryBuilder: any) => {
          const tableAlias = queryBuilder.getTableAlias()
          queryBuilder.where(sql.fragment`${tableAlias}.deleted_at IS NULL`)
        },
      }
    })

    return build.extend(args, {
      includeDeleted: {
        description: "Include soft-deleted records (default: false)",
        type: build.graphql.GraphQLBoolean,
        defaultValue: false,
      },
    })
  })
}
```

Register in `postgraphile.ts`:

```typescript
appendPlugins: [
  UndashedUuidPlugin,
  ConnectionFilterPlugin,
  SimplifyInflectionPlugin,
  EntitySpaceFilterPlugin,
  SoftDeleteFilterPlugin,
],
```

### 8. Migration Strategy

Per `grc20-v2-gaps.md`, this requires a **wipe and re-index**:

1. Add migration to add `deleted_at` columns
2. Deploy updated indexer with new storage functions
3. Deploy updated API with PostGraphile plugin
4. Wipe database and re-index from scratch

## Performance Considerations

1. **Partial indexes**: Use `WHERE deleted_at IS NOT NULL` to keep index small (only tombstones)
2. **Subquery cost**: The `NOT EXISTS` check is O(1) with proper indexing
3. **Write amplification**: None - we filter before writing, not after

## Testing

1. **Intra-edit ordering**: Edit with DeleteEntity then CreateEntity for same ID
2. **Cross-edit ordering**: Delete in edit N, create in edit N+1
3. **Restore behavior**: Delete then restore should allow subsequent updates
4. **Query filtering**: Deleted entities/relations excluded by default
5. **includeDeleted flag**: Can opt-in to see deleted records

## Open Questions

1. Should values have their own `deleted_at`? Currently they become orphans when entity is deleted.
2. Should we cascade-delete values when entity is deleted? (spec says no - entity delete just hides, values remain for history)
3. Do we need `deleted_at` on `value_versions` and `relation_versions` tables?
