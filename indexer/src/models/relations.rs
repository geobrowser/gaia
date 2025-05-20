use grc20::pb::ipfsv2::{op::Payload, Edit};

#[derive(Clone)]
pub enum RelationChangeType {
    SET,
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
    ) -> (Vec<RelationItem>, Vec<String>) {
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
                    _ => {}
                }
            }
        }

        let (created, deleted): (Vec<RelationItem>, Vec<RelationItem>) = relations
            .into_iter()
            .partition(|op| matches!(op.change_type, RelationChangeType::SET));

        return (created, deleted.iter().map(|op| op.id.clone()).collect());
    }
}
