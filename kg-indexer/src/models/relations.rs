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
    pub is_system: bool,
    // Context columns for grouping changes (GRC-20 Section 4.5)
    pub context_root_id: Option<Uuid>,
    pub context_edge_type_id: Option<Uuid>,
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
    // Context columns for grouping (GRC-20 Section 4.5). Without these,
    // relation_versions rows written for updates land with NULL context and
    // become invisible to context-aware diff discovery.
    pub context_root_id: Option<Uuid>,
    pub context_edge_type_id: Option<Uuid>,
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
    // Context columns — same rationale as UpdateRelationItem.
    pub context_root_id: Option<Uuid>,
    pub context_edge_type_id: Option<Uuid>,
}

/// Delete relation item with space context.
///
/// NOTE: Delete ops do not currently write a tombstone version row carrying
/// context, because the live `relations` row is removed before the version
/// table is touched. Adding delete-with-context attribution would require
/// fetching the relation's pre-delete state up the call chain — tracked
/// separately. Until then, deletions made under a contextual edit appear in
/// diffs only via the closure of `valid_to_key` on the existing version row,
/// without context surfacing in `queryContextEntities`.
#[derive(Clone, Debug)]
pub struct DeleteRelationItem {
    pub id: Uuid,
    pub space_id: Uuid,
}

/// Enum representing all relation operations for squashing
#[derive(Clone, Debug)]
pub enum RelationOp {
    Create(SetRelationItem),
    Update(UpdateRelationItem),
    Unset(UnsetRelationItem),
    Delete(DeleteRelationItem),
}

impl RelationOp {
    pub fn id(&self) -> Uuid {
        match self {
            RelationOp::Create(r) => r.id,
            RelationOp::Update(r) => r.id,
            RelationOp::Unset(r) => r.id,
            RelationOp::Delete(r) => r.id,
        }
    }

    #[allow(dead_code)]
    pub fn space_id(&self) -> Uuid {
        match self {
            RelationOp::Create(r) => r.space_id,
            RelationOp::Update(r) => r.space_id,
            RelationOp::Unset(r) => r.space_id,
            RelationOp::Delete(r) => r.space_id,
        }
    }
}
