//! Kafka integration for Atlas
//!
//! This module provides Kafka producer functionality for emitting
//! canonical graph updates to downstream consumers.

mod producer;

pub use producer::{AtlasProducer, ProducerError};
