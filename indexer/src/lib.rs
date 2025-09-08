use cache::PreprocessedEdit;
use stream::utils::BlockMetadata;

pub mod block_handler;
pub mod cache;
pub mod error;
pub mod models;
pub mod preprocess;
pub mod storage;
pub mod validators;

pub mod test_utils;

#[derive(Clone, Debug)]
pub struct PersonalSpace {
    pub dao_address: String,
    pub space_address: String,
    pub personal_plugin: String,
}

#[derive(Clone, Debug)]
pub struct PublicSpace {
    pub dao_address: String,
    pub space_address: String,
    pub membership_plugin: String,
    pub governance_plugin: String,
}

#[derive(Clone, Debug)]
pub enum CreatedSpace {
    Personal(PersonalSpace),
    Public(PublicSpace),
}

#[derive(Clone, Debug)]
pub struct AddedMember {
    pub dao_address: String,
    pub editor_address: String,
}

#[derive(Clone, Debug)]
pub struct RemovedMember {
    pub dao_address: String,
    pub editor_address: String,
}

#[derive(Clone, Debug)]
pub struct AddedSubspace {
    pub dao_address: String,
    pub subspace_address: String,
}

#[derive(Clone, Debug)]
pub struct RemovedSubspace {
    pub dao_address: String,
    pub subspace_address: String,
}

#[derive(Clone, Debug)]
pub struct KgData {
    pub block: BlockMetadata,
    pub edits: Vec<PreprocessedEdit>,
    pub added_editors: Vec<AddedMember>,
    pub removed_editors: Vec<RemovedMember>,
    pub added_members: Vec<AddedMember>,
    pub removed_members: Vec<RemovedMember>,
    pub added_subspaces: Vec<AddedSubspace>,
    pub removed_subspaces: Vec<RemovedSubspace>,
    // Note for now that we only need the dao address. Eventually we'll
    // index the plugin addresses as well.
    pub spaces: Vec<CreatedSpace>,
}
