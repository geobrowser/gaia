use std::collections::HashSet;

use indexer_utils::id;
use stream::utils::BlockMetadata;
use uuid::Uuid;
use wire::pb::grc20::op::Payload;
use wire::pb::grc20::Edit;

#[derive(Clone)]
pub struct EntityItem {
    pub id: Uuid,
    pub created_at: String,
    pub created_at_block: String,
    pub updated_at: String,
    pub updated_at_block: String,
}

pub struct EntitiesModel;

impl EntitiesModel {
    pub fn map_edit_to_entities(edit: &Edit, block: &BlockMetadata) -> Vec<EntityItem> {
        let mut entities: Vec<EntityItem> = Vec::new();
        let mut seen: HashSet<Uuid> = HashSet::new();

        for op in &edit.ops {
            if let Some(payload) = &op.payload {
                match payload {
                    Payload::UpdateEntity(entity) => {
                        let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                        if let Err(_) = entity_id_bytes {
                            tracing::error!(
                                "[Entities][UpdateEntity] Could not transform Vec<u8> for entity.id {:?}",
                                &entity.id
                            );
                            continue;
                        }

                        let entity_id = Uuid::from_bytes(entity_id_bytes.unwrap());

                        if !seen.contains(&entity_id) {
                            entities.push(EntityItem {
                                id: entity_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(entity_id);
                        }

                        for value in &entity.values {
                            let property_id_bytes = id::transform_id_bytes(value.property.clone());

                            if let Err(_) = property_id_bytes {
                                tracing::error!(
                                    "[Entities][UpdateEntity] Could not transform Vec<u8> for property.id {:?}",
                                    &entity.id
                                );
                                continue;
                            }

                            let property_id = Uuid::from_bytes(property_id_bytes.unwrap());

                            if !seen.contains(&property_id) {
                                entities.push(EntityItem {
                                    id: property_id,
                                    created_at: block.timestamp.clone(),
                                    created_at_block: block.block_number.to_string(),
                                    updated_at: block.timestamp.clone(),
                                    updated_at_block: block.block_number.to_string(),
                                });

                                seen.insert(property_id);
                            }
                        }
                    }
                    Payload::UnsetEntityValues(entity) => {
                        let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                        if let Err(_) = entity_id_bytes {
                            tracing::error!(
                                "[Entities][UnsetEntityValues] Could not transform Vec<u8> for entity.id {:?}",
                                &entity.id
                            );
                            continue;
                        }

                        let entity_id = Uuid::from_bytes(entity_id_bytes.unwrap());

                        if !seen.contains(&entity_id) {
                            entities.push(EntityItem {
                                id: entity_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(entity_id);
                        }

                        for property in &entity.properties {
                            let property_id_bytes = id::transform_id_bytes(property.clone());

                            if let Err(_) = property_id_bytes {
                                tracing::error!(
                                    "[Entities][UnsetEntityValues] Could not transform Vec<u8> for property id {:?}",
                                    &property_id_bytes
                                );
                                continue;
                            }

                            let property_id = Uuid::from_bytes(property_id_bytes.unwrap());

                            if !seen.contains(&property_id) {
                                entities.push(EntityItem {
                                    id: property_id,
                                    created_at: block.timestamp.clone(),
                                    created_at_block: block.block_number.to_string(),
                                    updated_at: block.timestamp.clone(),
                                    updated_at_block: block.block_number.to_string(),
                                });

                                seen.insert(property_id);
                            }
                        }
                    }
                    Payload::CreateRelation(relation) => {
                        let relation_id_bytes = id::transform_id_bytes(relation.id.clone());

                        if let Err(_) = relation_id_bytes {
                            tracing::error!(
                                "[Entities][CreateRelation] Could not transform Vec<u8> for relation.id {:?}",
                                &relation.id
                            );
                            continue;
                        }

                        let relation_id = Uuid::from_bytes(relation_id_bytes.unwrap());

                        if !seen.contains(&relation_id) {
                            entities.push(EntityItem {
                                id: relation_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_id);
                        }

                        let relation_entity_id_bytes =
                            id::transform_id_bytes(relation.entity.clone());

                        if let Err(_) = relation_entity_id_bytes {
                            tracing::error!(
                                "[Entities][CreateRelation] Could not transform Vec<u8> for relation.entity {:?}",
                                &relation.entity
                            );
                            continue;
                        }

                        let relation_entity_id =
                            Uuid::from_bytes(relation_entity_id_bytes.unwrap());

                        if !seen.contains(&relation_entity_id) {
                            entities.push(EntityItem {
                                id: relation_entity_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_entity_id);
                        }

                        let type_id_bytes = id::transform_id_bytes(relation.r#type.clone());

                        if let Err(_) = type_id_bytes {
                            tracing::error!(
                                "[Entities][CreateRelation] Could not transform Vec<u8> for relation.type {:?}",
                                &relation.r#type
                            );
                            continue;
                        }

                        let type_id = Uuid::from_bytes(type_id_bytes.unwrap());

                        if !seen.contains(&type_id) {
                            entities.push(EntityItem {
                                id: type_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(type_id);
                        }

                        let from_id_bytes = id::transform_id_bytes(relation.from_entity.clone());

                        if let Err(_) = from_id_bytes {
                            tracing::error!(
                                "[Entities][CreateRelation] Could not transform Vec<u8> for relation.from_entity {:?}",
                                &relation.from_entity
                            );
                            continue;
                        }

                        let from_id = Uuid::from_bytes(from_id_bytes.unwrap());

                        if !seen.contains(&from_id) {
                            entities.push(EntityItem {
                                id: from_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(from_id);
                        }

                        let to_id_bytes = id::transform_id_bytes(relation.to_entity.clone());

                        if let Err(_) = to_id_bytes {
                            tracing::error!(
                                "[Entities][CreateRelation] Could not transform Vec<u8> for relation.to_entity {:?}",
                                &relation.to_entity
                            );
                            continue;
                        }

                        let to_id = Uuid::from_bytes(to_id_bytes.unwrap());

                        if !seen.contains(&to_id) {
                            entities.push(EntityItem {
                                id: to_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(to_id);
                        }
                    }
                    Payload::DeleteRelation(relation_id) => {
                        let relation_id_bytes = id::transform_id_bytes(relation_id.clone());

                        if let Err(_) = relation_id_bytes {
                            tracing::error!(
                                "[Entities][DeleteRelation] Could not transform Vec<u8> for relation.id {:?}",
                                &relation_id_bytes
                            );
                            continue;
                        }

                        let relation_id = Uuid::from_bytes(relation_id_bytes.unwrap());

                        if !seen.contains(&relation_id) {
                            entities.push(EntityItem {
                                id: relation_id,
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_id);
                        }
                    }
                    // @TODO: Payload::UpdateRelation(relation)
                    // @TODO: Payload::UnsetRelationFields(relation)
                    _ => {
                        //
                    }
                }
            }
        }

        return entities;
    }
}
