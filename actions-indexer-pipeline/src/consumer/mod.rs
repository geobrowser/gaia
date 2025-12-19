//! Consumer module for the actions indexer pipeline.
//!
//! Provides the `ConsumeActions` trait for consuming blockchain action events
//! from data sources like substreams or Kafka. Acts as the entry point for the pipeline,
//! feeding data to processing and loading components.
use crate::errors::ConsumerError;

pub mod kafka;
pub mod stream;

use actions_indexer_shared::types::ActionRaw;
use async_trait::async_trait;
use stream::pb::sf::substreams::rpc::v2::BlockUndoSignal;
use tokio::sync::mpsc;

/// Message types that can be sent through the streaming channel.
///
/// Represents different types of events and data that flow through the consumer pipeline,
/// enabling communication between the stream provider and the orchestrator.
#[derive(Debug)]
pub enum StreamMessage {
    BlockData(BlockDataMessage),
    UndoSignal(BlockUndoSignal),
    Error(ConsumerError),
    StreamEnd,
}

#[derive(Debug)]
pub struct BlockDataMessage {
    pub actions: Vec<ActionRaw>,
    pub cursor: String,
    pub block_number: i64,
}
/// Consumer component responsible for orchestrating blockchain action streaming.
///
/// Acts as a coordinator between stream providers and the processing pipeline,
/// managing the flow of blockchain action data through channels. Provides a
/// clean abstraction over different streaming implementations.
pub struct ActionsConsumer {
    stream_provider: Box<dyn ConsumeActionsStream>,
}

impl ActionsConsumer {
    /// Creates a new `ActionsConsumer` with the specified stream provider and channel sender.
    ///
    /// # Arguments
    ///
    /// * `stream_provider` - A boxed trait object that implements `ConsumeActionsStream`
    /// * `sender` - Channel sender for communicating with the orchestrator
    ///
    /// # Returns
    ///
    /// A new `ActionsConsumer` instance ready to start streaming.
    pub fn new(stream_provider: Box<dyn ConsumeActionsStream>) -> Self {
        Self { stream_provider }
    }

    /// Starts the consumer and begins streaming blockchain action events.
    ///
    /// This method delegates to the underlying stream provider to initiate the
    /// streaming process. It will continue until the stream ends or an error occurs.
    /// 
    /// # Arguments
    ///
    /// * `sender` - Channel sender for streaming messages to the orchestrator
    /// * `cursor` - The cursor to start streaming from (optional)
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `ConsumerError` if streaming fails.
    ///
    /// # Errors
    ///
    /// Returns a `ConsumerError` if:
    /// - The stream provider fails to initialize or connect
    /// - Network connectivity issues occur during streaming
    /// - Data parsing or validation errors happen
    pub async fn run(&self, sender: mpsc::Sender<StreamMessage>, cursor: Option<String>) -> Result<(), ConsumerError> {
        self.stream_provider.stream_events(sender, cursor).await?;
        Ok(())
    }
}

/// Trait for consuming blockchain action events from data sources.
///
/// Provides a unified interface for different data sources (substreams, RPC, etc.).
#[async_trait]
pub trait ConsumeActionsStream: Send + Sync {
    /// Streams blockchain action events through a channel to decouple the consumer from orchestrator.
    ///
    /// This method runs the streaming loop and sends messages through the provided channel.
    /// It handles block data, undo signals, errors, and stream end notifications.
    ///
    /// # Arguments
    ///
    /// * `sender` - Channel sender for streaming messages to the orchestrator
    /// * `cursor` - The cursor to start streaming from (optional)
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `ConsumerError` if streaming fails.
    async fn stream_events(&self, sender: mpsc::Sender<StreamMessage>, cursor: Option<String>) -> Result<(), ConsumerError>;
}
