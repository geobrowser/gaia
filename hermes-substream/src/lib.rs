//! Hermes Substream
//!
//! Filters and emits raw Action events from the Space Registry contract.
//! Downstream consumers (hermes-relay) handle decoding into typed events.

pub mod helpers;
mod pb;

use pb::hermes::{Action, HermesOutput};
use substreams_ethereum::pb::eth;

// TODO: Replace with actual Space Registry contract address
const SPACE_REGISTRY_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

#[substreams::handlers::map]
fn hermes_out(block: eth::v2::Block) -> Result<HermesOutput, substreams::errors::Error> {
    let mut output = HermesOutput::default();

    for log in block.logs() {
        // Filter for Space Registry contract
        let address = helpers::format_hex(&log.address());
        if address != SPACE_REGISTRY_ADDRESS {
            continue;
        }

        // The Action event is anonymous with 4 indexed fields:
        // topic[0] = fromId (bytes16, left-padded to bytes32)
        // topic[1] = toId (bytes16, left-padded to bytes32)
        // topic[2] = action (bytes32)
        // topic[3] = topic (bytes32)
        // data = bytes payload
        let topics = log.topics();
        if topics.len() != 4 {
            continue;
        }

        output.actions.push(Action {
            from_id: topics[0][16..32].to_vec(), // bytes16 from right side of bytes32
            to_id: topics[1][16..32].to_vec(),
            action: topics[2].to_vec(),
            topic: topics[3].to_vec(),
            data: log.data().to_vec(),
        });
    }

    Ok(output)
}
