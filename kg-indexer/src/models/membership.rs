use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct MemberItem {
    pub address: String,
    pub space_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct EditorItem {
    pub address: String,
    pub space_id: Uuid,
}
