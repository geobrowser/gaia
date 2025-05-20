use std::collections::HashMap;

use grc20::pb::ipfsv2::{op::Payload, Edit};

#[derive(Clone)]
pub enum RelationChangeType {
    SET,
    UPDATE,
    DELETE,
}

#[derive(Clone)]
pub struct RelationItem {
    pub change_type: RelationChangeType,
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
                            relations.push(RelationItem {
                                change_type: RelationChangeType::SET,
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
                            relations.push(RelationItem {
                                change_type: RelationChangeType::DELETE,
                                id: relation_id,
                                space_id: space_id.clone(),

                                // These fields don't matter for a delete
                                entity_id: String::from(""),
                                position: None,
                                type_id: String::from(""),
                                from_id: String::from(""),
                                to_id: String::from(""),
                                to_space_id: None,
                                from_space_id: None,
                                from_version_id: None,
                                to_version_id: None,
                                verified: None,
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

                        if let Ok(relation_id) =
                            String::from_utf8(updated_relation.relation_id.clone())
                        {
                            relations.push(RelationItem {
                                change_type: RelationChangeType::UPDATE,
                                id: relation_id,
                                space_id: space_id.clone(),
                                position: updated_relation.position.clone(),
                                verified: updated_relation.verified.clone(),
                                to_space_id: to_space,
                                from_space_id: from_space,
                                from_version_id: None,
                                to_version_id: None,

                                // These fields don't matter for a delete
                                entity_id: String::from(""),
                                type_id: String::from(""),
                                from_id: String::from(""),
                                to_id: String::from(""),
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
            match relation.change_type {
                RelationChangeType::SET => set_relations.push(relation.clone()),
                RelationChangeType::UPDATE => update_relations.push(relation.clone()),
                RelationChangeType::DELETE => delete_relations.push(relation.id.clone()),
            }
        }

        return (set_relations, update_relations, delete_relations);
    }
}

fn squash_relations(relation_ops: &Vec<RelationItem>) -> Vec<RelationItem> {
    let mut hash: HashMap<String, RelationItem> = HashMap::new();

    for op in relation_ops {
        let seen = hash.get(&op.id);

        if let Some(existing) = seen {
            match (existing.change_type.clone(), op.change_type.clone()) {
                // create -> create: Overwrite with 2nd create
                (RelationChangeType::SET, RelationChangeType::SET) => {
                    hash.insert(op.id.clone(), op.clone());
                }
                // create -> update: Attempt to merge
                (RelationChangeType::SET, RelationChangeType::UPDATE) => {
                    let mut merged = existing.clone();
                    merged.from_space_id = op.from_space_id.clone();
                    merged.from_version_id = op.from_version_id.clone();
                    merged.to_space_id = op.to_space_id.clone();
                    merged.to_version_id = op.to_version_id.clone();
                    merged.position = op.position.clone();
                    merged.verified = op.verified;
                    hash.insert(op.id.clone(), merged);
                }
                // create -> delete: Overwrite with delete
                (RelationChangeType::SET, RelationChangeType::DELETE) => {
                    hash.insert(op.id.clone(), op.clone());
                }
                // update -> create: Overwrite with create
                (RelationChangeType::UPDATE, RelationChangeType::SET) => {
                    hash.insert(op.id.clone(), op.clone());
                }
                // update -> delete: Overwrite with delete
                (RelationChangeType::UPDATE, RelationChangeType::DELETE) => {
                    hash.insert(op.id.clone(), op.clone());
                }
                (RelationChangeType::UPDATE, RelationChangeType::UPDATE) => {
                    let mut merged = existing.clone();
                    merged.from_space_id = op.from_space_id.clone();
                    merged.from_version_id = op.from_version_id.clone();
                    merged.to_space_id = op.to_space_id.clone();
                    merged.to_version_id = op.to_version_id.clone();
                    merged.position = op.position.clone();
                    merged.verified = op.verified;
                    hash.insert(op.id.clone(), merged);
                }
                // delete -> create: Overwrite with create
                (RelationChangeType::DELETE, RelationChangeType::SET) => {
                    hash.insert(op.id.clone(), op.clone());
                }

                // delete -> update: Overwrite with update
                (RelationChangeType::DELETE, RelationChangeType::UPDATE) => {
                    // This is technically an error case as we can't update a deleted item
                    // But the requirement says to overwrite with update
                    hash.insert(op.id.clone(), op.clone());
                }

                // delete -> delete: Skip (to not write to memory again)
                (RelationChangeType::DELETE, RelationChangeType::DELETE) => {
                    // Do nothing - keep the existing delete
                }
            }
        } else {
            hash.insert(op.id.clone(), op.clone());
        }
    }

    return hash.into_values().collect();
}
