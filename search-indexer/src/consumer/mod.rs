//! Consumer module for the search indexer ingest.
//!
//! Provides Kafka consumer functionality for receiving entity events and score updates.

mod entities_consumer;
pub mod kafka_config;
mod messages;
mod scores_consumer;
mod space_topics_consumer;
pub mod topology_consumer;

pub use entities_consumer::EntitiesConsumer;
pub use messages::{EntityEvent, EntityEventType, ScoreEvent, ScoreEventType, SpaceTopicEvent, StreamMessage};
pub use scores_consumer::ScoresConsumer;
pub use space_topics_consumer::SpaceTopicsConsumer;
pub use topology_consumer::TopologyConsumer;
