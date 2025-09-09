use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use stream::utils::{self, BlockMetadata};
use tracing::{info, instrument, Instrument};

use crate::block_handler::{
    edit_handler, membership_handler, space_handler, subspace_handler, utils::handle_task_result,
};
use crate::cache::properties_cache::ImmutableCache;

use crate::error::IndexingError;
use crate::storage::StorageBackend;
use crate::KgData;

#[instrument(skip_all, fields(
    block_number = block_metadata.block_number,
    block_timestamp = block_metadata.timestamp,
    edit_count = output.edits.len(),
    space_count = output.spaces.len(),
    member_count = output.added_members.len(),
    editor_count = output.added_editors.len(),
    subspace_count = output.added_subspaces.len()
))]
pub async fn run<S, C>(
    output: &KgData,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
    properties_cache: &Arc<C>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
    C: ImmutableCache + Send + Sync + 'static,
{
    // Set up block context that child spans can inherit
    let current_span = tracing::Span::current();
    current_span.record("block_number", block_metadata.block_number);
    current_span.record("block_timestamp", &block_metadata.timestamp);
    let block_timestamp_seconds: i64 = block_metadata.timestamp.parse().unwrap_or(0);
    let block_datetime = DateTime::from_timestamp(block_timestamp_seconds, 0)
        .unwrap_or_else(|| Utc::now());
    let block_datetime_local = block_datetime.with_timezone(&Local);
    let drift_str = utils::format_drift(block_metadata);

    info!(
        block_number = block_metadata.block_number,
        block_time = %block_datetime_local.format("%Y-%m-%d %H:%M:%S"),
        drift = %drift_str,
        edit_count = output.edits.len(),
        space_count = output.spaces.len(),
        "Processing block"
    );

    let space_task = {
        let storage = Arc::clone(storage);
        let block_metadata = block_metadata.clone();
        let spaces = output.spaces.clone();
        let block_number = block_metadata.block_number;

        tokio::spawn(
            async move { space_handler::run(&spaces, &block_metadata, &storage).await }
                .instrument(tracing::info_span!("space_task", block_number = block_number))
        )
    };

    let edit_task = {
        let storage = Arc::clone(storage);
        let properties_cache = Arc::clone(properties_cache);
        let block_metadata = block_metadata.clone();
        let edits = output.edits.clone();
        let block_number = block_metadata.block_number;
        let edit_count = edits.len();
        
        tokio::spawn(
            async move {
                edit_handler::run(&edits, &block_metadata, &storage, &properties_cache).await
            }
            .instrument(tracing::info_span!("edit_task", block_number = block_number, edit_count = edit_count))
        )
    };

    let membership_task = {
        let storage = Arc::clone(storage);
        let block_metadata = block_metadata.clone();
        let added_members = output.added_members.clone();
        let removed_members = output.removed_members.clone();
        let added_editors = output.added_editors.clone();
        let removed_editors = output.removed_editors.clone();
        let block_number = block_metadata.block_number;
        let member_count = added_members.len() + removed_members.len();
        let editor_count = added_editors.len() + removed_editors.len();
        
        tokio::spawn(
            async move {
                membership_handler::run(
                    &added_members,
                    &removed_members,
                    &added_editors,
                    &removed_editors,
                    &block_metadata,
                    &storage,
                )
                .await
            }
            .instrument(tracing::info_span!("membership_task", 
                block_number = block_number,
                member_count = member_count,
                editor_count = editor_count
            ))
        )
    };

    let subspace_task = {
        let storage = Arc::clone(storage);
        let block_metadata = block_metadata.clone();
        let added_subspaces = output.added_subspaces.clone();
        let removed_subspaces = output.removed_subspaces.clone();
        let block_number = block_metadata.block_number;
        let subspace_count = added_subspaces.len() + removed_subspaces.len();
        
        tokio::spawn(
            async move {
                subspace_handler::run(
                    &added_subspaces,
                    &removed_subspaces,
                    &block_metadata,
                    &storage,
                )
                .await
            }
            .instrument(tracing::info_span!("subspace_task", 
                block_number = block_number,
                subspace_count = subspace_count
            ))
        )
    };

    let (space_result, edit_result, membership_result, subspace_result) =
        tokio::join!(space_task, edit_task, membership_task, subspace_task);

    handle_task_result(space_result)?;
    handle_task_result(edit_result)?;
    handle_task_result(membership_result)?;
    handle_task_result(subspace_result)?;

    info!(
        block_number = block_metadata.block_number,
        "Successfully processed block"
    );
    
    Ok(())
}
