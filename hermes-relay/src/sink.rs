//! Hermes sink traits for consuming blockchain events.
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::{Sink, HermesModule};
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
//! // Run against real substream
//! transformer.run(&endpoint_url, HermesModule::Actions, start_block, end_block).await?;
//!
//! // Or test with mock data
//! use hermes_relay::source::MockSource;
//! for block in MockSource::new(data).with_blocks(100, 110) {
//!     transformer.process_block_scoped_data(&block).await?;
//! }
//! ```

use std::{env, process::exit, sync::Arc};

use futures03::StreamExt;

use crate::{HermesModule, HERMES_SPKG};
use stream::{
    pb::sf::substreams::rpc::v2::{BlockScopedData, BlockUndoSignal},
    substreams::SubstreamsEndpoint,
    substreams_stream::{BlockResponse, SubstreamsStream},
};

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
