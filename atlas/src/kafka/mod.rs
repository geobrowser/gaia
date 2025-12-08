//! Kafka integration for Atlas
//!
//! This module provides Kafka producer functionality for emitting
//! canonical graph updates to downstream consumers.

mod emitter;
mod producer;

pub use emitter::CanonicalGraphEmitter;
pub use producer::{AtlasProducer, ProducerError};
