use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct SubspaceItem {
    pub subspace_id: Uuid,
    pub parent_space_id: Uuid,
}
