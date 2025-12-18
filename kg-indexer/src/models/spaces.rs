use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum SpaceType {
    Personal,
    Public,
}

#[derive(Clone, Debug)]
pub struct SpaceItem {
    pub id: Uuid,
    pub space_type: SpaceType,
    pub dao_address: String,
    pub space_address: String,
    pub voting_address: Option<String>,
    pub membership_address: Option<String>,
    pub personal_address: Option<String>,
}
