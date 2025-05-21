use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// Database schema types (matching your Drizzle schema)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Value {
    pub id: String,
    pub entity_id: String,
    pub space_id: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
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

// State tracking for pre-computation
#[derive(Debug, Clone)]
enum EntityState {
    Active(Entity),
    Deleted,
}

#[derive(Debug, Clone)]
enum RelationState {
    Active(Relation),
    Deleted,
}

// Main processor struct
pub struct EventProcessor {
    // State maps for tracking final state
    entity_states: HashMap<String, EntityState>,
    value_states: HashMap<String, Value>, // Key: entity_id + property_id
    relation_states: HashMap<String, RelationState>,

    // Track which values to delete for entities
    values_to_unset: HashMap<String, HashSet<String>>, // entity_id -> set of property_ids
}

impl EventProcessor {
    pub fn new() -> Self {
        Self {
            entity_states: HashMap::new(),
            value_states: HashMap::new(),
            relation_states: HashMap::new(),
            values_to_unset: HashMap::new(),
        }
    }

    /// Process an edit and return the pre-computed final states
    pub fn process_edit(&mut self, edit: &Edit) -> ProcessedEdit {
        // Process operations in order
        for op in &edit.ops {
            self.process_operation(op, &edit.space_id);
        }

        // Extract final states
        self.extract_final_states()
    }

    fn process_operation(&mut self, op: &Op, space_id: &str) {
        match op {
            Op::UpdateEntity(entity_msg) => {
                self.process_update_entity(entity_msg, space_id);
            }
            Op::DeleteEntity(entity_id) => {
                self.process_delete_entity(entity_id);
            }
            Op::CreateRelation(relation_msg) => {
                self.process_create_relation(relation_msg, space_id);
            }
            Op::UpdateRelation(relation_update) => {
                self.process_update_relation(relation_update);
            }
            Op::DeleteRelation(relation_id) => {
                self.process_delete_relation(relation_id);
            }
            Op::UnsetEntityValues(unset_msg) => {
                self.process_unset_entity_values(unset_msg);
            }
            Op::UnsetRelationFields(unset_msg) => {
                self.process_unset_relation_fields(unset_msg);
            }
        }
    }

    fn process_update_entity(&mut self, entity_msg: &EntityMsg, space_id: &str) {
        let entity_id = hex::encode(&entity_msg.id);

        // Create or update entity
        let entity = Entity {
            id: entity_id.clone(),
        };
        self.entity_states
            .insert(entity_id.clone(), EntityState::Active(entity));

        // Process values
        for value_msg in &entity_msg.values {
            let property_id = hex::encode(&value_msg.property_id);
            let value_key = format!("{}:{}", entity_id, property_id);

            let value = Value {
                id: value_key.clone(),
                entity_id: entity_id.clone(),
                space_id: space_id.to_string(),
                value: value_msg.value.clone(),
            };

            self.value_states.insert(value_key, value);
        }
    }

    fn process_delete_entity(&mut self, entity_id: &[u8]) {
        let entity_id_str = hex::encode(entity_id);

        // Mark entity as deleted
        self.entity_states
            .insert(entity_id_str.clone(), EntityState::Deleted);

        // Remove all values for this entity
        self.value_states
            .retain(|key, _| !key.starts_with(&format!("{}:", entity_id_str)));
    }

    fn process_create_relation(&mut self, relation_msg: &RelationMsg, space_id: &str) {
        let relation_id = hex::encode(&relation_msg.id);

        let relation = Relation {
            id: relation_id.clone(),
            entity_id: hex::encode(&relation_msg.entity),
            type_id: hex::encode(&relation_msg.type_id),
            from_entity_id: hex::encode(&relation_msg.from_entity),
            from_space_id: relation_msg.from_space.as_ref().map(|s| hex::encode(s)),
            from_version_id: relation_msg.from_version.as_ref().map(|s| hex::encode(s)),
            to_entity_id: hex::encode(&relation_msg.to_entity),
            to_space_id: relation_msg.to_space.as_ref().map(|s| hex::encode(s)),
            to_version_id: relation_msg.to_version.as_ref().map(|s| hex::encode(s)),
            position: relation_msg.position.clone(),
            space_id: space_id.to_string(),
            verified: relation_msg.verified,
        };

        self.relation_states
            .insert(relation_id, RelationState::Active(relation));
    }

    fn process_update_relation(&mut self, update_msg: &RelationUpdateMsg) {
        let relation_id = hex::encode(&update_msg.id);

        if let Some(RelationState::Active(relation)) = self.relation_states.get_mut(&relation_id) {
            // Update fields if provided
            if let Some(from_space) = &update_msg.from_space {
                relation.from_space_id = Some(hex::encode(from_space));
            }
            if let Some(from_version) = &update_msg.from_version {
                relation.from_version_id = Some(hex::encode(from_version));
            }
            if let Some(to_space) = &update_msg.to_space {
                relation.to_space_id = Some(hex::encode(to_space));
            }
            if let Some(to_version) = &update_msg.to_version {
                relation.to_version_id = Some(hex::encode(to_version));
            }
            if update_msg.position.is_some() {
                relation.position = update_msg.position.clone();
            }
            if update_msg.verified.is_some() {
                relation.verified = update_msg.verified;
            }
        }
    }

    fn process_delete_relation(&mut self, relation_id: &[u8]) {
        let relation_id_str = hex::encode(relation_id);
        self.relation_states
            .insert(relation_id_str, RelationState::Deleted);
    }

    fn process_unset_entity_values(&mut self, unset_msg: &UnsetEntityValuesMsg) {
        let entity_id = hex::encode(&unset_msg.id);

        // Remove specified property values
        for property_id_bytes in &unset_msg.properties {
            let property_id = hex::encode(property_id_bytes);
            let value_key = format!("{}:{}", entity_id, property_id);
            self.value_states.remove(&value_key);
        }
    }

    fn process_unset_relation_fields(&mut self, unset_msg: &UnsetRelationFieldsMsg) {
        let relation_id = hex::encode(&unset_msg.id);

        if let Some(RelationState::Active(relation)) = self.relation_states.get_mut(&relation_id) {
            if unset_msg.from_space.unwrap_or(false) {
                relation.from_space_id = None;
            }
            if unset_msg.from_version.unwrap_or(false) {
                relation.from_version_id = None;
            }
            if unset_msg.to_space.unwrap_or(false) {
                relation.to_space_id = None;
            }
            if unset_msg.to_version.unwrap_or(false) {
                relation.to_version_id = None;
            }
            if unset_msg.position.unwrap_or(false) {
                relation.position = None;
            }
            if unset_msg.verified.unwrap_or(false) {
                relation.verified = None;
            }
        }
    }

    fn extract_final_states(&self) -> ProcessedEdit {
        let mut entities_to_insert = Vec::new();
        let mut entities_to_delete = Vec::new();

        let mut values_to_insert = Vec::new();
        let mut values_to_update = Vec::new();
        let mut values_to_delete = Vec::new();

        let mut relations_to_insert = Vec::new();
        let mut relations_to_update = Vec::new();
        let mut relations_to_delete = Vec::new();

        // Process entities
        for (entity_id, state) in &self.entity_states {
            match state {
                EntityState::Active(entity) => {
                    entities_to_insert.push(entity.clone());
                }
                EntityState::Deleted => {
                    entities_to_delete.push(entity_id.clone());
                }
            }
        }

        // Process values - all are treated as upserts since we've pre-computed final state
        for (_, value) in &self.value_states {
            values_to_insert.push(value.clone());
        }

        // Process relations
        for (relation_id, state) in &self.relation_states {
            match state {
                RelationState::Active(relation) => {
                    relations_to_insert.push(relation.clone());
                }
                RelationState::Deleted => {
                    relations_to_delete.push(relation_id.clone());
                }
            }
        }

        ProcessedEdit {
            entities_to_insert,
            entities_to_delete,
            values_to_insert,
            values_to_update,
            values_to_delete,
            relations_to_insert,
            relations_to_update,
            relations_to_delete,
        }
    }

    /// Reset the processor state for the next edit
    pub fn reset(&mut self) {
        self.entity_states.clear();
        self.value_states.clear();
        self.relation_states.clear();
        self.values_to_unset.clear();
    }
}

#[derive(Debug)]
pub struct ProcessedEdit {
    pub entities_to_insert: Vec<Entity>,
    pub entities_to_delete: Vec<String>,
    pub values_to_insert: Vec<Value>,
    pub values_to_update: Vec<Value>,
    pub values_to_delete: Vec<String>,
    pub relations_to_insert: Vec<Relation>,
    pub relations_to_update: Vec<Relation>,
    pub relations_to_delete: Vec<String>,
}

impl ProcessedEdit {
    /// Returns the data organized for parallel processing
    pub fn into_parallel_batches(self) -> ParallelBatches {
        ParallelBatches {
            entity_operations: EntityOperations {
                inserts: self.entities_to_insert,
                deletes: self.entities_to_delete,
            },
            value_operations: ValueOperations {
                inserts: self.values_to_insert,
                updates: self.values_to_update,
                deletes: self.values_to_delete,
            },
            relation_operations: RelationOperations {
                inserts: self.relations_to_insert,
                updates: self.relations_to_update,
                deletes: self.relations_to_delete,
            },
        }
    }
}

#[derive(Debug)]
pub struct ParallelBatches {
    pub entity_operations: EntityOperations,
    pub value_operations: ValueOperations,
    pub relation_operations: RelationOperations,
}

#[derive(Debug)]
pub struct EntityOperations {
    pub inserts: Vec<Entity>,
    pub deletes: Vec<String>,
}

#[derive(Debug)]
pub struct ValueOperations {
    pub inserts: Vec<Value>,
    pub updates: Vec<Value>,
    pub deletes: Vec<String>,
}

#[derive(Debug)]
pub struct RelationOperations {
    pub inserts: Vec<Relation>,
    pub updates: Vec<Relation>,
    pub deletes: Vec<String>,
}

// Example usage and database interface
pub trait DatabaseWriter {
    async fn batch_insert_entities(
        &self,
        entities: Vec<Entity>,
    ) -> Result<(), Box<dyn std::error::Error>>;
    async fn batch_delete_entities(
        &self,
        entity_ids: Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error>>;
    async fn batch_upsert_values(
        &self,
        values: Vec<Value>,
    ) -> Result<(), Box<dyn std::error::Error>>;
    async fn batch_delete_values(
        &self,
        value_ids: Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error>>;
    async fn batch_upsert_relations(
        &self,
        relations: Vec<Relation>,
    ) -> Result<(), Box<dyn std::error::Error>>;
    async fn batch_delete_relations(
        &self,
        relation_ids: Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error>>;
}

/// Process multiple edits and write to database with parallelization
pub async fn process_and_write_edits<W: DatabaseWriter>(
    edits: Vec<Edit>,
    writer: &W,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut processor = EventProcessor::new();

    for edit in edits {
        let processed = processor.process_edit(&edit);
        let batches = processed.into_parallel_batches();

        // Execute database operations in parallel for each table type
        let entity_ops = async {
            if !batches.entity_operations.inserts.is_empty() {
                writer
                    .batch_insert_entities(batches.entity_operations.inserts)
                    .await?;
            }
            if !batches.entity_operations.deletes.is_empty() {
                writer
                    .batch_delete_entities(batches.entity_operations.deletes)
                    .await?;
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        };

        let value_ops = async {
            if !batches.value_operations.inserts.is_empty() {
                writer
                    .batch_upsert_values(batches.value_operations.inserts)
                    .await?;
            }
            if !batches.value_operations.deletes.is_empty() {
                writer
                    .batch_delete_values(batches.value_operations.deletes)
                    .await?;
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        };

        let relation_ops = async {
            if !batches.relation_operations.inserts.is_empty() {
                writer
                    .batch_upsert_relations(batches.relation_operations.inserts)
                    .await?;
            }
            if !batches.relation_operations.deletes.is_empty() {
                writer
                    .batch_delete_relations(batches.relation_operations.deletes)
                    .await?;
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        };

        // Execute all operations in parallel
        tokio::try_join!(entity_ops, value_ops, relation_ops)?;

        // Reset processor for next edit
        processor.reset();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_update_then_delete() {
        let mut processor = EventProcessor::new();

        // Create an edit with update then delete
        let edit = Edit {
            id: vec![1, 2, 3],
            name: "test".to_string(),
            space_id: "space1".to_string(),
            ops: vec![
                Op::UpdateEntity(EntityMsg {
                    id: vec![1],
                    values: vec![ValueMsg {
                        property_id: vec![2],
                        value: "test_value".to_string(),
                    }],
                }),
                Op::DeleteEntity(vec![1]),
            ],
            authors: vec![],
            language: None,
        };

        let processed = processor.process_edit(&edit);

        // Entity should be marked for deletion
        assert_eq!(processed.entities_to_delete.len(), 1);
        assert_eq!(processed.entities_to_insert.len(), 0);

        // Values should be cleared (no values to insert)
        assert_eq!(processed.values_to_insert.len(), 0);
    }

    #[test]
    fn test_entity_delete_then_update() {
        let mut processor = EventProcessor::new();

        // Create an edit with delete then update
        let edit = Edit {
            id: vec![1, 2, 3],
            name: "test".to_string(),
            space_id: "space1".to_string(),
            ops: vec![
                Op::DeleteEntity(vec![1]),
                Op::UpdateEntity(EntityMsg {
                    id: vec![1],
                    values: vec![ValueMsg {
                        property_id: vec![2],
                        value: "new_value".to_string(),
                    }],
                }),
            ],
            authors: vec![],
            language: None,
        };

        let processed = processor.process_edit(&edit);

        // Entity should be marked for insertion (final state after delete->update)
        assert_eq!(processed.entities_to_insert.len(), 1);
        assert_eq!(processed.entities_to_delete.len(), 0);

        // Values should be present
        assert_eq!(processed.values_to_insert.len(), 1);
        assert_eq!(processed.values_to_insert[0].value, "new_value");
    }
}
