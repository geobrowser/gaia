use std::collections::HashSet;

use grc20::pb::ipfs::Edit;
use stream::utils::BlockMetadata;

#[derive(Clone)]
pub struct EntityItem {
    pub id: String,
    pub created_at: String,
    pub created_at_block: String,
    pub updated_at: String,
    pub updated_at_block: String,
}

pub struct EntitiesModel;

impl EntitiesModel {
    pub fn map_edit_to_entities(edit: &Edit, block: &BlockMetadata) -> Vec<EntityItem> {
        let mut entities: Vec<EntityItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for op in &edit.ops {
            match op.r#type {
                // SET_TRIPLE
                1 => {
                    if op.triple.is_some() {
                        let triple = op.triple.clone().unwrap();
                        let entity_id = triple.entity.clone();
                        let attribute_id = triple.attribute.clone();

                        if !seen.contains(&entity_id) {
                            entities.push(EntityItem {
                                id: entity_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(entity_id);
                        }

                        if !seen.contains(&attribute_id) {
                            entities.push(EntityItem {
                                id: attribute_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(attribute_id);
                        }
                    }
                }
                // DELETE_TRIPLE
                2 => {
                    if op.triple.is_some() {
                        let triple = op.triple.clone().unwrap();
                        let entity_id = triple.entity.clone();
                        let attribute_id = triple.attribute.clone();

                        if !seen.contains(&entity_id) {
                            entities.push(EntityItem {
                                id: entity_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(entity_id);
                        }

                        if !seen.contains(&attribute_id) {
                            entities.push(EntityItem {
                                id: attribute_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(attribute_id);
                        }
                    }
                }
                // CREATE_RELATION
                5 => {
                    if op.relation.is_some() {
                        let relation = op.relation.clone().unwrap();

                        let relation_id = relation.id.clone();
                        let from_id = relation.from_entity.clone();
                        let to_id = relation.to_entity.clone();
                        let type_id = relation.r#type.clone();

                        if !seen.contains(&relation_id) {
                            entities.push(EntityItem {
                                id: relation_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_id);
                        }

                        if !seen.contains(&from_id) {
                            entities.push(EntityItem {
                                id: from_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(from_id);
                        }

                        if !seen.contains(&to_id) {
                            entities.push(EntityItem {
                                id: to_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(to_id);
                        }

                        if !seen.contains(&type_id) {
                            entities.push(EntityItem {
                                id: type_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(type_id);
                        }
                    }
                }
                // DELETE_RELATION
                6 => {
                    if op.relation.is_some() {
                        let relation = op.relation.clone().unwrap();
                        let relation_id = relation.id.clone();

                        if !seen.contains(&relation_id) {
                            entities.push(EntityItem {
                                id: relation_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_id);
                        }
                    }
                }
                _ => {}
            }
        }

        return entities;
    }
}
