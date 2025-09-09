use std::collections::HashMap;

use indexer_utils::id;
use tracing::{debug, instrument, warn};
use uuid::Uuid;
use wire::pb::grc20::{op::Payload, Edit};

#[derive(Clone, Debug)]
pub struct SetRelationItem {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub type_id: Uuid,
    pub from_id: Uuid,
    pub from_space_id: Option<String>,
    pub from_version_id: Option<String>,
    pub to_id: Uuid,
    pub to_space_id: Option<String>,
    pub to_version_id: Option<String>,
    pub position: Option<String>,
    pub space_id: Uuid,
    pub verified: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct UpdateRelationItem {
    pub id: Uuid,
    pub from_space_id: Option<String>,
    pub from_version_id: Option<String>,
    pub to_space_id: Option<String>,
    pub to_version_id: Option<String>,
    pub position: Option<String>,
    pub space_id: Uuid,
    pub verified: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct UnsetRelationItem {
    pub id: Uuid,
    pub from_space_id: Option<bool>,
    pub from_version_id: Option<bool>,
    pub to_space_id: Option<bool>,
    pub to_version_id: Option<bool>,
    pub position: Option<bool>,
    pub space_id: Uuid,
    pub verified: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct DeleteRelationItem {
    pub id: Uuid,
    pub space_id: Uuid,
}

#[derive(Clone, Debug)]
pub enum RelationItem {
    Create(SetRelationItem),
    Update(UpdateRelationItem),
    Unset(UnsetRelationItem),
    Delete(DeleteRelationItem),
}

// Getters for the main RelationItem enum
// Only getters for the main RelationItem enum
impl RelationItem {
    /// Get the id field, present in all variants
    pub fn id(&self) -> &Uuid {
        match self {
            RelationItem::Create(item) => &item.id,
            RelationItem::Update(item) => &item.id,
            RelationItem::Unset(item) => &item.id,
            RelationItem::Delete(item) => &item.id,
        }
    }

    /// Get the space_id field, present in all variants
    pub fn space_id(&self) -> &Uuid {
        match self {
            RelationItem::Create(item) => &item.space_id,
            RelationItem::Update(item) => &item.space_id,
            RelationItem::Unset(item) => &item.space_id,
            RelationItem::Delete(item) => &item.space_id,
        }
    }

    /// Get entity_id (only available in Set variant)
    pub fn entity_id(&self) -> Option<&Uuid> {
        match self {
            RelationItem::Create(item) => Some(&item.entity_id),
            _ => None,
        }
    }

    /// Get type_id (only available in Set variant)
    pub fn type_id(&self) -> Option<&Uuid> {
        match self {
            RelationItem::Create(item) => Some(&item.type_id),
            _ => None,
        }
    }

    /// Get from_id (only available in Set variant)
    pub fn from_id(&self) -> Option<&Uuid> {
        match self {
            RelationItem::Create(item) => Some(&item.from_id),
            _ => None,
        }
    }

    /// Get to_id (only available in Set variant)
    pub fn to_id(&self) -> Option<&Uuid> {
        match self {
            RelationItem::Create(item) => Some(&item.to_id),
            _ => None,
        }
    }

    /// Get verified (available in all variants except Delete)
    pub fn verified(&self) -> Option<bool> {
        match self {
            RelationItem::Create(item) => item.verified,
            RelationItem::Update(item) => item.verified,
            RelationItem::Unset(item) => item.verified,
            RelationItem::Delete(_) => None,
        }
    }

    /// Get from_space_id (available in Set and Update variants)
    pub fn from_space_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => item.from_space_id.as_deref(),
            RelationItem::Update(item) => item.from_space_id.as_deref(),
            _ => None,
        }
    }

    /// Get from_version_id (available in Set and Update variants)
    pub fn from_version_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => item.from_version_id.as_deref(),
            RelationItem::Update(item) => item.from_version_id.as_deref(),
            _ => None,
        }
    }

    /// Get to_space_id (available in Set and Update variants)
    pub fn to_space_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => item.to_space_id.as_deref(),
            RelationItem::Update(item) => item.to_space_id.as_deref(),
            _ => None,
        }
    }

    /// Get to_version_id (available in Set and Update variants)
    pub fn to_version_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => item.to_version_id.as_deref(),
            RelationItem::Update(item) => item.to_version_id.as_deref(),
            _ => None,
        }
    }

    /// Get position (available in Set and Update variants)
    pub fn position(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => item.position.as_deref(),
            RelationItem::Update(item) => item.position.as_deref(),
            _ => None,
        }
    }

    /// Check if this is a Set variant
    pub fn is_set(&self) -> bool {
        matches!(self, RelationItem::Create(_))
    }

    /// Check if this is an Update variant
    pub fn is_update(&self) -> bool {
        matches!(self, RelationItem::Update(_))
    }

    /// Check if this is an Unset variant
    pub fn is_unset(&self) -> bool {
        matches!(self, RelationItem::Unset(_))
    }

    /// Check if this is a Delete variant
    pub fn is_delete(&self) -> bool {
        matches!(self, RelationItem::Delete(_))
    }
}

pub struct RelationsModel;

impl RelationsModel {
    #[instrument(skip_all, fields(space_id = %space_id, op_count = edit.ops.len()))]
    pub fn map_edit_to_relations(
        edit: &Edit,
        space_id: &Uuid,
    ) -> (
        Vec<SetRelationItem>,
        Vec<UpdateRelationItem>,
        Vec<UnsetRelationItem>,
        Vec<Uuid>,
    ) {
        let mut relations = Vec::new();

        for op in &edit.ops {
            if let Some(op_type) = &op.payload {
                match op_type {
                    Payload::CreateRelation(relation) => {
                        let relation_id_bytes = id::transform_id_bytes(relation.id.clone());

                        if let Err(_) = relation_id_bytes {
                            warn!(
                                relation_bytes = ?relation.id,
                                "[Relations][CreateRelation] Could not transform Vec<u8> for relation.id"
                            );
                            continue;
                        }

                        let entity_id_bytes = id::transform_id_bytes(relation.entity.clone());

                        if let Err(_) = entity_id_bytes {
                            warn!(
                                entity_bytes = ?relation.entity,
                                "[Relations][CreateRelation] Could not transform Vec<u8> for relation.entity"
                            );
                            continue;
                        }

                        let type_id_bytes = id::transform_id_bytes(relation.r#type.clone());

                        if let Err(_) = type_id_bytes {
                            warn!(
                                type_bytes = ?relation.r#type,
                                "[Relations][CreateRelation] Could not transform Vec<u8> for relation.type"
                            );
                            continue;
                        }

                        let from_id_bytes = id::transform_id_bytes(relation.from_entity.clone());

                        if let Err(_) = from_id_bytes {
                            warn!(
                                from_entity_bytes = ?relation.from_entity,
                                "[Relations][CreateRelation] Could not transform Vec<u8> for relation.from_entity"
                            );
                            continue;
                        }

                        let to_id_bytes = id::transform_id_bytes(relation.to_entity.clone());

                        if let Err(_) = to_id_bytes {
                            warn!(
                                to_entity_bytes = ?relation.to_entity,
                                "[Relations][CreateRelation] Could not transform Vec<u8> for relation.to_entity"
                            );
                            continue;
                        }

                        let relation_id = Uuid::from_bytes(relation_id_bytes.unwrap());
                        let entity_id = Uuid::from_bytes(entity_id_bytes.unwrap());
                        let type_id = Uuid::from_bytes(type_id_bytes.unwrap());
                        let from_id = Uuid::from_bytes(from_id_bytes.unwrap());
                        let to_id = Uuid::from_bytes(to_id_bytes.unwrap());

                        let to_space = relation
                            .to_space
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        let from_space = relation
                            .from_space
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        let from_version = relation
                            .from_version
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        let to_version = relation
                            .to_version
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        relations.push(RelationItem::Create(SetRelationItem {
                            id: relation_id,
                            entity_id,
                            space_id: space_id.clone(),
                            position: relation.position.clone(),
                            type_id,
                            from_id,
                            from_space_id: from_space,
                            from_version_id: from_version,
                            to_id,
                            to_space_id: to_space,
                            to_version_id: to_version,
                            verified: relation.verified,
                        }));
                    }
                    Payload::DeleteRelation(relation_id) => {
                        let relation_id_bytes = id::transform_id_bytes(relation_id.clone());

                        if let Err(_) = relation_id_bytes {
                            warn!(
                                relation_bytes = ?relation_id,
                                "[Relations][DeleteRelation] Could not transform Vec<u8> for relation.id"
                            );
                            continue;
                        }

                        let relation_id = Uuid::from_bytes(relation_id_bytes.unwrap());

                        relations.push(RelationItem::Delete(DeleteRelationItem {
                            id: relation_id,
                            space_id: space_id.clone(),
                        }));
                    }
                    Payload::UpdateRelation(updated_relation) => {
                        let relation_id_bytes = id::transform_id_bytes(updated_relation.id.clone());

                        if let Err(_) = relation_id_bytes {
                            warn!(
                                relation_bytes = ?updated_relation.id,
                                "[Relations][UpdateRelation] Could not transform Vec<u8> for relation.id"
                            );
                            continue;
                        }

                        let relation_id = Uuid::from_bytes(relation_id_bytes.unwrap());

                        let to_space = updated_relation
                            .to_space
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        let from_space = updated_relation
                            .from_space
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        let from_version = updated_relation
                            .from_version
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        let to_version = updated_relation
                            .to_version
                            .clone()
                            .and_then(|s| id::transform_id_bytes(s).ok())
                            .map(|s| Uuid::from_bytes(s).to_string());

                        relations.push(RelationItem::Update(UpdateRelationItem {
                            id: relation_id,
                            space_id: space_id.clone(),
                            position: updated_relation.position.clone(),
                            verified: updated_relation.verified.clone(),
                            to_space_id: to_space,
                            from_space_id: from_space,
                            from_version_id: from_version,
                            to_version_id: to_version,
                        }));
                    }
                    Payload::UnsetRelationFields(unset_fields) => {
                        let relation_id_bytes = id::transform_id_bytes(unset_fields.id.clone());

                        if let Err(_) = relation_id_bytes {
                            warn!(
                                relation_bytes = ?unset_fields.id,
                                "[Relations][UnsetRelationFields] Could not transform Vec<u8> for relation.id"
                            );
                            continue;
                        }

                        let relation_id = Uuid::from_bytes(relation_id_bytes.unwrap());

                        relations.push(RelationItem::Unset(UnsetRelationItem {
                            id: relation_id,
                            space_id: space_id.clone(),
                            from_space_id: unset_fields.from_space,
                            from_version_id: unset_fields.from_version,
                            to_space_id: unset_fields.to_space,
                            to_version_id: unset_fields.to_version,
                            position: unset_fields.position,
                            verified: unset_fields.verified,
                        }));
                    }
                    _ => {}
                }
            }
        }

        // A single edit may have multiple CREATE, UPDATE, and DELETE relation ops
        // applied to the same relation id. We need to squash them down into a single
        // op so we can write to the db atomically using the final state of the ops.
        //
        // Ordering of these to-be-squashed ops matters. We use what the order is in
        // the edit.
        let original_count = relations.len();
        let squashed = squash_relations(&relations);
        
        if squashed.len() < original_count {
            debug!(
                original_count,
                squashed_count = squashed.len(),
                dropped = original_count - squashed.len(),
                "Squashed duplicate relation operations"
            );
        }

        let mut set_relations: Vec<SetRelationItem> = Vec::new();
        let mut update_relations: Vec<UpdateRelationItem> = Vec::new();
        let mut delete_relations: Vec<Uuid> = Vec::new();
        let mut unset_relations: Vec<UnsetRelationItem> = Vec::new();

        for relation in &squashed {
            match relation {
                RelationItem::Create(relation) => set_relations.push(relation.clone()),
                RelationItem::Update(relation) => update_relations.push(relation.clone()),
                RelationItem::Delete(relation) => delete_relations.push(relation.id),
                RelationItem::Unset(relation) => unset_relations.push(relation.clone()),
            }
        }

        debug!(
            set_count = set_relations.len(),
            update_count = update_relations.len(),
            unset_count = unset_relations.len(),
            delete_count = delete_relations.len(),
            "Processed relation operations"
        );
        
        return (
            set_relations,
            update_relations,
            unset_relations,
            delete_relations,
        );
    }
}

fn squash_relations(relation_ops: &Vec<RelationItem>) -> Vec<RelationItem> {
    let mut hash: HashMap<Uuid, RelationItem> = HashMap::new();

    for op in relation_ops {
        let seen = hash.get(op.id());
        if let Some(existing) = seen {
            let merged = match (existing.clone(), op.clone()) {
                // create -> create: Overwrite with 2nd create enum
                (RelationItem::Create { .. }, RelationItem::Create { .. }) => op.clone(),

                // create -> update: Attempt to merge optional fields into a create enum
                (
                    RelationItem::Create(SetRelationItem {
                        id,
                        entity_id,
                        type_id,
                        from_id,
                        to_id,
                        from_space_id: e_from_space_id,
                        from_version_id: e_from_version_id,
                        to_space_id: e_to_space_id,
                        to_version_id: e_to_version_id,
                        position: e_position,
                        verified: e_verified,
                        ..
                    }),
                    RelationItem::Update(UpdateRelationItem {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    }),
                ) => RelationItem::Create(SetRelationItem {
                    id,
                    entity_id,
                    type_id,
                    from_id,
                    to_id,
                    from_space_id: u_from_space_id.or(e_from_space_id),
                    from_version_id: u_from_version_id.or(e_from_version_id),
                    to_space_id: u_to_space_id.or(e_to_space_id),
                    to_version_id: u_to_version_id.or(e_to_version_id),
                    position: u_position.or(e_position),
                    space_id: u_space_id,
                    verified: u_verified.or(e_verified),
                }),

                // create -> delete: Overwrite with delete enum
                (RelationItem::Create { .. }, RelationItem::Delete { .. }) => op.clone(),

                // create -> unset: Attempt to merge optional fields into create enum
                (
                    RelationItem::Create(SetRelationItem {
                        id,
                        entity_id,
                        type_id,
                        from_id,
                        to_id,
                        from_space_id: e_from_space_id,
                        from_version_id: e_from_version_id,
                        to_space_id: e_to_space_id,
                        to_version_id: e_to_version_id,
                        position: e_position,
                        verified: e_verified,
                        ..
                    }),
                    RelationItem::Unset(UnsetRelationItem {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    }),
                ) => RelationItem::Create(SetRelationItem {
                    id,
                    entity_id,
                    type_id,
                    from_id,
                    to_id,
                    from_space_id: if u_from_space_id == Some(true) {
                        None
                    } else {
                        e_from_space_id
                    },
                    from_version_id: if u_from_version_id == Some(true) {
                        None
                    } else {
                        e_from_version_id
                    },
                    to_space_id: if u_to_space_id == Some(true) {
                        None
                    } else {
                        e_to_space_id
                    },
                    to_version_id: if u_to_version_id == Some(true) {
                        None
                    } else {
                        e_to_version_id
                    },
                    position: if u_position == Some(true) {
                        None
                    } else {
                        e_position
                    },
                    space_id: u_space_id,
                    verified: if u_verified == Some(true) {
                        None
                    } else {
                        e_verified
                    },
                }),

                // update -> create: Overwrite with create enum
                (RelationItem::Update { .. }, RelationItem::Create { .. }) => op.clone(),

                // update -> update: Attempt to merge optional fields into update enum
                (
                    RelationItem::Update(UpdateRelationItem {
                        id,
                        from_space_id: e_from_space_id,
                        from_version_id: e_from_version_id,
                        to_space_id: e_to_space_id,
                        to_version_id: e_to_version_id,
                        position: e_position,
                        verified: e_verified,
                        ..
                    }),
                    RelationItem::Update(UpdateRelationItem {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    }),
                ) => RelationItem::Update(UpdateRelationItem {
                    id,
                    from_space_id: u_from_space_id.or(e_from_space_id),
                    from_version_id: u_from_version_id.or(e_from_version_id),
                    to_space_id: u_to_space_id.or(e_to_space_id),
                    to_version_id: u_to_version_id.or(e_to_version_id),
                    position: u_position.or(e_position),
                    space_id: u_space_id,
                    verified: u_verified.or(e_verified),
                }),

                // update -> delete: Overwrite with delete enum
                (RelationItem::Update { .. }, RelationItem::Delete { .. }) => op.clone(),

                // update -> unset: Attempt to merge optional fields into update enum
                (
                    RelationItem::Update(UpdateRelationItem {
                        id,
                        from_space_id: e_from_space_id,
                        from_version_id: e_from_version_id,
                        to_space_id: e_to_space_id,
                        to_version_id: e_to_version_id,
                        position: e_position,
                        verified: e_verified,
                        ..
                    }),
                    RelationItem::Unset(UnsetRelationItem {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    }),
                ) => RelationItem::Update(UpdateRelationItem {
                    id,
                    from_space_id: if u_from_space_id == Some(true) {
                        None
                    } else {
                        e_from_space_id
                    },
                    from_version_id: if u_from_version_id == Some(true) {
                        None
                    } else {
                        e_from_version_id
                    },
                    to_space_id: if u_to_space_id == Some(true) {
                        None
                    } else {
                        e_to_space_id
                    },
                    to_version_id: if u_to_version_id == Some(true) {
                        None
                    } else {
                        e_to_version_id
                    },
                    position: if u_position == Some(true) {
                        None
                    } else {
                        e_position
                    },
                    space_id: u_space_id,
                    verified: if u_verified == Some(true) {
                        None
                    } else {
                        e_verified
                    },
                }),

                // delete -> create: Overwrite with create enum
                (RelationItem::Delete { .. }, RelationItem::Create { .. }) => op.clone(),

                // delete -> update: Overwrite with update enum
                (RelationItem::Delete { .. }, RelationItem::Update { .. }) => op.clone(),

                // delete -> unset: Keep delete enum (unset on deleted item is no-op)
                (RelationItem::Delete { .. }, RelationItem::Unset { .. }) => existing.clone(),

                // delete -> delete: Skip (keep existing delete)
                (RelationItem::Delete { .. }, RelationItem::Delete { .. }) => existing.clone(),

                // Handle any remaining combinations (shouldn't occur with proper enum design)
                _ => op.clone(),
            };

            hash.insert(*op.id(), merged);
        } else {
            hash.insert(*op.id(), op.clone());
        }
    }

    hash.into_values().collect()
}
