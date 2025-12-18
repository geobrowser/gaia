use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum ValueChangeType {
    Set,
    Delete,
}

#[derive(Clone, Debug)]
pub struct ValueOp {
    pub id: Uuid,
    pub change_type: ValueChangeType,
    pub entity_id: Uuid,
    pub property_id: Uuid,
    pub space_id: Uuid,
    pub language: Option<String>,
    pub unit: Option<String>,
    pub string: Option<String>,
    pub number: Option<f64>,
    pub boolean: Option<bool>,
    pub time: Option<String>,
    pub point: Option<String>,
}
