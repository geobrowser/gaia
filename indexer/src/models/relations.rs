use grc20::pb::ipfsv2::{Edit, OpType};

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
    pub from_property_id: Option<String>,
    pub to_id: String,
    pub to_space_id: Option<String>,
    pub index: Option<String>,
    pub space_id: String,
}

pub struct RelationsModel;

impl RelationsModel {
    pub fn map_edit_to_relations(
        edit: &Edit,
        space_id: &String,
    ) -> (Vec<RelationItem>, Vec<String>) {
        let mut relations = Vec::new();

        for op in &edit.ops {
            if let Ok(op_type) = OpType::try_from(op.r#type) {
                match op_type {
                    OpType::CreateRelation => {
                        if op.relation.is_some() {
                            let relation = op.relation.clone().unwrap();

                            let relation_id = String::from_utf8(relation.id);
                            let entity_id = String::from_utf8(relation.entity);
                            let type_id = String::from_utf8(relation.r#type);
                            let from_id = String::from_utf8(relation.from_entity);
                            let to_id = String::from_utf8(relation.to_entity);

                            // @TODO: What do we do with the optional fields?
                            let to_space =
                                relation.to_space.and_then(|s| String::from_utf8(s).ok());
                            let from_property = relation
                                .from_property
                                .and_then(|s| String::from_utf8(s).ok());
                            let index = relation.index.and_then(|s| String::from_utf8(s).ok());

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
                                    index,
                                    type_id: type_id.unwrap().to_string(),
                                    from_id: from_id.unwrap().to_string(),
                                    from_property_id: from_property,
                                    to_id: to_id.unwrap().to_string(),
                                    to_space_id: to_space,
                                });
                            }
                        }
                    }
                    OpType::DeleteRelation => {
                        if op.relation.is_some() {
                            if let Some(relation) = op.relation.clone() {
                                if let Ok(relation_id) = String::from_utf8(relation.id) {
                                    relations.push(RelationItem {
                                        change_type: RelationChangeType::DELETE,
                                        id: relation_id,
                                        space_id: space_id.clone(),

                                        // These fields don't matter for a delete
                                        entity_id: String::from(""),
                                        index: None,
                                        type_id: String::from(""),
                                        from_id: String::from(""),
                                        to_id: String::from(""),
                                        to_space_id: None,
                                        from_property_id: None,
                                    });
                                }
                            }
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
