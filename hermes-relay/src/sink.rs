//! Hermes sink traits for consuming blockchain events.
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::{Sink, StreamSource, HermesModule};
//!
//! struct MyTransformer { /* ... */ }
//!
//! impl Sink for MyTransformer {
//!     type Error = anyhow::Error;
//!
//!     async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
//!         // Process events...
//!         Ok(())
//!     }
//! }
//!
//! // Run with mock data (for development/testing)
//! transformer.run(StreamSource::mock()).await?;
//!
//! // Run with live substream (for production)
//! let source = StreamSource::live(
//!     "https://substreams.example.com",
//!     HermesModule::Actions,
//!     0,
//!     1000,
//! );
//! transformer.run(source).await?;
//! ```

use std::{env, process::exit, sync::Arc};

use futures03::StreamExt;

use crate::{source::MockSource, HermesModule, HERMES_SPKG};
use stream::{
    pb::sf::substreams::rpc::v2::{BlockScopedData, BlockUndoSignal},
    substreams::SubstreamsEndpoint,
    substreams_stream::{BlockResponse, SubstreamsStream},
};

/// Configuration for the stream source.
///
/// Use this to explicitly choose between mock and live data sources.
#[derive(Debug, Clone)]
pub enum StreamSource {
    /// Use mock test topology data.
    ///
    /// Generates deterministic test data with:
    /// - 18 space creations (11 canonical + 7 non-canonical)
    /// - 19 trust extensions (14 explicit + 5 topic-based)
    /// - 6 edit events
    ///
    /// All events are delivered in a single block.
    Mock,

    /// Connect to a live substream endpoint.
    Live {
        /// The substream endpoint URL
        endpoint_url: String,
        /// The hermes module to consume
        module: HermesModule,
        /// First block to consume (can be negative for relative positioning)
        start_block: i64,
        /// Last block to consume
        end_block: u64,
    },
}

impl StreamSource {
    /// Create a mock source that delivers all test topology events in a single block.
    pub fn mock() -> Self {
        Self::Mock
    }

    /// Create a live source with the given endpoint, module, and block range.
    pub fn live(
        endpoint_url: impl Into<String>,
        module: HermesModule,
        start_block: i64,
        end_block: u64,
    ) -> Self {
        Self::Live {
            endpoint_url: endpoint_url.into(),
            module,
            start_block,
            end_block,
        }
    }
}

/// Trait for processing hermes-substream blocks.
pub trait Sink: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn process_block_scoped_data(
        &self,
        data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
        unimplemented!("implement block undo handling, or request only final blocks")
    }

    fn persist_cursor(
        &self,
        _cursor: String,
        _block: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn load_persisted_cursor(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send {
        async { Ok(None) }
    }

    /// Run the sink with the specified source.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Development: use mock data
    /// sink.run(StreamSource::mock()).await?;
    ///
    /// // Production: use live substream
    /// let source = StreamSource::live(
    ///     "https://substreams.example.com",
    ///     HermesModule::Actions,
    ///     0,
    ///     1000,
    /// );
    /// sink.run(source).await?;
    /// ```
    fn run(
        &self,
        source: StreamSource,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send
    where
        Self::Error: Into<anyhow::Error>,
    {
        async move {
            match source {
                StreamSource::Mock => self.run_mock().await,
                StreamSource::Live {
                    endpoint_url,
                    module,
                    start_block,
                    end_block,
                } => {
                    self.run_live(&endpoint_url, module, start_block, end_block)
                        .await
                }
            }
        }
    }

    /// Run with mock data using the test topology.
    ///
    /// All test topology events are delivered in a single block (block 0).
    fn run_mock(&self) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send
    where
        Self::Error: Into<anyhow::Error>,
    {
        async move {
            println!("Running with mock test topology");
            // Use a single block (0) containing all test topology events
            let source = MockSource::test_topology().single_block(0);

            for block in source {
                let block_num = block.clock.as_ref().map(|c| c.number).unwrap_or(0);
                self.process_block_scoped_data(&block)
                    .await
                    .map_err(Into::into)?;
                self.persist_cursor(block.cursor, block_num)
                    .await
                    .map_err(Into::into)?;
            }

            println!("Mock stream consumed");
            Ok(())
        }
    }

    /// Run with a live substream connection.
    fn run_live(
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

/// Sink with a preprocessing step (e.g., protobuf decoding).
pub trait PreprocessedSink<P: Send>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn preprocess_block_scoped_data(
        &self,
        data: &BlockScopedData,
    ) -> impl std::future::Future<Output = Result<P, Self::Error>> + Send;

    fn process_block_scoped_data(
        &self,
        data: &BlockScopedData,
        preprocessed: P,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    fn process_block_undo_signal(&self, _undo_signal: &BlockUndoSignal) -> Result<(), Self::Error> {
        unimplemented!("implement block undo handling, or request only final blocks")
    }

    fn persist_cursor(
        &self,
        _cursor: String,
        _block: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn load_persisted_cursor(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send {
        async { Ok(None) }
    }

    /// Run the sink with the specified source.
    fn run(
        &self,
        source: StreamSource,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send
    where
        Self::Error: Into<anyhow::Error>,
    {
        async move {
            match source {
                StreamSource::Mock => self.run_mock().await,
                StreamSource::Live {
                    endpoint_url,
                    module,
                    start_block,
                    end_block,
                } => {
                    self.run_live(&endpoint_url, module, start_block, end_block)
                        .await
                }
            }
        }
    }

    /// Run with mock data using the test topology.
    ///
    /// All test topology events are delivered in a single block (block 0).
    fn run_mock(&self) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send
    where
        Self::Error: Into<anyhow::Error>,
    {
        async move {
            println!("Running with mock test topology");
            // Use a single block (0) containing all test topology events
            let source = MockSource::test_topology().single_block(0);

            for block in source {
                let block_num = block.clock.as_ref().map(|c| c.number).unwrap_or(0);
                let preprocessed = self
                    .preprocess_block_scoped_data(&block)
                    .await
                    .map_err(Into::into)?;
                self.process_block_scoped_data(&block, preprocessed)
                    .await
                    .map_err(Into::into)?;
                self.persist_cursor(block.cursor, block_num)
                    .await
                    .map_err(Into::into)?;
            }

            println!("Mock stream consumed");
            Ok(())
        }
    }

    /// Run with a live substream connection.
    fn run_live(
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
