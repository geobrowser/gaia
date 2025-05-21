use anyhow::Result;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

// Final state representations
#[derive(Debug, Clone)]
pub struct ProcessedState {
    pub entities: HashMap<String, EntityState>,
    pub values: HashMap<String, ValueState>,
    pub relations: HashMap<String, RelationState>,
    pub deleted_entities: HashSet<String>,
    pub deleted_relations: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct EntityState {
    pub id: String,
    pub values: HashMap<String, ValueState>, // property_id -> ValueState
}

#[derive(Debug, Clone)]
pub struct ValueState {
    pub id: String,
    pub entity_id: String,
    pub space_id: String,
    pub property_id: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct RelationState {
    pub id: String,
    pub entity_id: String,
    pub type_id: String,
    pub from_entity_id: String,
    pub from_space_id: Option<String>,
    pub from_version_id: Option<String>,
    pub to_entity_id: String,
    pub to_space_id: Option<String>,
    pub to_version_id: Option<String>,
    pub position: Option<String>,
    pub space_id: String,
    pub verified: Option<bool>,
}

// Database row structures for sqlx
#[derive(Debug, sqlx::FromRow)]
pub struct EntityRow {
    pub id: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ValueRow {
    pub id: String,
    pub entity_id: String,
    pub space_id: String,
    pub value: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct RelationRow {
    pub id: String,
    pub entity_id: String,
    pub type_id: String,
    pub from_entity_id: String,
    pub from_space_id: Option<String>,
    pub from_version_id: Option<String>,
    pub to_entity_id: String,
    pub to_space_id: Option<String>,
    pub to_version_id: Option<String>,
    pub position: Option<String>,
    pub space_id: String,
    pub verified: Option<bool>,
}

impl ProcessedState {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            values: HashMap::new(),
            relations: HashMap::new(),
            deleted_entities: HashSet::new(),
            deleted_relations: HashSet::new(),
        }
    }
}

pub async fn process_edit_and_write(pool: &PgPool, edit: Edit, space_id: &str) -> Result<()> {
    let mut tx = pool.begin().await?;

    // Step 1: Pre-compute final state from operations
    let final_state = compute_final_state(&edit.ops, space_id).await?;

    // Step 2: Write to database in a single transaction
    write_final_state(&mut tx, &final_state).await?;

    tx.commit().await?;
    Ok(())
}

async fn compute_final_state(ops: &[Op], default_space_id: &str) -> Result<ProcessedState> {
    let mut state = ProcessedState::new();

    // Process operations in order to compute final state
    for op in ops {
        match &op.payload {
            OpPayload::UpdateEntity(entity) => {
                process_entity_update(&mut state, entity, default_space_id).await?;
            }
            OpPayload::DeleteEntity(entity_id_bytes) => {
                process_entity_delete(&mut state, entity_id_bytes).await?;
            }
            OpPayload::CreateRelation(relation) => {
                process_relation_create(&mut state, relation, default_space_id).await?;
            }
            OpPayload::UpdateRelation(update) => {
                process_relation_update(&mut state, update).await?;
            }
            OpPayload::DeleteRelation(relation_id_bytes) => {
                process_relation_delete(&mut state, relation_id_bytes).await?;
            }
            OpPayload::UnsetEntityValues(unset) => {
                process_unset_entity_values(&mut state, unset).await?;
            }
            OpPayload::UnsetRelationFields(unset) => {
                process_unset_relation_fields(&mut state, unset).await?;
            }
        }
    }

    Ok(state)
}

async fn process_entity_update(
    state: &mut ProcessedState,
    entity: &Entity,
    default_space_id: &str,
) -> Result<()> {
    let entity_id = hex::encode(&entity.id);

    // Remove from deleted set if it was marked for deletion earlier
    state.deleted_entities.remove(&entity_id);

    // Create or update entity
    let entity_state = state
        .entities
        .entry(entity_id.clone())
        .or_insert(EntityState {
            id: entity_id.clone(),
            values: HashMap::new(),
        });

    // Process entity values
    for value in &entity.values {
        let property_id = hex::encode(&value.property_id);
        let value_id = format!("{}-{}-{}", entity_id, property_id, default_space_id);

        let value_state = ValueState {
            id: value_id.clone(),
            entity_id: entity_id.clone(),
            space_id: default_space_id.to_string(),
            property_id: property_id.clone(),
            value: value.value.clone(),
        };

        entity_state.values.insert(property_id, value_state.clone());
        state.values.insert(value_id, value_state);
    }

    Ok(())
}

async fn process_entity_delete(state: &mut ProcessedState, entity_id_bytes: &[u8]) -> Result<()> {
    let entity_id = hex::encode(entity_id_bytes);

    // Mark entity for deletion
    state.deleted_entities.insert(entity_id.clone());
    state.entities.remove(&entity_id);

    // Remove all values for this entity
    state
        .values
        .retain(|_, value_state| value_state.entity_id != entity_id);

    // Remove all relations for this entity
    let relations_to_delete: Vec<String> = state
        .relations
        .iter()
        .filter(|(_, relation_state)| {
            relation_state.entity_id == entity_id
                || relation_state.from_entity_id == entity_id
                || relation_state.to_entity_id == entity_id
        })
        .map(|(id, _)| id.clone())
        .collect();

    for relation_id in relations_to_delete {
        state.deleted_relations.insert(relation_id.clone());
        state.relations.remove(&relation_id);
    }

    Ok(())
}

async fn process_relation_create(
    state: &mut ProcessedState,
    relation: &Relation,
    default_space_id: &str,
) -> Result<()> {
    let relation_id = hex::encode(&relation.id);

    // Remove from deleted set if it was marked for deletion earlier
    state.deleted_relations.remove(&relation_id);

    let relation_state = RelationState {
        id: relation_id.clone(),
        entity_id: hex::encode(&relation.entity),
        type_id: hex::encode(&relation.r#type),
        from_entity_id: hex::encode(&relation.from_entity),
        from_space_id: relation.from_space.as_ref().map(|s| hex::encode(s)),
        from_version_id: relation.from_version.as_ref().map(|s| hex::encode(s)),
        to_entity_id: hex::encode(&relation.to_entity),
        to_space_id: relation.to_space.as_ref().map(|s| hex::encode(s)),
        to_version_id: relation.to_version.as_ref().map(|s| hex::encode(s)),
        position: relation.position.clone(),
        space_id: default_space_id.to_string(),
        verified: relation.verified,
    };

    state.relations.insert(relation_id, relation_state);
    Ok(())
}

async fn process_relation_update(
    state: &mut ProcessedState,
    update: &RelationUpdate,
) -> Result<()> {
    let relation_id = hex::encode(&update.id);

    // Only update if relation exists in our state
    if let Some(existing_relation) = state.relations.get_mut(&relation_id) {
        // Update fields that are provided
        if let Some(from_space) = &update.from_space {
            existing_relation.from_space_id = Some(hex::encode(from_space));
        }
        if let Some(from_version) = &update.from_version {
            existing_relation.from_version_id = Some(hex::encode(from_version));
        }
        if let Some(to_space) = &update.to_space {
            existing_relation.to_space_id = Some(hex::encode(to_space));
        }
        if let Some(to_version) = &update.to_version {
            existing_relation.to_version_id = Some(hex::encode(to_version));
        }
        if let Some(position) = &update.position {
            existing_relation.position = Some(position.clone());
        }
        if let Some(verified) = update.verified {
            existing_relation.verified = Some(verified);
        }
    }

    Ok(())
}

async fn process_relation_delete(
    state: &mut ProcessedState,
    relation_id_bytes: &[u8],
) -> Result<()> {
    let relation_id = hex::encode(relation_id_bytes);

    state.deleted_relations.insert(relation_id.clone());
    state.relations.remove(&relation_id);

    Ok(())
}

async fn process_unset_entity_values(
    state: &mut ProcessedState,
    unset: &UnsetEntityValues,
) -> Result<()> {
    let entity_id = hex::encode(&unset.id);

    if let Some(entity_state) = state.entities.get_mut(&entity_id) {
        for property_id_bytes in &unset.properties {
            let property_id = hex::encode(property_id_bytes);
            entity_state.values.remove(&property_id);

            // Also remove from global values map
            state.values.retain(|_, value| {
                !(value.entity_id == entity_id && value.property_id == property_id)
            });
        }
    }

    Ok(())
}

async fn process_unset_relation_fields(
    state: &mut ProcessedState,
    unset: &UnsetRelationFields,
) -> Result<()> {
    let relation_id = hex::encode(&unset.id);

    if let Some(relation_state) = state.relations.get_mut(&relation_id) {
        if unset.from_space.unwrap_or(false) {
            relation_state.from_space_id = None;
        }
        if unset.from_version.unwrap_or(false) {
            relation_state.from_version_id = None;
        }
        if unset.to_space.unwrap_or(false) {
            relation_state.to_space_id = None;
        }
        if unset.to_version.unwrap_or(false) {
            relation_state.to_version_id = None;
        }
        if unset.position.unwrap_or(false) {
            relation_state.position = None;
        }
        if unset.verified.unwrap_or(false) {
            relation_state.verified = None;
        }
    }

    Ok(())
}

async fn write_final_state(
    tx: &mut Transaction<'_, Postgres>,
    state: &ProcessedState,
) -> Result<()> {
    // 1. Handle deletions first
    if !state.deleted_entities.is_empty() {
        let entity_ids: Vec<&String> = state.deleted_entities.iter().collect();

        sqlx::query("DELETE FROM entities WHERE id = ANY($1)")
            .bind(&entity_ids)
            .execute(&mut **tx)
            .await?;

        sqlx::query("DELETE FROM values WHERE entity_id = ANY($1)")
            .bind(&entity_ids)
            .execute(&mut **tx)
            .await?;
    }

    if !state.deleted_relations.is_empty() {
        let relation_ids: Vec<&String> = state.deleted_relations.iter().collect();

        sqlx::query("DELETE FROM relations WHERE id = ANY($1)")
            .bind(&relation_ids)
            .execute(&mut **tx)
            .await?;
    }

    // 2. Upsert entities
    if !state.entities.is_empty() {
        for entity_state in state.entities.values() {
            sqlx::query("INSERT INTO entities (id) VALUES ($1) ON CONFLICT (id) DO NOTHING")
                .bind(&entity_state.id)
                .execute(&mut **tx)
                .await?;
        }
    }

    // 3. Upsert values
    if !state.values.is_empty() {
        // Delete existing values for entities being updated, then insert new ones
        let entity_ids: Vec<String> = state
            .values
            .values()
            .map(|v| v.entity_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        if !entity_ids.is_empty() {
            sqlx::query("DELETE FROM values WHERE entity_id = ANY($1)")
                .bind(&entity_ids)
                .execute(&mut **tx)
                .await?;
        }

        for value_state in state.values.values() {
            sqlx::query(
                "INSERT INTO values (id, entity_id, space_id, value) VALUES ($1, $2, $3, $4)",
            )
            .bind(&value_state.id)
            .bind(&value_state.entity_id)
            .bind(&value_state.space_id)
            .bind(&value_state.value)
            .execute(&mut **tx)
            .await?;
        }
    }

    // 4. Upsert relations
    if !state.relations.is_empty() {
        for relation_state in state.relations.values() {
            sqlx::query(
                r#"
                INSERT INTO relations (
                    id, entity_id, type_id, from_entity_id, from_space_id, from_version_id,
                    to_entity_id, to_space_id, to_version_id, position, space_id, verified
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                ON CONFLICT (id) DO UPDATE SET
                    entity_id = EXCLUDED.entity_id,
                    type_id = EXCLUDED.type_id,
                    from_entity_id = EXCLUDED.from_entity_id,
                    from_space_id = EXCLUDED.from_space_id,
                    from_version_id = EXCLUDED.from_version_id,
                    to_entity_id = EXCLUDED.to_entity_id,
                    to_space_id = EXCLUDED.to_space_id,
                    to_version_id = EXCLUDED.to_version_id,
                    position = EXCLUDED.position,
                    space_id = EXCLUDED.space_id,
                    verified = EXCLUDED.verified
                "#,
            )
            .bind(&relation_state.id)
            .bind(&relation_state.entity_id)
            .bind(&relation_state.type_id)
            .bind(&relation_state.from_entity_id)
            .bind(&relation_state.from_space_id)
            .bind(&relation_state.from_version_id)
            .bind(&relation_state.to_entity_id)
            .bind(&relation_state.to_space_id)
            .bind(&relation_state.to_version_id)
            .bind(&relation_state.position)
            .bind(&relation_state.space_id)
            .bind(&relation_state.verified)
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(())
}
