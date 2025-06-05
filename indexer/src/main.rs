use futures::future::join_all;
use grc20::pb::chain::GeoOutput;
use indexer::{
    block_handler::root_handler,
    cache::{
        postgres::PostgresCache, properties_cache::PropertiesCache, CacheBackend, PreprocessedEdit,
    },
    error::IndexingError,
    storage::postgres::PostgresStorage,
    CreatedSpace, KgData, PersonalSpace, PublicSpace,
};
use indexer_utils::get_blocklist;
use prost::Message;
use std::{env, sync::Arc};
use tokio::{sync::Mutex, task};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use dotenv::dotenv;
use stream::{pb::sf::substreams::rpc::v2::BlockScopedData, PreprocessedSink};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

/// Matches spaces with their corresponding plugins based on DAO address
/// Returns a vector of CreatedSpace variants (Public or Personal)
fn match_spaces_with_plugins(
    spaces: &[grc20::pb::chain::GeoSpaceCreated],
    governance_plugins: &[grc20::pb::chain::GeoGovernancePluginCreated],
    personal_plugins: &[grc20::pb::chain::GeoPersonalSpaceAdminPluginCreated],
) -> Vec<CreatedSpace> {
    let mut created_spaces = Vec::new();
    
    for space in spaces {
        // Try to find a matching governance plugin first (for public spaces)
        if let Some(governance_plugin) = governance_plugins
            .iter()
            .find(|plugin| plugin.dao_address == space.dao_address)
        {
            created_spaces.push(CreatedSpace::Public(PublicSpace {
                dao_address: space.dao_address.clone(),
                space_address: space.space_address.clone(),
                membership_plugin: governance_plugin.member_access_address.clone(),
                governance_plugin: governance_plugin.main_voting_address.clone(),
            }));
        }
        // Otherwise, try to find a matching personal plugin (for personal spaces)
        else if let Some(personal_plugin) = personal_plugins
            .iter()
            .find(|plugin| plugin.dao_address == space.dao_address)
        {
            created_spaces.push(CreatedSpace::Personal(PersonalSpace {
                dao_address: space.dao_address.clone(),
                space_address: space.space_address.clone(),
                personal_plugin: personal_plugin.personal_admin_address.clone(),
            }));
        }
        // If no matching plugin is found, we skip this space
        // This could happen if events arrive in different blocks
    }

    created_spaces
}

struct KgIndexer {
    storage: Arc<PostgresStorage>,
    ipfs_cache: Arc<PostgresCache>,
    properties_cache: Arc<PropertiesCache>,
}

impl KgIndexer {
    pub fn new(
        storage: PostgresStorage,
        ipfs_cache: PostgresCache,
        properties_cache: PropertiesCache,
    ) -> Self {
        KgIndexer {
            storage: Arc::new(storage),
            ipfs_cache: Arc::new(ipfs_cache),
            properties_cache: Arc::new(properties_cache),
        }
    }


}

impl PreprocessedSink<KgData> for KgIndexer {
    type Error = IndexingError;

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        Ok(Some("".to_string()))
    }

    async fn persist_cursor(&self, _cursor: String) -> Result<(), Self::Error> {
        Ok(())
    }



    /**
    We can pre-process any edits we care about in the chain in this separate function.
    There's lots of decoding steps and filtering done to the Knowledge Graphs events
    so it's helpful to do this decoding/filtering/data-fetching ahead of time so the
    process steps can focus purely on mapping and writing data to the sink.
    */
    async fn preprocess_block_scoped_data(
        &self,
        block_data: &BlockScopedData,
    ) -> Result<(), Self::Error> {
        let output = stream::utils::output(block_data);
        let block_metadata = stream::utils::block_metadata(block_data);
        let geo = GeoOutput::decode(output.value.as_slice())?;
        let cache = &self.ipfs_cache;
        let edits = Arc::new(Mutex::new(Vec::<PreprocessedEdit>::new()));

        let mut handles = Vec::new();

        // @TODO: We can separate this cache reading step into a separate module
        for chain_edit in geo.edits_published.clone() {
            if get_blocklist()
                .dao_addresses
                .contains(&chain_edit.dao_address.as_str())
            {
                continue;
            }

            let cache = cache.clone();
            let edits_clone = edits.clone();

            let handle = task::spawn(async move {
                // We retry requests to the cache in the case that the cache is
                // still populating. For now we assume writing to + reading from
                // the cache can't fail
                let retry = ExponentialBackoff::from_millis(10)
                    .factor(2)
                    .max_delay(std::time::Duration::from_secs(5))
                    .map(jitter);
                let cached_edit_entry =
                    Retry::spawn(retry, async || cache.get(&chain_edit.content_uri).await).await?;

                {
                    let mut edits_guard = edits_clone.lock().await;
                    edits_guard.push(cached_edit_entry);
                }

                Ok::<(), IndexingError>(())
            });

            handles.push(handle);
        }

        join_all(handles).await;

        // Extract the edits from the Arc<Mutex<>> for further processing
        let final_edits = {
            let edits_guard = edits.lock().await;
            edits_guard.clone() // Clone the vector to move it out of the mutex
        };

        let created_spaces = match_spaces_with_plugins(
            &geo.spaces_created,
            &geo.governance_plugins_created,
            &geo.personal_plugins_created,
        );

        self.process_block_scoped_data(
            block_data,
            KgData {
                edits: final_edits,
                spaces: created_spaces,
                block: block_metadata,
            },
        )
        .await?;

        Ok(())
    }

    async fn process_block_scoped_data(
        &self,
        _block_data: &BlockScopedData,
        decoded_data: KgData,
    ) -> Result<(), Self::Error> {
        // @TODO: Need to figure out to abstract the different types of streams so
        // people can write their own sinks over specific events however they want.
        //
        // One idea is implementing the decoding at the stream level, so anybody
        // consuming the stream just gets the block data + the already-decoded contents
        // of each event.
        //
        // async fn process_block(&self, block_data: &DecodedBlockData, _raw_block_data: &BlockScopedData);
        root_handler::run(
            &decoded_data,
            &decoded_data.block,
            &self.storage,
            &self.properties_cache,
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grc20::pb::chain::{GeoSpaceCreated, GeoGovernancePluginCreated, GeoPersonalSpaceAdminPluginCreated};

    fn create_test_space(dao_address: &str, space_address: &str) -> GeoSpaceCreated {
        GeoSpaceCreated {
            dao_address: dao_address.to_string(),
            space_address: space_address.to_string(),
        }
    }

    fn create_test_governance_plugin(
        dao_address: &str,
        main_voting_address: &str,
        member_access_address: &str,
    ) -> GeoGovernancePluginCreated {
        GeoGovernancePluginCreated {
            dao_address: dao_address.to_string(),
            main_voting_address: main_voting_address.to_string(),
            member_access_address: member_access_address.to_string(),
        }
    }

    fn create_test_personal_plugin(
        dao_address: &str,
        personal_admin_address: &str,
        initial_editor: &str,
    ) -> GeoPersonalSpaceAdminPluginCreated {
        GeoPersonalSpaceAdminPluginCreated {
            dao_address: dao_address.to_string(),
            personal_admin_address: personal_admin_address.to_string(),
            initial_editor: initial_editor.to_string(),
        }
    }

    #[test]
    fn test_match_public_space() {
        let spaces = vec![create_test_space("dao1", "space1")];
        let governance_plugins = vec![create_test_governance_plugin("dao1", "voting1", "member1")];
        let personal_plugins = vec![];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 1);
        match &result[0] {
            CreatedSpace::Public(public_space) => {
                assert_eq!(public_space.dao_address, "dao1");
                assert_eq!(public_space.space_address, "space1");
                assert_eq!(public_space.governance_plugin, "voting1");
                assert_eq!(public_space.membership_plugin, "member1");
            }
            CreatedSpace::Personal(_) => panic!("Expected public space, got personal space"),
        }
    }

    #[test]
    fn test_match_personal_space() {
        let spaces = vec![create_test_space("dao2", "space2")];
        let governance_plugins = vec![];
        let personal_plugins = vec![create_test_personal_plugin("dao2", "admin2", "editor2")];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 1);
        match &result[0] {
            CreatedSpace::Personal(personal_space) => {
                assert_eq!(personal_space.dao_address, "dao2");
                assert_eq!(personal_space.space_address, "space2");
                assert_eq!(personal_space.personal_plugin, "admin2");
            }
            CreatedSpace::Public(_) => panic!("Expected personal space, got public space"),
        }
    }

    #[test]
    fn test_space_with_no_matching_plugin() {
        let spaces = vec![create_test_space("dao3", "space3")];
        let governance_plugins = vec![create_test_governance_plugin("dao1", "voting1", "member1")];
        let personal_plugins = vec![create_test_personal_plugin("dao2", "admin2", "editor2")];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_multiple_spaces_mixed_types() {
        let spaces = vec![
            create_test_space("dao1", "space1"),
            create_test_space("dao2", "space2"),
            create_test_space("dao3", "space3"), // No matching plugin
        ];
        let governance_plugins = vec![create_test_governance_plugin("dao1", "voting1", "member1")];
        let personal_plugins = vec![create_test_personal_plugin("dao2", "admin2", "editor2")];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 2);

        // Check first result (public space)
        match &result[0] {
            CreatedSpace::Public(public_space) => {
                assert_eq!(public_space.dao_address, "dao1");
                assert_eq!(public_space.space_address, "space1");
            }
            CreatedSpace::Personal(_) => panic!("Expected public space"),
        }

        // Check second result (personal space)
        match &result[1] {
            CreatedSpace::Personal(personal_space) => {
                assert_eq!(personal_space.dao_address, "dao2");
                assert_eq!(personal_space.space_address, "space2");
            }
            CreatedSpace::Public(_) => panic!("Expected personal space"),
        }
    }

    #[test]
    fn test_governance_plugin_takes_precedence_over_personal_plugin() {
        // If both types of plugins exist for the same DAO, governance plugin should take precedence
        let spaces = vec![create_test_space("dao1", "space1")];
        let governance_plugins = vec![create_test_governance_plugin("dao1", "voting1", "member1")];
        let personal_plugins = vec![create_test_personal_plugin("dao1", "admin1", "editor1")];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 1);
        match &result[0] {
            CreatedSpace::Public(public_space) => {
                assert_eq!(public_space.dao_address, "dao1");
                assert_eq!(public_space.governance_plugin, "voting1");
                assert_eq!(public_space.membership_plugin, "member1");
            }
            CreatedSpace::Personal(_) => panic!("Expected public space (governance should take precedence)"),
        }
    }

    #[test]
    fn test_empty_inputs() {
        let spaces = vec![];
        let governance_plugins = vec![];
        let personal_plugins = vec![];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_multiple_plugins_same_dao_different_spaces() {
        // Test that each space gets matched with the correct plugin even if there are multiple plugins for the same DAO
        let spaces = vec![
            create_test_space("dao1", "space1"),
            create_test_space("dao1", "space2"),
        ];
        let governance_plugins = vec![create_test_governance_plugin("dao1", "voting1", "member1")];
        let personal_plugins = vec![];

        let result = match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        assert_eq!(result.len(), 2);
    
        for space in &result {
            match space {
                CreatedSpace::Public(public_space) => {
                    assert_eq!(public_space.dao_address, "dao1");
                    assert_eq!(public_space.governance_plugin, "voting1");
                    assert_eq!(public_space.membership_plugin, "member1");
                }
                CreatedSpace::Personal(_) => panic!("Expected public spaces"),
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = PostgresStorage::new(&database_url).await;

    match storage {
        Ok(result) => {
            let cache = PostgresCache::new().await?;
            let properties_cache = PropertiesCache::new();
            let indexer = KgIndexer::new(result, cache, properties_cache);

            let endpoint_url =
                env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT not set");

            let _result = indexer
                .run(&endpoint_url, PKG_FILE, MODULE_NAME, START_BLOCK, 0)
                .await;
        }
        Err(error) => {
            println!("Error initializing stream {}", error);
        }
    }

    Ok(())
}
