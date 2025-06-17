use futures::future::join_all;
use grc20::pb::chain::GeoOutput;
use indexer_utils::get_blocklist;
use prost::Message;
use std::sync::Arc;
use stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use tokio::{sync::Mutex, task};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::{
    cache::{postgres::PostgresCache, CacheBackend, PreprocessedEdit},
    error::IndexingError,
    AddedMember, CreatedSpace, KgData, PersonalSpace, PublicSpace,
};

/// Matches spaces with their corresponding plugins based on DAO address
/// Returns a vector of CreatedSpace variants (Public or Personal)
pub fn match_spaces_with_plugins(
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

/// Maps editor events to AddedMember structs
pub fn map_editors_added(editors: &[grc20::pb::chain::EditorAdded]) -> Vec<AddedMember> {
    editors
        .iter()
        .map(|e| AddedMember {
            dao_address: e.dao_address.clone(),
            editor_address: e.editor_address.clone(),
        })
        .collect()
}

/// Maps initial editor events to AddedMember structs, flattening multiple addresses per event
pub fn map_initial_editors_added(initial_editors: &[grc20::pb::chain::InitialEditorAdded]) -> Vec<AddedMember> {
    initial_editors
        .iter()
        .flat_map(|e| {
            e.addresses.iter().map(|address| AddedMember {
                dao_address: e.dao_address.clone(),
                editor_address: address.clone(),
            })
        })
        .collect()
}

/// Maps member events to AddedMember structs
pub fn map_members_added(members: &[grc20::pb::chain::MemberAdded]) -> Vec<AddedMember> {
    members
        .iter()
        .map(|e| AddedMember {
            dao_address: e.dao_address.clone(),
            editor_address: e.member_address.clone(),
        })
        .collect()
}

/// Preprocesses block scoped data from the substream
pub async fn preprocess_block_scoped_data(
    block_data: &BlockScopedData,
    ipfs_cache: &Arc<PostgresCache>,
) -> Result<KgData, IndexingError> {
    let output = stream::utils::output(block_data);
    let block_metadata = stream::utils::block_metadata(block_data);
    let geo = GeoOutput::decode(output.value.as_slice())?;
    let cache = ipfs_cache;
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

    let mut added_editors = map_editors_added(&geo.editors_added);
    
    // Merge initial editors into added_editors
    let initial_editors = map_initial_editors_added(&geo.initial_editors_added);
    added_editors.extend(initial_editors);

    let added_members = map_members_added(&geo.members_added);

    Ok(KgData {
        edits: final_edits,
        spaces: created_spaces,
        added_editors,
        added_members,
        removed_editors: vec![],
        removed_members: vec![],
        block: block_metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grc20::pb::chain::{
        GeoGovernancePluginCreated, GeoPersonalSpaceAdminPluginCreated, GeoSpaceCreated,
    };

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

    fn create_test_editor_added(dao_address: &str, editor_address: &str) -> grc20::pb::chain::EditorAdded {
        grc20::pb::chain::EditorAdded {
            dao_address: dao_address.to_string(),
            editor_address: editor_address.to_string(),
            main_voting_plugin_address: "voting_plugin".to_string(),
            change_type: "0".to_string(),
        }
    }

    fn create_test_initial_editor_added(dao_address: &str, addresses: Vec<&str>) -> grc20::pb::chain::InitialEditorAdded {
        grc20::pb::chain::InitialEditorAdded {
            dao_address: dao_address.to_string(),
            addresses: addresses.into_iter().map(|s| s.to_string()).collect(),
            plugin_address: "plugin".to_string(),
        }
    }

    fn create_test_member_added(dao_address: &str, member_address: &str) -> grc20::pb::chain::MemberAdded {
        grc20::pb::chain::MemberAdded {
            dao_address: dao_address.to_string(),
            member_address: member_address.to_string(),
            main_voting_plugin_address: "voting_plugin".to_string(),
            change_type: "0".to_string(),
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
            CreatedSpace::Personal(_) => {
                panic!("Expected public space (governance should take precedence)")
            }
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

    #[test]
    fn test_map_editors_added_empty() {
        let editors = vec![];
        let result = map_editors_added(&editors);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_editors_added_single() {
        let editors = vec![create_test_editor_added("dao1", "editor1")];
        let result = map_editors_added(&editors);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
    }

    #[test]
    fn test_map_editors_added_multiple() {
        let editors = vec![
            create_test_editor_added("dao1", "editor1"),
            create_test_editor_added("dao2", "editor2"),
            create_test_editor_added("dao1", "editor3"),
        ];
        let result = map_editors_added(&editors);
        
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
        assert_eq!(result[1].dao_address, "dao2");
        assert_eq!(result[1].editor_address, "editor2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].editor_address, "editor3");
    }

    #[test]
    fn test_map_initial_editors_added_empty() {
        let initial_editors = vec![];
        let result = map_initial_editors_added(&initial_editors);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_initial_editors_added_single_event_single_address() {
        let initial_editors = vec![create_test_initial_editor_added("dao1", vec!["editor1"])];
        let result = map_initial_editors_added(&initial_editors);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
    }

    #[test]
    fn test_map_initial_editors_added_single_event_multiple_addresses() {
        let initial_editors = vec![create_test_initial_editor_added("dao1", vec!["editor1", "editor2", "editor3"])];
        let result = map_initial_editors_added(&initial_editors);
        
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
        assert_eq!(result[1].dao_address, "dao1");
        assert_eq!(result[1].editor_address, "editor2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].editor_address, "editor3");
    }

    #[test]
    fn test_map_initial_editors_added_multiple_events() {
        let initial_editors = vec![
            create_test_initial_editor_added("dao1", vec!["editor1", "editor2"]),
            create_test_initial_editor_added("dao2", vec!["editor3"]),
            create_test_initial_editor_added("dao1", vec!["editor4", "editor5", "editor6"]),
        ];
        let result = map_initial_editors_added(&initial_editors);
        
        assert_eq!(result.len(), 6);
        // First event - dao1 with 2 editors
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
        assert_eq!(result[1].dao_address, "dao1");
        assert_eq!(result[1].editor_address, "editor2");
        // Second event - dao2 with 1 editor
        assert_eq!(result[2].dao_address, "dao2");
        assert_eq!(result[2].editor_address, "editor3");
        // Third event - dao1 with 3 editors
        assert_eq!(result[3].dao_address, "dao1");
        assert_eq!(result[3].editor_address, "editor4");
        assert_eq!(result[4].dao_address, "dao1");
        assert_eq!(result[4].editor_address, "editor5");
        assert_eq!(result[5].dao_address, "dao1");
        assert_eq!(result[5].editor_address, "editor6");
    }

    #[test]
    fn test_map_initial_editors_added_empty_addresses() {
        let initial_editors = vec![create_test_initial_editor_added("dao1", vec![])];
        let result = map_initial_editors_added(&initial_editors);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_members_added_empty() {
        let members = vec![];
        let result = map_members_added(&members);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_members_added_single() {
        let members = vec![create_test_member_added("dao1", "member1")];
        let result = map_members_added(&members);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "member1");
    }

    #[test]
    fn test_map_members_added_multiple() {
        let members = vec![
            create_test_member_added("dao1", "member1"),
            create_test_member_added("dao2", "member2"),
            create_test_member_added("dao1", "member3"),
        ];
        let result = map_members_added(&members);
        
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "member1");
        assert_eq!(result[1].dao_address, "dao2");
        assert_eq!(result[1].editor_address, "member2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].editor_address, "member3");
    }

    #[test]
    fn test_combined_editor_mapping_workflow() {
        // Test the typical workflow of combining regular and initial editors
        let editors = vec![
            create_test_editor_added("dao1", "editor1"),
            create_test_editor_added("dao2", "editor2"),
        ];
        let initial_editors_events = vec![
            create_test_initial_editor_added("dao1", vec!["initial1", "initial2"]),
            create_test_initial_editor_added("dao3", vec!["initial3"]),
        ];

        let mut added_editors = map_editors_added(&editors);
        let initial_editors = map_initial_editors_added(&initial_editors_events);
        added_editors.extend(initial_editors);

        assert_eq!(added_editors.len(), 5);
        
        // Check regular editors
        assert_eq!(added_editors[0].dao_address, "dao1");
        assert_eq!(added_editors[0].editor_address, "editor1");
        assert_eq!(added_editors[1].dao_address, "dao2");
        assert_eq!(added_editors[1].editor_address, "editor2");
        
        // Check initial editors
        assert_eq!(added_editors[2].dao_address, "dao1");
        assert_eq!(added_editors[2].editor_address, "initial1");
        assert_eq!(added_editors[3].dao_address, "dao1");
        assert_eq!(added_editors[3].editor_address, "initial2");
        assert_eq!(added_editors[4].dao_address, "dao3");
        assert_eq!(added_editors[4].editor_address, "initial3");
    }
}