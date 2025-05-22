use std::collections::HashMap;

use grc20::pb::ipfsv2::{op::Payload, Edit};

#[derive(Clone)]
pub enum RelationItem {
    Set {
        id: String,
        entity_id: String,
        type_id: String,
        from_id: String,
        from_space_id: Option<String>,
        from_version_id: Option<String>,
        to_id: String,
        to_space_id: Option<String>,
        to_version_id: Option<String>,
        position: Option<String>,
        space_id: String,
        verified: Option<bool>,
    },
    Update {
        id: String,
        from_space_id: Option<String>,
        from_version_id: Option<String>,
        to_space_id: Option<String>,
        to_version_id: Option<String>,
        position: Option<String>,
        space_id: String,
        verified: Option<bool>,
    },
    Unset {
        id: String,
        from_space_id: Option<bool>,
        from_version_id: Option<bool>,
        to_space_id: Option<bool>,
        to_version_id: Option<bool>,
        position: Option<bool>,
        space_id: String,
        verified: Option<bool>,
    },
    Delete {
        id: String,
        space_id: String,
    },
}

impl RelationItem {
    pub fn id(&self) -> &str {
        match self {
            RelationItem::Set { id, .. } => id,
            RelationItem::Update { id, .. } => id,
            RelationItem::Unset { id, .. } => id,
            RelationItem::Delete { id, .. } => id,
        }
    }

    pub fn entity_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { entity_id, .. } => Some(entity_id),
            _ => None,
        }
    }

    pub fn type_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { type_id, .. } => Some(type_id),
            _ => None,
        }
    }

    pub fn from_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { from_id, .. } => Some(from_id),
            _ => None,
        }
    }

    pub fn from_space_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { from_space_id, .. } => from_space_id.as_deref(),
            RelationItem::Update { from_space_id, .. } => from_space_id.as_deref(),
            _ => None,
        }
    }

    pub fn from_version_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set {
                from_version_id, ..
            } => from_version_id.as_deref(),
            RelationItem::Update {
                from_version_id, ..
            } => from_version_id.as_deref(),
            _ => None,
        }
    }

    pub fn to_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { to_id, .. } => Some(to_id),
            _ => None,
        }
    }

    pub fn to_space_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { to_space_id, .. } => to_space_id.as_deref(),
            RelationItem::Update { to_space_id, .. } => to_space_id.as_deref(),
            _ => None,
        }
    }

    pub fn to_version_id(&self) -> Option<&str> {
        match self {
            RelationItem::Set { to_version_id, .. } => to_version_id.as_deref(),
            RelationItem::Update { to_version_id, .. } => to_version_id.as_deref(),
            _ => None,
        }
    }

    pub fn position(&self) -> Option<&str> {
        match self {
            RelationItem::Set { position, .. } => position.as_deref(),
            RelationItem::Update { position, .. } => position.as_deref(),
            _ => None,
        }
    }

    pub fn space_id(&self) -> &str {
        match self {
            RelationItem::Set { space_id, .. } => space_id,
            RelationItem::Update { space_id, .. } => space_id,
            RelationItem::Unset { space_id, .. } => space_id,
            RelationItem::Delete { space_id, .. } => space_id,
        }
    }

    pub fn verified(&self) -> Option<bool> {
        match self {
            RelationItem::Set { verified, .. } => *verified,
            RelationItem::Update { verified, .. } => *verified,
            RelationItem::Unset { verified, .. } => *verified,
            _ => None,
        }
    }
}

pub struct RelationsModel;

impl RelationsModel {
    pub fn map_edit_to_relations(
        edit: &Edit,
        space_id: &String,
    ) -> (Vec<RelationItem>, Vec<RelationItem>, Vec<String>) {
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

                        // @TODO: What do we do with the optional fields?
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
                            relations.push(RelationItem::Set {
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
                            });
                        }
                    }
                    Payload::DeleteRelation(relation_id) => {
                        if let Ok(relation_id) = String::from_utf8(relation_id.clone()) {
                            relations.push(RelationItem::Delete {
                                id: relation_id,
                                space_id: space_id.clone(),
                            });
                        }
                    }
                    Payload::UpdateRelation(updated_relation) => {
                        // @TODO: What do we do with the optional fields?
                        let to_space = updated_relation
                            .to_space
                            .clone()
                            .and_then(|s| String::from_utf8(s).ok());

                        let from_space = updated_relation
                            .from_space
                            .clone()
                            .and_then(|s| String::from_utf8(s).ok());

                        if let Ok(relation_id) = String::from_utf8(updated_relation.id.clone()) {
                            relations.push(RelationItem::Update {
                                id: relation_id,
                                space_id: space_id.clone(),
                                position: updated_relation.position.clone(),
                                verified: updated_relation.verified.clone(),
                                to_space_id: to_space,
                                from_space_id: from_space,
                                from_version_id: None,
                                to_version_id: None,
                            });
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

        let mut set_relations = Vec::new();
        let mut update_relations = Vec::new();
        let mut delete_relations = Vec::new();

        for relation in &squashed {
            match relation {
                RelationItem::Set { .. } => set_relations.push(relation.clone()),
                RelationItem::Update { .. } => update_relations.push(relation.clone()),
                RelationItem::Delete { id, .. } => delete_relations.push(id.clone()),
                RelationItem::Unset { .. } => {}
            }
        }

        return (set_relations, update_relations, delete_relations);
    }
}

fn squash_relations(relation_ops: &Vec<RelationItem>) -> Vec<RelationItem> {
    let mut hash: HashMap<String, RelationItem> = HashMap::new();

    for op in relation_ops {
        let seen = hash.get(op.id());
        if let Some(existing) = seen {
            let merged = match (existing.clone(), op.clone()) {
                // create -> create: Overwrite with 2nd create enum
                (RelationItem::Set { .. }, RelationItem::Set { .. }) => op.clone(),

                // create -> update: Attempt to merge optional fields into a create enum
                (
                    RelationItem::Set {
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
                    },
                    RelationItem::Update {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    },
                ) => RelationItem::Set {
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
                },

                // create -> delete: Overwrite with delete enum
                (RelationItem::Set { .. }, RelationItem::Delete { .. }) => op.clone(),

                // create -> unset: Attempt to merge optional fields into create enum
                (
                    RelationItem::Set {
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
                    },
                    RelationItem::Unset {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    },
                ) => RelationItem::Set {
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
                },

                // update -> create: Overwrite with create enum
                (RelationItem::Update { .. }, RelationItem::Set { .. }) => op.clone(),

                // update -> update: Attempt to merge optional fields into update enum
                (
                    RelationItem::Update {
                        id,
                        from_space_id: e_from_space_id,
                        from_version_id: e_from_version_id,
                        to_space_id: e_to_space_id,
                        to_version_id: e_to_version_id,
                        position: e_position,
                        verified: e_verified,
                        ..
                    },
                    RelationItem::Update {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    },
                ) => RelationItem::Update {
                    id,
                    from_space_id: u_from_space_id.or(e_from_space_id),
                    from_version_id: u_from_version_id.or(e_from_version_id),
                    to_space_id: u_to_space_id.or(e_to_space_id),
                    to_version_id: u_to_version_id.or(e_to_version_id),
                    position: u_position.or(e_position),
                    space_id: u_space_id,
                    verified: u_verified.or(e_verified),
                },

                // update -> delete: Overwrite with delete enum
                (RelationItem::Update { .. }, RelationItem::Delete { .. }) => op.clone(),

                // update -> unset: Attempt to merge optional fields into update enum
                (
                    RelationItem::Update {
                        id,
                        from_space_id: e_from_space_id,
                        from_version_id: e_from_version_id,
                        to_space_id: e_to_space_id,
                        to_version_id: e_to_version_id,
                        position: e_position,
                        verified: e_verified,
                        ..
                    },
                    RelationItem::Unset {
                        from_space_id: u_from_space_id,
                        from_version_id: u_from_version_id,
                        to_space_id: u_to_space_id,
                        to_version_id: u_to_version_id,
                        position: u_position,
                        space_id: u_space_id,
                        verified: u_verified,
                        ..
                    },
                ) => RelationItem::Update {
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
                },

                // delete -> create: Overwrite with create enum
                (RelationItem::Delete { .. }, RelationItem::Set { .. }) => op.clone(),

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
