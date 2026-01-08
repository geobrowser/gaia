use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum SpaceType {
    DAO,
    Personal,
}

#[derive(Clone, Debug)]
pub struct SpaceItem {
    pub id: Uuid,
    pub space_type: SpaceType,
    pub address: String,
    pub topic_id: Option<Uuid>,
}
