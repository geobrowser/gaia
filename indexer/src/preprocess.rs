use futures::future::join_all;
use indexer_utils::get_blocklist;
use prost::Message;
use std::{collections::HashSet, sync::Arc};
use stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use tokio::{sync::Mutex, task};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};
use tracing::{debug, info, instrument, warn};
use wire::pb::chain::GeoOutput;

use crate::{
    cache::{postgres::PostgresCache, CacheBackend, PreprocessedEdit},
    error::IndexingError,
    AddedMember, AddedSubspace, CreatedSpace, ExecutedProposal, KgData, PersonalSpace,
    ProposalCreated, PublicSpace, RemovedMember, RemovedSubspace,
};
use indexer_utils::id;
use uuid::Uuid;

/// Matches spaces with their corresponding plugins based on DAO address
/// Returns a vector of CreatedSpace variants (Public or Personal)
#[instrument(skip_all, fields(space_count = spaces.len(), governance_plugin_count = governance_plugins.len(), personal_plugin_count = personal_plugins.len()))]
pub fn match_spaces_with_plugins(
    spaces: &[wire::pb::chain::GeoSpaceCreated],
    governance_plugins: &[wire::pb::chain::GeoGovernancePluginCreated],
    personal_plugins: &[wire::pb::chain::GeoPersonalSpaceAdminPluginCreated],
) -> Vec<CreatedSpace> {
    let mut created_spaces = Vec::new();
    let mut unmatched_spaces = 0;

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
        else {
            unmatched_spaces += 1;
            debug!(
                dao_address = %space.dao_address,
                space_address = %space.space_address,
                "Space has no matching plugin, skipping"
            );
        }
    }

    if unmatched_spaces > 0 {
        warn!(
            unmatched_count = unmatched_spaces,
            total_spaces = spaces.len(),
            "Some spaces had no matching plugins"
        );
    }

    created_spaces
}

/// Maps editor events to AddedMember structs
pub fn map_editors_added(editors: &[wire::pb::chain::EditorAdded]) -> Vec<AddedMember> {
    editors
        .iter()
        .map(|e| AddedMember {
            dao_address: e.dao_address.clone(),
            editor_address: e.editor_address.clone(),
        })
        .collect()
}

/// Maps initial editor events to AddedMember structs, flattening multiple addresses per event
pub fn map_initial_editors_added(
    initial_editors: &[wire::pb::chain::InitialEditorAdded],
) -> Vec<AddedMember> {
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
pub fn map_members_added(members: &[wire::pb::chain::MemberAdded]) -> Vec<AddedMember> {
    members
        .iter()
        .map(|e| AddedMember {
            dao_address: e.dao_address.clone(),
            editor_address: e.member_address.clone(),
        })
        .collect()
}

/// Maps subspace added events to AddedSubspace structs
pub fn map_subspaces_added(subspaces: &[wire::pb::chain::SubspaceAdded]) -> Vec<AddedSubspace> {
    subspaces
        .iter()
        .map(|s| AddedSubspace {
            dao_address: s.dao_address.clone(),
            subspace_address: s.subspace.clone(),
        })
        .collect()
}

/// Maps subspace removed events to RemovedSubspace structs
pub fn map_subspaces_removed(
    subspaces: &[wire::pb::chain::SubspaceRemoved],
) -> Vec<RemovedSubspace> {
    subspaces
        .iter()
        .map(|s| RemovedSubspace {
            dao_address: s.dao_address.clone(),
            subspace_address: s.subspace.clone(),
        })
        .collect()
}

/// Maps member removed events to RemovedMember structs
pub fn map_members_removed(members: &[wire::pb::chain::MemberRemoved]) -> Vec<RemovedMember> {
    members
        .iter()
        .map(|m| RemovedMember {
            dao_address: m.dao_address.clone(),
            editor_address: m.member_address.clone(),
        })
        .collect()
}

/// Maps editor removed events to RemovedMember structs
pub fn map_editors_removed(editors: &[wire::pb::chain::EditorRemoved]) -> Vec<RemovedMember> {
    editors
        .iter()
        .map(|e| RemovedMember {
            dao_address: e.dao_address.clone(),
            editor_address: e.editor_address.clone(),
        })
        .collect()
}

/// Maps executed proposal events to ExecutedProposal structs
pub fn map_executed_proposals(
    proposals: &[wire::pb::chain::ProposalExecuted],
) -> Vec<ExecutedProposal> {
    proposals
        .iter()
        .map(|p| ExecutedProposal {
            proposal_id: p.proposal_id.clone(),
            plugin_address: p.plugin_address.clone(),
        })
        .collect()
}

/// Maps created proposal events to ProposalCreated enum variants
pub async fn map_created_proposals(geo: &wire::pb::chain::GeoOutput, cache: &Arc<impl CacheBackend>) -> Result<Vec<ProposalCreated>, IndexingError> {
    let mut proposals = Vec::new();

    // Map PublishEdit proposals concurrently
    if !geo.edits.is_empty() {
        // Create futures for concurrent cache reads
        let cache_futures = geo.edits.iter().map(|p| {
            let cache = cache.clone();
            let content_uri = p.content_uri.clone();
            async move {
                (content_uri.clone(), cache.get(&content_uri).await)
            }
        });

        // Execute all cache reads concurrently
        let cache_results = futures::future::join_all(cache_futures).await;

        // Process results and create proposals
        for (p, (content_uri, cache_result)) in geo.edits.iter().zip(cache_results.iter()) {
            let edit_id = match cache_result {
                Ok(cached_edit) if !cached_edit.is_errored => {
                    if let Some(edit) = &cached_edit.edit {
                        // Transform the edit.id to UUID using the same logic as entities
                        match id::transform_id_bytes(edit.id.clone()) {
                            Ok(bytes) => Some(Uuid::from_bytes(bytes)),
                            Err(_) => {
                                tracing::warn!(
                                    content_uri = %content_uri,
                                    "Failed to transform edit.id bytes, using None"
                                );
                                None
                            }
                        }
                    } else {
                        tracing::warn!(
                            content_uri = %content_uri,
                            "Cached edit has no edit data, using None"
                        );
                        None
                    }
                }
                Ok(_) => {
                    tracing::warn!(
                        content_uri = %content_uri,
                        "Cached edit is errored, using None"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        content_uri = %content_uri,
                        error = %e,
                        "Failed to read edit from cache, using None"
                    );
                    None
                }
            };

            proposals.push(ProposalCreated::PublishEdit {
                proposal_id: p.proposal_id.clone(),
                creator: p.creator.clone(),
                start_time: p.start_time.clone(),
                end_time: p.end_time.clone(),
                content_uri: p.content_uri.clone(),
                dao_address: p.dao_address.clone(),
                plugin_address: p.plugin_address.clone(),
                edit_id,
            });
        }
    }

    // Map AddMember proposals
    for p in &geo.proposed_added_members {
        proposals.push(ProposalCreated::AddMember {
            proposal_id: p.proposal_id.clone(),
            creator: p.creator.clone(),
            start_time: p.start_time.clone(),
            end_time: p.end_time.clone(),
            member: p.member.clone(),
            dao_address: p.dao_address.clone(),
            plugin_address: p.plugin_address.clone(),
            change_type: p.change_type.clone(),
        });
    }

    // Map RemoveMember proposals
    for p in &geo.proposed_removed_members {
        proposals.push(ProposalCreated::RemoveMember {
            proposal_id: p.proposal_id.clone(),
            creator: p.creator.clone(),
            start_time: p.start_time.clone(),
            end_time: p.end_time.clone(),
            member: p.member.clone(),
            dao_address: p.dao_address.clone(),
            plugin_address: p.plugin_address.clone(),
            change_type: p.change_type.clone(),
        });
    }

    // Map AddEditor proposals
    for p in &geo.proposed_added_editors {
        proposals.push(ProposalCreated::AddEditor {
            proposal_id: p.proposal_id.clone(),
            creator: p.creator.clone(),
            start_time: p.start_time.clone(),
            end_time: p.end_time.clone(),
            editor: p.editor.clone(),
            dao_address: p.dao_address.clone(),
            plugin_address: p.plugin_address.clone(),
            change_type: p.change_type.clone(),
        });
    }

    // Map RemoveEditor proposals
    for p in &geo.proposed_removed_editors {
        proposals.push(ProposalCreated::RemoveEditor {
            proposal_id: p.proposal_id.clone(),
            creator: p.creator.clone(),
            start_time: p.start_time.clone(),
            end_time: p.end_time.clone(),
            editor: p.editor.clone(),
            dao_address: p.dao_address.clone(),
            plugin_address: p.plugin_address.clone(),
            change_type: p.change_type.clone(),
        });
    }

    // Map AddSubspace proposals
    for p in &geo.proposed_added_subspaces {
        proposals.push(ProposalCreated::AddSubspace {
            proposal_id: p.proposal_id.clone(),
            creator: p.creator.clone(),
            start_time: p.start_time.clone(),
            end_time: p.end_time.clone(),
            subspace: p.subspace.clone(),
            dao_address: p.dao_address.clone(),
            plugin_address: p.plugin_address.clone(),
            change_type: p.change_type.clone(),
        });
    }

    // Map RemoveSubspace proposals
    for p in &geo.proposed_removed_subspaces {
        proposals.push(ProposalCreated::RemoveSubspace {
            proposal_id: p.proposal_id.clone(),
            creator: p.creator.clone(),
            start_time: p.start_time.clone(),
            end_time: p.end_time.clone(),
            subspace: p.subspace.clone(),
            dao_address: p.dao_address.clone(),
            plugin_address: p.plugin_address.clone(),
            change_type: p.change_type.clone(),
        });
    }

    Ok(proposals)
}

/// Preprocesses block scoped data from the substream
#[instrument(skip_all, fields(
    block_number = block_data.clock.as_ref().map(|c| c.number).unwrap_or(0),
    block_timestamp = block_data.clock.as_ref().and_then(|c| c.timestamp.as_ref()).map(|t| t.seconds).unwrap_or(0)
))]
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
    let mut blocklisted_count = 0;
    let total_edits = geo.edits_published.len();

    // @TODO: We can separate this cache reading step into a separate module
    for chain_edit in geo.edits_published.clone() {
        if get_blocklist()
            .dao_addresses
            .contains(&chain_edit.dao_address.as_str())
        {
            blocklisted_count += 1;
            debug!(
                dao_address = %chain_edit.dao_address,
                content_uri = %chain_edit.content_uri,
                "Skipping blocklisted DAO"
            );
            continue;
        }

        let cache = cache.clone();
        let edits_clone = edits.clone();

        let content_uri = chain_edit.content_uri.clone();
        let dao_address = chain_edit.dao_address.clone();

        let handle = task::spawn(async move {
            // We retry requests to the cache in the case that the cache is
            // still populating. For now we assume writing to + reading from
            // the cache can't fail
            let retry = ExponentialBackoff::from_millis(10)
                .factor(2)
                .max_delay(std::time::Duration::from_secs(5))
                .map(jitter);

            match Retry::spawn(retry, async || cache.get(&content_uri).await).await {
                Ok(cached_edit_entry) => {
                    if cached_edit_entry.is_errored {
                        warn!(
                            dao_address = %dao_address,
                            content_uri = %content_uri,
                            "Cached edit entry is errored"
                        );
                    }

                    {
                        let mut edits_guard = edits_clone.lock().await;
                        edits_guard.push(cached_edit_entry);
                    }
                    Ok::<(), IndexingError>(())
                }
                Err(e) => {
                    warn!(
                        dao_address = %dao_address,
                        content_uri = %content_uri,
                        error = %e,
                        "Failed to fetch edit from cache after retries"
                    );
                    Err(IndexingError::CacheError(e))
                }
            }
        });

        handles.push(handle);
    }

    let results = join_all(handles).await;

    let mut failed_fetches = 0;
    for result in results {
        if let Err(_) = result {
            failed_fetches += 1;
        }
    }

    if failed_fetches > 0 {
        warn!(
            failed_count = failed_fetches,
            "Some edits failed to fetch from cache"
        );
    }

    // Extract the edits from the Arc<Mutex<>> for further processing
    let final_edits = {
        let edits_guard = edits.lock().await;
        edits_guard.clone() // Clone the vector to move it out of the mutex
    };


    if blocklisted_count > 0 {
        info!(
            blocklisted_count,
            processed_count = final_edits.len(),
            total_count = total_edits,
            "Filtered blocklisted DAOs from edits"
        );
    }

    let created_spaces = match_spaces_with_plugins(
        &geo.spaces_created,
        &geo.governance_plugins_created,
        &geo.personal_plugins_created,
    );

    let mut added_editors = map_editors_added(&geo.editors_added);

    // Merge initial editors into added_editors
    let initial_editors = map_initial_editors_added(&geo.initial_editors_added);
    added_editors.extend(initial_editors.clone());

    let mut added_members = map_members_added(&geo.members_added);

    // If any added editors come from a space created at the same time, add
    // them as initial members
    let created_space_dao_addresses: HashSet<String> = created_spaces
        .iter()
        .map(|space| match space {
            CreatedSpace::Personal(personal_space) => personal_space.dao_address.clone(),
            CreatedSpace::Public(public_space) => public_space.dao_address.clone(),
        })
        .collect();

    for editor in &added_editors {
        if created_space_dao_addresses.contains(&editor.dao_address) {
            added_members.push(AddedMember {
                dao_address: editor.dao_address.clone(),
                editor_address: editor.editor_address.clone(),
            });
        }
    }

    let added_subspaces = map_subspaces_added(&geo.subspaces_added);
    let removed_subspaces = map_subspaces_removed(&geo.subspaces_removed);
    
    let removed_members = map_members_removed(&geo.members_removed);
    let removed_editors = map_editors_removed(&geo.editors_removed);
    
    let executed_proposals = map_executed_proposals(&geo.executed_proposals);
    let created_proposals = map_created_proposals(&geo, &ipfs_cache).await?;

    let kg_data = KgData {
        edits: final_edits.clone(),
        spaces: created_spaces.clone(),
        added_editors: added_editors.clone(),
        added_members: added_members.clone(),
        removed_editors: removed_editors.clone(),
        removed_members: removed_members.clone(),
        added_subspaces: added_subspaces.clone(),
        removed_subspaces: removed_subspaces.clone(),
        block: block_metadata,
        executed_proposals: executed_proposals.clone(),
        created_proposals: created_proposals.clone(),
    };

    info!(
        edit_count = kg_data.edits.len(),
        space_count = kg_data.spaces.len(),
        editor_count = kg_data.added_editors.len(),
        member_count = kg_data.added_members.len(),
        removed_editor_count = kg_data.removed_editors.len(),
        removed_member_count = kg_data.removed_members.len(),
        subspace_added_count = kg_data.added_subspaces.len(),
        subspace_removed_count = kg_data.removed_subspaces.len(),
        executed_proposal_count = kg_data.executed_proposals.len(),
        created_proposal_count = kg_data.created_proposals.len(),
        "Preprocessed block data"
    );

    Ok(kg_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::pb::chain::{
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

    fn create_test_editor_added(
        dao_address: &str,
        editor_address: &str,
    ) -> wire::pb::chain::EditorAdded {
        wire::pb::chain::EditorAdded {
            dao_address: dao_address.to_string(),
            editor_address: editor_address.to_string(),
            main_voting_plugin_address: "voting_plugin".to_string(),
            change_type: "0".to_string(),
        }
    }

    fn create_test_initial_editor_added(
        dao_address: &str,
        addresses: Vec<&str>,
    ) -> wire::pb::chain::InitialEditorAdded {
        wire::pb::chain::InitialEditorAdded {
            dao_address: dao_address.to_string(),
            addresses: addresses.into_iter().map(|s| s.to_string()).collect(),
            plugin_address: "plugin".to_string(),
        }
    }

    fn create_test_member_added(
        dao_address: &str,
        member_address: &str,
    ) -> wire::pb::chain::MemberAdded {
        wire::pb::chain::MemberAdded {
            dao_address: dao_address.to_string(),
            member_address: member_address.to_string(),
            main_voting_plugin_address: "voting_plugin".to_string(),
            change_type: "0".to_string(),
        }
    }

    fn create_test_subspace_added(
        dao_address: &str,
        subspace: &str,
    ) -> wire::pb::chain::SubspaceAdded {
        wire::pb::chain::SubspaceAdded {
            dao_address: dao_address.to_string(),
            subspace: subspace.to_string(),
            plugin_address: "plugin".to_string(),
            change_type: "0".to_string(),
        }
    }

    fn create_test_subspace_removed(
        dao_address: &str,
        subspace: &str,
    ) -> wire::pb::chain::SubspaceRemoved {
        wire::pb::chain::SubspaceRemoved {
            dao_address: dao_address.to_string(),
            subspace: subspace.to_string(),
            plugin_address: "plugin".to_string(),
            change_type: "0".to_string(),
        }
    }

    fn create_test_member_removed(
        dao_address: &str,
        member_address: &str,
    ) -> wire::pb::chain::MemberRemoved {
        wire::pb::chain::MemberRemoved {
            dao_address: dao_address.to_string(),
            member_address: member_address.to_string(),
            plugin_address: "plugin".to_string(),
            change_type: "0".to_string(),
        }
    }

    fn create_test_editor_removed(
        dao_address: &str,
        editor_address: &str,
    ) -> wire::pb::chain::EditorRemoved {
        wire::pb::chain::EditorRemoved {
            dao_address: dao_address.to_string(),
            editor_address: editor_address.to_string(),
            plugin_address: "plugin".to_string(),
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
        let initial_editors = vec![create_test_initial_editor_added(
            "dao1",
            vec!["editor1", "editor2", "editor3"],
        )];
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

    #[test]
    fn test_editors_from_newly_created_spaces_added_to_members() {
        // Create test spaces
        let spaces = vec![
            create_test_space("dao1", "space1"),
            create_test_space("dao2", "space2"),
        ];

        // Create matching plugins for the spaces
        let governance_plugins = vec![create_test_governance_plugin("dao1", "member1", "voting1")];
        let personal_plugins = vec![create_test_personal_plugin("dao2", "admin2", "initial2")];

        // Create editors for the same DAOs that have spaces created
        let editors = vec![
            create_test_editor_added("dao1", "editor1"),
            create_test_editor_added("dao1", "editor2"),
            create_test_editor_added("dao2", "editor3"),
            create_test_editor_added("dao3", "editor4"), // This DAO has no space created
        ];

        // Create some regular members
        let members = vec![create_test_member_added("dao1", "member1")];

        // Match spaces with plugins
        let created_spaces =
            match_spaces_with_plugins(&spaces, &governance_plugins, &personal_plugins);

        // Map editors and members
        let added_editors = map_editors_added(&editors);
        let mut added_members = map_members_added(&members);

        // Simulate the logic from preprocess_block_scoped_data
        let created_space_dao_addresses: std::collections::HashSet<String> = created_spaces
            .iter()
            .map(|space| match space {
                CreatedSpace::Personal(personal_space) => personal_space.dao_address.clone(),
                CreatedSpace::Public(public_space) => public_space.dao_address.clone(),
            })
            .collect();

        for editor in &added_editors {
            if created_space_dao_addresses.contains(&editor.dao_address) {
                added_members.push(AddedMember {
                    dao_address: editor.dao_address.clone(),
                    editor_address: editor.editor_address.clone(),
                });
            }
        }

        // Verify results
        assert_eq!(created_spaces.len(), 2); // dao1 and dao2 should have spaces created
        assert_eq!(added_editors.len(), 4); // All 4 editors should be mapped

        // added_members should include:
        // - 1 original member (member1 from dao1)
        // - 3 editors from newly created spaces (editor1, editor2 from dao1; editor3 from dao2)
        // - editor4 from dao3 should NOT be included since dao3 has no space created
        assert_eq!(added_members.len(), 4);

        // Check that the original member is still there
        assert!(added_members
            .iter()
            .any(|m| m.dao_address == "dao1" && m.editor_address == "member1"));

        // Check that editors from newly created spaces are added as members
        assert!(added_members
            .iter()
            .any(|m| m.dao_address == "dao1" && m.editor_address == "editor1"));
        assert!(added_members
            .iter()
            .any(|m| m.dao_address == "dao1" && m.editor_address == "editor2"));
        assert!(added_members
            .iter()
            .any(|m| m.dao_address == "dao2" && m.editor_address == "editor3"));

        // Check that editor4 from dao3 (no space created) is NOT added as a member
        assert!(!added_members
            .iter()
            .any(|m| m.dao_address == "dao3" && m.editor_address == "editor4"));
    }

    #[test]
    fn test_map_subspaces_added_empty() {
        let subspaces = vec![];
        let result = map_subspaces_added(&subspaces);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_subspaces_added_single() {
        let subspaces = vec![create_test_subspace_added("dao1", "subspace1")];
        let result = map_subspaces_added(&subspaces);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].subspace_address, "subspace1");
    }

    #[test]
    fn test_map_subspaces_added_multiple() {
        let subspaces = vec![
            create_test_subspace_added("dao1", "subspace1"),
            create_test_subspace_added("dao2", "subspace2"),
            create_test_subspace_added("dao1", "subspace3"),
        ];
        let result = map_subspaces_added(&subspaces);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].subspace_address, "subspace1");
        assert_eq!(result[1].dao_address, "dao2");
        assert_eq!(result[1].subspace_address, "subspace2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].subspace_address, "subspace3");
    }

    #[test]
    fn test_map_subspaces_removed_empty() {
        let subspaces = vec![];
        let result = map_subspaces_removed(&subspaces);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_subspaces_removed_single() {
        let subspaces = vec![create_test_subspace_removed("dao1", "subspace1")];
        let result = map_subspaces_removed(&subspaces);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].subspace_address, "subspace1");
    }

    #[test]
    fn test_map_subspaces_removed_multiple() {
        let subspaces = vec![
            create_test_subspace_removed("dao1", "subspace1"),
            create_test_subspace_removed("dao2", "subspace2"),
            create_test_subspace_removed("dao1", "subspace3"),
        ];
        let result = map_subspaces_removed(&subspaces);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].subspace_address, "subspace1");
        assert_eq!(result[1].dao_address, "dao2");
        assert_eq!(result[1].subspace_address, "subspace2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].subspace_address, "subspace3");
    }

    #[test]
    fn test_map_members_removed_empty() {
        let members = vec![];
        let result = map_members_removed(&members);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_members_removed_single() {
        let members = vec![create_test_member_removed("dao1", "member1")];
        let result = map_members_removed(&members);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "member1");
    }

    #[test]
    fn test_map_members_removed_multiple() {
        let members = vec![
            create_test_member_removed("dao1", "member1"),
            create_test_member_removed("dao2", "member2"),
            create_test_member_removed("dao1", "member3"),
        ];
        let result = map_members_removed(&members);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "member1");
        assert_eq!(result[1].dao_address, "dao2");
        assert_eq!(result[1].editor_address, "member2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].editor_address, "member3");
    }

    #[test]
    fn test_map_editors_removed_empty() {
        let editors = vec![];
        let result = map_editors_removed(&editors);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_editors_removed_single() {
        let editors = vec![create_test_editor_removed("dao1", "editor1")];
        let result = map_editors_removed(&editors);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
    }

    #[test]
    fn test_map_editors_removed_multiple() {
        let editors = vec![
            create_test_editor_removed("dao1", "editor1"),
            create_test_editor_removed("dao2", "editor2"),
            create_test_editor_removed("dao1", "editor3"),
        ];
        let result = map_editors_removed(&editors);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].dao_address, "dao1");
        assert_eq!(result[0].editor_address, "editor1");
        assert_eq!(result[1].dao_address, "dao2");
        assert_eq!(result[1].editor_address, "editor2");
        assert_eq!(result[2].dao_address, "dao1");
        assert_eq!(result[2].editor_address, "editor3");
    }

    // Tests for cache processing functionality
    mod cache_tests {
        use super::*;
        use crate::cache::{CacheBackend, CacheError};
        use async_trait::async_trait;
        use std::collections::HashMap;
        use tokio::sync::Mutex;
        use wire::pb::grc20::Edit;

        // Mock cache implementation for testing
        pub struct MockCache {
            data: Arc<Mutex<HashMap<String, PreprocessedEdit>>>,
        }

        impl MockCache {
            pub fn new() -> Self {
                Self {
                    data: Arc::new(Mutex::new(HashMap::new())),
                }
            }

            pub async fn insert(&self, uri: String, edit: PreprocessedEdit) {
                let mut data = self.data.lock().await;
                data.insert(uri, edit);
            }
        }

        #[async_trait]
        impl CacheBackend for MockCache {
            async fn get(&self, uri: &String) -> Result<PreprocessedEdit, CacheError> {
                let data = self.data.lock().await;
                data.get(uri).cloned().ok_or(CacheError::NotFound)
            }
        }

        fn create_test_edit(id_bytes: Vec<u8>) -> Edit {
            Edit {
                id: id_bytes,
                name: "Test Edit".to_string(),
                ops: vec![],
                authors: vec![],
                language: None,
            }
        }

        fn create_test_proposal_created_event(
            proposal_id: &str,
            content_uri: &str,
        ) -> wire::pb::chain::PublishEditProposalCreated {
            wire::pb::chain::PublishEditProposalCreated {
                proposal_id: proposal_id.to_string(),
                creator: "0x1234567890123456789012345678901234567890".to_string(),
                start_time: "1000000000".to_string(),
                end_time: "2000000000".to_string(),
                content_uri: content_uri.to_string(),
                dao_address: "0xdao1234567890123456789012345678901234567890".to_string(),
                plugin_address: "0xplugin1234567890123456789012345678901234567890".to_string(),
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_with_cache_success() {
            let cache = Arc::new(MockCache::new());
            
            // Create test Edit with known ID bytes
            let edit_id_bytes = vec![
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            ];
            let test_edit = create_test_edit(edit_id_bytes.clone());
            
            // Create preprocessed edit
            let preprocessed_edit = PreprocessedEdit {
                cid: "ipfs://QmTest123".to_string(),
                edit: Some(test_edit),
                is_errored: false,
                space_id: Uuid::new_v4(),
            };

            // Insert into mock cache
            cache.insert("ipfs://QmTest123".to_string(), preprocessed_edit).await;

            // Create test GeoOutput with proposal
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![create_test_proposal_created_event("proposal123", "ipfs://QmTest123")],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 1);
            
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[0] {
                assert_eq!(content_uri, "ipfs://QmTest123");
                assert_eq!(proposal_id, "proposal123");
                assert!(edit_id.is_some(), "Edit ID should be extracted from cache");
                
                // Verify the edit_id was correctly transformed from bytes
                let expected_uuid = Uuid::from_bytes(edit_id_bytes.as_slice().try_into().unwrap());
                assert_eq!(edit_id.unwrap(), expected_uuid);
            } else {
                panic!("Expected PublishEdit proposal");
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_with_cache_not_found() {
            let cache = Arc::new(MockCache::new());
            
            // Don't insert anything into cache to simulate cache miss

            // Create test GeoOutput with proposal
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![create_test_proposal_created_event("proposal123", "ipfs://QmNotFound")],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 1);
            
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[0] {
                assert_eq!(content_uri, "ipfs://QmNotFound");
                assert_eq!(proposal_id, "proposal123");
                assert!(edit_id.is_none(), "Edit ID should be None when cache miss occurs");
            } else {
                panic!("Expected PublishEdit proposal");
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_with_errored_cache_entry() {
            let cache = Arc::new(MockCache::new());
            
            // Create errored preprocessed edit
            let preprocessed_edit = PreprocessedEdit {
                cid: "ipfs://QmErrored".to_string(),
                edit: None,
                is_errored: true,
                space_id: Uuid::new_v4(),
            };

            // Insert into mock cache
            cache.insert("ipfs://QmErrored".to_string(), preprocessed_edit).await;

            // Create test GeoOutput with proposal
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![create_test_proposal_created_event("proposal123", "ipfs://QmErrored")],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 1);
            
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[0] {
                assert_eq!(content_uri, "ipfs://QmErrored");
                assert_eq!(proposal_id, "proposal123");
                assert!(edit_id.is_none(), "Edit ID should be None when cache entry is errored");
            } else {
                panic!("Expected PublishEdit proposal");
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_with_invalid_edit_id() {
            let cache = Arc::new(MockCache::new());
            
            // Create test Edit with invalid ID bytes (wrong length)
            let invalid_edit_id_bytes = vec![0x01, 0x02, 0x03]; // Too short for UUID
            let test_edit = create_test_edit(invalid_edit_id_bytes);
            
            // Create preprocessed edit
            let preprocessed_edit = PreprocessedEdit {
                cid: "ipfs://QmInvalidId".to_string(),
                edit: Some(test_edit),
                is_errored: false,
                space_id: Uuid::new_v4(),
            };

            // Insert into mock cache
            cache.insert("ipfs://QmInvalidId".to_string(), preprocessed_edit).await;

            // Create test GeoOutput with proposal
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![create_test_proposal_created_event("proposal123", "ipfs://QmInvalidId")],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 1);
            
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[0] {
                assert_eq!(content_uri, "ipfs://QmInvalidId");
                assert_eq!(proposal_id, "proposal123");
                assert!(edit_id.is_none(), "Edit ID should be None when transformation fails");
            } else {
                panic!("Expected PublishEdit proposal");
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_concurrent_cache_reads() {
            let cache = Arc::new(MockCache::new());
            
            // Create multiple test Edits with different IDs
            let edit_id_bytes_1 = vec![
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            ];
            let edit_id_bytes_2 = vec![
                0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
                0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
            ];
            
            let test_edit_1 = create_test_edit(edit_id_bytes_1.clone());
            let test_edit_2 = create_test_edit(edit_id_bytes_2.clone());
            
            // Create preprocessed edits
            let preprocessed_edit_1 = PreprocessedEdit {
                cid: "ipfs://QmTest1".to_string(),
                edit: Some(test_edit_1),
                is_errored: false,
                space_id: Uuid::new_v4(),
            };
            let preprocessed_edit_2 = PreprocessedEdit {
                cid: "ipfs://QmTest2".to_string(),
                edit: Some(test_edit_2),
                is_errored: false,
                space_id: Uuid::new_v4(),
            };

            // Insert into mock cache
            cache.insert("ipfs://QmTest1".to_string(), preprocessed_edit_1).await;
            cache.insert("ipfs://QmTest2".to_string(), preprocessed_edit_2).await;

            // Create test GeoOutput with multiple proposals
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![
                    create_test_proposal_created_event("proposal1", "ipfs://QmTest1"),
                    create_test_proposal_created_event("proposal2", "ipfs://QmTest2"),
                    create_test_proposal_created_event("proposal3", "ipfs://QmNotFound"), // Cache miss
                ],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 3);
            
            // Check first proposal (cache hit)
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[0] {
                assert_eq!(content_uri, "ipfs://QmTest1");
                assert_eq!(proposal_id, "proposal1");
                assert!(edit_id.is_some());
                let expected_uuid_1 = Uuid::from_bytes(edit_id_bytes_1.as_slice().try_into().unwrap());
                assert_eq!(edit_id.unwrap(), expected_uuid_1);
            } else {
                panic!("Expected PublishEdit proposal");
            }
            
            // Check second proposal (cache hit)
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[1] {
                assert_eq!(content_uri, "ipfs://QmTest2");
                assert_eq!(proposal_id, "proposal2");
                assert!(edit_id.is_some());
                let expected_uuid_2 = Uuid::from_bytes(edit_id_bytes_2.as_slice().try_into().unwrap());
                assert_eq!(edit_id.unwrap(), expected_uuid_2);
            } else {
                panic!("Expected PublishEdit proposal");
            }
            
            // Check third proposal (cache miss)
            if let ProposalCreated::PublishEdit { edit_id, content_uri, proposal_id, .. } = &result[2] {
                assert_eq!(content_uri, "ipfs://QmNotFound");
                assert_eq!(proposal_id, "proposal3");
                assert!(edit_id.is_none());
            } else {
                panic!("Expected PublishEdit proposal");
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_with_mixed_proposal_types() {
            let cache = Arc::new(MockCache::new());
            
            // Create test Edit for PublishEdit proposal
            let edit_id_bytes = vec![
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            ];
            let test_edit = create_test_edit(edit_id_bytes.clone());
            
            let preprocessed_edit = PreprocessedEdit {
                cid: "ipfs://QmEdit123".to_string(),
                edit: Some(test_edit),
                is_errored: false,
                space_id: Uuid::new_v4(),
            };

            cache.insert("ipfs://QmEdit123".to_string(), preprocessed_edit).await;

            // Create test GeoOutput with mixed proposal types
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![create_test_proposal_created_event("edit_proposal", "ipfs://QmEdit123")],
                proposed_added_members: vec![wire::pb::chain::AddMemberProposalCreated {
                    proposal_id: "member_proposal".to_string(),
                    creator: "0x1234567890123456789012345678901234567890".to_string(),
                    start_time: "1000000000".to_string(),
                    end_time: "2000000000".to_string(),
                    member: "0xmember1234567890123456789012345678901234567890".to_string(),
                    dao_address: "0xdao1234567890123456789012345678901234567890".to_string(),
                    plugin_address: "0xplugin1234567890123456789012345678901234567890".to_string(),
                    change_type: "add".to_string(),
                }],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 2);
            
            // Check PublishEdit proposal (should have edit_id from cache)
            let publish_edit = result.iter().find(|p| {
                matches!(p, ProposalCreated::PublishEdit { proposal_id, .. } if proposal_id == "edit_proposal")
            }).expect("Should find PublishEdit proposal");
            
            if let ProposalCreated::PublishEdit { edit_id, .. } = publish_edit {
                assert!(edit_id.is_some(), "PublishEdit should have edit_id from cache");
            }
            
            // Check AddMember proposal (should not have edit_id)
            let add_member = result.iter().find(|p| {
                matches!(p, ProposalCreated::AddMember { proposal_id, .. } if proposal_id == "member_proposal")
            }).expect("Should find AddMember proposal");
            
            if let ProposalCreated::AddMember { proposal_id, member, .. } = add_member {
                assert_eq!(proposal_id, "member_proposal");
                assert_eq!(member, "0xmember1234567890123456789012345678901234567890");
            }
        }

        #[tokio::test]
        async fn test_map_created_proposals_empty_edits() {
            let cache = Arc::new(MockCache::new());

            // Create test GeoOutput with no edit proposals
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![],
                proposed_added_members: vec![wire::pb::chain::AddMemberProposalCreated {
                    proposal_id: "member_proposal".to_string(),
                    creator: "0x1234567890123456789012345678901234567890".to_string(),
                    start_time: "1000000000".to_string(),
                    end_time: "2000000000".to_string(),
                    member: "0xmember1234567890123456789012345678901234567890".to_string(),
                    dao_address: "0xdao1234567890123456789012345678901234567890".to_string(),
                    plugin_address: "0xplugin1234567890123456789012345678901234567890".to_string(),
                    change_type: "add".to_string(),
                }],
                ..Default::default()
            };

            // Test the function
            let result = map_created_proposals(&geo, &cache).await.unwrap();

            assert_eq!(result.len(), 1);
            
            // Should only have the AddMember proposal, no cache interactions for PublishEdit
            if let ProposalCreated::AddMember { proposal_id, .. } = &result[0] {
                assert_eq!(proposal_id, "member_proposal");
            } else {
                panic!("Expected AddMember proposal");
            }
        }

        #[tokio::test]
        async fn test_proposal_item_uses_edit_id_from_cache() {
            let cache = Arc::new(MockCache::new());
            
            // Create test Edit with known ID bytes
            let edit_id_bytes = vec![
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            ];
            let expected_edit_uuid = Uuid::from_bytes(edit_id_bytes.as_slice().try_into().unwrap());
            let test_edit = create_test_edit(edit_id_bytes.clone());
            
            // Create preprocessed edit
            let preprocessed_edit = PreprocessedEdit {
                cid: "ipfs://QmTestEditId".to_string(),
                edit: Some(test_edit),
                is_errored: false,
                space_id: Uuid::new_v4(),
            };

            // Insert into mock cache
            cache.insert("ipfs://QmTestEditId".to_string(), preprocessed_edit).await;

            // Create test GeoOutput with proposal that has different proposal_id than edit_id
            let geo = wire::pb::chain::GeoOutput {
                edits: vec![create_test_proposal_created_event("different_proposal_id", "ipfs://QmTestEditId")],
                ..Default::default()
            };

            // Test map_created_proposals
            let result = map_created_proposals(&geo, &cache).await.unwrap();
            assert_eq!(result.len(), 1);
            
            if let ProposalCreated::PublishEdit { edit_id, proposal_id, .. } = &result[0] {
                assert_eq!(proposal_id, "different_proposal_id");
                assert!(edit_id.is_some());
                assert_eq!(edit_id.unwrap(), expected_edit_uuid);
            } else {
                panic!("Expected PublishEdit proposal");
            }

            // Now test that ProposalsModel::map_created_proposals uses the Edit ID for the ProposalItem.id
            use crate::models::proposals::ProposalsModel;
            let proposal_items = ProposalsModel::map_created_proposals(&result, 100);
            
            assert_eq!(proposal_items.len(), 1);
            let proposal_item = &proposal_items[0];
            
            // Verify that the ProposalItem.id uses the Edit ID from cache, NOT the proposal_id
            assert_eq!(proposal_item.id, expected_edit_uuid, 
                "ProposalItem.id should use Edit ID from cache, not proposal_id");
            
            // Verify proposal_id would have been different if parsed
            let proposal_uuid = Uuid::parse_str("different_proposal_id").unwrap_or_else(|_| Uuid::new_v4());
            assert_ne!(proposal_item.id, proposal_uuid, 
                "ProposalItem.id should NOT use the proposal_id when Edit ID is available");
        }
    }
}
