use uuid::Uuid;

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

/// Enum representing all relation operations for squashing
#[derive(Clone, Debug)]
pub enum RelationOp {
    Create(SetRelationItem),
    Update(UpdateRelationItem),
    Unset(UnsetRelationItem),
    Delete(Uuid),
}

impl RelationOp {
    pub fn id(&self) -> Uuid {
        match self {
            RelationOp::Create(r) => r.id,
            RelationOp::Update(r) => r.id,
            RelationOp::Unset(r) => r.id,
            RelationOp::Delete(id) => *id,
        }
    }
}
