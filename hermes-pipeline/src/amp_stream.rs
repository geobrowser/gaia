use hermes_amp::{AmpStreamConfig, stream_actions};
use hermes_relay::Actions;

use crate::{Pipeline, PipelineError};

pub async fn run_amp_stream(pipeline: &Pipeline) -> Result<(), PipelineError> {
    let config = AmpStreamConfig::from_env();
    let pipeline_ref = pipeline;

    stream_actions(config, move |block| {
        let pipeline_ref = pipeline_ref;
        async move {
            let actions = Actions {
                actions: block.actions,
            };
            pipeline_ref
                .process_actions_block(actions, block.block_num, block.timestamp_secs, block.cursor)
                .await
                .map_err(anyhow::Error::from)
        }
    })
    .await
    .map_err(PipelineError::from)
}
