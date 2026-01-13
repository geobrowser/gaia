use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct MemberItem {
    pub member_space_id: Uuid,
    pub space_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct EditorItem {
    pub member_space_id: Uuid,
    pub space_id: Uuid,
}
