use std::collections::HashSet;

use grc20::pb::ipfsv2::{Edit, OpType};
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
            if let Ok(op_type) = OpType::try_from(op.r#type) {
                match op_type {
                    OpType::UpdateEntity | OpType::UnsetProperties | OpType::CreateEntity => {
                        if op.entity.is_some() {
                            let entity = op.entity.clone().unwrap();
                            let entity_id = String::from_utf8(entity.id);

                            if let Ok(id) = entity_id {
                                if !seen.contains(&id) {
                                    seen.insert(id.clone());
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });
                                }
                            }

                            for value in &entity.values {
                                let property_id = String::from_utf8(value.property_id.clone());

                                if let Ok(id) = property_id {
                                    if !seen.contains(&id) {
                                        entities.push(EntityItem {
                                            id: id.clone(),
                                            created_at: block.timestamp.clone(),
                                            created_at_block: block.block_number.to_string(),
                                            updated_at: block.timestamp.clone(),
                                            updated_at_block: block.block_number.to_string(),
                                        });

                                        seen.insert(id.clone());
                                    }
                                }
                            }
                        }
                    }
                    OpType::CreateRelation => {
                        if op.relation.is_some() {
                            let relation = op.relation.clone().unwrap();

                            let relation_id = String::from_utf8(relation.id.clone());
                            let relation_entity_id = String::from_utf8(relation.entity.clone());
                            let from_id = String::from_utf8(relation.from_entity.clone());
                            let to_id = String::from_utf8(relation.to_entity.clone());
                            let type_id = String::from_utf8(relation.r#type.clone());

                            if !relation_id.is_ok()
                                && !relation_entity_id.is_ok()
                                && !from_id.is_ok()
                                && !to_id.is_ok()
                                && !type_id.is_ok()
                            {
                                continue;
                            }

                            if let Ok(id) = relation_id {
                                if !seen.contains(&id) {
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });

                                    seen.insert(id);
                                }
                            }

                            if let Ok(id) = relation_entity_id {
                                if !seen.contains(&id) {
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });

                                    seen.insert(id);
                                }
                            }

                            if let Ok(id) = from_id {
                                if !seen.contains(&id) {
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });

                                    seen.insert(id);
                                }
                            }

                            if let Ok(id) = to_id {
                                if !seen.contains(&id) {
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });

                                    seen.insert(id);
                                }
                            }

                            if let Ok(id) = type_id {
                                if !seen.contains(&id) {
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });

                                    seen.insert(id);
                                }
                            }
                        }
                    }
                    OpType::DeleteRelation => {
                        if op.relation.is_some() {
                            let relation = op.relation.clone().unwrap();
                            let relation_id = String::from_utf8(relation.id.clone());

                            if let Ok(id) = relation_id {
                                if !seen.contains(&id) {
                                    entities.push(EntityItem {
                                        id: id.clone(),
                                        created_at: block.timestamp.clone(),
                                        created_at_block: block.block_number.to_string(),
                                        updated_at: block.timestamp.clone(),
                                        updated_at_block: block.block_number.to_string(),
                                    });

                                    seen.insert(id);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        return entities;
    }
}
