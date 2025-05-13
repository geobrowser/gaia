use grc20::pb::ipfs::{Edit, OpType};
use indexer_utils::graph_uri;

#[derive(Clone)]
pub enum RelationChangeType {
    SET,
    DELETE,
}

#[derive(Clone)]
pub struct RelationItem {
    pub id: String,
    pub change_type: RelationChangeType,
    pub type_id: String,
    pub from_id: String,
    pub to_id: String,
    pub to_space_id: Option<String>,
    pub index: String,
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
                            let type_id = graph_uri::to_entity_id(relation.r#type.as_str());
                            let from_id = graph_uri::to_entity_id(relation.from_entity.as_str());
                            let to_id = graph_uri::to_entity_id(relation.to_entity.as_str());
                            let to_space_id = graph_uri::to_space_id(relation.to_entity.as_str());

                            if from_id.is_some() && to_id.is_some() && type_id.is_some() {
                                relations.push(RelationItem {
                                    change_type: RelationChangeType::SET,
                                    id: relation.id,
                                    space_id: space_id.clone(),
                                    index: relation.index,
                                    type_id: type_id.unwrap().to_string(),
                                    from_id: from_id.unwrap().to_string(),
                                    to_id: to_id.unwrap().to_string(),
                                    to_space_id: to_space_id.map(|s| s.to_string()),
                                });
                            }
                        }
                    }
                    OpType::DeleteRelation => {
                        if op.relation.is_some() {
                            let relation = op.relation.clone().unwrap();

                            relations.push(RelationItem {
                                change_type: RelationChangeType::DELETE,
                                id: relation.id,
                                space_id: space_id.clone(),

                                // These fields don't matter for a delete
                                index: String::from(""),
                                type_id: String::from(""),
                                from_id: String::from(""),
                                to_id: String::from(""),
                                to_space_id: None,
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
