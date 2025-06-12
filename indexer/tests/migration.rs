use bytes::Bytes;
use grc20::pb::grc20::Edit;
use prost::Message;
use std::{env, fs, sync::Arc};
use stream::utils::BlockMetadata;
use uuid::Uuid;

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler,
    cache::{properties_cache::PropertiesCache, PreprocessedEdit},
    error::IndexingError,
    storage::postgres::PostgresStorage,
    CreatedSpace, KgData, PersonalSpace, PublicSpace,
};

struct TestIndexer {
    storage: Arc<PostgresStorage>,
    properties_cache: Arc<PropertiesCache>,
}

impl TestIndexer {
    pub fn new(storage: Arc<PostgresStorage>, properties_cache: Arc<PropertiesCache>) -> Self {
        TestIndexer {
            storage,
            properties_cache,
        }
    }

    pub async fn run(&self, blocks: &Vec<KgData>) -> Result<(), IndexingError> {
        for block in blocks {
            root_handler::run(block, &block.block, &self.storage, &self.properties_cache).await?;
        }

        Ok(())
    }
}

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());

    let root_space_bytes = fs::read("./tests/25omwWh6HYgeRQKCaSpVpa_ops");
    let crypto_space_bytes = fs::read("./tests/SgjATMbm41LX6naizMqBVd_ops");

    let root_space_edit = Edit::decode(Bytes::from(root_space_bytes.unwrap()));
    let crypto_space_edit = Edit::decode(Bytes::from(crypto_space_bytes.unwrap()));

    let root_space_preprocessed_edit = PreprocessedEdit {
        // For now we use a random UUID instead of the correct UUID for the root space
        space_id: Uuid::parse_str("8ef40bdd-cf69-4ad7-a9a1-f71c15653994").unwrap(),
        edit: Some(root_space_edit.clone().unwrap()),
        is_errored: false,
    };

    let crypto_space_preprocessed_edit = PreprocessedEdit {
        // For now we use a random UUID instead of the correct UUID for the crypto space
        space_id: Uuid::parse_str("aa84b08d-779a-495c-93f1-44e667baf6d7").unwrap(),
        edit: Some(crypto_space_edit.clone().unwrap()),
        is_errored: false,
    };

    let block = BlockMetadata {
        cursor: String::from("5"),
        block_number: 1,
        timestamp: String::from("5"),
    };

    let root_space = CreatedSpace::Public(PublicSpace {
        dao_address: "0x1234567890123456789012345678901234567890".to_string(),
        space_address: "0xABCDEF1234567890123456789012345678901234".to_string(),
        membership_plugin: "0x1111111111111111111111111111111111111111".to_string(),
        governance_plugin: "0x3333333333333333333333333333333333333333".to_string(),
    });

    let crypto_space = CreatedSpace::Personal(PersonalSpace {
        dao_address: "0x0987654321098765432109876543210987654321".to_string(),
        space_address: "0xFEDCBA0987654321098765432109876543210987".to_string(),
        personal_plugin: "0x2222222222222222222222222222222222222222".to_string(),
    });

    let indexer = TestIndexer::new(storage.clone(), properties_cache.clone());

    indexer
        .run(&vec![KgData {
            block,
            edits: vec![root_space_preprocessed_edit, crypto_space_preprocessed_edit],
            spaces: vec![root_space, crypto_space],
        }])
        .await?;

    Ok(())
}
