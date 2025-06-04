use cache::PreprocessedEdit;
use stream::utils::BlockMetadata;

pub mod block_handler;
pub mod cache;
pub mod error;
pub mod models;
pub mod storage;
pub mod validators;

pub struct PersonalSpace {
    pub dao_address: String,
    // pub space_address: String,
    // pub personal_plugin: String,
}

pub struct PublicSpace {
    pub dao_address: String,
    // pub space_address: String,
    // pub membership_plugin: String,
    // pub governance_plugin: String,
}

pub enum CreatedSpace {
    Personal(PersonalSpace),
    Public(PublicSpace),
}

#[derive(Clone)]
pub struct KgData {
    pub block: BlockMetadata,
    pub edits: Vec<PreprocessedEdit>,
    // Note for now that we only need the dao address. Eventually we'll
    // index the plugin addresses as well.
    pub spaces: Vec<CreatedSpace>,
}
