use tracing::debug;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::{
    entities::EntityItem,
    membership::{EditorItem, MemberItem},
    properties::PropertyItem,
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    spaces::SpaceItem,
    subspaces::SubspaceItem,
    values::ValueOp,
};
use crate::storage::Storage;

/// Accumulates database operations for batch writing
#[derive(Default)]
pub struct Batch {
    // Entities
    pub entities: Vec<EntityItem>,

    // Properties
    pub properties: Vec<PropertyItem>,

    // Values
    pub set_values: Vec<ValueOp>,
    pub delete_values: Vec<(Uuid, Uuid)>, // (value_id, space_id)

    // Relations
    pub set_relations: Vec<SetRelationItem>,
    pub update_relations: Vec<UpdateRelationItem>,
    pub unset_relations: Vec<UnsetRelationItem>,
    pub delete_relations: Vec<(Uuid, Uuid)>, // (relation_id, space_id)

    // Spaces
    pub spaces: Vec<SpaceItem>,

    // Membership
    pub add_members: Vec<MemberItem>,
    pub remove_members: Vec<MemberItem>,
    pub add_editors: Vec<EditorItem>,
    pub remove_editors: Vec<EditorItem>,

    // Subspaces
    pub add_subspaces: Vec<SubspaceItem>,
    pub remove_subspaces: Vec<SubspaceItem>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the total number of pending operations
    pub fn len(&self) -> usize {
        self.entities.len()
            + self.properties.len()
            + self.set_values.len()
            + self.delete_values.len()
            + self.set_relations.len()
            + self.update_relations.len()
            + self.unset_relations.len()
            + self.delete_relations.len()
            + self.spaces.len()
            + self.add_members.len()
            + self.remove_members.len()
            + self.add_editors.len()
            + self.remove_editors.len()
            + self.add_subspaces.len()
            + self.remove_subspaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Flush all accumulated operations to the database in a single transaction
    pub async fn flush(&mut self, storage: &Storage) -> Result<(), IndexerError> {
        if self.is_empty() {
            return Ok(());
        }

        let batch_size = self.len();
        let mut tx = storage.pool.begin().await?;

        // Insert spaces first (they may be referenced by other entities)
        if !self.spaces.is_empty() {
            storage.insert_spaces(&self.spaces, &mut tx).await?;
        }

        // Insert entities (they may be referenced by values/relations)
        if !self.entities.is_empty() {
            storage.insert_entities(&self.entities, &mut tx).await?;
        }

        // Insert properties (must exist before values reference them)
        if !self.properties.is_empty() {
            storage.insert_properties(&self.properties, &mut tx).await?;
        }

        // Insert/delete values
        if !self.set_values.is_empty() {
            storage.insert_values(&self.set_values, &mut tx).await?;
        }

        // Group delete_values by space_id for batch deletion
        if !self.delete_values.is_empty() {
            let mut by_space: std::collections::HashMap<Uuid, Vec<Uuid>> =
                std::collections::HashMap::new();
            for (value_id, space_id) in &self.delete_values {
                by_space.entry(*space_id).or_default().push(*value_id);
            }
            for (space_id, value_ids) in by_space {
                storage
                    .delete_values(&value_ids, &space_id, &mut tx)
                    .await?;
            }
        }

        // Insert/update/delete relations
        if !self.set_relations.is_empty() {
            storage
                .insert_relations(&self.set_relations, &mut tx)
                .await?;
        }
        if !self.update_relations.is_empty() {
            storage
                .update_relations(&self.update_relations, &mut tx)
                .await?;
        }
        if !self.unset_relations.is_empty() {
            storage
                .unset_relation_fields(&self.unset_relations, &mut tx)
                .await?;
        }

        // Group delete_relations by space_id for batch deletion
        if !self.delete_relations.is_empty() {
            let mut by_space: std::collections::HashMap<Uuid, Vec<Uuid>> =
                std::collections::HashMap::new();
            for (relation_id, space_id) in &self.delete_relations {
                by_space.entry(*space_id).or_default().push(*relation_id);
            }
            for (space_id, relation_ids) in by_space {
                storage
                    .delete_relations(&relation_ids, &space_id, &mut tx)
                    .await?;
            }
        }

        // Membership operations
        if !self.add_members.is_empty() {
            storage.insert_members(&self.add_members, &mut tx).await?;
        }
        if !self.remove_members.is_empty() {
            storage.remove_members(&self.remove_members, &mut tx).await?;
        }
        if !self.add_editors.is_empty() {
            storage.insert_editors(&self.add_editors, &mut tx).await?;
        }
        if !self.remove_editors.is_empty() {
            storage.remove_editors(&self.remove_editors, &mut tx).await?;
        }

        // Subspace operations
        if !self.add_subspaces.is_empty() {
            storage
                .insert_subspaces(&self.add_subspaces, &mut tx)
                .await?;
        }
        if !self.remove_subspaces.is_empty() {
            storage
                .remove_subspaces(&self.remove_subspaces, &mut tx)
                .await?;
        }

        tx.commit().await?;

        debug!(
            batch_size = batch_size,
            entities = self.entities.len(),
            properties = self.properties.len(),
            set_values = self.set_values.len(),
            delete_values = self.delete_values.len(),
            set_relations = self.set_relations.len(),
            spaces = self.spaces.len(),
            add_members = self.add_members.len(),
            add_editors = self.add_editors.len(),
            add_subspaces = self.add_subspaces.len(),
            "Flushed batch"
        );

        // Clear all accumulated data
        self.clear();

        Ok(())
    }

    /// Clear all accumulated data without flushing
    pub fn clear(&mut self) {
        self.entities.clear();
        self.properties.clear();
        self.set_values.clear();
        self.delete_values.clear();
        self.set_relations.clear();
        self.update_relations.clear();
        self.unset_relations.clear();
        self.delete_relations.clear();
        self.spaces.clear();
        self.add_members.clear();
        self.remove_members.clear();
        self.add_editors.clear();
        self.remove_editors.clear();
        self.add_subspaces.clear();
        self.remove_subspaces.clear();
    }
}
