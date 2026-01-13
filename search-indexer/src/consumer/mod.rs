//! Consumer module for the search indexer ingest.
//!
//! Provides Kafka consumer functionality for receiving entity events and score updates.

mod entities_consumer;
mod kafka_config;
mod messages;
mod scores_consumer;

pub use entities_consumer::EntitiesConsumer;
pub use messages::{EntityEvent, EntityEventType, ScoreEvent, ScoreEventType, StreamMessage};
pub use scores_consumer::ScoresConsumer;
