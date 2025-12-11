//! Hermes sink traits for consuming blockchain events.
//!
//! These traits wrap the low-level `stream` crate sinks with hermes-specific
//! configuration, providing type-safe module selection via [`HermesModule`].
//!
//! ## Cursor Persistence
//!
//! Consumers must implement `persist_cursor` and `load_persisted_cursor` to
//! enable resuming from the last processed block after a restart. The default
//! implementations are no-ops (cursor is not persisted).
//!
//! See `indexer/src/storage/postgres.rs` for an example of cursor persistence
//! with PostgreSQL.

use std::{env, process::exit, sync::Arc};

use futures03::StreamExt;

use crate::{HermesModule, HERMES_SPKG};
use stream::{
    pb::sf::substreams::rpc::v2::{BlockScopedData, BlockUndoSignal},
    substreams::SubstreamsEndpoint,
    substreams_stream::{BlockResponse, SubstreamsStream},
};

/// Trait for processing hermes-substream blocks.
///
/// Implement this trait to create a transformer that consumes events from
/// hermes-substream. The `run` method handles connection setup and streaming,
/// while you implement the block processing and cursor persistence logic.
///
/// # Example
///
/// ```ignore
/// use hermes_relay::{Sink, HermesModule};
///
/// struct SpacesTransformer { /* ... */ }
///
/// impl Sink for SpacesTransformer {
///     type Error = anyhow::Error;
///
///     async fn process_block_scoped_data(
///         &self,
///         data: &BlockScopedData,
///     ) -> Result<(), Self::Error> {
///         // Process space events...
///         Ok(())
///     }
///
///     async fn persist_cursor(&self, cursor: String, block: u64) -> Result<(), Self::Error> {
///         // Save cursor to database...
///         Ok(())
///     }
///
///     async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
///         // Load cursor from database...
///         Ok(None)
///     }
/// }
///
/// // Run the transformer
/// let transformer = SpacesTransformer { /* ... */ };
/// transformer.run(
///     &endpoint_url,
///     HermesModule::Actions,
///     start_block,
///     end_block,
/// ).await?;
/// ```
pub trait Sink: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Process a new block of data.
    fn process_block_scoped_data(
        &self,
        data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Handle a block undo signal (chain reorganization).
    ///
    /// You must delete any data recorded after the block height specified
    /// in the undo signal.
    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
        unimplemented!(
            "you must implement block undo handling, or request only final blocks"
        )
    }

    /// Persist the cursor after successfully processing a block.
    ///
    /// The cursor allows resuming from the correct position after a restart.
    fn persist_cursor(
        &self,
        _cursor: String,
        _block: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    /// Load the previously persisted cursor.
    ///
    /// Returns `None` if no cursor has been saved (start from beginning).
    fn load_persisted_cursor(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send {
        async { Ok(None) }
    }

    /// Run the sink, consuming events from hermes-substream.
    fn run(
        &self,
        endpoint_url: &str,
        module: HermesModule,
        start_block: i64,
        end_block: u64,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send {
        async move {
            let token = env::var("SUBSTREAMS_API_TOKEN").ok();
            let cursor = self.load_persisted_cursor().await?;

            let package = stream::read_package(HERMES_SPKG).await?;
            let endpoint = Arc::new(SubstreamsEndpoint::new(endpoint_url, token).await?);

            let mut stream = SubstreamsStream::new(
                endpoint,
                cursor,
                package.modules.clone(),
                module.to_string(),
                start_block,
                end_block,
            );

            loop {
                match stream.next().await {
                    None => {
                        println!("Stream consumed");
                        break;
                    }
                    Some(Ok(BlockResponse::New(data))) => {
                        self.process_block_scoped_data(&data).await?;
                        self.persist_cursor(data.cursor, data.clock.unwrap().number)
                            .await?;
                    }
                    Some(Ok(BlockResponse::Undo(undo_signal))) => {
                        self.process_block_undo_signal(&undo_signal)?;
                        self.persist_cursor(
                            undo_signal.last_valid_cursor,
                            undo_signal.last_valid_block.unwrap().number,
                        )
                        .await?;
                    }
                    Some(Err(err)) => {
                        println!("Stream terminated with error: {:?}", err);
                        exit(1);
                    }
                }
            }

            Ok(())
        }
    }
}

/// Trait for processing hermes-substream blocks with a preprocessing step.
///
/// Similar to [`Sink`], but allows decoding/preprocessing the block data
/// before the main processing step. Useful when you need to decode protobuf
/// messages before processing.
pub trait PreprocessedSink<P: Send>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Preprocess/decode the block data.
    fn preprocess_block_scoped_data(
        &self,
        data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<P, Self::Error>> + Send;

    /// Process the preprocessed block data.
    fn process_block_scoped_data(
        &self,
        data: &BlockScopedData,
        preprocessed: P,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Handle a block undo signal (chain reorganization).
    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
        unimplemented!(
            "you must implement block undo handling, or request only final blocks"
        )
    }

    /// Persist the cursor after successfully processing a block.
    fn persist_cursor(
        &self,
        _cursor: String,
        _block: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    /// Load the previously persisted cursor.
    fn load_persisted_cursor(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send {
        async { Ok(None) }
    }

    /// Run the sink, consuming events from hermes-substream.
    fn run(
        &self,
        endpoint_url: &str,
        module: HermesModule,
        start_block: i64,
        end_block: u64,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send {
        async move {
            let token = env::var("SUBSTREAMS_API_TOKEN").ok();
            let cursor = self.load_persisted_cursor().await?;

            let package = stream::read_package(HERMES_SPKG).await?;
            let endpoint = Arc::new(SubstreamsEndpoint::new(endpoint_url, token).await?);

            let mut stream = SubstreamsStream::new(
                endpoint,
                cursor,
                package.modules.clone(),
                module.to_string(),
                start_block,
                end_block,
            );

            loop {
                match stream.next().await {
                    None => {
                        println!("Stream consumed");
                        break;
                    }
                    Some(Ok(BlockResponse::New(data))) => {
                        let preprocessed = self.preprocess_block_scoped_data(&data).await?;
                        self.process_block_scoped_data(&data, preprocessed).await?;
                        self.persist_cursor(data.cursor, data.clock.unwrap().number)
                            .await?;
                    }
                    Some(Ok(BlockResponse::Undo(undo_signal))) => {
                        self.process_block_undo_signal(&undo_signal)?;
                        self.persist_cursor(
                            undo_signal.last_valid_cursor,
                            undo_signal.last_valid_block.unwrap().number,
                        )
                        .await?;
                    }
                    Some(Err(err)) => {
                        println!("Stream terminated with error: {:?}", err);
                        exit(1);
                    }
                }
            }

            Ok(())
        }
    }
}
