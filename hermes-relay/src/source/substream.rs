//! Production implementation of BlockSource wrapping SubstreamsStream.

use std::{env, sync::Arc};

use async_trait::async_trait;
use futures03::StreamExt;

use crate::{HermesModule, HERMES_SPKG};
use stream::{
    substreams::SubstreamsEndpoint,
    substreams_stream::{BlockResponse as StreamBlockResponse, SubstreamsStream},
};

use super::{BlockData, BlockResponse, BlockSource, UndoSignal};

/// A block source that reads from a real substream.
///
/// This wraps the existing `SubstreamsStream` to implement the `BlockSource` trait,
/// allowing transformers to use the same code path for both real and mock data.
///
/// # Example
///
/// ```ignore
/// use hermes_relay::source::SubstreamSource;
/// use hermes_relay::HermesModule;
///
/// let source = SubstreamSource::connect(
///     "https://mainnet.eth.streamingfast.io:443",
///     HermesModule::Actions,
///     None,  // cursor
///     0,     // start block
///     1000,  // end block
/// ).await?;
///
/// while let Some(result) = source.next().await {
///     // Process blocks...
/// }
/// ```
pub struct SubstreamSource {
    stream: SubstreamsStream,
    current_cursor: Option<String>,
}

impl SubstreamSource {
    /// Connect to a substream endpoint.
    ///
    /// # Arguments
    ///
    /// * `endpoint_url` - The URL of the substreams endpoint
    /// * `module` - The hermes module to consume
    /// * `cursor` - Optional cursor to resume from
    /// * `start_block` - The block number to start from (negative for relative)
    /// * `end_block` - The block number to stop at (0 for live)
    ///
    /// # Environment Variables
    ///
    /// * `SUBSTREAMS_API_TOKEN` - Optional authentication token
    pub async fn connect(
        endpoint_url: &str,
        module: HermesModule,
        cursor: Option<String>,
        start_block: i64,
        end_block: u64,
    ) -> Result<Self, anyhow::Error> {
        let token = env::var("SUBSTREAMS_API_TOKEN").ok();
        let package = stream::read_package(HERMES_SPKG).await?;
        let endpoint = Arc::new(SubstreamsEndpoint::new(endpoint_url, token).await?);

        let stream = SubstreamsStream::new(
            endpoint,
            cursor.clone(),
            package.modules.clone(),
            module.to_string(),
            start_block,
            end_block,
        );

        Ok(Self {
            stream,
            current_cursor: cursor,
        })
    }
}

#[async_trait]
impl BlockSource for SubstreamSource {
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>> {
        match self.stream.next().await {
            None => None,
            Some(Ok(StreamBlockResponse::New(data))) => {
                self.current_cursor = Some(data.cursor.clone());

                let clock = data.clock.as_ref();
                let block_number = clock.map(|c| c.number).unwrap_or(0);
                let timestamp = clock
                    .and_then(|c| c.timestamp.as_ref())
                    .map(|t| t.seconds as u64)
                    .unwrap_or(0);

                // Extract the output data from the MapModuleOutput
                let (output, module_name) = data
                    .output
                    .as_ref()
                    .map(|o| {
                        let output_data = o
                            .map_output
                            .as_ref()
                            .map(|any| any.value.clone())
                            .unwrap_or_default();
                        (output_data, o.name.clone())
                    })
                    .unwrap_or_default();

                Some(Ok(BlockResponse::New(BlockData {
                    block_number,
                    timestamp,
                    cursor: data.cursor,
                    output,
                    module_name,
                })))
            }
            Some(Ok(StreamBlockResponse::Undo(signal))) => {
                self.current_cursor = Some(signal.last_valid_cursor.clone());
                Some(Ok(BlockResponse::Undo(UndoSignal {
                    last_valid_block: signal
                        .last_valid_block
                        .as_ref()
                        .map(|b| b.number)
                        .unwrap_or(0),
                    last_valid_cursor: signal.last_valid_cursor,
                })))
            }
            Some(Err(e)) => Some(Err(e)),
        }
    }

    fn cursor(&self) -> Option<&str> {
        self.current_cursor.as_deref()
    }
}
