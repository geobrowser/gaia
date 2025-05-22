use std::collections::HashMap;

use grc20::pb::ipfsv2::{op::Payload, Edit};

#[derive(Clone)]
pub struct SetRelationItem {
    pub id: String,
    pub entity_id: String,
    pub type_id: String,
    pub from_id: String,
    pub from_space_id: Option<String>,
    pub from_version_id: Option<String>,
    pub to_id: String,
    pub to_space_id: Option<String>,
    pub to_version_id: Option<String>,
    pub position: Option<String>,
    pub space_id: String,
    pub verified: Option<bool>,
}

#[derive(Clone)]
pub struct UpdateRelationItem {
    pub id: String,
    pub from_space_id: Option<String>,
    pub from_version_id: Option<String>,
    pub to_space_id: Option<String>,
    pub to_version_id: Option<String>,
    pub position: Option<String>,
    pub space_id: String,
    pub verified: Option<bool>,
}

#[derive(Clone)]
pub struct UnsetRelationItem {
    pub id: String,
    pub from_space_id: Option<bool>,
    pub from_version_id: Option<bool>,
    pub to_space_id: Option<bool>,
    pub to_version_id: Option<bool>,
    pub position: Option<bool>,
    pub space_id: String,
    pub verified: Option<bool>,
}

#[derive(Clone)]
pub struct DeleteRelationItem {
    pub id: String,
    pub space_id: String,
}

#[derive(Clone)]
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
    pub fn id(&self) -> &str {
        match self {
            RelationItem::Create(item) => &item.id,
            RelationItem::Update(item) => &item.id,
            RelationItem::Unset(item) => &item.id,
            RelationItem::Delete(item) => &item.id,
        }
    }

    /// Get the space_id field, present in all variants
    pub fn space_id(&self) -> &str {
        match self {
            RelationItem::Create(item) => &item.space_id,
            RelationItem::Update(item) => &item.space_id,
            RelationItem::Unset(item) => &item.space_id,
            RelationItem::Delete(item) => &item.space_id,
        }
    }

    /// Get entity_id (only available in Set variant)
    pub fn entity_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => Some(&item.entity_id),
            _ => None,
        }
    }

    /// Get type_id (only available in Set variant)
    pub fn type_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => Some(&item.type_id),
            _ => None,
        }
    }

    /// Get from_id (only available in Set variant)
    pub fn from_id(&self) -> Option<&str> {
        match self {
            RelationItem::Create(item) => Some(&item.from_id),
            _ => None,
        }
    }

    /// Get to_id (only available in Set variant)
    pub fn to_id(&self) -> Option<&str> {
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
    pub fn map_edit_to_relations(
        edit: &Edit,
        space_id: &String,
    ) -> (
        Vec<SetRelationItem>,
        Vec<UpdateRelationItem>,
        Vec<UnsetRelationItem>,
        Vec<String>,
    ) {
        let mut relations = Vec::new();

        for op in &edit.ops {
            if let Some(op_type) = &op.payload {
                match op_type {
                    Payload::CreateRelation(relation) => {
                        let relation_id = String::from_utf8(relation.id.clone());
                        let entity_id = String::from_utf8(relation.entity.clone());
                        let type_id = String::from_utf8(relation.r#type.clone());
                        let from_id = String::from_utf8(relation.from_entity.clone());
                        let to_id = String::from_utf8(relation.to_entity.clone());

                        let to_space = relation
                            .to_space
                            .clone()
                            .and_then(|s| String::from_utf8(s).ok());

                        if relation_id.is_ok()
                            && entity_id.is_ok()
                            && from_id.is_ok()
                            && to_id.is_ok()
                            && type_id.is_ok()
                        {
                            relations.push(RelationItem::Create(SetRelationItem {
                                id: relation_id.unwrap(),
                                entity_id: entity_id.unwrap(),
                                space_id: space_id.clone(),
                                position: relation.position.clone(),
                                type_id: type_id.unwrap().to_string(),
                                from_id: from_id.unwrap().to_string(),
                                from_space_id: None,
                                from_version_id: None,
                                to_id: to_id.unwrap().to_string(),
                                to_space_id: to_space,
                                to_version_id: None,
                                verified: relation.verified,
                            }));
                        }
                    }
                    Payload::DeleteRelation(relation_id) => {
                        if let Ok(relation_id) = String::from_utf8(relation_id.clone()) {
                            relations.push(RelationItem::Delete(DeleteRelationItem {
                                id: relation_id,
                                space_id: space_id.clone(),
                            }));
                        }
                    }
                    Payload::UpdateRelation(updated_relation) => {
                        let to_space = updated_relation
                            .to_space
                            .clone()
                            .and_then(|s| String::from_utf8(s).ok());

                        let from_space = updated_relation
                            .from_space
                            .clone()
                            .and_then(|s| String::from_utf8(s).ok());

                        if let Ok(relation_id) = String::from_utf8(updated_relation.id.clone()) {
                            relations.push(RelationItem::Update(UpdateRelationItem {
                                id: relation_id,
                                space_id: space_id.clone(),
                                position: updated_relation.position.clone(),
                                verified: updated_relation.verified.clone(),
                                to_space_id: to_space,
                                from_space_id: from_space,
                                from_version_id: None,
                                to_version_id: None,
                            }));
                        }
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
        let squashed = squash_relations(&relations);

        let mut set_relations: Vec<SetRelationItem> = Vec::new();
        let mut update_relations: Vec<UpdateRelationItem> = Vec::new();
        let mut delete_relations: Vec<String> = Vec::new();
        let mut unset_relations: Vec<UnsetRelationItem> = Vec::new();

        for relation in &squashed {
            match relation {
                RelationItem::Create(relation) => set_relations.push(relation.clone()),
                RelationItem::Update(relation) => update_relations.push(relation.clone()),
                RelationItem::Delete(relation) => delete_relations.push(relation.id.clone()),
                RelationItem::Unset(relation) => unset_relations.push(relation.clone()),
            }
        }

        return (
            set_relations,
            update_relations,
            unset_relations,
            delete_relations,
        );
    }
}

fn squash_relations(relation_ops: &Vec<RelationItem>) -> Vec<RelationItem> {
    let mut hash: HashMap<String, RelationItem> = HashMap::new();

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

            hash.insert(op.id().to_string(), merged);
        } else {
            hash.insert(op.id().to_string(), op.clone());
        }
    }

    hash.into_values().collect()
}
