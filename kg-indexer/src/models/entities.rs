use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct EntityItem {
    pub id: Uuid,
    pub created_at: String,
    pub created_at_block: String,
    pub updated_at: String,
    pub updated_at_block: String,
}
