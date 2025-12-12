//! Spaces transformer - implements the Sink trait for processing space events.

use prost::Message;
use std::fmt;

use hermes_kafka::BaseProducer;

use hermes_relay::{actions, Actions, Sink};
use hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData;
use hermes_relay::stream::utils;
use hermes_schema::pb::space::HermesSpaceTrustExtension;

use crate::conversion::{convert_space_registered, convert_subspace_added, convert_subspace_removed};
use crate::kafka::{send_space_creation, send_trust_extension};

/// Error type for the spaces transformer that implements std::error::Error
#[derive(Debug)]
pub struct TransformerError(anyhow::Error);

impl fmt::Display for TransformerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TransformerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<anyhow::Error> for TransformerError {
    fn from(err: anyhow::Error) -> Self {
        TransformerError(err)
    }
}

impl From<prost::DecodeError> for TransformerError {
    fn from(err: prost::DecodeError) -> Self {
        TransformerError(anyhow::Error::from(err))
    }
}

/// Spaces transformer that filters and processes space-related events.
///
/// Subscribes to `HermesModule::Actions` and filters client-side for:
/// - `SPACE_REGISTERED` - new space registrations
/// - `SUBSPACE_ADDED` - trust extensions (verified/related/subtopic)
/// - `SUBSPACE_REMOVED` - trust revocations
pub struct SpacesTransformer {
    producer: BaseProducer,
}

impl SpacesTransformer {
    pub fn new(producer: BaseProducer) -> Self {
        Self { producer }
    }
}

impl Sink for SpacesTransformer {
    type Error = TransformerError;

    async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
        let output = utils::output(data);
        let block_meta = utils::block_metadata(data);

        // Decode the Actions message from the block output
        let actions_msg = Actions::decode(output.value.as_slice())?;

        let mut space_count = 0;
        let mut trust_count = 0;

        for action in &actions_msg.actions {
            let action_type = action.action.as_slice();

            if actions::matches(action_type, &actions::SPACE_REGISTERED) {
                let hermes_space = convert_space_registered(action, &block_meta)?;
                send_space_creation(&self.producer, &hermes_space)?;
                space_count += 1;

                println!(
                    "Block {}: Space registered: {}",
                    block_meta.block_number,
                    hex::encode(&hermes_space.space_id)
                );
            } else if actions::matches(action_type, &actions::SUBSPACE_ADDED) {
                let trust_ext = convert_subspace_added(action, &block_meta)?;
                send_trust_extension(&self.producer, &trust_ext)?;
                trust_count += 1;

                println!(
                    "Block {}: Subspace added: {} -> {}",
                    block_meta.block_number,
                    hex::encode(&trust_ext.source_space_id),
                    get_extension_type(&trust_ext)
                );
            } else if actions::matches(action_type, &actions::SUBSPACE_REMOVED) {
                let trust_ext = convert_subspace_removed(action, &block_meta)?;
                send_trust_extension(&self.producer, &trust_ext)?;
                trust_count += 1;

                println!(
                    "Block {}: Subspace removed: {} -> {}",
                    block_meta.block_number,
                    hex::encode(&trust_ext.source_space_id),
                    get_extension_type(&trust_ext)
                );
            }
            // Other action types are ignored (e.g., EDITS_PUBLISHED)
        }

        if space_count > 0 || trust_count > 0 {
            let drift = utils::format_drift(&block_meta);
            println!(
                "Block {} processed: {} spaces, {} trust extensions (drift: {})",
                block_meta.block_number, space_count, trust_count, drift
            );
        }

        Ok(())
    }

    fn process_block_undo_signal(
        &self,
        undo_signal: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockUndoSignal,
    ) -> std::result::Result<(), Self::Error> {
        // For now, just log the undo signal
        // In a production system, we would delete any data recorded after this block
        println!(
            "Block undo signal received: rolling back to block {}",
            undo_signal.last_valid_block.as_ref().map_or(0, |b| b.number)
        );

        // TODO: Implement actual rollback logic when cursor persistence is added
        // This would involve deleting Kafka messages or updating state

        Ok(())
    }
}

fn get_extension_type(ext: &HermesSpaceTrustExtension) -> &'static str {
    match &ext.extension {
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Verified(_)) => {
            "verified"
        }
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Related(_)) => {
            "related"
        }
        Some(hermes_schema::pb::space::hermes_space_trust_extension::Extension::Subtopic(_)) => {
            "subtopic"
        }
        None => "unknown",
    }
}
